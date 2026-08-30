use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use image::{Rgba, RgbaImage, imageops::FilterType};
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::{
    mesh_texture::MeshTextureSource,
    render::render_tile,
    texture::{AerialSource, AerialTile},
    tile_codec::{EXTENSION, encode_image},
    world::World,
};

pub const ART_ZOOM: u8 = 8;
const TILE_SIZE: u32 = 256;
const COMPLETE_FILE: &str = ".complete";
const INVENTORY_FILE: &str = ".inventory";
const GROUND: Rgba<u8> = Rgba([217, 209, 195, 255]);

pub fn build(
    world: &World,
    aerial: &AerialSource,
    mesh_textures: &MeshTextureSource,
    root: &Path,
) -> io::Result<()> {
    fs::create_dir_all(root)?;
    fs::remove_file(root.join(COMPLETE_FILE)).or_else(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        }
    })?;
    let mut changed = render_leaves(world, aerial, mesh_textures, root)?;
    for z in (0..ART_ZOOM).rev() {
        changed = derive_level(root, z, &changed)?;
        println!("built z{z} from z{}: {} written", z + 1, changed.len());
    }
    write_inventory(root)?;
    fs::write(root.join(COMPLETE_FILE), b"complete\n")?;
    println!("tile pyramid complete");
    Ok(())
}

pub struct TileInventory {
    entries: HashMap<(u8, u32, u32), TileFingerprint>,
}

struct TileFingerprint {
    bytes: u64,
    sha256: [u8; 32],
}

impl TileInventory {
    pub fn tile_keys(&self) -> Vec<String> {
        let mut entries: Vec<_> = self.entries.keys().copied().collect();
        entries.sort_unstable();
        entries
            .into_iter()
            .map(|(z, x, y)| format!("{z}/{x}/{y}"))
            .collect()
    }

    pub fn expected_bytes(&self, z: u8, x: u32, y: u32) -> Option<u64> {
        self.entries.get(&(z, x, y)).map(|entry| entry.bytes)
    }

    pub fn matches(&self, z: u8, x: u32, y: u32, bytes: &[u8]) -> bool {
        self.entries.get(&(z, x, y)).is_some_and(|expected| {
            expected.bytes == bytes.len() as u64
                && expected.sha256 == <[u8; 32]>::from(Sha256::digest(bytes))
        })
    }
}

pub fn read_inventory(root: &Path) -> io::Result<TileInventory> {
    if !root.join(COMPLETE_FILE).is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "tile pyramid is incomplete",
        ));
    }
    let contents = fs::read_to_string(root.join(INVENTORY_FILE))?;
    let mut entries = HashMap::new();
    for line in contents.lines() {
        let values: Vec<_> = line.split('/').collect();
        let [z, x, y, size, digest] = values.as_slice() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid tile inventory",
            ));
        };
        let z = z
            .parse::<u8>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid inventory zoom"))?;
        let x = x
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid inventory x"))?;
        let y = y
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid inventory y"))?;
        let size = size
            .parse::<u64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid inventory size"))?;
        if z > ART_ZOOM || x >= 1 << z || y >= 1 << z || size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tile inventory entry is out of range",
            ));
        }
        if entries
            .insert(
                (z, x, y),
                TileFingerprint {
                    bytes: size,
                    sha256: parse_digest(digest)?,
                },
            )
            .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tile inventory contains a duplicate",
            ));
        }
    }
    if entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tile inventory is empty",
        ));
    }
    Ok(TileInventory { entries })
}

pub fn validate_complete(root: &Path) -> io::Result<()> {
    let inventory = read_inventory(root)?;
    inventory
        .entries
        .par_iter()
        .try_for_each(|(&(z, x, y), expected)| {
            let bytes = fs::read(tile_path(root, z, x, y))?;
            if expected.bytes != bytes.len() as u64
                || expected.sha256 != <[u8; 32]>::from(Sha256::digest(&bytes))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("tile does not match inventory: {z}/{x}/{y}"),
                ));
            }
            Ok(())
        })
}

fn parse_digest(value: &str) -> io::Result<[u8; 32]> {
    if value.len() != 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid inventory digest",
        ));
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid inventory digest"))?;
    }
    Ok(digest)
}

pub fn tile_path(root: &Path, z: u8, x: u32, y: u32) -> PathBuf {
    root.join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.{EXTENSION}"))
}

