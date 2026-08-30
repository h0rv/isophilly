use std::{collections::HashMap, io};

use rstar::AABB;
use tiny_skia::{Color, Pixmap};

use crate::{
    building_render::{draw_city_building_parts, draw_city_buildings},
    mesh_render::draw_textured_faces,
    mesh_texture::MeshTextureSource,
    projection::Projection,
    texture::AerialTile,
    tile_codec::encode_rgba,
    world::{Bounds, PRIMARY_MESH_TEXTURE_LIMIT, Ring, View, World},
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
    let sampling_block = block_size(bounds);
    draw_ground(
        &mut pixmap,
        world,
        bounds,
        scale,
        sampling_block,
        aerial,
        View::SouthEast,
    );

    let projection = Projection {
        bounds,
        scale,
        view: View::SouthEast,
    };
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
            .locate_in_envelope_intersecting(query)
            .filter_map(|item| {
                let building = &world.buildings[item.index];
                (!world.building_covered_by_mesh[item.index]
                    && !world.building_detailed_by_parts[item.index])
                    .then_some(building)
            }),
        &projection,
        aerial,
        sampling_block,
        &mut depth,
    );
    draw_city_building_parts(
        &mut pixmap,
        world
            .building_part_iso_tree
            .locate_in_envelope_intersecting(query)
            .filter_map(|item| {
                (!world.building_part_covered_by_mesh[item.index])
                    .then_some(&world.building_parts[item.index])
            }),
        &projection,
        aerial,
        sampling_block,
        &mut depth,
    );
    draw_textured_faces(
        &mut pixmap,
        world
            .mesh_face_tree
            .locate_in_envelope_intersecting(query)
            .map(|item| &world.mesh_faces[item.index]),
        &projection,
        mesh_textures,
        &mut depth,
    )?;
    encode_rgba(pixmap.data(), TILE_SIZE, TILE_SIZE)
}

pub fn render_rich_tile(
    world: &World,
    aerial: &AerialTile,
    mesh_textures: &MeshTextureSource,
    view: View,
    bounds: Bounds,
) -> io::Result<Vec<u8>> {
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    let sampling_block = rich_block_size(bounds);
    draw_ground(
        &mut pixmap,
        world,
        bounds,
        scale,
        sampling_block,
        aerial,
        view,
    );
    let projection = Projection {
        bounds,
        scale,
        view,
    };
    let mut depth = vec![f32::NEG_INFINITY; (TILE_SIZE * TILE_SIZE) as usize];
    let query = bounds.source_envelope_for(world.max_height, view);
    draw_city_buildings(
        &mut pixmap,
        world
            .building_source_tree
            .locate_in_envelope_intersecting(query)
            .filter_map(|item| {
                let building = &world.buildings[item.index];
                (!world.building_covered_by_primary_mesh[item.index]
                    && !world.building_detailed_by_parts[item.index])
                    .then_some(building)
            }),
        &projection,
        aerial,
        sampling_block,
        &mut depth,
    );
    draw_city_building_parts(
        &mut pixmap,
        world
            .building_part_source_tree
            .locate_in_envelope_intersecting(query)
            .filter_map(|item| {
                (!world.building_part_covered_by_primary_mesh[item.index])
                    .then_some(&world.building_parts[item.index])
            }),
        &projection,
        aerial,
        sampling_block,
        &mut depth,
    );
    draw_textured_faces(
        &mut pixmap,
        world
            .mesh_face_source_tree
            .locate_in_envelope_intersecting(query)
            .map(|item| &world.mesh_faces[item.index])
            .filter(|face| face.texture_id < PRIMARY_MESH_TEXTURE_LIMIT),
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

fn draw_ground(
    pixmap: &mut Pixmap,
    world: &World,
    bounds: Bounds,
    scale: f32,
    block_size: f32,
    aerial: &AerialTile,
    view: View,
) {
    let fallback = [217_u8, 209, 195];
    let source_bounds = bounds.ground_source_bounds_for(view);
    let source_query = AABB::from_corners(
        [source_bounds.min_x, source_bounds.min_y],
        [source_bounds.max_x, source_bounds.max_y],
    );
    let water: Vec<_> = world
        .water_tree
        .locate_in_envelope_intersecting(source_query)
        .map(|item| &world.water[item.index])
        .collect();
    let parks: Vec<_> = world
        .park_tree
        .locate_in_envelope_intersecting(source_query)
        .map(|item| &world.parks[item.index])
        .collect();
    let mut colors = HashMap::new();
    for py in 0..TILE_SIZE {
        for px in 0..TILE_SIZE {
            let iso_x = (f32::from(px as u16) + 0.5).mul_add(1.0 / scale, bounds.min_x);
            let iso_y = (f32::from(py as u16) + 0.5).mul_add(1.0 / scale, bounds.min_y);
            let (source_x, source_y) = view.inverse(iso_x, iso_y);
            let key = (
                (source_x / block_size).floor() as i32,
                (source_y / block_size).floor() as i32,
            );
            let color = *colors.entry(key).or_insert_with(|| {
                let sampled = aerial.sample(source_x, source_y, block_size);
                let aerial_color = if missing_imagery(sampled) {
                    fallback
                } else {
                    mix_rgb(fallback, sampled.unwrap_or(fallback), 0.9)
                };
                grade_ground(aerial_color, (source_x, source_y), &water, &parks)
            });
            let offset = ((py * TILE_SIZE + px) * 4) as usize;
            pixmap.data_mut()[offset..offset + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

fn grade_ground(color: [u8; 3], point: (f32, f32), water: &[&Ring], parks: &[&Ring]) -> [u8; 3] {
    if water.iter().any(|ring| ring.contains(point)) {
        return mix_rgb(color, [64, 128, 160], 0.42);
    }
    let vegetation =
        color[1].saturating_add(12) >= color[0] && color[1] > color[2].saturating_add(4);
    if parks.iter().any(|ring| ring.contains(point)) && vegetation {
        return mix_rgb(color, [80, 144, 72], 0.38);
    }
    if vegetation {
        return mix_rgb(color, [80, 136, 72], 0.18);
    }
    color
}

fn block_size(bounds: Bounds) -> f32 {
    bounds.width() / 128.0
}

fn rich_block_size(bounds: Bounds) -> f32 {
    bounds.width() / TILE_SIZE as f32
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
    use super::{block_size, grade_ground, rich_block_size};
    use crate::world::{Bounds, Ring};

    fn square() -> Ring {
        Ring {
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 10.0,
                max_y: 10.0,
            },
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
        }
    }

    #[test]
    fn aerial_blocks_are_two_output_pixels_wide() {
        let bounds = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 256.0,
            max_y: 256.0,
        };

        assert_eq!(block_size(bounds), 2.0);
        assert_eq!(rich_block_size(bounds), 1.0);
    }

    #[test]
    fn water_mask_shifts_aerial_color_toward_blue() {
        let water = square();
        let source = [112, 104, 88];
        let result = grade_ground(source, (5.0, 5.0), &[&water], &[]);

        assert!(result[2] > source[2]);
        assert!(result[2] > result[0]);
    }

    #[test]
    fn park_mask_shifts_only_vegetation_toward_green() {
        let park = square();
        let vegetation = grade_ground([112, 112, 80], (5.0, 5.0), &[], &[&park]);
        let pavement = grade_ground([144, 128, 120], (5.0, 5.0), &[], &[&park]);

        assert!(vegetation[1] > vegetation[0]);
        assert_eq!(pavement, [144, 128, 120]);
    }
}
