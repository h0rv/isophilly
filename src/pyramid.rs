use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use image::{Rgba, RgbaImage, imageops::FilterType};
use rayon::prelude::*;

use crate::{
    render::render_tile,
    texture::{AerialSource, AerialTile},
    world::World,
};

pub const ART_ZOOM: u8 = 8;
const TILE_SIZE: u32 = 256;
const COMPLETE_FILE: &str = ".complete";
const GROUND: Rgba<u8> = Rgba([217, 209, 195, 255]);

pub fn build(world: &World, aerial: &AerialSource, root: &Path) -> io::Result<()> {
    if is_complete(root) {
        println!("tile pyramid is already complete");
        return Ok(());
    }
    fs::create_dir_all(root)?;
    render_leaves(world, aerial, root)?;
    for z in (0..ART_ZOOM).rev() {
        derive_level(root, z)?;
        println!("built z{z} from z{}", z + 1);
    }
    fs::write(root.join(COMPLETE_FILE), b"complete\n")?;
    println!("tile pyramid complete");
    Ok(())
}

pub fn is_complete(root: &Path) -> bool {
    root.join(COMPLETE_FILE).is_file()
}

pub fn tile_path(root: &Path, z: u8, x: u32, y: u32) -> PathBuf {
    root.join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"))
}

fn render_leaves(world: &World, aerial: &AerialSource, root: &Path) -> io::Result<()> {
    let count = 1_u32 << ART_ZOOM;
    let tiles: Vec<u32> = (0..count * count)
        .into_par_iter()
        .filter(|index| {
            let bounds = world
                .iso_bounds
                .tile(ART_ZOOM, index % count, index / count);
            world.has_content(&world.source_envelope(bounds))
        })
        .collect();
    println!("z{ART_ZOOM} contains {} tiles", tiles.len());
    let rendered = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let mut pending = tiles;
    for pass in 1..=3 {
        let results: Vec<(u32, io::Result<()>)> = pending
            .into_par_iter()
            .map(|index| {
                (
                    index,
                    render_leaf(world, aerial, root, count, index, &rendered, &skipped),
                )
            })
            .collect();
        let mut first_error = None;
        pending = results
            .into_iter()
            .filter_map(|(index, result)| match result {
                Ok(()) => None,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    Some(index)
                }
            })
            .collect();
        if pending.is_empty() {
            break;
        }
        println!(
            "retrying {} failed z{ART_ZOOM} tiles after pass {pass}",
            pending.len()
        );
        if pass == 3 {
            return Err(first_error.unwrap_or_else(|| io::Error::other("tile build failed")));
        }
    }
    println!(
        "built z{ART_ZOOM}: {} rendered, {} reused",
        rendered.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed)
    );
    Ok(())
}

fn render_leaf(
    world: &World,
    aerial: &AerialSource,
    root: &Path,
    count: u32,
    index: u32,
    rendered: &AtomicUsize,
    skipped: &AtomicUsize,
) -> io::Result<()> {
    let x = index % count;
    let y = index / count;
    let path = tile_path(root, ART_ZOOM, x, y);
    if path.is_file() {
        skipped.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    let bounds = world.iso_bounds.tile(ART_ZOOM, x, y);
    let aerial = AerialTile::for_isometric_tile(aerial, bounds, ART_ZOOM, x, y)?;
    let png = render_tile(world, &aerial, ART_ZOOM, x, y)?;
    write_atomic(&path, &png)?;
    let done = rendered.fetch_add(1, Ordering::Relaxed) + 1;
    if done.is_multiple_of(256) {
        println!("rendered {done} z{ART_ZOOM} tiles");
    }
    Ok(())
}

fn derive_level(root: &Path, z: u8) -> io::Result<()> {
    let count = 1_u32 << z;
    (0..count * count)
        .into_par_iter()
        .try_for_each(|index| derive_parent(root, z, index % count, index / count).map(|_| ()))
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
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
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
    let mut bytes = Vec::new();
    parent
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(io::Error::other)?;
    write_atomic(&tile_path(root, z, x, y), &bytes)?;
    Ok(true)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tile path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("png.part");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use image::{Rgba, RgbaImage};

    use super::{derive_parent, tile_path};

    #[test]
    fn parent_combines_available_children_and_fills_missing_quadrants()
    -> Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("geo-philly-pyramid-test-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let child = RgbaImage::from_pixel(256, 256, Rgba([255, 0, 0, 255]));
        let path = tile_path(&root, 1, 0, 0);
        let Some(parent_dir) = path.parent() else {
            return Err("child has no parent directory".into());
        };
        fs::create_dir_all(parent_dir)?;
        child.save(&path)?;

        assert!(derive_parent(&root, 0, 0, 0)?);
        let parent = image::open(tile_path(&root, 0, 0, 0))?.into_rgba8();
        assert_eq!(parent.get_pixel(32, 32).0, [255, 0, 0, 255]);
        assert_eq!(parent.get_pixel(224, 224).0, [217, 209, 195, 255]);

        fs::remove_dir_all(root)?;
        Ok(())
    }
}