fn render_leaves(
    world: &World,
    aerial: &AerialSource,
    mesh_textures: &MeshTextureSource,
    root: &Path,
) -> io::Result<Vec<u32>> {
    let count = 1_u32 << ART_ZOOM;
    let mut tiles: Vec<u32> = (0..count * count)
        .into_par_iter()
        .filter(|index| {
            let bounds = world
                .iso_bounds
                .tile(ART_ZOOM, index % count, index / count);
            world.has_content(&world.source_envelope(bounds))
        })
        .collect();
    tiles.par_sort_unstable_by_key(|index| morton_code(index % count, index / count));
    println!("z{ART_ZOOM} contains {} tiles", tiles.len());
    let started = Instant::now();
    let rendered = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let builder = LeafBuilder {
        world,
        aerial,
        mesh_textures,
        root,
        count,
        rendered: &rendered,
        skipped: &skipped,
        started,
    };
    let changed = tiles
        .into_par_iter()
        .map(|index| {
            builder
                .render(index)
                .map(|written| written.then_some(index))
        })
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    println!(
        "built z{ART_ZOOM}: {} rendered, {} reused",
        rendered.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed)
    );
    Ok(changed)
}

fn morton_code(x: u32, y: u32) -> u32 {
    let mut code = 0;
    for bit in 0..16 {
        code |= ((x >> bit) & 1) << (bit * 2);
        code |= ((y >> bit) & 1) << (bit * 2 + 1);
    }
    code
}

struct LeafBuilder<'a> {
    world: &'a World,
    aerial: &'a AerialSource,
    mesh_textures: &'a MeshTextureSource,
    root: &'a Path,
    count: u32,
    rendered: &'a AtomicUsize,
    skipped: &'a AtomicUsize,
    started: Instant,
}

impl LeafBuilder<'_> {
    fn render(&self, index: u32) -> io::Result<bool> {
        let x = index % self.count;
        let y = index / self.count;
        let path = tile_path(self.root, ART_ZOOM, x, y);
        if valid_tile(&path)? {
            self.skipped.fetch_add(1, Ordering::Relaxed);
            return Ok(false);
        }
        let bounds = self.world.iso_bounds.tile(ART_ZOOM, x, y);
        let aerial =
            AerialTile::for_source_bounds(self.aerial, self.world.aerial_source_bounds(bounds))?;
        let image = render_tile(self.world, &aerial, self.mesh_textures, ART_ZOOM, x, y)?;
        write_atomic(&path, &image)?;
        let done = self.rendered.fetch_add(1, Ordering::Relaxed) + 1;
        if done.is_multiple_of(128) {
            let seconds = self.started.elapsed().as_secs_f64().max(0.001);
            println!(
                "rendered {done} new z{ART_ZOOM} tiles ({:.1}/s)",
                done as f64 / seconds
            );
        }
        Ok(true)
    }
}

fn derive_level(root: &Path, z: u8, dirty_children: &[u32]) -> io::Result<Vec<u32>> {
    let count = 1_u32 << z;
    let child_count = count * 2;
    let dirty: HashSet<u32> = dirty_children
        .iter()
        .map(|index| {
            let child_x = index % child_count;
            let child_y = index / child_count;
            child_y.div_euclid(2) * count + child_x.div_euclid(2)
        })
        .collect();
    let candidates: Vec<u32> = (0..count * count)
        .into_par_iter()
        .map(|index| {
            if dirty.contains(&index)
                || !valid_tile(&tile_path(root, z, index % count, index / count))?
            {
                Ok(Some(index))
            } else {
                Ok(None)
            }
        })
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let results: Vec<(u32, io::Result<bool>)> = candidates
        .into_par_iter()
        .map(|index| (index, derive_parent(root, z, index % count, index / count)))
        .collect();
    results
        .into_iter()
        .filter_map(|(index, result)| match result {
            Ok(true) => Some(Ok(index)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn derive_parent(root: &Path, z: u8, x: u32, y: u32) -> io::Result<bool> {
    let mut canvas = RgbaImage::from_pixel(TILE_SIZE * 2, TILE_SIZE * 2, GROUND);
    let mut found = false;
    for child_y in 0..2 {
        for child_x in 0..2 {
            let path = tile_path(root, z + 1, x * 2 + child_x, y * 2 + child_y);
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP)
                .map_err(io::Error::other)?
                .into_rgba8();
            if image.dimensions() != (TILE_SIZE, TILE_SIZE) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tile pyramid child has an unexpected image size",
                ));
            }
            image::imageops::replace(
                &mut canvas,
                &image,
                i64::from(child_x * TILE_SIZE),
                i64::from(child_y * TILE_SIZE),
            );
            found = true;
        }
    }
    if !found {
        return Ok(false);
    }
    let parent = image::imageops::resize(&canvas, TILE_SIZE, TILE_SIZE, FilterType::Triangle);
    let bytes = encode_image(&parent)?;
    write_atomic(&tile_path(root, z, x, y), &bytes)?;
    Ok(true)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tile path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("{EXTENSION}.part"));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

