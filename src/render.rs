use std::io;

use rstar::AABB;
use tiny_skia::{Color, Pixmap};

use crate::{
    mesh_render::draw_textured_faces,
    mesh_texture::MeshTextureSource,
    projection::Projection,
    texture::AerialTile,
    world::{Bounds, World, inverse_isometric},
};

const TILE_SIZE: u32 = 256;

pub fn render_tile(
    world: &World,
    aerial: &AerialTile,
    mesh_textures: &MeshTextureSource,
    z: u8,
    x: u32,
    y: u32,
) -> io::Result<Vec<u8>> {
    let bounds = world.iso_bounds.tile(z, x, y);
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    draw_ground(&mut pixmap, bounds, scale, aerial);

    let projection = Projection { bounds, scale };
    let margin = 1.0 / scale;
    let query = AABB::from_corners(
        [bounds.min_x - margin, bounds.min_y - margin],
        [bounds.max_x + margin, bounds.max_y + margin],
    );
    draw_textured_faces(
        &mut pixmap,
        world
            .mesh_face_tree
            .locate_in_envelope_intersecting(&query)
            .map(|item| &world.mesh_faces[item.index]),
        &projection,
        mesh_textures,
    )?;
    pixmap.encode_png().map_err(io::Error::other)
}

pub fn render_blank_tile() -> io::Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    pixmap.fill(ground());
    pixmap.encode_png().map_err(io::Error::other)
}

fn ground() -> Color {
    Color::from_rgba8(217, 209, 195, 255)
}

fn draw_ground(pixmap: &mut Pixmap, bounds: Bounds, scale: f32, aerial: &AerialTile) {
    let fallback = [217_u8, 209, 195];
    let block_size = bounds.width() / 96.0;
    for py in 0..TILE_SIZE {
        for px in 0..TILE_SIZE {
            let iso_x = (f32::from(px as u16) + 0.5).mul_add(1.0 / scale, bounds.min_x);
            let iso_y = (f32::from(py as u16) + 0.5).mul_add(1.0 / scale, bounds.min_y);
            let (source_x, source_y) = inverse_isometric(iso_x, iso_y);
            let sampled = aerial.sample(source_x, source_y, block_size);
            let color = if missing_imagery(sampled) {
                fallback
            } else {
                mix_rgb(fallback, sampled, 0.9)
            };
            let offset = ((py * TILE_SIZE + px) * 4) as usize;
            pixmap.data_mut()[offset..offset + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

fn missing_imagery(color: [u8; 3]) -> bool {
    color.iter().all(|channel| *channel >= 246)
}

fn mix_rgb(left: [u8; 3], right: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|index| {
        (f32::from(left[index]) * (1.0 - amount) + f32::from(right[index]) * amount).round() as u8
    })
}
