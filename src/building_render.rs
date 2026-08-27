use tiny_skia::{Color, FillRule, LineCap, LineJoin, PathBuilder, Pixmap, Stroke, Transform};

use crate::{
    render::{missing_imagery, mix_color, paint, palette, pixel, shade},
    texture::{AerialTile, TextureMode},
    world::{Bounds, Building, BuildingPart, Ring, RoofShape, inverse_isometric, isometric},
};

const TILE_SIZE: i32 = 256;
const MAX_TEXTURED_ROOF_PIXELS: usize = 100_000;
const MAX_FACADE_MARKS: usize = 20_000;

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

pub fn building_color(
    ring: &Ring,
    center: (f32, f32),
    height: f32,
    aerial: Option<&AerialTile>,
    texture: TextureMode,
    block_size: f32,
) -> Color {
    let variation = color_variation(center);
    let fallback = if height >= 80.0 {
        palette(
            variation,
            &[(101, 108, 112), (111, 116, 117), (121, 121, 118)],
        )
    } else if height >= 30.0 {
        palette(
            variation,
            &[(125, 114, 104), (137, 124, 111), (145, 132, 117)],
        )
    } else {
        palette(
            variation,
            &[(132, 105, 88), (145, 116, 96), (153, 126, 104)],
        )
    };
    let Some(aerial) = aerial else {
        return fallback;
    };
    if texture == TextureMode::None {
        return fallback;
    }
    let sample_point = roof_sample_point(ring, center);
    if !aerial.contains(sample_point.0, sample_point.1) {
        return fallback;
    }
    let sampled = aerial.sample(sample_point.0, sample_point.1, texture, block_size);
    if missing_imagery(sampled) {
        return fallback;
    }
    let roof = Color::from_rgba8(sampled[0], sampled[1], sampled[2], 255);
    let amount = if texture == TextureMode::Pixel {
        0.84
    } else {
        0.74
    };
    mix_color(fallback, roof, amount)
}

pub fn draw_building(pixmap: &mut Pixmap, building: &Building, context: &mut RenderContext<'_>) {
    let aerial = context
        .aerial
        .filter(|_| structure_fits_tile(&building.ring, 0.0, building.height, context));
    let color = building_color(
        &building.ring,
        building.center,
        building.height,
        aerial,
        context.texture,
        context.block_size,
    );
    draw_walls(pixmap, &building.ring, 0.0, building.height, color, context);
    let roof = projected(&building.ring, building.height, context);
    fill_shape(pixmap, &roof, color, context.texture);
    draw_textured_roof(pixmap, &roof, building.height, context, color);
}

