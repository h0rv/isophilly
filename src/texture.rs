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

const SOURCE_SIZE: u32 = 2048;
const SOURCE_CELL_METERS: f32 = 1536.0;
const SOURCE_PIXEL_METERS: f32 = SOURCE_CELL_METERS / SOURCE_SIZE as f32;
const SOURCE_CACHE_MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DOWNLOAD_LOCKS: usize = 256;
const DOWNLOAD_SLOTS: usize = 8;
const DECODED_CACHE_MAX_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AerialDataset {
    export_url: &'static str,
    image_layer: u8,
    spatial_reference: u32,
    cache_namespace: &'static str,
}

const AERIAL_DATASET: AerialDataset = AerialDataset {
    export_url: "https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer/export",
    image_layer: 3,
    spatial_reference: 32129,
    cache_namespace: "2025-3in-1536m-grid-v1",
};

pub struct AerialSource {
    client: Client,
    dataset: AerialDataset,
    root: PathBuf,
    cached_bytes: AtomicU64,
    temporary_id: AtomicU64,
    download_locks: [Mutex<()>; DOWNLOAD_LOCKS],
    download_slots: [Mutex<()>; DOWNLOAD_SLOTS],
    decoded: Mutex<DecodedCache>,
}

#[derive(Default)]
struct DecodedCache {
    images: HashMap<PathBuf, Arc<RgbImage>>,
    order: VecDeque<PathBuf>,
    bytes: usize,
}

