use tiny_skia::{Color, LineCap, LineJoin, Pixmap, Stroke, Transform};

use crate::{
    landmarks::{CITY_HALL_SOURCE, WILLIAM_PENN_BASE, WILLIAM_PENN_HEIGHT},
    render::{fill_points, missing_imagery, mix_color, paint, palette, pixel, shade},
    texture::{AerialTile, TextureMode},
    world::{Bounds, Building, BuildingPart, Ring, RoofShape, inverse_isometric, isometric},
};

const TILE_SIZE: i32 = 256;
const MAX_TEXTURED_ROOF_PIXELS: usize = 100_000;
const MAX_FACADE_MARKS: usize = 20_000;

struct SolidBox {
    center: (f32, f32),
    width: f32,
    depth: f32,
    bottom: f32,
    height: f32,
}

pub struct RenderContext<'a> {
    pub aerial: Option<&'a AerialTile>,
    pub texture: TextureMode,
    pub block_size: f32,
    pub zoom: u8,
    pub bounds: Bounds,
    pub scale: f32,
    roof_pixels: usize,
    facade_marks: usize,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        aerial: Option<&'a AerialTile>,
        texture: TextureMode,
        block_size: f32,
        zoom: u8,
        bounds: Bounds,
        scale: f32,
    ) -> Self {
        Self {
            aerial,
            texture,
            block_size,
            zoom,
            bounds,
            scale,
            roof_pixels: 0,
            facade_marks: 0,
        }
    }
}

pub fn building_color(center: (f32, f32), height: f32) -> Color {
    let variation = color_variation(center);
    if height >= 80.0 {
        palette(
            variation,
            &[(104, 108, 129), (115, 116, 137), (123, 122, 139)],
        )
    } else if height >= 30.0 {
        palette(variation, &[(149, 96, 75), (158, 104, 80), (166, 112, 85)])
    } else {
        palette(variation, &[(170, 82, 55), (181, 91, 59), (188, 101, 67)])
    }
}

pub fn draw_building(pixmap: &mut Pixmap, building: &Building, context: &mut RenderContext<'_>) {
    let color = building_color(building.center, building.height);
    draw_walls(pixmap, &building.ring, 0.0, building.height, color, context);
    let roof = projected(
        &building.ring,
        building.height,
        context.bounds,
        context.scale,
    );
    fill_points(pixmap, &roof, color);
    draw_textured_roof(pixmap, &roof, building.height, context, color);
}

pub fn draw_building_part(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    context: &mut RenderContext<'_>,
) {
    let base = if is_city_hall_part(part.osm_id) {
        palette(
            color_variation(part.center),
            &[(185, 176, 159), (198, 190, 174), (171, 162, 146)],
        )
    } else {
        building_color(part.center, part.height)
    };
    let wall_top = (part.height - part.roof_height).max(part.min_height);
    draw_walls(pixmap, &part.ring, part.min_height, wall_top, base, context);
    match part.roof_shape {
        RoofShape::Flat => {
            let roof = projected(&part.ring, part.height, context.bounds, context.scale);
            fill_points(pixmap, &roof, base);
            draw_textured_roof(pixmap, &roof, part.height, context, base);
        }
        RoofShape::Gabled if part.ring.points.len() == 4 => {
            draw_gabled_roof(pixmap, part, wall_top, context.bounds, context.scale, base);
        }
        RoofShape::Dome => draw_tiered_roof(
            pixmap,
            part,
            wall_top,
            context.bounds,
            context.scale,
            base,
            0.55,
        ),
        RoofShape::Mansard => {
            draw_tiered_roof(
                pixmap,
                part,
                wall_top,
                context.bounds,
                context.scale,
                base,
                0.72,
            );
        }
        RoofShape::Gabled | RoofShape::Hipped | RoofShape::Pyramidal | RoofShape::Cone => {
            draw_pointed_roof(pixmap, part, wall_top, context.bounds, context.scale, base)
        }
    }
}

