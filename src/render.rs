use std::{cmp::Ordering, io};

use rstar::AABB;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use crate::world::{Bounds, Building, Ring, Street, World, isometric};

const TILE_SIZE: u32 = 256;
const OVERVIEW_ZOOM: u8 = 3;
const EXTRUSION_ZOOM: u8 = 5;
const OVERVIEW_LIMIT: usize = 60_000;

pub fn render_tile(world: &World, z: u8, x: u32, y: u32) -> io::Result<Vec<u8>> {
    let bounds = world.iso_bounds.tile(z, x, y);
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    pixmap.fill(ground());
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
    draw_streets(&mut pixmap, world, query, bounds, scale, z);
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
                &paint(building_color(building)),
                Transform::identity(),
                None,
            );
        }
    } else if z < EXTRUSION_ZOOM {
        for building in buildings {
            fill_projected_ring(
                &mut pixmap,
                &building.ring,
                building.height,
                bounds,
                scale,
                building_color(building),
            );
        }
    } else {
        buildings.sort_by(|left, right| {
            (left.center.0 + left.center.1)
                .partial_cmp(&(right.center.0 + right.center.1))
                .unwrap_or(Ordering::Equal)
        });
        for building in buildings {
            draw_building(&mut pixmap, building, bounds, scale);
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
fn draw_building(pixmap: &mut Pixmap, building: &Building, bounds: Bounds, scale: f32) {
    let roof = projected(&building.ring, building.height, bounds, scale);
    let ground = projected(&building.ring, 0.0, bounds, scale);
    let roof_color = building_color(building);
    for index in 0..roof.len() {
        let next = (index + 1) % roof.len();
        let (x1, y1) = building.ring.points[index];
        let (x2, y2) = building.ring.points[next];
        let light = if (x2 - x1).abs() >= (y2 - y1).abs() {
            0.78
        } else {
            0.64
        };
        fill_points(
            pixmap,
            &[ground[index], ground[next], roof[next], roof[index]],
            shade(roof_color, light),
        );
    }
    fill_points(pixmap, &roof, roof_color);
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
fn building_color(building: &Building) -> Color {
    let variation = color_variation(building.center);
    if building.height >= 80.0 {
        palette(
            variation,
            &[(104, 108, 129), (115, 116, 137), (123, 122, 139)],
        )
    } else if building.height >= 30.0 {
        palette(variation, &[(149, 96, 75), (158, 104, 80), (166, 112, 85)])
    } else {
        palette(variation, &[(170, 82, 55), (181, 91, 59), (188, 101, 67)])
    }
}
fn color_variation((x, y): (f32, f32)) -> usize {
    let x = x.to_bits();
    let y = y.to_bits();
    (x.wrapping_mul(0x9e37_79b9) ^ y.rotate_left(13)) as usize
}
fn palette(index: usize, colors: &[(u8, u8, u8)]) -> Color {
    let (red, green, blue) = colors[index % colors.len()];
    Color::from_rgba8(red, green, blue, 255)
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