impl AerialSource {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let dataset = AERIAL_DATASET;
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("isophilly/0.1 (public-data texture cache)")
            .build()
            .map_err(io::Error::other)?;
        let root = root.into();
        prune_obsolete_namespaces(&root, dataset.cache_namespace)?;
        let cached_bytes = prune_active_cache(&root.join(dataset.cache_namespace))?;
        Ok(Self {
            client,
            dataset,
            root,
            cached_bytes: AtomicU64::new(cached_bytes),
            temporary_id: AtomicU64::new(0),
            download_locks: std::array::from_fn(|_| Mutex::new(())),
            download_slots: std::array::from_fn(|_| Mutex::new(())),
            decoded: Mutex::new(DecodedCache::default()),
        })
    }

    fn tile(&self, key: (i32, i32)) -> io::Result<Arc<RgbImage>> {
        let (x, y) = key;
        let path = self
            .root
            .join(self.dataset.cache_namespace)
            .join(x.to_string())
            .join(format!("{y}.jpg"));
        let _guard = self.download_locks[download_shard(x, y)]
            .lock()
            .map_err(|_| io::Error::other("aerial download lock poisoned"))?;
        if let Some(image) = self.decoded(&path)? {
            return Ok(image);
        }
        let image = match fs::read(&path) {
            Ok(bytes) => match decode_source(&bytes) {
                Ok(image) => image,
                Err(_) => {
                    fs::remove_file(&path)?;
                    self.cached_bytes
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                            Some(current.saturating_sub(bytes.len() as u64))
                        })
                        .map_err(|_| io::Error::other("aerial cache byte count update failed"))?;
                    let _slot = self.download_slots[download_shard(x, y) % DOWNLOAD_SLOTS]
                        .lock()
                        .map_err(|_| io::Error::other("aerial download slot poisoned"))?;
                    self.download(&path, source_cell_bounds(key))?.1
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let _slot = self.download_slots[download_shard(x, y) % DOWNLOAD_SLOTS]
                    .lock()
                    .map_err(|_| io::Error::other("aerial download slot poisoned"))?;
                self.download(&path, source_cell_bounds(key))?.1
            }
            Err(error) => return Err(error),
        };
        let image = Arc::new(image);
        self.remember(path, Arc::clone(&image))?;
        Ok(image)
    }

    fn decoded(&self, path: &Path) -> io::Result<Option<Arc<RgbImage>>> {
        let mut cache = self
            .decoded
            .lock()
            .map_err(|_| io::Error::other("decoded aerial cache poisoned"))?;
        let image = cache.images.get(path).cloned();
        if image.is_some() {
            cache.order.retain(|cached| cached != path);
            cache.order.push_back(path.to_path_buf());
        }
        Ok(image)
    }

    fn remember(&self, path: PathBuf, image: Arc<RgbImage>) -> io::Result<()> {
        let mut cache = self
            .decoded
            .lock()
            .map_err(|_| io::Error::other("decoded aerial cache poisoned"))?;
        if cache.images.contains_key(&path) {
            return Ok(());
        }
        let bytes = image.as_raw().len();
        while cache.bytes.saturating_add(bytes) > DECODED_CACHE_MAX_BYTES {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            if let Some(removed) = cache.images.remove(&oldest) {
                cache.bytes = cache.bytes.saturating_sub(removed.as_raw().len());
            }
        }
        cache.order.push_back(path.clone());
        cache.bytes = cache.bytes.saturating_add(bytes);
        cache.images.insert(path, image);
        Ok(())
    }

    fn download(&self, path: &Path, bounds: Bounds) -> io::Result<(Vec<u8>, RgbImage)> {
        let (response, image) = self.fetch(bounds)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("aerial cache path has no parent"))?;
        fs::create_dir_all(parent)?;
        let _cached = self.write_cached(path, &response)?;
        Ok((response, image))
    }

    fn fetch(&self, bounds: Bounds) -> io::Result<(Vec<u8>, RgbImage)> {
        let bbox = format!(
            "{},{},{},{}",
            bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y
        );
        let size = format!("{SOURCE_SIZE},{SOURCE_SIZE}");
        let layers = format!("show:{}", self.dataset.image_layer);
        let spatial_reference = self.dataset.spatial_reference.to_string();
        let query = [
            ("bbox", bbox.as_str()),
            ("bboxSR", spatial_reference.as_str()),
            ("size", size.as_str()),
            ("imageSR", spatial_reference.as_str()),
            ("layers", layers.as_str()),
            ("format", "jpg"),
            ("transparent", "false"),
            ("f", "image"),
        ];
        let mut last_error = None;
        for attempt in 0..6 {
            let result = self
                .client
                .get(self.dataset.export_url)
                .query(&query)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .and_then(reqwest::blocking::Response::bytes)
                .map_err(io::Error::other)
                .and_then(|bytes| {
                    let bytes = bytes.to_vec();
                    decode_source(&bytes).map(|image| (bytes, image))
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
            let _removed = fs::remove_file(&temporary);
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
                let _removed = fs::remove_file(&temporary);
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

fn download_shard(x: i32, y: i32) -> usize {
    let hash = (x as u32)
        .wrapping_mul(31)
        .wrapping_add((y as u32).wrapping_mul(17));
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
    images: HashMap<(i32, i32), Arc<RgbImage>>,
}

pub(crate) fn missing_imagery(color: Option<[u8; 3]>) -> bool {
    color.is_none_or(|color| color.iter().all(|channel| *channel >= 246))
}

impl AerialTile {
    pub fn for_source_bounds(source: &AerialSource, bounds: Bounds) -> io::Result<Self> {
        let bounds = bounds.pad(SOURCE_PIXEL_METERS * 2.0);
        let min_x = source_cell(bounds.min_x);
        let max_x = source_cell(bounds.max_x);
        let min_y = source_cell(bounds.min_y);
        let max_y = source_cell(bounds.max_y);
        let mut images = HashMap::new();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                images.insert((x, y), source.tile((x, y))?);
            }
        }
        Ok(Self { images })
    }

    pub fn sample(&self, x: f32, y: f32, block_size: f32) -> Option<[u8; 3]> {
        let x = (x / block_size)
            .floor()
            .mul_add(block_size, block_size * 0.5);
        let y = (y / block_size)
            .floor()
            .mul_add(block_size, block_size * 0.5);
        self.box_average(x, y).map(|color| color.map(posterize))
    }

    fn box_average(&self, x: f32, y: f32) -> Option<[u8; 3]> {
        let center_x = (x / SOURCE_PIXEL_METERS - 0.5).round() as i64;
        let center_y = (y / SOURCE_PIXEL_METERS - 0.5).round() as i64;
        let mut sum = [0_u32; 3];
        for offset_y in -1..=1 {
            for offset_x in -1..=1 {
                let pixel_x = center_x + offset_x;
                let pixel_y = center_y + offset_y;
                let cell_x = pixel_x.div_euclid(i64::from(SOURCE_SIZE));
                let cell_y = pixel_y.div_euclid(i64::from(SOURCE_SIZE));
                let key = (i32::try_from(cell_x).ok()?, i32::try_from(cell_y).ok()?);
                let image = self.images.get(&key)?;
                let image_x = u32::try_from(pixel_x.rem_euclid(i64::from(SOURCE_SIZE))).ok()?;
                let image_y = SOURCE_SIZE
                    - 1
                    - u32::try_from(pixel_y.rem_euclid(i64::from(SOURCE_SIZE))).ok()?;
                let pixel = image.get_pixel(image_x, image_y).0;
                for channel in 0..3 {
                    sum[channel] += u32::from(pixel[channel]);
                }
            }
        }
        Some(sum.map(|channel| (channel / 9) as u8))
    }
}

fn source_cell(value: f32) -> i32 {
    (value / SOURCE_CELL_METERS).floor() as i32
}

fn source_cell_bounds((x, y): (i32, i32)) -> Bounds {
    Bounds {
        min_x: x as f32 * SOURCE_CELL_METERS,
        min_y: y as f32 * SOURCE_CELL_METERS,
        max_x: (x + 1) as f32 * SOURCE_CELL_METERS,
        max_y: (y + 1) as f32 * SOURCE_CELL_METERS,
    }
}

fn prune_active_cache(root: &Path) -> io::Result<u64> {
    let mut files = Vec::new();
    collect_cache_files(root, &mut files)?;
    let mut bytes = files.iter().map(|(_, _, bytes)| bytes).sum::<u64>();
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, path, size) in files {
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains(".part-"))
            || bytes > SOURCE_CACHE_MAX_BYTES
        {
            fs::remove_file(path)?;
            bytes = bytes.saturating_sub(size);
        }
    }
    Ok(bytes)
}

