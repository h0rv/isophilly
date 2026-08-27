use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use clap::ValueEnum;
use image::RgbImage;
use reqwest::blocking::Client;
use serde::Serialize;

use crate::world::Bounds;

const SOURCE_CACHE_VERSION: &str = "2025-1024-v2";
const SOURCE_SIZE: u32 = 1024;
const SOURCE_OVERLAP_PIXELS: f32 = 2.0;
const SOURCE_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const SOURCE_URL: &str = "https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer/export";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TextureMode {
    None,
    Full,
    #[default]
    Pixel,
}

impl TextureMode {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Pixel => "pixel",
        }
    }
}

pub struct AerialSource {
    client: Client,
    root: PathBuf,
    cached_bytes: AtomicU64,
    temporary_id: AtomicU64,
    download_lock: Mutex<()>,
}

impl AerialSource {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("geo-philly/0.1 (public-data texture cache)")
            .build()
            .map_err(io::Error::other)?;
        let root = root.into();
        let cached_bytes = directory_size(&root)?;
        Ok(Self {
            client,
            root,
            cached_bytes: AtomicU64::new(cached_bytes),
            temporary_id: AtomicU64::new(0),
            download_lock: Mutex::new(()),
        })
    }

    pub fn tile(&self, bounds: Bounds, z: u8, x: u32, y: u32) -> io::Result<AerialTile> {
        let path = self
            .root
            .join(SOURCE_CACHE_VERSION)
            .join(z.to_string())
            .join(x.to_string())
            .join(format!("{y}.jpg"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let _guard = self
                    .download_lock
                    .lock()
                    .map_err(|_| io::Error::other("aerial download lock poisoned"))?;
                match fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        self.download(&path, bounds)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        let image = decode_source(&bytes)?;
        Ok(AerialTile { bounds, image })
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
        let response = (0..3)
            .find_map(|attempt| {
                match self
                    .client
                    .get(SOURCE_URL)
                    .query(&query)
                    .send()
                    .and_then(reqwest::blocking::Response::error_for_status)
                {
                    Ok(response) => Some(Ok(response)),
                    Err(error) if attempt < 2 && retryable(&error) => {
                        std::thread::sleep(Duration::from_millis(250_u64 << attempt));
                        None
                    }
                    Err(error) => Some(Err(error)),
                }
            })
            .ok_or_else(|| io::Error::other("aerial request exhausted its retries"))?
            .map_err(io::Error::other)?
            .bytes()
            .map_err(io::Error::other)?
            .to_vec();
        let _image = decode_source(&response)?;
        Ok(response)
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

fn retryable(error: &reqwest::Error) -> bool {
    error.is_timeout()
        || error.is_connect()
        || error
            .status()
            .is_some_and(|status| status.is_server_error())
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
    image: RgbImage,
}

impl AerialTile {
    pub fn for_isometric_tile(
        source: &AerialSource,
        iso_bounds: Bounds,
        z: u8,
        x: u32,
        y: u32,
    ) -> io::Result<Self> {
        let source_bounds = iso_bounds.ground_source_bounds();
        let overlap = source_bounds.width().max(source_bounds.height()) / SOURCE_SIZE as f32
            * SOURCE_OVERLAP_PIXELS;
        source.tile(source_bounds.pad(overlap), z, x, y)
    }

    pub fn sample(&self, x: f32, y: f32, mode: TextureMode, block_size: f32) -> [u8; 3] {
        let (x, y) = match mode {
            TextureMode::Pixel => (
                (x / block_size)
                    .floor()
                    .mul_add(block_size, block_size * 0.5),
                (y / block_size)
                    .floor()
                    .mul_add(block_size, block_size * 0.5),
            ),
            TextureMode::Full | TextureMode::None => (x, y),
        };
        let u = ((x - self.bounds.min_x) / self.bounds.width())
            .clamp(0.0, 1.0)
            .mul_add(self.image.width().saturating_sub(1) as f32, 0.0);
        let v = ((self.bounds.max_y - y) / self.bounds.height())
            .clamp(0.0, 1.0)
            .mul_add(self.image.height().saturating_sub(1) as f32, 0.0);
        let sampled = match mode {
            TextureMode::Full | TextureMode::None => self.bilinear(u, v),
            TextureMode::Pixel => self.box_average(u, v),
        };
        if mode == TextureMode::Pixel {
            sampled.map(posterize)
        } else {
            sampled
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.bounds.min_x
            && x <= self.bounds.max_x
            && y >= self.bounds.min_y
            && y <= self.bounds.max_y
    }

    fn bilinear(&self, u: f32, v: f32) -> [u8; 3] {
        let x0 = u.floor() as u32;
        let y0 = v.floor() as u32;
        let x1 = (x0 + 1).min(self.image.width() - 1);
        let y1 = (y0 + 1).min(self.image.height() - 1);
        let tx = u - x0 as f32;
        let ty = v - y0 as f32;
        let top = mix_pixel(
            *self.image.get_pixel(x0, y0),
            *self.image.get_pixel(x1, y0),
            tx,
        );
        let bottom = mix_pixel(
            *self.image.get_pixel(x0, y1),
            *self.image.get_pixel(x1, y1),
            tx,
        );
        mix_rgb(top, bottom, ty)
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

fn mix_pixel(left: image::Rgb<u8>, right: image::Rgb<u8>, amount: f32) -> [u8; 3] {
    mix_rgb(left.0, right.0, amount)
}

fn mix_rgb(left: [u8; 3], right: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|index| {
        (f32::from(left[index]) * (1.0 - amount) + f32::from(right[index]) * amount).round() as u8
    })
}

fn posterize(channel: u8) -> u8 {
    ((u16::from(channel) + 16) / 32 * 32).min(255) as u8
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{AerialSource, SOURCE_CACHE_MAX_BYTES, TextureMode, posterize};

    #[test]
    fn modes_have_stable_cache_slugs() {
        assert_eq!(TextureMode::None.slug(), "none");
        assert_eq!(TextureMode::Full.slug(), "full");
        assert_eq!(TextureMode::Pixel.slug(), "pixel");
    }

    #[test]
    fn posterize_uses_fixed_thirty_two_value_steps() {
        assert_eq!(posterize(0), 0);
        assert_eq!(posterize(47), 32);
        assert_eq!(posterize(250), 255);
    }

    #[test]
    fn source_cache_has_a_fixed_one_gibibyte_limit() -> std::io::Result<()> {
        assert_eq!(SOURCE_CACHE_MAX_BYTES, 1_073_741_824);
        let source = AerialSource::open("target/nonexistent-aerial-test-cache")?;
        source
            .cached_bytes
            .store(SOURCE_CACHE_MAX_BYTES - 1, Ordering::Relaxed);
        assert!(source.reserve_cache_bytes(1));
        assert!(!source.reserve_cache_bytes(1));
        Ok(())
    }
}
