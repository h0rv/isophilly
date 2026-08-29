use std::{
    collections::{HashMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use image::{RgbImage, imageops::FilterType};
use sha2::{Digest, Sha256};

const MAX_TEXTURE_EDGE: u32 = 1024;
const DECODED_TEXTURES: usize = 96;
const LOAD_LOCKS: usize = 64;

pub struct MeshTextureSource {
    root: PathBuf,
    decoded: Mutex<DecodedCache>,
    load_locks: [Mutex<()>; LOAD_LOCKS],
}

#[derive(Default)]
struct DecodedCache {
    images: HashMap<u32, Arc<RgbImage>>,
    order: VecDeque<u32>,
}

impl MeshTextureSource {
    pub fn open(
        root: impl Into<PathBuf>,
        texture_ids: &[u32],
        expected_sha256: [u8; 32],
    ) -> io::Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the building texture atlases are missing; run `uv run --locked poe ingest`",
            ));
        }
        verify_digest(&root, texture_ids, expected_sha256)?;
        Ok(Self {
            root,
            decoded: Mutex::new(DecodedCache::default()),
            load_locks: std::array::from_fn(|_| Mutex::new(())),
        })
    }

    pub fn load(&self, texture_id: u32) -> io::Result<Arc<RgbImage>> {
        if let Some(image) = self.decoded(texture_id)? {
            return Ok(image);
        }
        let _load = self.load_locks[texture_id as usize % LOAD_LOCKS]
            .lock()
            .map_err(|_| io::Error::other("mesh texture load lock poisoned"))?;
        if let Some(image) = self.decoded(texture_id)? {
            return Ok(image);
        }

        let bytes = fs::read(self.root.join(format!("{texture_id}.jpg")))?;
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
            .map_err(io::Error::other)?
            .into_rgb8();
        let scale = (MAX_TEXTURE_EDGE as f32 / image.width().max(image.height()) as f32).min(1.0);
        let image = if scale < 1.0 {
            image::imageops::resize(
                &image,
                (image.width() as f32 * scale).round().max(1.0) as u32,
                (image.height() as f32 * scale).round().max(1.0) as u32,
                FilterType::Triangle,
            )
        } else {
            image
        };
        let image = Arc::new(image);
        self.remember(texture_id, Arc::clone(&image))?;
        Ok(image)
    }

    fn decoded(&self, texture_id: u32) -> io::Result<Option<Arc<RgbImage>>> {
        let cache = self
            .decoded
            .lock()
            .map_err(|_| io::Error::other("mesh texture cache poisoned"))?;
        Ok(cache.images.get(&texture_id).cloned())
    }

    fn remember(&self, texture_id: u32, image: Arc<RgbImage>) -> io::Result<()> {
        let mut cache = self
            .decoded
            .lock()
            .map_err(|_| io::Error::other("mesh texture cache poisoned"))?;
        if cache.images.contains_key(&texture_id) {
            return Ok(());
        }
        while cache.images.len() >= DECODED_TEXTURES {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            cache.images.remove(&oldest);
        }
        cache.order.push_back(texture_id);
        cache.images.insert(texture_id, image);
        Ok(())
    }
}

fn verify_digest(root: &Path, texture_ids: &[u32], expected: [u8; 32]) -> io::Result<()> {
    let mut digest = Sha256::new();
    for texture_id in texture_ids {
        digest.update(texture_id.to_le_bytes());
        digest.update(fs::read(root.join(format!("{texture_id}.jpg")))?);
    }
    if digest.finalize().as_slice() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the building texture atlases do not match philly.bin; rerun `uv run --locked poe ingest`",
        ));
    }
    Ok(())
}
