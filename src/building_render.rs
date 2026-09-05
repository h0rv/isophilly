use tiny_skia::Pixmap;

use crate::{
    palette,
    projection::Projection,
    texture::AerialTile,
    world::{
        Building, BuildingContext, BuildingKind as WorldBuildingKind, BuildingPart, Ring, RoofShape,
    },
};

const TILE_SIZE: usize = 256;
const MIN_FACE_AREA: f32 = 0.3;

pub fn draw_city_buildings<'a>(
    pixmap: &mut Pixmap,
    buildings: impl IntoIterator<Item = (&'a Building, &'a BuildingContext)>,
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
    for (building, context) in buildings {
        rasterizer.draw_building(building, context);
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
    fn draw_building(&mut self, building: &Building, context: &BuildingContext) {
        if building.ring.points.len() < 3 {
            return;
        }
        let seed = facade_seed(building.ring.center());
        let style = self.contextual_building_style(&building.ring, building.height, seed, context);
        for index in 0..building.ring.points.len() {
            let left = building.ring.points[index];
            let right = building.ring.points[(index + 1) % building.ring.points.len()];
            self.draw_wall(left, right, 0.0, building.height, index, style);
        }
        self.draw_flat_roof(&building.ring, building.height, style);
        self.draw_roof_furniture(&building.ring, building.height, style);
    }

    fn draw_part(&mut self, part: &BuildingPart) {
        if part.ring.points.len() < 3 {
            return;
        }
        let wall_top = (part.height - part.roof_height).max(part.min_height);
        let style = self.building_style(&part.ring, wall_top - part.min_height, part.osm_id);
        for index in 0..part.ring.points.len() {
            let left = part.ring.points[index];
            let right = part.ring.points[(index + 1) % part.ring.points.len()];
            self.draw_wall(left, right, part.min_height, wall_top, index, style);
        }
        if part.roof_shape == RoofShape::Flat || part.roof_height <= f32::EPSILON {
            self.draw_flat_roof(&part.ring, part.height, style);
            self.draw_roof_furniture(&part.ring, part.height, style);
        } else {
            self.draw_pitched_roof(part, wall_top, style.facade);
        }
    }

    fn building_style(&self, ring: &Ring, height: f32, seed: u64) -> BuildingStyle {
        let kind = classify_building(ring, height);
        let sampled = self.facade_palette(ring);
        BuildingStyle {
            kind,
            facade: wall_material(
                sampled,
                ring.center(),
                seed,
                block_seed(ring.center()),
                kind,
            ),
            seed,
            short_side: ring.bounds.width().min(ring.bounds.height()),
            party_edge_mask: 0,
        }
    }

    fn contextual_building_style(
        &self,
        ring: &Ring,
        height: f32,
        seed: u64,
        context: &BuildingContext,
    ) -> BuildingStyle {
        let kind = match context.kind {
            WorldBuildingKind::Rowhouse
            | WorldBuildingKind::RowhouseLike
            | WorldBuildingKind::Twin => FacadeKind::Rowhouse,
            WorldBuildingKind::Warehouse => FacadeKind::Industrial,
            WorldBuildingKind::Detached | WorldBuildingKind::Generic => {
                classify_building(ring, height)
            }
        };
        let material_group_seed = match context.kind {
            WorldBuildingKind::Rowhouse
            | WorldBuildingKind::RowhouseLike
            | WorldBuildingKind::Twin => context.material_group_seed,
            _ => block_seed(ring.center()),
        };
        let sampled = self.facade_palette(ring);
        BuildingStyle {
            kind,
            facade: wall_material(sampled, ring.center(), seed, material_group_seed, kind),
            seed,
            short_side: ring.bounds.width().min(ring.bounds.height()),
            party_edge_mask: context.party_edge_mask,
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
            palette::soften(std::array::from_fn(|channel| {
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
        edge_index: usize,
        building: BuildingStyle,
    ) {
        let ground_left = Vertex::world(left, bottom, self.projection);
        let ground_right = Vertex::world(right, bottom, self.projection);
        let roof_left = Vertex::world(left, top, self.projection);
        let roof_right = Vertex::world(right, top, self.projection);
        let edge = (right.0 - left.0, right.1 - left.1);
        let light = wall_light(edge);
        let edge_length = edge.0.hypot(edge.1);
        let style = WallStyle {
            facade: building.facade,
            kind: building.kind,
            frontage: building.kind == FacadeKind::Rowhouse
                && (edge_index >= u64::BITS as usize
                    || building.party_edge_mask & (1_u64 << edge_index) == 0)
                && edge_length <= (building.short_side * 1.35).max(4.5),
            light,
            bottom,
            top,
            left,
            right,
            seed: building.seed ^ facade_seed(((left.0 + right.0) * 0.5, (left.1 + right.1) * 0.5)),
        };
        self.draw_wall_triangle([ground_left, ground_right, roof_right], style);
        self.draw_wall_triangle([ground_left, roof_right, roof_left], style);
    }

    fn draw_flat_roof(&mut self, ring: &Ring, height: f32, style: BuildingStyle) {
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
                    .unwrap_or(style.facade);
                let color = roof_material(color, source, ring, style);
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

    fn draw_roof_furniture(&mut self, ring: &Ring, roof_height: f32, style: BuildingStyle) {
        let area = polygon_area(&ring.points);
        let count = match style.kind {
            FacadeKind::Rowhouse if style.seed.is_multiple_of(4) && area >= 480.0 => 1,
            FacadeKind::Industrial if area >= 26_900.0 => 3,
            FacadeKind::Industrial if area >= 7_000.0 => 2,
            FacadeKind::MidRise | FacadeKind::Tower if area >= 3_770.0 => 1,
            FacadeKind::LowRise if area >= 10_760.0 => 1,
            _ => 0,
        };
        for index in 0..count {
            let Some((equipment, height)) = roof_feature(ring, style, index, count) else {
                continue;
            };
            let equipment_style = BuildingStyle {
                kind: FacadeKind::LowRise,
                facade: palette::mix(style.facade, [135, 137, 132], 0.62),
                seed: style.seed.rotate_left(index as u32 + 7),
                short_side: equipment.bounds.width().min(equipment.bounds.height()),
                party_edge_mask: 0,
            };
            for edge_index in 0..equipment.points.len() {
                let left = equipment.points[edge_index];
                let right = equipment.points[(edge_index + 1) % equipment.points.len()];
                self.draw_wall(
                    left,
                    right,
                    roof_height,
                    roof_height + height,
                    edge_index,
                    equipment_style,
                );
            }
            self.draw_flat_roof(&equipment, roof_height + height, equipment_style);
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
    kind: FacadeKind,
    frontage: bool,
    light: f32,
    bottom: f32,
    top: f32,
    left: (f32, f32),
    right: (f32, f32),
    seed: u64,
}

#[derive(Clone, Copy)]
struct BuildingStyle {
    kind: FacadeKind,
    facade: [u8; 3],
    seed: u64,
    short_side: f32,
    party_edge_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FacadeKind {
    Rowhouse,
    LowRise,
    Industrial,
    MidRise,
    Tower,
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

fn facade_seed(center: (f32, f32)) -> u64 {
    (center.0.round() as u64).wrapping_mul(0x9e37_79b9) ^ (center.1.round() as u64).rotate_left(23)
}

fn classify_building(ring: &Ring, height: f32) -> FacadeKind {
    let width = ring.bounds.width().abs();
    let depth = ring.bounds.height().abs();
    let short = width.min(depth);
    let long = width.max(depth);
    let area = polygon_area(&ring.points);
    let box_area = width * depth;
    let compactness = if box_area > f32::EPSILON {
        area / box_area
    } else {
        0.0
    };
    if (5.5..=16.0).contains(&height)
        && (320.0..=2_580.0).contains(&area)
        && (10.0..=38.0).contains(&short)
        && (23.0..=105.0).contains(&long)
        && long / short.max(0.1) >= 1.25
        && compactness >= 0.5
    {
        FacadeKind::Rowhouse
    } else if height <= 18.0 && (area >= 7_000.0 || short >= 59.0) {
        FacadeKind::Industrial
    } else if height >= 55.0 || (height >= 30.0 && height / short.max(1.0) >= 2.5) {
        FacadeKind::Tower
    } else if height >= 18.0 {
        FacadeKind::MidRise
    } else {
        FacadeKind::LowRise
    }
}

fn polygon_area(points: &[(f32, f32)]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(left, right)| left.0 * right.1 - right.0 * left.1)
        .sum::<f32>()
        .abs()
        * 0.5
}

// The citywide walls are deliberately illustrative. Their material starts with the
// building's robust aerial-derived roof palette, then gets a small deterministic
// variation so long blocks do not read as a single sheet of painted cardboard.
fn wall_material(
    facade: [u8; 3],
    center: (f32, f32),
    seed: u64,
    material_group_seed: u64,
    kind: FacadeKind,
) -> [u8; 3] {
    let block = block_seed(center);
    let (reference, amount) = match kind {
        FacadeKind::Rowhouse => {
            let family = &palette::ROWHOUSE_FAMILIES
                [(material_group_seed as usize) % palette::ROWHOUSE_FAMILIES.len()];
            let variant = (seed as usize) % family.len();
            (family[variant], 0.42)
        }
        FacadeKind::LowRise => (
            palette::LOW_RISE_FACADES[(block as usize) % palette::LOW_RISE_FACADES.len()],
            0.24,
        ),
        FacadeKind::Industrial => (
            palette::INDUSTRIAL_FACADES[(block as usize) % palette::INDUSTRIAL_FACADES.len()],
            0.3,
        ),
        FacadeKind::MidRise | FacadeKind::Tower => (
            palette::HIGH_RISE_FACADES[(block as usize) % palette::HIGH_RISE_FACADES.len()],
            0.18,
        ),
    };
    let tone = 0.94 + f32::from((seed & 7) as u8) * 0.014;
    palette::scale(palette::mix(facade, reference, amount), tone)
}

fn block_seed(center: (f32, f32)) -> u64 {
    // A block-scale seed makes neighboring inferred facades belong to the same
    // material family while the per-building seed supplies restrained variance.
    let block = ((center.0 / 400.0).floor(), (center.1 / 400.0).floor());
    facade_seed(block)
}

fn roof_material(aerial: [u8; 3], point: (f32, f32), ring: &Ring, style: BuildingStyle) -> [u8; 3] {
    let (reference, amount) = match style.kind {
        FacadeKind::Rowhouse => ([112, 105, 94], 0.38),
        FacadeKind::Industrial => ([170, 166, 151], 0.3),
        FacadeKind::LowRise => (style.facade, 0.2),
        FacadeKind::MidRise | FacadeKind::Tower => ([159, 158, 151], 0.18),
    };
    let mut color = palette::mix(aerial, reference, amount);
    if style.kind == FacadeKind::Rowhouse && distance_to_ring(point, &ring.points) <= 1.4 {
        color = palette::scale(palette::mix(color, style.facade, 0.32), 0.8);
    }
    color
}

fn roof_feature(
    ring: &Ring,
    style: BuildingStyle,
    index: usize,
    count: usize,
) -> Option<(Ring, f32)> {
    let width = ring.bounds.width();
    let depth = ring.bounds.height();
    if width <= 2.0 || depth <= 2.0 {
        return None;
    }
    let (feature_width, feature_depth, height) = match style.kind {
        FacadeKind::Rowhouse => (width.min(3.8), depth.min(3.8), 1.15),
        FacadeKind::Industrial => (
            (width * 0.1).clamp(8.0, 26.0),
            (depth * 0.12).clamp(8.0, 33.0),
            1.4 + (style.seed & 3) as f32 * 0.35,
        ),
        FacadeKind::MidRise | FacadeKind::Tower => (
            (width * 0.2).clamp(10.0, 40.0),
            (depth * 0.18).clamp(10.0, 40.0),
            2.4 + (style.seed & 3) as f32 * 0.7,
        ),
        FacadeKind::LowRise => (
            (width * 0.12).clamp(8.0, 26.0),
            (depth * 0.12).clamp(8.0, 26.0),
            1.5,
        ),
    };
    let slot = (index + 1) as f32 / (count + 1) as f32;
    let jitter_x = (((style.seed >> (index * 5)) & 7) as f32 - 3.5) * 0.012;
    let jitter_y = (((style.seed >> (index * 5 + 3)) & 7) as f32 - 3.5) * 0.012;
    let center_x = ring.bounds.min_x + width * (0.26 + slot * 0.48 + jitter_x);
    let center_y = ring.bounds.min_y + depth * (0.68 + jitter_y);
    let bounds = crate::world::Bounds {
        min_x: center_x - feature_width * 0.5,
        min_y: center_y - feature_depth * 0.5,
        max_x: center_x + feature_width * 0.5,
        max_y: center_y + feature_depth * 0.5,
    };
    let points = vec![
        (bounds.min_x, bounds.min_y),
        (bounds.max_x, bounds.min_y),
        (bounds.max_x, bounds.max_y),
        (bounds.min_x, bounds.max_y),
    ];
    points
        .iter()
        .all(|&point| ring.contains(point))
        .then_some((Ring { bounds, points }, height))
}

fn distance_to_ring(point: (f32, f32), ring: &[(f32, f32)]) -> f32 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .take(ring.len())
        .map(|(&left, &right)| distance_to_segment(point, left, right))
        .fold(f32::INFINITY, f32::min)
}

fn distance_to_segment(point: (f32, f32), left: (f32, f32), right: (f32, f32)) -> f32 {
    let edge = (right.0 - left.0, right.1 - left.1);
    let length_squared = edge.0.mul_add(edge.0, edge.1 * edge.1);
    if length_squared <= f32::EPSILON {
        return (point.0 - left.0).hypot(point.1 - left.1);
    }
    let amount = (((point.0 - left.0) * edge.0 + (point.1 - left.1) * edge.1) / length_squared)
        .clamp(0.0, 1.0);
    let closest = (left.0 + edge.0 * amount, left.1 + edge.1 * amount);
    (point.0 - closest.0).hypot(point.1 - closest.1)
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
    if style.kind == FacadeKind::Rowhouse {
        return rowhouse_detail(point, z, style);
    }
    if style.kind == FacadeKind::Industrial {
        return industrial_detail(point, z, style);
    }
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
    let bay_width = 8.7 + f32::from(((style.seed >> 13) & 7) as u8) * 0.36;
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
        return palette::mix(
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

fn industrial_detail(point: (f32, f32), z: f32, style: WallStyle) -> [u8; 3] {
    let height = style.top - style.bottom;
    let edge = (style.right.0 - style.left.0, style.right.1 - style.left.1);
    let length = edge.0.hypot(edge.1);
    if height < 4.0 || length < 8.0 {
        return style.facade;
    }
    let along = ((point.0 - style.left.0) * edge.0 + (point.1 - style.left.1) * edge.1) / length;
    let bay_width = 17.0 + f32::from((style.seed & 7) as u8);
    let bay = along.rem_euclid(bay_width);
    if bay < 0.7 || bay > bay_width - 0.7 {
        return style.facade.map(|channel| scale_channel(channel, 0.78));
    }
    let relative_z = z - style.bottom;
    if relative_z < height.min(4.2) && (2.4..=bay_width - 2.4).contains(&bay) {
        return palette::mix(style.facade, [74, 78, 77], 0.36);
    }
    if height - relative_z < 0.35 {
        return style.facade.map(|channel| scale_channel(channel, 0.76));
    }
    style.facade
}

fn rowhouse_detail(point: (f32, f32), z: f32, style: WallStyle) -> [u8; 3] {
    let height = style.top - style.bottom;
    if height < 5.5 {
        return style.facade;
    }
    let relative_z = z - style.bottom;
    let edge = (style.right.0 - style.left.0, style.right.1 - style.left.1);
    let length = edge.0.hypot(edge.1);
    if length <= f32::EPSILON {
        return style.facade;
    }
    let along = ((point.0 - style.left.0) * edge.0 + (point.1 - style.left.1) * edge.1) / length;

    // A dark cornice and aligned floor courses make a run of simple boxes read
    // as Philadelphia rowhouses even when no facade photograph exists.
    if height - relative_z < 0.42 {
        return style.facade.map(|channel| scale_channel(channel, 0.7));
    }
    let floor_height = 2.85 + f32::from(((style.seed >> 7) & 3) as u8) * 0.1;
    let floor = relative_z.rem_euclid(floor_height);
    if floor < 0.11 {
        return style.facade.map(|channel| scale_channel(channel, 0.84));
    }
    if !style.frontage {
        // Long party/side walls stay mostly solid. A restrained brick course
        // supplies scale without painting implausible repeating windows.
        return if relative_z.rem_euclid(0.72) < 0.055 {
            style.facade.map(|channel| scale_channel(channel, 0.94))
        } else {
            style.facade
        };
    }

    let bays = if length < 6.4 { 2.0 } else { 3.0 };
    let bay_width = length / bays;
    let column = along.rem_euclid(bay_width);
    let window_left = bay_width * 0.22;
    let window_right = bay_width * 0.78;
    let upper_window = (0.76..=2.22).contains(&floor)
        && (window_left..=window_right).contains(&column)
        && relative_z >= floor_height * 0.72;
    let ground_window = (0.92..=2.3).contains(&relative_z)
        && along >= bay_width * 1.12
        && (window_left..=window_right).contains(&column);
    let door = (0.12..=2.55).contains(&relative_z)
        && (bay_width * 0.2..=bay_width * 0.68).contains(&along);
    if door {
        return palette::mix(style.facade, [66, 58, 52], 0.68);
    }
    if upper_window || ground_window {
        let glass = if style.seed & 1 == 0 {
            [62, 78, 86]
        } else {
            [73, 76, 74]
        };
        return palette::mix(style.facade, glass, 0.58);
    }
    style.facade
}

#[cfg(test)]
mod tests {
    use super::{
        FacadeKind, WallStyle, block_seed, classify_building, facade_detail, point_in_polygon,
        polygon_area, roof_feature, shade, wall_light, wall_material, wall_surface_light,
    };
    use crate::world::{Bounds, Ring};

    fn ring(width: f32, depth: f32) -> Ring {
        Ring {
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: width,
                max_y: depth,
            },
            points: vec![(0.0, 0.0), (width, 0.0), (width, depth), (0.0, depth)],
        }
    }

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
        assert_eq!(crate::palette::soften([255, 0, 0]), [183, 56, 56]);
        assert_eq!(crate::palette::soften([255, 255, 255]), [208, 208, 208]);
        assert_eq!(crate::palette::soften([0, 0, 0]), [56, 56, 56]);
    }

    #[test]
    fn facade_detail_adds_floor_bands_and_windows_to_tall_walls() {
        let base = [160, 144, 128];
        let style = WallStyle {
            facade: base,
            kind: FacadeKind::MidRise,
            frontage: false,
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
        let center = (820_000.0, 72_000.0);
        let group = block_seed(center);
        let first = wall_material(facade, center, 42, group, FacadeKind::LowRise);
        assert_eq!(
            first,
            wall_material(facade, center, 42, group, FacadeKind::LowRise)
        );
        assert_ne!(
            first,
            wall_material(facade, center, 43, group, FacadeKind::LowRise)
        );
        for channel in 0..3 {
            assert!((i16::from(first[channel]) - i16::from(facade[channel])).abs() <= 42);
        }
    }

    #[test]
    fn morphology_separates_rowhouses_warehouses_and_towers() {
        assert_eq!(
            classify_building(&ring(17.0, 49.0), 9.2),
            FacadeKind::Rowhouse
        );
        assert_eq!(
            classify_building(&ring(140.0, 200.0), 10.0),
            FacadeKind::Industrial
        );
        assert_eq!(
            classify_building(&ring(72.0, 80.0), 90.0),
            FacadeKind::Tower
        );
        assert!((polygon_area(&ring(17.0, 49.0).points) - 833.0).abs() < 0.01);
    }

    #[test]
    fn nearby_rowhouses_share_a_material_family_but_keep_variation() {
        let aerial = [142, 139, 130];
        let group = block_seed((820_010.0, 72_010.0));
        let left = wall_material(
            aerial,
            (820_010.0, 72_010.0),
            31,
            group,
            FacadeKind::Rowhouse,
        );
        let right = wall_material(
            aerial,
            (820_020.0, 72_020.0),
            48,
            group,
            FacadeKind::Rowhouse,
        );
        assert_eq!(
            block_seed((820_010.0, 72_010.0)),
            block_seed((820_020.0, 72_020.0))
        );
        assert_ne!(left, right);
        assert!(left[0] > left[2] && right[0] > right[2]);
    }

    #[test]
    fn rowhouse_fronts_get_openings_but_long_sides_stay_solid() {
        let base = [152, 91, 68];
        let common = WallStyle {
            facade: base,
            kind: FacadeKind::Rowhouse,
            frontage: true,
            light: 1.0,
            bottom: 0.0,
            top: 9.0,
            left: (0.0, 0.0),
            right: (6.0, 0.0),
            seed: 0,
        };
        assert_ne!(facade_detail((2.0, 0.0), 4.4, common), base);
        assert_eq!(
            facade_detail(
                (2.0, 0.0),
                4.4,
                WallStyle {
                    frontage: false,
                    ..common
                }
            ),
            base
        );
    }

    #[test]
    fn synthesized_roof_feature_is_deterministic_and_inside_its_roof() -> Result<(), &'static str> {
        let roof = ring(40.0, 30.0);
        let style = super::BuildingStyle {
            kind: FacadeKind::Industrial,
            facade: [160, 150, 140],
            seed: 42,
            short_side: 30.0,
            party_edge_mask: 0,
        };
        let first = roof_feature(&roof, style, 0, 2);
        let second = roof_feature(&roof, style, 0, 2);
        let (feature, height) = first.ok_or("roof feature")?;

        assert_eq!(feature.points, second.ok_or("roof feature")?.0.points);
        assert!(feature.points.iter().all(|&point| roof.contains(point)));
        assert!((1.4..=2.45).contains(&height));
        Ok::<(), &'static str>(())
    }

    #[test]
    fn wall_lighting_uses_orientation_and_vertical_depth() {
        assert_ne!(wall_light((10.0, 0.0)), wall_light((10.0, 10.0)));
        let style = WallStyle {
            facade: [160, 144, 128],
            kind: FacadeKind::MidRise,
            frontage: false,
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
