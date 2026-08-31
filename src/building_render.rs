use tiny_skia::Pixmap;

use crate::{
    projection::Projection,
    texture::AerialTile,
    world::{Building, BuildingPart, Ring, RoofShape},
};

const TILE_SIZE: usize = 256;
const MIN_FACE_AREA: f32 = 0.3;

pub fn draw_city_buildings<'a>(
    pixmap: &mut Pixmap,
    buildings: impl IntoIterator<Item = &'a Building>,
    projection: &Projection,
    aerial: &AerialTile,
    block_size: f32,
    depth: &mut [f32],
) {
    let mut rasterizer = Rasterizer {
        pixmap,
        projection,
        aerial,
        block_size,
        depth,
    };
    for building in buildings {
        rasterizer.draw_building(building);
    }
}

pub fn draw_city_building_parts<'a>(
    pixmap: &mut Pixmap,
    parts: impl IntoIterator<Item = &'a BuildingPart>,
    projection: &Projection,
    aerial: &AerialTile,
    block_size: f32,
    depth: &mut [f32],
) {
    let mut rasterizer = Rasterizer {
        pixmap,
        projection,
        aerial,
        block_size,
        depth,
    };
    for part in parts {
        rasterizer.draw_part(part);
    }
}

struct Rasterizer<'a, 'b> {
    pixmap: &'a mut Pixmap,
    projection: &'b Projection,
    aerial: &'b AerialTile,
    block_size: f32,
    depth: &'a mut [f32],
}

impl Rasterizer<'_, '_> {
    fn draw_building(&mut self, building: &Building) {
        if building.ring.points.len() < 3 {
            return;
        }
        let facade = self.facade_palette(&building.ring);
        let seed = facade_seed(building.ring.center());
        for index in 0..building.ring.points.len() {
            let left = building.ring.points[index];
            let right = building.ring.points[(index + 1) % building.ring.points.len()];
            self.draw_wall(left, right, 0.0, building.height, facade, seed);
        }
        self.draw_flat_roof(&building.ring, building.height, facade);
    }

    fn draw_part(&mut self, part: &BuildingPart) {
        if part.ring.points.len() < 3 {
            return;
        }
        let facade = self.facade_palette(&part.ring);
        let wall_top = (part.height - part.roof_height).max(part.min_height);
        for index in 0..part.ring.points.len() {
            let left = part.ring.points[index];
            let right = part.ring.points[(index + 1) % part.ring.points.len()];
            self.draw_wall(left, right, part.min_height, wall_top, facade, part.osm_id);
        }
        if part.roof_shape == RoofShape::Flat || part.roof_height <= f32::EPSILON {
            self.draw_flat_roof(&part.ring, part.height, facade);
        } else {
            self.draw_pitched_roof(part, wall_top, facade);
        }
    }

    fn facade_palette(&self, ring: &Ring) -> [u8; 3] {
        let center = ring.center();
        let mut samples = Vec::with_capacity(ring.points.len() * 2 + 1);
        if let Some(sample) = self.aerial.sample(center.0, center.1, self.block_size) {
            samples.push(sample);
        }
        for &point in &ring.points {
            for amount in [0.35, 0.7] {
                let inside = (
                    point.0 + (center.0 - point.0) * amount,
                    point.1 + (center.1 - point.1) * amount,
                );
                if let Some(sample) = self.aerial.sample(inside.0, inside.1, self.block_size) {
                    samples.push(sample);
                }
            }
        }
        samples.retain(|sample| {
            let luminance =
                u32::from(sample[0]) * 3 + u32::from(sample[1]) * 6 + u32::from(sample[2]);
            (400..=2_200).contains(&luminance)
        });
        if samples.is_empty() {
            [128, 128, 128]
        } else {
            soften(std::array::from_fn(|channel| {
                samples.sort_unstable_by_key(|sample| sample[channel]);
                samples[samples.len() / 2][channel]
            }))
        }
    }

    fn draw_wall(
        &mut self,
        left: (f32, f32),
        right: (f32, f32),
        bottom: f32,
        top: f32,
        facade: [u8; 3],
        seed: u64,
    ) {
        let ground_left = Vertex::world(left, bottom, self.projection);
        let ground_right = Vertex::world(right, bottom, self.projection);
        let roof_left = Vertex::world(left, top, self.projection);
        let roof_right = Vertex::world(right, top, self.projection);
        let edge = (right.0 - left.0, right.1 - left.1);
        let light = wall_light(edge);
        let style = WallStyle {
            facade: wall_material(facade, seed, top - bottom),
            light,
            bottom,
            top,
            left,
            right,
            seed: seed ^ facade_seed(((left.0 + right.0) * 0.5, (left.1 + right.1) * 0.5)),
        };
        self.draw_wall_triangle([ground_left, ground_right, roof_right], style);
        self.draw_wall_triangle([ground_left, roof_right, roof_left], style);
    }