fn collect_cache_files(
    path: &Path,
    files: &mut Vec<(std::time::SystemTime, PathBuf, u64)>,
) -> io::Result<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_cache_files(&entry.path(), files)?;
        } else {
            files.push((metadata.modified()?, entry.path(), metadata.len()));
        }
    }
    Ok(())
}

fn prune_obsolete_namespaces(root: &Path, active_namespace: &str) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.file_name() != active_namespace {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn posterize(channel: u8) -> u8 {
    let compressed = 24 + (u16::from(channel) * 208 / 255);
    ((compressed + 8) / 16 * 16).min(240) as u8
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, atomic::Ordering},
    };

    use image::{Rgb, RgbImage};

    use super::{
        AERIAL_DATASET, AerialSource, AerialTile, DOWNLOAD_LOCKS, SOURCE_CACHE_MAX_BYTES,
        SOURCE_CELL_METERS, SOURCE_PIXEL_METERS, SOURCE_SIZE, download_shard, posterize,
        prune_active_cache, prune_obsolete_namespaces, source_cell_bounds,
    };

    #[test]
    fn active_source_is_2025_imagery_on_the_fixed_state_plane_grid() {
        assert!(
            AERIAL_DATASET
                .export_url
                .contains("PhiladelphiaImagery2025")
        );
        assert_eq!(AERIAL_DATASET.image_layer, 3);
        assert_eq!(AERIAL_DATASET.spatial_reference, 32129);
        assert_eq!(AERIAL_DATASET.cache_namespace, "2025-3in-1536m-grid-v1");
        assert_eq!(SOURCE_CELL_METERS, 1536.0);
        assert_eq!(SOURCE_PIXEL_METERS, 0.75);
    }

    #[test]
    fn posterize_compresses_extremes_into_fixed_sixteen_value_steps() {
        assert_eq!(posterize(0), 32);
        assert_eq!(posterize(47), 64);
        assert_eq!(posterize(250), 224);
    }

    #[test]
    fn source_cache_has_a_fixed_eight_gibibyte_limit() -> std::io::Result<()> {
        assert_eq!(SOURCE_CACHE_MAX_BYTES, 8_589_934_592);
        let source = AerialSource::open("target/nonexistent-aerial-test-cache")?;
        source
            .cached_bytes
            .store(SOURCE_CACHE_MAX_BYTES - 1, Ordering::Relaxed);
        assert!(source.reserve_cache_bytes(1));
        assert!(!source.reserve_cache_bytes(1));
        Ok(())
    }

    #[test]
    fn cache_pruning_keeps_only_the_active_namespace() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "isophilly-aerial-cache-test-{}",
            std::process::id()
        ));
        let old = root.join("old-grid");
        let active = root.join(AERIAL_DATASET.cache_namespace);
        std::fs::create_dir_all(&old)?;
        std::fs::create_dir_all(&active)?;
        std::fs::write(old.join("tile.jpg"), b"old")?;

        prune_obsolete_namespaces(&root, AERIAL_DATASET.cache_namespace)?;

        assert!(!old.exists());
        assert!(active.exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn active_cache_pruning_removes_orphan_parts() -> std::io::Result<()> {
        let root =
            std::env::temp_dir().join(format!("isophilly-aerial-part-test-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let valid = root.join("tile.jpg");
        let orphan = root.join("tile.jpg.part-1-1");
        std::fs::write(&valid, b"valid")?;
        std::fs::write(&orphan, b"orphan")?;

        assert_eq!(prune_active_cache(&root)?, 5);
        assert!(valid.exists());
        assert!(!orphan.exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn sampling_outside_the_downloaded_grid_never_clamps_to_an_edge_pixel() {
        let image = Arc::new(RgbImage::from_pixel(
            SOURCE_SIZE,
            SOURCE_SIZE,
            Rgb([16, 32, 48]),
        ));
        let tile = AerialTile {
            images: HashMap::from([((0, 0), image)]),
        };

        assert_eq!(tile.sample(192.0, 192.0, 1.0), Some([32, 48, 64]));
        assert_eq!(tile.sample(-1.0, 50.0, 1.0), None);
        assert_eq!(tile.sample(50.0, SOURCE_CELL_METERS + 1.0, 1.0), None);
    }

    #[test]
    fn box_filter_crosses_fixed_grid_cells_without_clamping() {
        let tile = AerialTile {
            images: HashMap::from([
                (
                    (0, 0),
                    Arc::new(RgbImage::from_pixel(
                        SOURCE_SIZE,
                        SOURCE_SIZE,
                        Rgb([16, 32, 48]),
                    )),
                ),
                (
                    (1, 0),
                    Arc::new(RgbImage::from_pixel(
                        SOURCE_SIZE,
                        SOURCE_SIZE,
                        Rgb([160, 176, 192]),
                    )),
                ),
            ]),
        };

        let y = SOURCE_CELL_METERS * 0.5;
        assert_eq!(
            tile.sample(SOURCE_CELL_METERS - 1.0, y, 0.25),
            Some([32, 48, 64])
        );
        assert_eq!(
            tile.sample(SOURCE_CELL_METERS, y, 0.25),
            Some([112, 128, 144])
        );
    }

    #[test]
    fn adjacent_source_cells_share_one_exact_pixel_grid() {
        let left = source_cell_bounds((2_138, 185));
        let right = source_cell_bounds((2_139, 185));

        assert_eq!(left.max_x, right.min_x);
        assert_eq!(left.width() / SOURCE_SIZE as f32, SOURCE_PIXEL_METERS);
        assert_eq!(right.width() / SOURCE_SIZE as f32, SOURCE_PIXEL_METERS);
    }

    #[test]
    fn download_locks_keep_the_same_tile_single_flight() -> std::io::Result<()> {
        let first = download_shard(12, 8);

        assert!(first < DOWNLOAD_LOCKS);
        assert_eq!(first, download_shard(12, 8));
        assert_eq!(
            (0..DOWNLOAD_LOCKS)
                .map(|x| download_shard(x as i32, 8))
                .collect::<std::collections::HashSet<_>>()
                .len(),
            DOWNLOAD_LOCKS
        );
        let source = AerialSource::open("target/nonexistent-aerial-lock-test-cache")?;
        let guard = source.download_locks[first]
            .lock()
            .map_err(|_| std::io::Error::other("test download lock poisoned"))?;
        assert!(
            source.download_locks[download_shard(12, 8)]
                .try_lock()
                .is_err()
        );
        drop(guard);
        Ok(())
    }
}
