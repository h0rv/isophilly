use tiny_skia::Pixmap;

use crate::{
    projection::Projection,
    texture::AerialTile,
    world::{Building, inverse_isometric, view_depth},
};

const TILE_SIZE: usize = 256;
const MIN_FACE_AREA: f32 = 0.3;

pub fn draw_city_buildings<'a>(
    pixmap: &mut Pixmap,
    buildings: impl IntoIterator<Item = &'a Building>,
    projection: &Projection,
    aerial: &AerialTile,
    block_size: f32,
) {
    let mut rasterizer = Rasterizer {
        pixmap,
        projection,
        aerial,
        block_size,
        depth: vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE],
    };
    for building in buildings {
        rasterizer.draw_building(building);
    }
}

struct Rasterizer<'a, 'b> {
    pixmap: &'a mut Pixmap,
    projection: &'b Projection,
    aerial: &'b AerialTile,
    block_size: f32,
    depth: Vec<f32>,
}

impl Rasterizer<'_, '_> {
    fn draw_building(&mut self, building: &Building) {
        if building.ring.points.len() < 3 {
            return;
        }
        let facade = self.facade_palette(building);
        for index in 0..building.ring.points.len() {
            let left = building.ring.points[index];
            let right = building.ring.points[(index + 1) % building.ring.points.len()];
            self.draw_wall(left, right, building.height, facade);
        }
        self.draw_roof(building);
    }

    fn facade_palette(&self, building: &Building) -> [u8; 3] {
        let mut sum = [0_u32; 3];
        let mut count = 0_u32;
        for &point in &building.ring.points {
            let sample = self.aerial.sample(point.0, point.1, self.block_size);
            let luminance =
                u32::from(sample[0]) * 3 + u32::from(sample[1]) * 6 + u32::from(sample[2]);
            if (400..=2_200).contains(&luminance) {
                for channel in 0..3 {
                    sum[channel] += u32::from(sample[channel]);
                }
                count += 1;
            }
        }
        let average = if count == 0 {
            [128, 128, 128]
        } else {
            sum.map(|channel| (channel / count) as u8)
        };
        soften(average)
    }

    fn draw_wall(&mut self, left: (f32, f32), right: (f32, f32), height: f32, facade: [u8; 3]) {
        let length = (right.0 - left.0).hypot(right.1 - left.1);
        let ground_left = Vertex::world(left, 0.0, 0.0, self.projection);
        let ground_right = Vertex::world(right, 0.0, length, self.projection);
        let roof_left = Vertex::world(left, height, 0.0, self.projection);
        let roof_right = Vertex::world(right, height, length, self.projection);
        let edge = (right.0 - left.0, right.1 - left.1);
        let light = if edge.0 + edge.1 >= 0.0 { 0.72 } else { 0.84 };
        self.draw_wall_triangle(
            [ground_left, ground_right, roof_right],
            facade,
            light,
            height,
        );
        self.draw_wall_triangle([ground_left, roof_right, roof_left], facade, light, height);
    }