pub fn draw_william_penn(pixmap: &mut Pixmap, context: &mut RenderContext<'_>) {
    if context.zoom < 6 {
        return;
    }
    let bronze = Color::from_rgba8(80, 105, 83, 255);
    let (x, y) = CITY_HALL_SOURCE;
    for shape in [
        SolidBox {
            center: (x, y),
            width: 2.8,
            depth: 2.8,
            bottom: WILLIAM_PENN_BASE,
            height: 1.0,
        },
        SolidBox {
            center: (x - 0.42, y),
            width: 0.5,
            depth: 0.65,
            bottom: 156.8,
            height: 2.8,
        },
        SolidBox {
            center: (x + 0.42, y),
            width: 0.5,
            depth: 0.65,
            bottom: 156.8,
            height: 2.8,
        },
        SolidBox {
            center: (x, y),
            width: 1.65,
            depth: 1.05,
            bottom: 159.2,
            height: 4.1,
        },
        SolidBox {
            center: (x, y),
            width: 0.78,
            depth: 0.72,
            bottom: 163.3,
            height: 1.05,
        },
        SolidBox {
            center: (x, y),
            width: 1.55,
            depth: 0.82,
            bottom: 164.35,
            height: 0.28,
        },
    ] {
        draw_box(pixmap, &shape, bronze, context);
    }
    stroke_3d(
        pixmap,
        (x - 0.6, y, 162.8),
        (x - 3.2, y + 0.7, 162.0),
        context.bounds,
        context.scale,
        bronze,
        0.55,
    );
    stroke_3d(
        pixmap,
        (x + 0.55, y, 162.7),
        (x + 2.4, y + 1.7, 163.0),
        context.bounds,
        context.scale,
        bronze,
        0.55,
    );
    let top = WILLIAM_PENN_BASE + WILLIAM_PENN_HEIGHT;
    let marker = pixel(isometric(x, y, top), context.bounds, context.scale);
    if context.zoom >= 9 {
        let dot = [
            (marker.0 - 0.7, marker.1),
            (marker.0, marker.1 - 0.8),
            (marker.0 + 0.7, marker.1),
        ];
        fill_points(pixmap, &dot, bronze);
    }
}

fn draw_walls(
    pixmap: &mut Pixmap,
    ring: &Ring,
    bottom_height: f32,
    top_height: f32,
    color: Color,
    context: &mut RenderContext<'_>,
) {
    let top = projected(ring, top_height, context.bounds, context.scale);
    let bottom = projected(ring, bottom_height, context.bounds, context.scale);
    for index in 0..top.len() {
        let next = (index + 1) % top.len();
        let (x1, y1) = ring.points[index];
        let (x2, y2) = ring.points[next];
        let light = if (x2 - x1).abs() >= (y2 - y1).abs() {
            0.78
        } else {
            0.64
        };
        let face = [bottom[index], bottom[next], top[next], top[index]];
        let wall_color = shade(color, light);
        fill_points(pixmap, &face, wall_color);
        if context.zoom >= 8 && context.facade_marks < MAX_FACADE_MARKS {
            draw_facade_grid(
                pixmap,
                face,
                top_height - bottom_height,
                wall_color,
                context,
            );
        }
    }
}

fn draw_facade_grid(
    pixmap: &mut Pixmap,
    face: [(f32, f32); 4],
    height: f32,
    color: Color,
    context: &mut RenderContext<'_>,
) {
    let wall_width = distance(face[0], face[1]);
    let wall_height = distance(face[0], face[3]);
    if wall_width < 8.0 || wall_height < 7.0 {
        return;
    }
    let physical_floors = (height / 3.2).floor().clamp(2.0, 40.0) as usize;
    let visible_floors = (wall_height / 3.0).floor() as usize;
    let floors = physical_floors.min(visible_floors.max(2));
    let grid = translucent(shade(color, 0.48), 150);
    for floor in 1..floors {
        if context.facade_marks >= MAX_FACADE_MARKS {
            return;
        }
        let t = floor as f32 / floors as f32;
        stroke_line(
            pixmap,
            lerp(face[0], face[3], t),
            lerp(face[1], face[2], t),
            grid,
            0.55,
        );
        context.facade_marks += 1;
    }
}

