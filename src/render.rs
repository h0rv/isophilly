use std::io;

use rstar::AABB;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use crate::{
    building_render::{RenderContext, building_color, draw_building, draw_building_part},
    texture::{AerialTile, TextureMode},
    world::{Bounds, Building, BuildingPart, Ring, Street, World, inverse_isometric, isometric},
};

const TILE_SIZE: u32 = 256;
const OVERVIEW_ZOOM: u8 = 3;
const EXTRUSION_ZOOM: u8 = 5;
const OVERVIEW_LIMIT: usize = 60_000;

enum Structure<'a> {
    Building(usize, &'a Building),
    Part(&'a BuildingPart),
}

impl Structure<'_> {
    fn depth(&self) -> f32 {
        let ring = match self {
            Self::Building(_, building) => &building.ring,
            Self::Part(part) => &part.ring,
        };
        ring.points
            .iter()
            .map(|(x, y)| x + y)
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn stable_id(&self) -> u64 {
        match self {
            Self::Building(index, _) => *index as u64,
            Self::Part(part) => part.osm_id | (1_u64 << 63),
        }
    }
}

pub fn render_tile(
    world: &World,
    aerial: Option<&AerialTile>,
    texture: TextureMode,
    z: u8,
    x: u32,
    y: u32,
) -> io::Result<Vec<u8>> {
    let bounds = world.iso_bounds.tile(z, x, y);
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    let block_size = bounds.width() / 96.0;
    draw_ground(&mut pixmap, bounds, scale, aerial, texture, block_size, z);
    let query = world.source_envelope(bounds);
    draw_rings(
        &mut pixmap,
        &world.water,
        &world.water_tree,
        query,
        bounds,
        scale,
        thematic_color(93, 132, 144, texture),
    );
    draw_rings(
        &mut pixmap,
        &world.parks,
        &world.park_tree,
        query,
        bounds,
        scale,
        thematic_color(119, 146, 103, texture),
    );
    draw_streets(&mut pixmap, world, query, bounds, scale, z);
    let mut buildings: Vec<(usize, &Building)> = world
        .building_tree
        .locate_in_envelope_intersecting(&query)
        .map(|item| (item.index, &world.buildings[item.index]))
        .collect();
    if z <= OVERVIEW_ZOOM {
        let step = (buildings.len() / OVERVIEW_LIMIT).max(1);
        for (_, building) in buildings.iter().step_by(step) {
            let (px, py) = pixel(
                isometric(building.center.0, building.center.1, building.height),
                bounds,
                scale,
            );
            let dot = Rect::from_xywh(px - 1.0, py - 1.0, 2.0, 2.0)
                .ok_or_else(|| io::Error::other("overview dot"))?;
            pixmap.fill_rect(
                dot,
                &paint(building_color(
                    &building.ring,
                    building.center,
                    building.height,
                    None,
                    texture,
                    block_size,
                )),
                Transform::identity(),
                None,
            );
        }
    } else if z < EXTRUSION_ZOOM {
        for (_, building) in buildings {
            fill_projected_building(
                &mut pixmap,
                &building.ring,
                building.height,
                bounds,
                scale,
                building_color(
                    &building.ring,
                    building.center,
                    building.height,
                    None,
                    texture,
                    block_size,
                ),
                texture,
            );
        }
    } else {
        let mut structures: Vec<Structure<'_>> = buildings
            .drain(..)
            .filter(|(index, _)| !world.detailed_buildings[*index])
            .map(|(index, building)| Structure::Building(index, building))
            .chain(
                world
                    .building_part_tree
                    .locate_in_envelope_intersecting(&query)
                    .map(|item| Structure::Part(&world.building_parts[item.index])),
            )
            .collect();
        structures.sort_by(|left, right| {
            left.depth()
                .total_cmp(&right.depth())
                .then_with(|| left.stable_id().cmp(&right.stable_id()))
        });
        let mut context = RenderContext::new(aerial, texture, block_size, z, bounds, scale);
        for structure in structures {
            match structure {
                Structure::Building(_, building) => {
                    draw_building(&mut pixmap, building, &mut context);
                }
                Structure::Part(part) => draw_building_part(&mut pixmap, part, &mut context),
            }
        }
    }
    pixmap.encode_png().map_err(io::Error::other)
}

fn draw_streets(
    pixmap: &mut Pixmap,
    world: &World,
    query: AABB<[f32; 2]>,
    bounds: Bounds,
    scale: f32,
    zoom: u8,
) {
    for item in world.street_tree.locate_in_envelope_intersecting(&query) {
        let street = &world.streets[item.index];
        if street_visible(street, zoom) {
            stroke_street(pixmap, street, bounds, scale);
        }
    }
}

fn street_visible(street: &Street, zoom: u8) -> bool {
    let major = matches!(street.class, 1 | 2 | 9 | 10);
    match zoom {
        0..=2 => false,
        3 => major,
        4 => major || street.class == 3,
        _ => true,
    }
}

fn stroke_street(pixmap: &mut Pixmap, street: &Street, bounds: Bounds, scale: f32) {
    let mut path = PathBuilder::new();
    let first = pixel(
        isometric(street.points[0].0, street.points[0].1, 0.0),
        bounds,
        scale,
    );
    path.move_to(first.0, first.1);
    for &(x, y) in &street.points[1..] {
        let point = pixel(isometric(x, y, 0.0), bounds, scale);
        path.line_to(point.0, point.1);
    }
    let Some(path) = path.finish() else {
        return;
    };
    let width = match street.class {
        1 => 3.2,
        2 => 2.4,
        3 => 1.7,
        9 | 10 => 1.4,
        _ => 0.85,
    };
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(
        &path,
        &paint(Color::from_rgba8(190, 178, 159, 255)),
        &stroke,
        Transform::identity(),
        None,
    );
}

pub fn render_blank_tile() -> io::Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    pixmap.fill(ground());
    pixmap.encode_png().map_err(io::Error::other)
}