    fn draw_flat_roof(&mut self, ring: &Ring, height: f32, fallback: [u8; 3]) {
        let projected: Vec<_> = ring
            .points
            .iter()
            .map(|&point| self.projection.point(point, height))
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
                let source = self.projection.inverse((iso_x, iso_y + height));
                if !point_in_polygon(source, &ring.points) {
                    continue;
                }
                let offset = py * TILE_SIZE + px;
                let depth = self.projection.depth(source, height);
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                let color = self
                    .aerial
                    .sample(source.0, source.1, self.block_size)
                    .unwrap_or(fallback);
                let color = mix_rgb(color, fallback, 0.18);
                self.set_pixel(offset, color.map(|channel| shade(channel, 1.02)));
            }
        }
    }

    fn draw_pitched_roof(&mut self, part: &BuildingPart, wall_top: f32, facade: [u8; 3]) {
        let center = part.ring.center();
        let apex = Vertex::world(center, part.height, self.projection);
        for index in 0..part.ring.points.len() {
            let left = part.ring.points[index];
            let right = part.ring.points[(index + 1) % part.ring.points.len()];
            let light = if index % 2 == 0 { 0.92 } else { 0.8 };
            self.draw_solid_triangle(
                [
                    Vertex::world(left, wall_top, self.projection),
                    Vertex::world(right, wall_top, self.projection),
                    apex,
                ],
                facade,
                light,
            );
        }
    }

    fn draw_wall_triangle(&mut self, triangle: [Vertex; 3], style: WallStyle) {
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
                let depth = self.projection.depth((source_x, source_y), z);
                let offset = y * TILE_SIZE + x;
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                let mut color = facade_detail((source_x, source_y), z, style);
                let base = wall_surface_light(z, style);
                for channel in &mut color {
                    *channel = scale_channel(*channel, style.light * base);
                }
                self.set_pixel(offset, color);
            }
        }
    }

    fn draw_solid_triangle(&mut self, triangle: [Vertex; 3], color: [u8; 3], light: f32) {
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
                let offset = y * TILE_SIZE + x;
                let depth = self.projection.depth((source_x, source_y), z);
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                self.set_pixel(offset, color.map(|channel| shade(channel, light)));
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
}

#[derive(Clone, Copy)]
struct WallStyle {
    facade: [u8; 3],
    light: f32,
    bottom: f32,
    top: f32,
    left: (f32, f32),
    right: (f32, f32),
    seed: u64,
}

impl Vertex {
    fn world(point: (f32, f32), z: f32, projection: &Projection) -> Self {
        let screen = projection.point(point, z);
        Self {
            screen_x: screen.0,
            screen_y: screen.1,
            source_x: point.0,
            source_y: point.1,
            z,
        }
    }