fn draw_textured_roof(
    pixmap: &mut Pixmap,
    roof: &[(f32, f32)],
    height: f32,
    context: &mut RenderContext<'_>,
    base: Color,
) {
    let Some(aerial) = context.aerial else {
        return;
    };
    if context.texture == TextureMode::None
        || context.zoom < 7
        || roof.len() < 3
        || context.roof_pixels >= MAX_TEXTURED_ROOF_PIXELS
    {
        return;
    }
    let min_x = roof
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let max_x = roof
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let min_y = roof
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let max_y = roof
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    for py in min_y.max(0)..max_y.min(TILE_SIZE) {
        for px in min_x.max(0)..max_x.min(TILE_SIZE) {
            let screen = (px as f32 + 0.5, py as f32 + 0.5);
            if !inside(screen, roof) {
                continue;
            }
            if context.roof_pixels >= MAX_TEXTURED_ROOF_PIXELS {
                return;
            }
            let iso_x = screen.0.mul_add(1.0 / context.scale, context.bounds.min_x);
            let iso_y = screen.1.mul_add(1.0 / context.scale, context.bounds.min_y);
            let source = inverse_isometric(iso_x, iso_y + height);
            if !aerial.contains(source.0, source.1) {
                continue;
            }
            let sampled = aerial.sample(source.0, source.1, context.texture, context.block_size);
            if missing_imagery(sampled) {
                continue;
            }
            let photo = Color::from_rgba8(sampled[0], sampled[1], sampled[2], 255);
            let amount = if context.texture == TextureMode::Full {
                0.86
            } else {
                0.74
            };
            let color = mix_color(base, photo, amount);
            let offset = ((py * TILE_SIZE + px) * 4) as usize;
            pixmap.data_mut()[offset..offset + 4].copy_from_slice(&[
                (color.red() * 255.0).round() as u8,
                (color.green() * 255.0).round() as u8,
                (color.blue() * 255.0).round() as u8,
                255,
            ]);
            context.roof_pixels += 1;
        }
    }
}

fn draw_pointed_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    bounds: Bounds,
    scale: f32,
    color: Color,
) {
    let rim = projected(&part.ring, wall_top, bounds, scale);
    let apex = pixel(
        isometric(part.center.0, part.center.1, part.height),
        bounds,
        scale,
    );
    for index in 0..rim.len() {
        let next = (index + 1) % rim.len();
        let light = if index % 2 == 0 { 0.95 } else { 0.78 };
        fill_points(pixmap, &[rim[index], rim[next], apex], shade(color, light));
    }
}

fn draw_tiered_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    bounds: Bounds,
    scale: f32,
    color: Color,
    inset: f32,
) {
    let rim = projected(&part.ring, wall_top, bounds, scale);
    let middle_height = wall_top + part.roof_height * 0.62;
    let middle = scaled_ring(&part.ring, part.center, inset);
    let middle = projected(&middle, middle_height, bounds, scale);
    for index in 0..rim.len() {
        let next = (index + 1) % rim.len();
        fill_points(
            pixmap,
            &[rim[index], rim[next], middle[next], middle[index]],
            shade(color, if index % 2 == 0 { 0.9 } else { 0.74 }),
        );
    }
    let apex = pixel(
        isometric(part.center.0, part.center.1, part.height),
        bounds,
        scale,
    );
    for index in 0..middle.len() {
        let next = (index + 1) % middle.len();
        fill_points(
            pixmap,
            &[middle[index], middle[next], apex],
            shade(color, 0.92),
        );
    }
}

fn draw_gabled_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    bounds: Bounds,
    scale: f32,
    color: Color,
) {
    let points = &part.ring.points;
    let edge_a = distance(points[0], points[1]);
    let edge_b = distance(points[1], points[2]);
    let (left, right) = if edge_a >= edge_b {
        (
            midpoint(points[0], points[3]),
            midpoint(points[1], points[2]),
        )
    } else {
        (
            midpoint(points[0], points[1]),
            midpoint(points[3], points[2]),
        )
    };
    let rim = projected(&part.ring, wall_top, bounds, scale);
    let ridge_left = pixel(isometric(left.0, left.1, part.height), bounds, scale);
    let ridge_right = pixel(isometric(right.0, right.1, part.height), bounds, scale);
    if edge_a >= edge_b {
        fill_points(
            pixmap,
            &[rim[0], rim[1], ridge_right, ridge_left],
            shade(color, 0.94),
        );
        fill_points(
            pixmap,
            &[rim[3], rim[2], ridge_right, ridge_left],
            shade(color, 0.78),
        );
    } else {
        fill_points(
            pixmap,
            &[rim[0], rim[3], ridge_left, ridge_right],
            shade(color, 0.94),
        );
        fill_points(
            pixmap,
            &[rim[1], rim[2], ridge_left, ridge_right],
            shade(color, 0.78),
        );
    }
}

fn draw_box(pixmap: &mut Pixmap, shape: &SolidBox, color: Color, context: &mut RenderContext<'_>) {
    let ring = rectangle(shape.center.0, shape.center.1, shape.width, shape.depth);
    draw_walls(
        pixmap,
        &ring,
        shape.bottom,
        shape.bottom + shape.height,
        color,
        context,
    );
    fill_points(
        pixmap,
        &projected(
            &ring,
            shape.bottom + shape.height,
            context.bounds,
            context.scale,
        ),
        color,
    );
}