fn valid_tile(path: &Path) -> io::Result<bool> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Ok(image) = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP) else {
        return Ok(false);
    };
    Ok(image.width() == TILE_SIZE && image.height() == TILE_SIZE)
}

fn write_inventory(root: &Path) -> io::Result<()> {
    let mut contents = String::new();
    for z in 0..=ART_ZOOM {
        let count = 1_u32 << z;
        for y in 0..count {
            for x in 0..count {
                let path = tile_path(root, z, x, y);
                let bytes = match fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                let digest = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                contents.push_str(&format!("{z}/{x}/{y}/{}/{digest}\n", bytes.len()));
            }
        }
    }
    if contents.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cannot complete an empty tile pyramid",
        ));
    }
    let path = root.join(INVENTORY_FILE);
    let temporary = root.join(".inventory.part");
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use image::{Rgba, RgbaImage};

    use super::{
        COMPLETE_FILE, derive_level, derive_parent, read_inventory, tile_path, valid_tile,
        validate_complete, write_atomic, write_inventory,
    };
    use crate::tile_codec::encode_image;

    #[test]
    fn parent_combines_available_children_and_fills_missing_quadrants()
    -> Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("isophilly-pyramid-test-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let child = RgbaImage::from_pixel(256, 256, Rgba([255, 0, 0, 255]));
        let path = tile_path(&root, 1, 0, 0);
        let Some(parent_dir) = path.parent() else {
            return Err("child has no parent directory".into());
        };
        fs::create_dir_all(parent_dir)?;
        write_atomic(&path, &encode_image(&child)?)?;

        assert!(derive_parent(&root, 0, 0, 0)?);
        let parent = image::open(tile_path(&root, 0, 0, 0))?.into_rgba8();
        assert_eq!(parent.get_pixel(32, 32).0, [255, 0, 0, 255]);
        assert_eq!(parent.get_pixel(224, 224).0, [217, 209, 195, 255]);

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn dirty_child_rebuilds_only_its_existing_parent() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "isophilly-pyramid-dirty-test-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let path = tile_path(&root, 1, 0, 0);
        let Some(parent_dir) = path.parent() else {
            return Err("child has no parent directory".into());
        };
        fs::create_dir_all(parent_dir)?;
        write_atomic(
            &path,
            &encode_image(&RgbaImage::from_pixel(256, 256, Rgba([255, 0, 0, 255])))?,
        )?;
        assert_eq!(derive_level(&root, 0, &[])?, vec![0]);

        write_atomic(
            &path,
            &encode_image(&RgbaImage::from_pixel(256, 256, Rgba([0, 255, 0, 255])))?,
        )?;
        assert_eq!(derive_level(&root, 0, &[0])?, vec![0]);
        let parent = image::open(tile_path(&root, 0, 0, 0))?.into_rgba8();
        assert_eq!(parent.get_pixel(32, 32).0, [0, 255, 0, 255]);

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn inventory_distinguishes_expected_tiles_from_empty_space()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "isophilly-pyramid-inventory-test-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let path = tile_path(&root, 0, 0, 0);
        write_atomic(
            &path,
            &encode_image(&RgbaImage::from_pixel(256, 256, Rgba([1, 2, 3, 255])))?,
        )?;
        assert!(valid_tile(&path)?);
        write_inventory(&root)?;
        fs::write(root.join(COMPLETE_FILE), b"complete\n")?;

        let inventory = read_inventory(&root)?;
        assert_eq!(
            inventory.expected_bytes(0, 0, 0),
            Some(fs::metadata(path)?.len())
        );
        validate_complete(&root)?;
        let path = tile_path(&root, 0, 0, 0);
        let size = fs::metadata(&path)?.len() as usize;
        fs::write(path, vec![0_u8; size])?;
        assert!(validate_complete(&root).is_err());
        assert_eq!(inventory.expected_bytes(1, 0, 0), None);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