    fn screen(screen_x: f32, screen_y: f32) -> Self {
        Self {
            screen_x,
            screen_y,
            source_x: 0.0,
            source_y: 0.0,
            z: 0.0,
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

fn scale_channel(channel: u8, light: f32) -> u8 {
    (f32::from(channel) * light).round().clamp(0.0, 255.0) as u8
}

fn soften(color: [u8; 3]) -> [u8; 3] {
    let luminance = (u16::from(color[0]) * 3 + u16::from(color[1]) * 6 + u16::from(color[2])) / 10;
    std::array::from_fn(|index| {
        let mixed = (u16::from(color[index]) * 3 + luminance * 2) / 5;
        mixed.clamp(56, 208) as u8
    })
}

fn mix_rgb(left: [u8; 3], right: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|index| {
        (f32::from(left[index]) * (1.0 - amount) + f32::from(right[index]) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

fn facade_seed(center: (f32, f32)) -> u64 {
    (center.0.round() as u64).wrapping_mul(0x9e37_79b9) ^ (center.1.round() as u64).rotate_left(23)
}

// The citywide walls are deliberately illustrative. Their material starts with the
// building's robust aerial-derived roof palette, then gets a small deterministic
// variation so long blocks do not read as a single sheet of painted cardboard.
fn wall_material(facade: [u8; 3], seed: u64, height: f32) -> [u8; 3] {
    let low_rise = [
        [151, 96, 72],
        [167, 112, 82],
        [143, 145, 139],
        [188, 169, 141],
    ];
    let high_rise = [
        [151, 137, 121],
        [132, 145, 150],
        [174, 164, 145],
        [146, 142, 136],
    ];
    let references = if height <= 16.0 { low_rise } else { high_rise };
    let reference = references[(seed as usize) % references.len()];
    let amount = if height <= 16.0 {
        0.14 + f32::from(((seed >> 11) & 7) as u8) * 0.012
    } else {
        0.08 + f32::from(((seed >> 11) & 7) as u8) * 0.009
    };
    let tone = 0.94 + f32::from(((seed >> 19) & 7) as u8) * 0.011;
    mix_rgb(facade, reference, amount).map(|channel| scale_channel(channel, tone))
}

fn wall_light(edge: (f32, f32)) -> f32 {
    let length = edge.0.hypot(edge.1);
    if length <= f32::EPSILON {
        return 0.76;
    }
    let directional = ((edge.0 - edge.1) / length).abs().min(1.0);
    let exposed_side = if edge.0 + edge.1 >= 0.0 { 0.0 } else { 0.055 };
    (0.71 + directional * 0.105 + exposed_side).clamp(0.70, 0.87)
}

fn wall_surface_light(z: f32, style: WallStyle) -> f32 {
    let height = style.top - style.bottom;
    if height <= f32::EPSILON {
        return 1.0;
    }
    let relative = ((z - style.bottom) / height).clamp(0.0, 1.0);
    let mut light = 0.965 + relative * 0.065;
    if height > 5.0 {
        let ground_ao = ((z - style.bottom) / 1.8).clamp(0.0, 1.0);
        light *= 0.80 + ground_ao * 0.20;
    }
    if height >= 8.0 && style.top - z < 0.32 {
        light *= 0.86;
    }
    light
}

fn facade_detail(point: (f32, f32), z: f32, style: WallStyle) -> [u8; 3] {
    if style.top - style.bottom < 8.0 {
        return style.facade;
    }
    let edge = (style.right.0 - style.left.0, style.right.1 - style.left.1);
    let length = edge.0.hypot(edge.1);
    if length < 4.0 {
        return style.facade;
    }
    let along = ((point.0 - style.left.0) * edge.0 + (point.1 - style.left.1) * edge.1) / length;
    let floor_height = 3.0 + f32::from(((style.seed >> 7) & 3) as u8) * 0.12;
    let floor = (z - style.bottom).rem_euclid(floor_height);
    let bay_width = 2.65 + f32::from(((style.seed >> 13) & 7) as u8) * 0.11;
    let column = (along + (style.seed % 13) as f32 * 0.19).rem_euclid(bay_width);
    if !(0.18..=floor_height - 0.18).contains(&floor) {
        return style.facade.map(|channel| scale_channel(channel, 0.84));
    }
    let window_right = bay_width - 0.65;
    if (0.76..=2.42).contains(&floor) && (0.48..=window_right).contains(&column) {
        let glass = if style.seed & 1 == 0 {
            [72, 88, 96]
        } else {
            [82, 85, 82]
        };
        return mix_rgb(
            style.facade,
            glass,
            if style.top - style.bottom >= 30.0 {
                0.48
            } else {
                0.34
            },
        );
    }
    style.facade
}

#[cfg(test)]
mod tests {
    use super::{
        WallStyle, facade_detail, point_in_polygon, shade, soften, wall_light, wall_material,
        wall_surface_light,
    };

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

    #[test]
    fn facade_detail_adds_floor_bands_and_windows_to_tall_walls() {
        let base = [160, 144, 128];
        let style = WallStyle {
            facade: base,
            light: 1.0,
            bottom: 0.0,
            top: 40.0,
            left: (0.0, 0.0),
            right: (20.0, 0.0),
            seed: 0,
        };
        let band = facade_detail((3.0, 0.0), 3.0, style);
        let window = facade_detail((1.0, 0.0), 1.5, style);

        assert_ne!(band, base);
        assert_ne!(window, base);
        assert_ne!(window, band);
    }

    #[test]
    fn wall_material_is_stable_and_keeps_the_aerial_palette_dominant() {
        let facade = [160, 144, 128];
        let first = wall_material(facade, 42, 10.0);
        assert_eq!(first, wall_material(facade, 42, 10.0));
        assert_ne!(first, wall_material(facade, 43, 10.0));
        for channel in 0..3 {
            assert!((i16::from(first[channel]) - i16::from(facade[channel])).abs() <= 32);
        }
    }

    #[test]
    fn wall_lighting_uses_orientation_and_vertical_depth() {
        assert_ne!(wall_light((10.0, 0.0)), wall_light((10.0, 10.0)));
        let style = WallStyle {
            facade: [160, 144, 128],
            light: 1.0,
            bottom: 0.0,
            top: 20.0,
            left: (0.0, 0.0),
            right: (20.0, 0.0),
            seed: 0,
        };
        assert!(wall_surface_light(0.0, style) < wall_surface_light(10.0, style));
        assert!(wall_surface_light(19.9, style) < wall_surface_light(10.0, style));
    }
}