fn stroke_3d(
    pixmap: &mut Pixmap,
    start: (f32, f32, f32),
    end: (f32, f32, f32),
    bounds: Bounds,
    scale: f32,
    color: Color,
    width: f32,
) {
    stroke_line(
        pixmap,
        pixel(isometric(start.0, start.1, start.2), bounds, scale),
        pixel(isometric(end.0, end.1, end.2), bounds, scale),
        color,
        (width * scale).clamp(1.0, 4.0),
    );
}

fn stroke_line(pixmap: &mut Pixmap, start: (f32, f32), end: (f32, f32), color: Color, width: f32) {
    let mut path = tiny_skia::PathBuilder::new();
    path.move_to(start.0, start.1);
    path.line_to(end.0, end.1);
    let Some(path) = path.finish() else {
        return;
    };
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
}

fn projected(ring: &Ring, height: f32, bounds: Bounds, scale: f32) -> Vec<(f32, f32)> {
    ring.points
        .iter()
        .map(|&(x, y)| pixel(isometric(x, y, height), bounds, scale))
        .collect()
}

fn rectangle(x: f32, y: f32, width: f32, depth: f32) -> Ring {
    let half_width = width * 0.5;
    let half_depth = depth * 0.5;
    Ring {
        points: vec![
            (x - half_width, y - half_depth),
            (x + half_width, y - half_depth),
            (x + half_width, y + half_depth),
            (x - half_width, y + half_depth),
        ],
        bounds: Bounds {
            min_x: x - half_width,
            min_y: y - half_depth,
            max_x: x + half_width,
            max_y: y + half_depth,
        },
    }
}

fn scaled_ring(ring: &Ring, center: (f32, f32), factor: f32) -> Ring {
    let points = ring
        .points
        .iter()
        .map(|point| {
            (
                center.0 + (point.0 - center.0) * factor,
                center.1 + (point.1 - center.1) * factor,
            )
        })
        .collect();
    Ring {
        points,
        bounds: ring.bounds,
    }
}

fn inside(point: (f32, f32), polygon: &[(f32, f32)]) -> bool {
    let mut result = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let (x1, y1) = polygon[current];
        let (x2, y2) = polygon[previous];
        if (y1 > point.1) != (y2 > point.1)
            && point.0 < (x2 - x1).mul_add((point.1 - y1) / (y2 - y1), x1)
        {
            result = !result;
        }
        previous = current;
    }
    result
}

fn is_city_hall_part(osm_id: u64) -> bool {
    matches!(
        osm_id,
        333_316_163..=333_316_171
            | 335_828_082..=335_828_092
            | 336_681_405..=336_681_406
            | 336_855_470..=336_855_475
            | 369_003_677
    )
}

fn color_variation((x, y): (f32, f32)) -> usize {
    let x = x.to_bits();
    let y = y.to_bits();
    (x.wrapping_mul(0x9e37_79b9) ^ y.rotate_left(13)) as usize
}

fn translucent(color: Color, alpha: u8) -> Color {
    Color::from_rgba(
        color.red(),
        color.green(),
        color.blue(),
        f32::from(alpha) / 255.0,
    )
    .unwrap_or(color)
}

fn midpoint(left: (f32, f32), right: (f32, f32)) -> (f32, f32) {
    ((left.0 + right.0) * 0.5, (left.1 + right.1) * 0.5)
}

fn lerp(left: (f32, f32), right: (f32, f32), amount: f32) -> (f32, f32) {
    (
        (right.0 - left.0).mul_add(amount, left.0),
        (right.1 - left.1).mul_add(amount, left.1),
    )
}

fn distance(left: (f32, f32), right: (f32, f32)) -> f32 {
    (right.0 - left.0).hypot(right.1 - left.1)
}

#[cfg(test)]
mod tests {
    use super::{inside, is_city_hall_part};

    #[test]
    fn point_in_polygon_handles_inside_and_outside() {
        let square = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        assert!(inside((2.0, 2.0), &square));
        assert!(!inside((5.0, 2.0), &square));
    }

    #[test]
    fn city_hall_part_ids_are_explicit() {
        assert!(is_city_hall_part(333_316_163));
        assert!(is_city_hall_part(369_003_677));
        assert!(!is_city_hall_part(333_204_631));
    }
}
