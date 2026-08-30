use std::{collections::HashMap, io};

use rstar::AABB;
use tiny_skia::{Color, Pixmap};

use crate::{
    building_render::draw_city_buildings,
    mesh_render::draw_textured_faces,
    mesh_texture::MeshTextureSource,
    projection::Projection,
    texture::AerialTile,
    tile_codec::encode_rgba,
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
    let mut depth = vec![f32::NEG_INFINITY; (TILE_SIZE * TILE_SIZE) as usize];
    let margin = 1.0 / scale;
    let query = AABB::from_corners(
        [bounds.min_x - margin, bounds.min_y - margin],
        [bounds.max_x + margin, bounds.max_y + margin],
    );
    draw_city_buildings(
        &mut pixmap,
        world
            .building_iso_tree
            .locate_in_envelope_intersecting(&query)
            .filter_map(|item| {
                let building = &world.buildings[item.index];
                (!world.building_covered_by_mesh[item.index]).then_some(building)
            }),
        &projection,
        aerial,
        block_size(bounds),
        &mut depth,
    );
    draw_textured_faces(
        &mut pixmap,
        world
            .mesh_face_tree
            .locate_in_envelope_intersecting(&query)
            .map(|item| &world.mesh_faces[item.index]),
        &projection,
        mesh_textures,
        &mut depth,
    )?;
    encode_rgba(pixmap.data(), TILE_SIZE, TILE_SIZE)
}

pub fn render_blank_tile() -> io::Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    pixmap.fill(ground());
    encode_rgba(pixmap.data(), TILE_SIZE, TILE_SIZE)
}

fn ground() -> Color {
    Color::from_rgba8(217, 209, 195, 255)
}

fn draw_ground(pixmap: &mut Pixmap, bounds: Bounds, scale: f32, aerial: &AerialTile) {
    let fallback = [217_u8, 209, 195];
    let block_size = block_size(bounds);
    let mut colors = HashMap::new();
    for py in 0..TILE_SIZE {
        for px in 0..TILE_SIZE {
            let iso_x = (f32::from(px as u16) + 0.5).mul_add(1.0 / scale, bounds.min_x);
            let iso_y = (f32::from(py as u16) + 0.5).mul_add(1.0 / scale, bounds.min_y);
            let (source_x, source_y) = inverse_isometric(iso_x, iso_y);
            let key = (
                (source_x / block_size).floor() as i32,
                (source_y / block_size).floor() as i32,
            );
            let color = *colors.entry(key).or_insert_with(|| {
                let sampled = aerial.sample(source_x, source_y, block_size);
                if missing_imagery(sampled) {
                    fallback
                } else {
                    mix_rgb(fallback, sampled.unwrap_or(fallback), 0.9)
                }
            });
            let offset = ((py * TILE_SIZE + px) * 4) as usize;
            pixmap.data_mut()[offset..offset + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

fn block_size(bounds: Bounds) -> f32 {
    bounds.width() / 128.0
}

fn missing_imagery(color: Option<[u8; 3]>) -> bool {
    color.is_none_or(|color| color.iter().all(|channel| *channel >= 246))
}

fn mix_rgb(left: [u8; 3], right: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|index| {
        (f32::from(left[index]) * (1.0 - amount) + f32::from(right[index]) * amount).round() as u8
    })
}

#[cfg(test)]
mod tests {
    use super::block_size;
    use crate::world::Bounds;

    #[test]
    fn aerial_blocks_are_two_output_pixels_wide() {
        let bounds = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 256.0,
            max_y: 256.0,
        };

        assert_eq!(block_size(bounds), 2.0);
    }
}