fn ground() -> Color {
    Color::from_rgba8(217, 209, 195, 255)
}

fn draw_rings(
    pixmap: &mut Pixmap,
    rings: &[Ring],
    tree: &rstar::RTree<crate::world::Indexed>,
    query: AABB<[f32; 2]>,
    bounds: Bounds,
    scale: f32,
    color: Color,
) {
    for item in tree.locate_in_envelope_intersecting(&query) {
        fill_projected_ring(pixmap, &rings[item.index], 0.0, bounds, scale, color);
    }
}
fn fill_projected_ring(
    pixmap: &mut Pixmap,
    ring: &Ring,
    height: f32,
    bounds: Bounds,
    scale: f32,
    color: Color,
) {
    fill_points(pixmap, &projected(ring, height, bounds, scale), color);
}
fn fill_projected_building(
    pixmap: &mut Pixmap,
    ring: &Ring,
    height: f32,
    bounds: Bounds,
    scale: f32,
    color: Color,
    texture: TextureMode,
) {
    let mut points = projected(ring, height, bounds, scale);
    if texture == TextureMode::Pixel {
        for point in &mut points {
            point.0 = point.0.round();
            point.1 = point.1.round();
        }
    }
    fill_points_with_antialias(pixmap, &points, color, texture != TextureMode::Pixel);
}
fn projected(ring: &Ring, height: f32, bounds: Bounds, scale: f32) -> Vec<(f32, f32)> {
    ring.points
        .iter()
        .map(|&(x, y)| pixel(isometric(x, y, height), bounds, scale))
        .collect()
}
pub(crate) fn pixel(point: (f32, f32), bounds: Bounds, scale: f32) -> (f32, f32) {
    (
        (point.0 - bounds.min_x) * scale,
        (point.1 - bounds.min_y) * scale,
    )
}
pub(crate) fn fill_points(pixmap: &mut Pixmap, points: &[(f32, f32)], color: Color) {
    fill_points_with_antialias(pixmap, points, color, true);
}
fn fill_points_with_antialias(
    pixmap: &mut Pixmap,
    points: &[(f32, f32)],
    color: Color,
    anti_alias: bool,
) {
    if points.len() < 3 {
        return;
    }
    let mut path = PathBuilder::new();
    path.move_to(points[0].0, points[0].1);
    for &(x, y) in &points[1..] {
        path.line_to(x, y);
    }
    path.close();
    if let Some(path) = path.finish() {
        let mut fill = paint(color);
        fill.anti_alias = anti_alias;
        pixmap.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
    }
}
pub(crate) fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint
}
pub(crate) fn palette(index: usize, colors: &[(u8, u8, u8)]) -> Color {
    let (red, green, blue) = colors[index % colors.len()];
    Color::from_rgba8(red, green, blue, 255)
}
pub(crate) fn shade(color: Color, factor: f32) -> Color {
    Color::from_rgba(
        color.red() * factor,
        color.green() * factor,
        color.blue() * factor,
        1.0,
    )
    .unwrap_or(color)
}

fn draw_ground(
    pixmap: &mut Pixmap,
    bounds: Bounds,
    scale: f32,
    aerial: Option<&AerialTile>,
    texture: TextureMode,
    block_size: f32,
    zoom: u8,
) {
    let Some(aerial) = aerial else {
        pixmap.fill(ground());
        return;
    };
    let ground = [217_u8, 209, 195];
    for py in 0..TILE_SIZE {
        for px in 0..TILE_SIZE {
            let iso_x = (f32::from(px as u16) + 0.5).mul_add(1.0 / scale, bounds.min_x);
            let iso_y = (f32::from(py as u16) + 0.5).mul_add(1.0 / scale, bounds.min_y);
            let (source_x, source_y) = inverse_isometric(iso_x, iso_y);
            let sampled = aerial.sample(source_x, source_y, texture, block_size);
            let color = if missing_imagery(sampled) {
                ground
            } else {
                mix_rgb(ground, sampled, texture_amount(texture, zoom))
            };
            let offset = ((py * TILE_SIZE + px) * 4) as usize;
            pixmap.data_mut()[offset..offset + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

fn texture_amount(texture: TextureMode, zoom: u8) -> f32 {
    match (texture, zoom) {
        (TextureMode::None, _) => 0.0,
        (TextureMode::Pixel, 0..=3) => 0.34,
        (TextureMode::Pixel, 4) => 0.56,
        (TextureMode::Pixel, _) => 0.76,
        (TextureMode::Full, 0..=3) => 0.44,
        (TextureMode::Full, 4) => 0.68,
        (TextureMode::Full, _) => 0.9,
    }
}

fn thematic_color(red: u8, green: u8, blue: u8, texture: TextureMode) -> Color {
    Color::from_rgba8(
        red,
        green,
        blue,
        if texture == TextureMode::None {
            255
        } else {
            132
        },
    )
}

pub(crate) fn missing_imagery(color: [u8; 3]) -> bool {
    color.iter().all(|channel| *channel >= 246)
}

fn mix_rgb(left: [u8; 3], right: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|index| {
        (f32::from(left[index]) * (1.0 - amount) + f32::from(right[index]) * amount).round() as u8
    })
}

pub(crate) fn mix_color(left: Color, right: Color, amount: f32) -> Color {
    Color::from_rgba(
        left.red() * (1.0 - amount) + right.red() * amount,
        left.green() * (1.0 - amount) + right.green() * amount,
        left.blue() * (1.0 - amount) + right.blue() * amount,
        1.0,
    )
    .unwrap_or(left)
}