    fn draw_roof(&mut self, building: &Building) {
        let projected: Vec<_> = building
            .ring
            .points
            .iter()
            .map(|&point| self.projection.point(point, building.height))
            .collect();
        let min_x = projected
            .iter()
            .map(|point| point.0)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_x = projected
            .iter()
            .map(|point| point.0)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((TILE_SIZE - 1) as f32) as usize;
        let min_y = projected
            .iter()
            .map(|point| point.1)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_y = projected
            .iter()
            .map(|point| point.1)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((TILE_SIZE - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            return;
        }
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let iso_x = (px as f32 + 0.5)
                    .mul_add(1.0 / self.projection.scale, self.projection.bounds.min_x);
                let iso_y = (py as f32 + 0.5)
                    .mul_add(1.0 / self.projection.scale, self.projection.bounds.min_y);
                let source = inverse_isometric(iso_x, iso_y + building.height);
                if !point_in_polygon(source, &building.ring.points) {
                    continue;
                }
                let offset = py * TILE_SIZE + px;
                let depth = view_depth(source.0, source.1, building.height);
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                let color = self.aerial.sample(source.0, source.1, self.block_size);
                self.set_pixel(offset, color.map(|channel| shade(channel, 1.04)));
            }
        }
    }

    fn draw_wall_triangle(
        &mut self,
        triangle: [Vertex; 3],
        facade: [u8; 3],
        light: f32,
        height: f32,
    ) {
        let area = edge(triangle[0], triangle[1], triangle[2]);
        if area.abs() < MIN_FACE_AREA {
            return;
        }
        let min_x = triangle
            .iter()
            .map(|vertex| vertex.screen_x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_x = triangle
            .iter()
            .map(|vertex| vertex.screen_x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((TILE_SIZE - 1) as f32) as usize;
        let min_y = triangle
            .iter()
            .map(|vertex| vertex.screen_y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_y = triangle
            .iter()
            .map(|vertex| vertex.screen_y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((TILE_SIZE - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            return;
        }
        let inverse_area = 1.0 / area;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample = Vertex::screen(x as f32 + 0.5, y as f32 + 0.5);
                let weights = [
                    edge(triangle[1], triangle[2], sample) * inverse_area,
                    edge(triangle[2], triangle[0], sample) * inverse_area,
                    edge(triangle[0], triangle[1], sample) * inverse_area,
                ];
                if weights.iter().any(|weight| *weight < -f32::EPSILON) {
                    continue;
                }
                let source_x = interpolate(&weights, &triangle, |vertex| vertex.source_x);
                let source_y = interpolate(&weights, &triangle, |vertex| vertex.source_y);
                let z = interpolate(&weights, &triangle, |vertex| vertex.z);
                let along = interpolate(&weights, &triangle, |vertex| vertex.along);
                let depth = view_depth(source_x, source_y, z);
                let offset = y * TILE_SIZE + x;
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                let mut color = facade;
                let floor = (z / 3.2).floor() as i32;
                let floor_fraction = (z / 3.2).fract();
                let column = (along / 2.2).floor() as i32;
                let is_window = height >= 9.0
                    && (0.26..=0.68).contains(&floor_fraction)
                    && column.rem_euclid(2) == 0;
                let horizontal = if is_window {
                    0.66
                } else if floor.rem_euclid(2) == 0 {
                    1.0
                } else {
                    0.95
                };
                let base = if height > 5.0 && z < 1.2 { 0.82 } else { 1.0 };
                for channel in &mut color {
                    *channel = shade(*channel, light * horizontal * base);
                }
                self.set_pixel(offset, color);
            }
        }
    }

    fn set_pixel(&mut self, offset: usize, color: [u8; 3]) {
        let byte_offset = offset * 4;
        self.pixmap.data_mut()[byte_offset..byte_offset + 4]
            .copy_from_slice(&[color[0], color[1], color[2], 255]);
    }
}

#[derive(Clone, Copy)]
struct Vertex {
    screen_x: f32,
    screen_y: f32,
    source_x: f32,
    source_y: f32,
    z: f32,
    along: f32,
}

impl Vertex {
    fn world(point: (f32, f32), z: f32, along: f32, projection: &Projection) -> Self {
        let screen = projection.point(point, z);
        Self {
            screen_x: screen.0,
            screen_y: screen.1,
            source_x: point.0,
            source_y: point.1,
            z,
            along,
        }
    }

    fn screen(screen_x: f32, screen_y: f32) -> Self {
        Self {
            screen_x,
            screen_y,
            source_x: 0.0,
            source_y: 0.0,
            z: 0.0,
            along: 0.0,
        }
    }
}

fn edge(left: Vertex, right: Vertex, point: Vertex) -> f32 {
    (point.screen_x - left.screen_x).mul_add(
        right.screen_y - left.screen_y,
        -((point.screen_y - left.screen_y) * (right.screen_x - left.screen_x)),
    )
}

fn interpolate(weights: &[f32; 3], triangle: &[Vertex; 3], field: impl Fn(Vertex) -> f32) -> f32 {
    weights[0].mul_add(
        field(triangle[0]),
        weights[1].mul_add(field(triangle[1]), weights[2] * field(triangle[2])),
    )
}

fn point_in_polygon(point: (f32, f32), ring: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut previous = ring[ring.len() - 1];
    for &current in ring {
        let crosses = (current.1 > point.1) != (previous.1 > point.1)
            && point.0
                < (previous.0 - current.0) * (point.1 - current.1) / (previous.1 - current.1)
                    + current.0;
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn shade(channel: u8, light: f32) -> u8 {
    let lit = (f32::from(channel) * light).round().clamp(0.0, 255.0) as u16;
    ((lit + 8) / 16 * 16).min(255) as u8
}

fn soften(color: [u8; 3]) -> [u8; 3] {
    let luminance = (u16::from(color[0]) * 3 + u16::from(color[1]) * 6 + u16::from(color[2])) / 10;
    std::array::from_fn(|index| {
        let mixed = (u16::from(color[index]) * 3 + luminance * 2) / 5;
        mixed.clamp(56, 208) as u8
    })
}

#[cfg(test)]
mod tests {
    use super::{point_in_polygon, shade, soften};

    #[test]
    fn point_in_polygon_handles_concave_footprint() {
        let ring = [
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ];

        assert!(point_in_polygon((0.5, 3.0), &ring));
        assert!(point_in_polygon((3.0, 0.5), &ring));
        assert!(!point_in_polygon((3.0, 3.0), &ring));
    }

    #[test]
    fn shading_keeps_pixel_palette_steps() {
        assert_eq!(shade(127, 1.0), 128);
        assert_eq!(shade(255, 1.1), 255);
        assert_eq!(shade(64, 0.5), 32);
    }

    #[test]
    fn facade_palette_rejects_extreme_saturation_and_brightness() {
        assert_eq!(soften([255, 0, 0]), [183, 56, 56]);
        assert_eq!(soften([255, 255, 255]), [208, 208, 208]);
        assert_eq!(soften([0, 0, 0]), [56, 56, 56]);
    }
}