pub fn draw_building_part(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    context: &mut RenderContext<'_>,
) {
    let aerial = context
        .aerial
        .filter(|_| structure_fits_tile(&part.ring, part.min_height, part.height, context));
    let roof_color = building_color(
        &part.ring,
        part.center,
        part.height,
        aerial,
        context.texture,
        context.block_size,
    );
    let wall_top = (part.height - part.roof_height).max(part.min_height);
    let wall_color = part.facade_color.map_or(roof_color, |color| {
        mix_color(
            roof_color,
            Color::from_rgba8(color[0], color[1], color[2], 255),
            0.72,
        )
    });
    draw_walls(
        pixmap,
        &part.ring,
        part.min_height,
        wall_top,
        wall_color,
        context,
    );
    match part.roof_shape {
        RoofShape::Flat => {
            let roof = projected(&part.ring, part.height, context);
            fill_shape(pixmap, &roof, roof_color, context.texture);
            draw_textured_roof(pixmap, &roof, part.height, context, roof_color);
        }
        RoofShape::Gabled if part.ring.points.len() == 4 => {
            draw_gabled_roof(pixmap, part, wall_top, roof_color, context);
        }
        RoofShape::Hipped if part.ring.points.len() == 4 => {
            draw_hipped_roof(pixmap, part, wall_top, roof_color, context);
        }
        RoofShape::Dome => draw_tiered_roof(pixmap, part, wall_top, roof_color, 0.55, context),
        RoofShape::Mansard => draw_mansard_roof(pixmap, part, wall_top, roof_color, context),
        RoofShape::Gabled | RoofShape::Hipped | RoofShape::Pyramidal | RoofShape::Cone => {
            draw_pointed_roof(pixmap, part, wall_top, roof_color, context)
        }
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
    let top = projected(ring, top_height, context);
    let bottom = projected(ring, bottom_height, context);
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
        fill_shape(pixmap, &face, wall_color, context.texture);
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
            context.texture,
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
    color: Color,
    context: &RenderContext<'_>,
) {
    let rim = projected(&part.ring, wall_top, context);
    let apex = projected_point(part.center, part.height, context);
    for index in 0..rim.len() {
        let next = (index + 1) % rim.len();
        let light = if index % 2 == 0 { 0.95 } else { 0.78 };
        fill_shape(
            pixmap,
            &[rim[index], rim[next], apex],
            shade(color, light),
            context.texture,
        );
    }
}

fn draw_tiered_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    color: Color,
    inset: f32,
    context: &RenderContext<'_>,
) {
    let rim = projected(&part.ring, wall_top, context);
    let middle_height = wall_top + part.roof_height * 0.62;
    let middle = scaled_ring(&part.ring, part.center, inset);
    let middle = projected(&middle, middle_height, context);
    for index in 0..rim.len() {
        let next = (index + 1) % rim.len();
        fill_shape(
            pixmap,
            &[rim[index], rim[next], middle[next], middle[index]],
            shade(color, if index % 2 == 0 { 0.9 } else { 0.74 }),
            context.texture,
        );
    }
    let apex = projected_point(part.center, part.height, context);
    for index in 0..middle.len() {
        let next = (index + 1) % middle.len();
        fill_shape(
            pixmap,
            &[middle[index], middle[next], apex],
            shade(color, 0.92),
            context.texture,
        );
    }
}

fn draw_gabled_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    color: Color,
    context: &RenderContext<'_>,
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
    let rim = projected(&part.ring, wall_top, context);
    let ridge_left = projected_point(left, part.height, context);
    let ridge_right = projected_point(right, part.height, context);
    if edge_a >= edge_b {
        fill_shape(
            pixmap,
            &[rim[0], rim[1], ridge_right, ridge_left],
            shade(color, 0.94),
            context.texture,
        );
        fill_shape(
            pixmap,
            &[rim[3], rim[2], ridge_right, ridge_left],
            shade(color, 0.78),
            context.texture,
        );
    } else {
        fill_shape(
            pixmap,
            &[rim[0], rim[3], ridge_left, ridge_right],
            shade(color, 0.94),
            context.texture,
        );
        fill_shape(
            pixmap,
            &[rim[1], rim[2], ridge_left, ridge_right],
            shade(color, 0.78),
            context.texture,
        );
    }
}

fn draw_hipped_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    color: Color,
    context: &RenderContext<'_>,
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
    let ridge_left = projected_point(lerp(left, right, 0.2), part.height, context);
    let ridge_right = projected_point(lerp(left, right, 0.8), part.height, context);
    let rim = projected(&part.ring, wall_top, context);
    let faces: [(&[(f32, f32)], f32); 4] = if edge_a >= edge_b {
        [
            (&[rim[0], rim[1], ridge_right, ridge_left], 0.94),
            (&[rim[3], rim[2], ridge_right, ridge_left], 0.78),
            (&[rim[0], rim[3], ridge_left], 0.86),
            (&[rim[1], rim[2], ridge_right], 0.72),
        ]
    } else {
        [
            (&[rim[0], rim[3], ridge_right, ridge_left], 0.94),
            (&[rim[1], rim[2], ridge_right, ridge_left], 0.78),
            (&[rim[0], rim[1], ridge_left], 0.86),
            (&[rim[3], rim[2], ridge_right], 0.72),
        ]
    };
    for (face, light) in faces {
        fill_shape(pixmap, face, shade(color, light), context.texture);
    }
}

