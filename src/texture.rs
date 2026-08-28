use std::{
    collections::{HashMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use image::RgbImage;
use reqwest::blocking::Client;

use crate::world::Bounds;

const SOURCE_CACHE_VERSION: &str = "2025-512-v4-classic-iso";
const SOURCE_SIZE: u32 = 512;
const SOURCE_ZOOM: u8 = 8;
const SOURCE_OVERLAP_PIXELS: f32 = 2.0;
const SOURCE_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_LOCKS: usize = 256;
const DECODED_CACHE_IMAGES: usize = 64;
const SOURCE_URL: &str = "https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer/export";

pub struct AerialSource {
    client: Client,
    root: PathBuf,
    cached_bytes: AtomicU64,
    temporary_id: AtomicU64,
    download_locks: [Mutex<()>; DOWNLOAD_LOCKS],
    decoded: Mutex<DecodedCache>,
}

#[derive(Default)]
struct DecodedCache {
    images: HashMap<PathBuf, Arc<RgbImage>>,
    order: VecDeque<PathBuf>,
}

impl AerialSource {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("geo-philly/0.1 (public-data texture cache)")
            .build()
            .map_err(io::Error::other)?;
        let root = root.into();
        let cached_bytes = directory_size(&root.join(SOURCE_CACHE_VERSION))?;
        Ok(Self {
            client,
            root,
            cached_bytes: AtomicU64::new(cached_bytes),
            temporary_id: AtomicU64::new(0),
            download_locks: std::array::from_fn(|_| Mutex::new(())),
            decoded: Mutex::new(DecodedCache::default()),
        })
    }

    pub fn tile(&self, bounds: Bounds, z: u8, x: u32, y: u32) -> io::Result<AerialTile> {
        let path = self
            .root
            .join(SOURCE_CACHE_VERSION)
            .join(z.to_string())
            .join(x.to_string())
            .join(format!("{y}.jpg"));
        let _guard = self.download_locks[download_shard(z, x, y)]
            .lock()
            .map_err(|_| io::Error::other("aerial download lock poisoned"))?;
        if let Some(image) = self.decoded(&path)? {
            return Ok(AerialTile { bounds, image });
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.download(&path, bounds)?
            }
            Err(error) => return Err(error),
        };
        let image = Arc::new(decode_source(&bytes)?);
        self.remember(path, Arc::clone(&image))?;
        Ok(AerialTile { bounds, image })
    }

    fn decoded(&self, path: &Path) -> io::Result<Option<Arc<RgbImage>>> {
        let cache = self
            .decoded
            .lock()
            .map_err(|_| io::Error::other("decoded aerial cache poisoned"))?;
        Ok(cache.images.get(path).cloned())
    }

    fn remember(&self, path: PathBuf, image: Arc<RgbImage>) -> io::Result<()> {
        let mut cache = self
            .decoded
            .lock()
            .map_err(|_| io::Error::other("decoded aerial cache poisoned"))?;
        if cache.images.contains_key(&path) {
            return Ok(());
        }
        while cache.images.len() >= DECODED_CACHE_IMAGES {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            cache.images.remove(&oldest);
        }
        cache.order.push_back(path.clone());
        cache.images.insert(path, image);
        Ok(())
    }

    fn download(&self, path: &Path, bounds: Bounds) -> io::Result<Vec<u8>> {
        let response = self.fetch(bounds)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("aerial cache path has no parent"))?;
        fs::create_dir_all(parent)?;
        let _cached = self.write_cached(path, &response)?;
        Ok(response)
    }

    fn fetch(&self, bounds: Bounds) -> io::Result<Vec<u8>> {
        let bbox = format!(
            "{},{},{},{}",
            bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y
        );
        let size = format!("{SOURCE_SIZE},{SOURCE_SIZE}");
        let query = [
            ("bbox", bbox.as_str()),
            ("bboxSR", "32129"),
            ("size", size.as_str()),
            ("imageSR", "32129"),
            ("format", "jpg"),
            ("transparent", "false"),
            ("f", "image"),
        ];
        let mut last_error = None;
        for attempt in 0..6 {
            let result = self
                .client
                .get(SOURCE_URL)
                .query(&query)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .and_then(reqwest::blocking::Response::bytes)
                .map_err(io::Error::other)
                .and_then(|bytes| {
                    let bytes = bytes.to_vec();
                    decode_source(&bytes).map(|_| bytes)
                });
            match result {
                Ok(response) => return Ok(response),
                Err(error) if attempt < 5 => {
                    last_error = Some(error);
                    std::thread::sleep(Duration::from_millis(250_u64 << attempt));
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("aerial request failed")))
    }

    fn write_cached(&self, path: &Path, response: &[u8]) -> io::Result<bool> {
        let bytes = response.len() as u64;
        if !self.reserve_cache_bytes(bytes) {
            return Ok(false);
        }
        let id = self.temporary_id.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_extension(format!("jpg.part-{}-{id}", std::process::id()));
        if let Err(error) = fs::write(&temporary, response) {
            self.cached_bytes.fetch_sub(bytes, Ordering::Relaxed);
            return Err(error);
        }
        match fs::rename(&temporary, path) {
            Ok(()) => return Ok(true),
            Err(_) if path.exists() => {
                let _removed = fs::remove_file(&temporary);
                self.cached_bytes.fetch_sub(bytes, Ordering::Relaxed);
            }
            Err(error) => {
                self.cached_bytes.fetch_sub(bytes, Ordering::Relaxed);
                return Err(error);
            }
        }
        Ok(false)
    }

    fn reserve_cache_bytes(&self, bytes: u64) -> bool {
        self.cached_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current
                    .checked_add(bytes)
                    .filter(|next| *next <= SOURCE_CACHE_MAX_BYTES)
            })
            .is_ok()
    }
}

