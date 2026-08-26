use std::{cmp::Ordering, io};

use rstar::AABB;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use crate::world::{Aerial, Bounds, Building, Ring, World, isometric};

const TILE_SIZE: u32 = 256;
const OVERVIEW_ZOOM: u8 = 3;
const OVERVIEW_LIMIT: usize = 60_000;
const DETAIL_LIMIT: usize = 6_000;

pub fn render_tile(world: &World, z: u8, x: u32, y: u32) -> io::Result<Vec<u8>> {
    let bounds = world.iso_bounds.tile(z, x, y);
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    pixmap.fill(Color::from_rgba8(217, 209, 195, 255));
    if let Some(aerial) = &world.aerial {
        draw_aerial(&mut pixmap, aerial, world.source_bounds, bounds, scale);
    }
    let query = bounds.source_envelope();
    draw_rings(
        &mut pixmap,
        &world.water,
        &world.water_tree,
        query,
        bounds,
        scale,
        Color::from_rgba8(93, 132, 144, 255),
    );
    draw_rings(
        &mut pixmap,
        &world.parks,
        &world.park_tree,
        query,
        bounds,
        scale,
        Color::from_rgba8(119, 146, 103, 255),
    );
    let mut buildings: Vec<&Building> = world
        .building_tree
        .locate_in_envelope_intersecting(&query)
        .map(|item| &world.buildings[item.index])
        .collect();
    if z <= OVERVIEW_ZOOM {
        let step = (buildings.len() / OVERVIEW_LIMIT).max(1);
        for building in buildings.iter().step_by(step) {
            let (px, py) = pixel(
                isometric(building.center.0, building.center.1, building.height),
                bounds,
                scale,
            );
            let dot = Rect::from_xywh(px - 1.0, py - 1.0, 2.0, 2.0)
                .ok_or_else(|| io::Error::other("overview dot"))?;
            pixmap.fill_rect(
                dot,
                &paint(color(building.height)),
                Transform::identity(),
                None,
            );
        }
    } else if z == OVERVIEW_ZOOM + 1 {
        for building in buildings {
            fill_ring(
                &mut pixmap,
                &building.ring,
                bounds,
                scale,
                color(building.height),
            );
        }
    } else {
        buildings.sort_by(|left, right| {
            (left.center.0 + left.center.1)
                .partial_cmp(&(right.center.0 + right.center.1))
                .unwrap_or(Ordering::Equal)
        });
        let step = (buildings.len() / DETAIL_LIMIT).max(1);
        for building in buildings.iter().step_by(step) {
            draw_building(&mut pixmap, building, bounds, scale);
        }
    }
    pixmap.encode_png().map_err(io::Error::other)
}

fn draw_aerial(pixmap: &mut Pixmap, aerial: &Aerial, source: Bounds, tile: Bounds, scale: f32) {
    let data = pixmap.data_mut();
    for pixel_y in 0..TILE_SIZE {
        let iso_y = tile.min_y + (pixel_y as f32 + 0.5) / scale;
        for pixel_x in 0..TILE_SIZE {
            let iso_x = tile.min_x + (pixel_x as f32 + 0.5) / scale;
            let source_x = (iso_x + 2.0 * iso_y) * 0.5;
            let source_y = (2.0 * iso_y - iso_x) * 0.5;
            let image_x = ((source_x - source.min_x) / source.width() * aerial.width as f32) as i32;
            let image_y =
                ((source.max_y - source_y) / source.height() * aerial.height as f32) as i32;
            if image_x < 0
                || image_y < 0
                || image_x >= aerial.width as i32
                || image_y >= aerial.height as i32
            {
                continue;
            }
            let target = ((pixel_y * TILE_SIZE + pixel_x) * 4) as usize;
            let image = ((image_y as u32 * aerial.width + image_x as u32) * 4) as usize;
            let brightness = aerial.data[image] as u16
                + aerial.data[image + 1] as u16
                + aerial.data[image + 2] as u16;
            if brightness < 270 {
                continue;
            }
            for channel in 0..3 {
                data[target + channel] = ((data[target + channel] as u16
                    + aerial.data[image + channel] as u16)
                    / 2) as u8;
            }
        }
    }
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
        fill_ring(pixmap, &rings[item.index], bounds, scale, color);
    }
}
fn draw_building(pixmap: &mut Pixmap, building: &Building, bounds: Bounds, scale: f32) {
    let roof = projected(&building.ring, building.height, bounds, scale);
    let ground = projected(&building.ring, 0.0, bounds, scale);
    let roof_color = color(building.height);
    for index in 0..roof.len() {
        let next = (index + 1) % roof.len();
        fill_points(
            pixmap,
            &[ground[index], ground[next], roof[next], roof[index]],
            shade(roof_color, if index % 2 == 0 { 0.86 } else { 0.72 }),
        );
    }
    fill_points(pixmap, &roof, roof_color);
}
fn fill_ring(pixmap: &mut Pixmap, ring: &Ring, bounds: Bounds, scale: f32, color: Color) {
    fill_points(pixmap, &projected(ring, 0.0, bounds, scale), color);
}
fn projected(ring: &Ring, height: f32, bounds: Bounds, scale: f32) -> Vec<(f32, f32)> {
    ring.points
        .iter()
        .map(|&(x, y)| pixel(isometric(x, y, height), bounds, scale))
        .collect()
}
fn pixel(point: (f32, f32), bounds: Bounds, scale: f32) -> (f32, f32) {
    (
        (point.0 - bounds.min_x) * scale,
        (point.1 - bounds.min_y) * scale,
    )
}
fn fill_points(pixmap: &mut Pixmap, points: &[(f32, f32)], color: Color) {
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
        pixmap.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}
fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint
}
fn color(height: f32) -> Color {
    if height >= 80.0 {
        Color::from_rgba8(112, 113, 135, 255)
    } else if height >= 30.0 {
        Color::from_rgba8(157, 103, 79, 255)
    } else {
        Color::from_rgba8(178, 91, 61, 255)
    }
}
fn shade(color: Color, factor: f32) -> Color {
    Color::from_rgba(
        color.red() * factor,
        color.green() * factor,
        color.blue() * factor,
        1.0,
    )
    .unwrap_or(color)
}