fn draw_mansard_roof(
    pixmap: &mut Pixmap,
    part: &BuildingPart,
    wall_top: f32,
    color: Color,
    context: &mut RenderContext<'_>,
) {
    let rim = projected(&part.ring, wall_top, context);
    let upper_ring = scaled_ring(&part.ring, part.center, 0.72);
    let upper = projected(&upper_ring, part.height, context);
    for index in 0..rim.len() {
        let next = (index + 1) % rim.len();
        fill_shape(
            pixmap,
            &[rim[index], rim[next], upper[next], upper[index]],
            shade(color, if index % 2 == 0 { 0.9 } else { 0.74 }),
            context.texture,
        );
    }
    fill_shape(pixmap, &upper, color, context.texture);
    draw_textured_roof(pixmap, &upper, part.height, context, color);
}

fn stroke_line(
    pixmap: &mut Pixmap,
    start: (f32, f32),
    end: (f32, f32),
    color: Color,
    texture: TextureMode,
) {
    let crisp = texture == TextureMode::Pixel;
    let start = snap(start, crisp);
    let end = snap(end, crisp);
    let mut path = PathBuilder::new();
    path.move_to(start.0, start.1);
    path.line_to(end.0, end.1);
    let Some(path) = path.finish() else {
        return;
    };
    let stroke = Stroke {
        width: if crisp { 1.0 } else { 0.55 },
        line_cap: if crisp { LineCap::Butt } else { LineCap::Round },
        line_join: if crisp {
            LineJoin::Bevel
        } else {
            LineJoin::Round
        },
        ..Stroke::default()
    };
    let mut stroke_paint = paint(color);
    stroke_paint.anti_alias = !crisp;
    pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
}

fn projected(ring: &Ring, height: f32, context: &RenderContext<'_>) -> Vec<(f32, f32)> {
    ring.points
        .iter()
        .map(|&point| projected_point(point, height, context))
        .collect()
}

pub(crate) fn projected_point(
    point: (f32, f32),
    height: f32,
    context: &RenderContext<'_>,
) -> (f32, f32) {
    let screen = pixel(
        isometric(point.0, point.1, height),
        context.bounds,
        context.scale,
    );
    snap(screen, context.texture == TextureMode::Pixel)
}

fn structure_fits_tile(
    ring: &Ring,
    bottom_height: f32,
    top_height: f32,
    context: &RenderContext<'_>,
) -> bool {
    ring.points.iter().all(|&point| {
        [bottom_height, top_height].into_iter().all(|height| {
            let screen = projected_point(point, height, context);
            (0.0..=TILE_SIZE as f32).contains(&screen.0)
                && (0.0..=TILE_SIZE as f32).contains(&screen.1)
        })
    })
}

pub(crate) fn fill_shape(
    pixmap: &mut Pixmap,
    points: &[(f32, f32)],
    color: Color,
    texture: TextureMode,
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
    let Some(path) = path.finish() else {
        return;
    };
    let mut fill = paint(color);
    fill.anti_alias = texture != TextureMode::Pixel;
    pixmap.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
}

fn snap(point: (f32, f32), crisp: bool) -> (f32, f32) {
    if crisp {
        (point.0.round(), point.1.round())
    } else {
        point
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

fn roof_sample_point(ring: &Ring, center: (f32, f32)) -> (f32, f32) {
    if inside(center, &ring.points) {
        return center;
    }
    let anchor = ring.points[0];
    for edge in ring.points[1..].windows(2) {
        let candidate = (
            (anchor.0 + edge[0].0 + edge[1].0) / 3.0,
            (anchor.1 + edge[0].1 + edge[1].1) / 3.0,
        );
        if inside(candidate, &ring.points) {
            return candidate;
        }
    }
    anchor
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
    use super::{inside, roof_sample_point};
    use crate::world::{Bounds, Ring};

    #[test]
    fn point_in_polygon_handles_inside_and_outside() {
        let square = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        assert!(inside((2.0, 2.0), &square));
        assert!(!inside((5.0, 2.0), &square));
    }

    #[test]
    fn roof_sample_falls_inside_a_concave_ring() {
        let ring = Ring {
            points: vec![
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 1.0),
                (1.0, 1.0),
                (1.0, 4.0),
                (0.0, 4.0),
            ],
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 4.0,
                max_y: 4.0,
            },
        };
        let sample = roof_sample_point(&ring, (2.0, 2.0));

        assert!(inside(sample, &ring.points));
    }
}