fn download_shard(z: u8, x: u32, y: u32) -> usize {
    let hash = x
        .wrapping_mul(31)
        .wrapping_add(y.wrapping_mul(17))
        .wrapping_add(u32::from(z));
    hash as usize % DOWNLOAD_LOCKS
}

fn decode_source(bytes: &[u8]) -> io::Result<RgbImage> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(io::Error::other)?
        .into_rgb8();
    if image.dimensions() != (SOURCE_SIZE, SOURCE_SIZE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "aerial source returned an unexpected image size",
        ));
    }
    Ok(image)
}

pub struct AerialTile {
    bounds: Bounds,
    image: Arc<RgbImage>,
}

impl AerialTile {
    pub fn for_isometric_tile(
        source: &AerialSource,
        iso_bounds: Bounds,
        z: u8,
        x: u32,
        y: u32,
    ) -> io::Result<Self> {
        let (source_bounds, source_z, source_x, source_y) = if z <= SOURCE_ZOOM {
            (iso_bounds, z, x, y)
        } else {
            let factor = 1_u32 << (z - SOURCE_ZOOM);
            let width = iso_bounds.width();
            let height = iso_bounds.height();
            let offset_x = (x % factor) as f32;
            let offset_y = (y % factor) as f32;
            (
                Bounds {
                    min_x: iso_bounds.min_x - offset_x * width,
                    min_y: iso_bounds.min_y - offset_y * height,
                    max_x: iso_bounds.min_x + (factor as f32 - offset_x) * width,
                    max_y: iso_bounds.min_y + (factor as f32 - offset_y) * height,
                },
                SOURCE_ZOOM,
                x / factor,
                y / factor,
            )
        };
        let source_bounds = source_bounds.ground_source_bounds();
        let overlap = source_bounds.width().max(source_bounds.height()) / SOURCE_SIZE as f32
            * SOURCE_OVERLAP_PIXELS;
        source.tile(source_bounds.pad(overlap), source_z, source_x, source_y)
    }

    pub fn sample(&self, x: f32, y: f32, block_size: f32) -> [u8; 3] {
        let x = (x / block_size)
            .floor()
            .mul_add(block_size, block_size * 0.5);
        let y = (y / block_size)
            .floor()
            .mul_add(block_size, block_size * 0.5);
        let u = ((x - self.bounds.min_x) / self.bounds.width())
            .clamp(0.0, 1.0)
            .mul_add(self.image.width().saturating_sub(1) as f32, 0.0);
        let v = ((self.bounds.max_y - y) / self.bounds.height())
            .clamp(0.0, 1.0)
            .mul_add(self.image.height().saturating_sub(1) as f32, 0.0);
        self.box_average(u, v).map(posterize)
    }

    fn box_average(&self, u: f32, v: f32) -> [u8; 3] {
        let center_x = u.round() as i32;
        let center_y = v.round() as i32;
        let mut sum = [0_u32; 3];
        for y in -1..=1 {
            for x in -1..=1 {
                let sample_x = (center_x + x).clamp(0, self.image.width() as i32 - 1) as u32;
                let sample_y = (center_y + y).clamp(0, self.image.height() as i32 - 1) as u32;
                let pixel = self.image.get_pixel(sample_x, sample_y).0;
                for channel in 0..3 {
                    sum[channel] += u32::from(pixel[channel]);
                }
            }
        }
        sum.map(|channel| (channel / 9) as u8)
    }
}

fn directory_size(path: &Path) -> io::Result<u64> {
    let mut bytes = 0_u64;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        bytes = bytes.saturating_add(if metadata.is_dir() {
            directory_size(&entry.path())?
        } else {
            metadata.len()
        });
    }
    Ok(bytes)
}

fn posterize(channel: u8) -> u8 {
    ((u16::from(channel) + 16) / 32 * 32).min(255) as u8
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{AerialSource, DOWNLOAD_LOCKS, SOURCE_CACHE_MAX_BYTES, download_shard, posterize};

    #[test]
    fn posterize_uses_fixed_thirty_two_value_steps() {
        assert_eq!(posterize(0), 0);
        assert_eq!(posterize(47), 32);
        assert_eq!(posterize(250), 255);
    }

    #[test]
    fn source_cache_has_a_fixed_two_gibibyte_limit() -> std::io::Result<()> {
        assert_eq!(SOURCE_CACHE_MAX_BYTES, 2_147_483_648);
        let source = AerialSource::open("target/nonexistent-aerial-test-cache")?;
        source
            .cached_bytes
            .store(SOURCE_CACHE_MAX_BYTES - 1, Ordering::Relaxed);
        assert!(source.reserve_cache_bytes(1));
        assert!(!source.reserve_cache_bytes(1));
        Ok(())
    }

    #[test]
    fn download_locks_keep_the_same_tile_single_flight() -> std::io::Result<()> {
        let first = download_shard(5, 12, 8);

        assert!(first < DOWNLOAD_LOCKS);
        assert_eq!(first, download_shard(5, 12, 8));
        assert_eq!(
            (0..DOWNLOAD_LOCKS)
                .map(|x| download_shard(5, x as u32, 8))
                .collect::<std::collections::HashSet<_>>()
                .len(),
            DOWNLOAD_LOCKS
        );
        let source = AerialSource::open("target/nonexistent-aerial-lock-test-cache")?;
        let guard = source.download_locks[first]
            .lock()
            .map_err(|_| std::io::Error::other("test download lock poisoned"))?;
        assert!(
            source.download_locks[download_shard(5, 12, 8)]
                .try_lock()
                .is_err()
        );
        drop(guard);
        Ok(())
    }
}
