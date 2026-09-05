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
const ROWHOUSE_AREA_SQUARE_METERS: std::ops::RangeInclusive<f32> = 29.728_973..=239.689_85;
const ROWHOUSE_SHORT_SIDE_METERS: std::ops::RangeInclusive<f32> = 3.048..=11.582_4;
const ROWHOUSE_LONG_SIDE_METERS: std::ops::RangeInclusive<f32> = 7.010_4..=32.004;
const INDUSTRIAL_AREA_SQUARE_METERS: f32 = 650.321_3; // 7,000 sq ft
const INDUSTRIAL_SHORT_SIDE_METERS: f32 = 17.983_2; // 59 ft
const ROWHOUSE_ROOF_FEATURE_AREA_SQUARE_METERS: f32 = 44.593_46; // 480 sq ft
const LARGE_INDUSTRIAL_ROOF_AREA_SQUARE_METERS: f32 = 2_499.091_8; // 26,900 sq ft
const MIDRISE_ROOF_FEATURE_AREA_SQUARE_METERS: f32 = 350.244_45; // 3,770 sq ft
const LOW_RISE_ROOF_FEATURE_AREA_SQUARE_METERS: f32 = 999.636_7; // 10,760 sq ft
const MINIMUM_ROOF_FEATURE_SIDE_METERS: f32 = 0.609_6; // 2 ft
const ROWHOUSE_ROOF_FEATURE_MAX_SIDE_METERS: f32 = 1.158_24; // 3.8 ft
const INDUSTRIAL_ROOF_FEATURE_SIDE_METERS: std::ops::RangeInclusive<f32> = 2.438_4..=7.924_8;
const INDUSTRIAL_ROOF_FEATURE_DEPTH_METERS: std::ops::RangeInclusive<f32> = 2.438_4..=10.058_4;
const TALL_ROOF_FEATURE_SIDE_METERS: std::ops::RangeInclusive<f32> = 3.048..=12.192;
// These intentionally use the packed EPSG:32129 metre coordinates, rather
// than the feet and square-feet values from the source records.
const INFERRED_ROOF_AREA_SQUARE_METERS: std::ops::RangeInclusive<f32> = 36.0..=300.0;
const INFERRED_ROOF_SHORT_SIDE_METERS: std::ops::RangeInclusive<f32> = 4.8..=15.0;
const INFERRED_ROOF_LONG_SIDE_METERS: std::ops::RangeInclusive<f32> = 4.8..=25.0;
const INFERRED_GABLE_RATIO: f32 = 1.4;
const INFERRED_HIP_RATIO: f32 = 1.3;
const INFERRED_ROOF_MIN_RISE_METERS: f32 = 1.0;
const INFERRED_ROOF_MAX_RISE_METERS: f32 = 2.8;
const ROWHOUSE_CORNICE_EDGE_METERS: std::ops::RangeInclusive<f32> = 3.048..=9.144;
const ROWHOUSE_CORNICE_OUTSET_METERS: f32 = 0.24;
const ROWHOUSE_CORNICE_HEIGHT_METERS: f32 = 0.42;
const ROWHOUSE_CORNICE_NORMAL_PROBE_METERS: f32 = 0.05;
const ROWHOUSE_STOOP_LOWER_DEPTH_METERS: f32 = 0.70;
const ROWHOUSE_STOOP_UPPER_DEPTH_METERS: f32 = 0.38;
const ROWHOUSE_STOOP_LOWER_HEIGHT_METERS: f32 = 0.18;
const ROWHOUSE_STOOP_UPPER_HEIGHT_METERS: f32 = 0.36;
const ROWHOUSE_STOOP_LOWER_WIDTH_METERS: f32 = 1.25;
const ROWHOUSE_STOOP_UPPER_WIDTH_METERS: f32 = 1.0;
const ROWHOUSE_STOOP_SIDE_CLEARANCE_METERS: f32 = 0.45;
const ROWHOUSE_STOOP_MINIMUM_WIDTH_METERS: f32 = 0.85;
const ROWHOUSE_WINDOW_TRIM_WIDTH_METERS: f32 = 0.24;
const ROWHOUSE_WINDOW_TRIM_STORIES: std::ops::RangeInclusive<usize> = 1..=2;

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
        let style = self.contextual_building_style(
            &building.ring,
            building.height,
            seed,
            context,
            building.frontage_edge.map(usize::from),
        );
        let entry = rowhouse_entry_layout(building, context, style);
        let inferred_roof = infer_pitched_roof(building, context);
        let wall_top = inferred_roof.map_or(building.height, |roof| roof.wall_top);
        for index in 0..building.ring.points.len() {
            let left = building.ring.points[index];
            let right = building.ring.points[(index + 1) % building.ring.points.len()];
            self.draw_wall(
                left,
                right,
                (0.0, wall_top),
                index,
                style,
                entry.filter(|entry| entry.edge_index == index),
            );
        }
        if let Some(entry) = entry {
            self.draw_rowhouse_entry_stoop(entry, style);
        }
        if let Some(roof) = inferred_roof {
            self.draw_inferred_pitched_roof(&building.ring, roof, style);
        } else {
            self.draw_rowhouse_cornices(building, context, style);
            self.draw_flat_roof(&building.ring, building.height, style);
            self.draw_roof_furniture(&building.ring, building.height, style);
        }
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
            self.draw_wall(left, right, (part.min_height, wall_top), index, style, None);
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
            frontage_edge: None,
        }
    }

    fn contextual_building_style(
        &self,
        ring: &Ring,
        height: f32,
        seed: u64,
        context: &BuildingContext,
        frontage_edge: Option<usize>,
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
            frontage_edge: if context.kind == WorldBuildingKind::Rowhouse {
                frontage_edge
            } else {
                None
            },
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
        vertical: (f32, f32),
        edge_index: usize,
        building: BuildingStyle,
        entry: Option<RowhouseEntryLayout>,
    ) {
        let (bottom, top) = vertical;
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
            frontage: rowhouse_frontage_edge(building, edge_index, edge_length),
            light,
            bottom,
            top,
            left,
            right,
            seed: wall_seed(building.seed, left, right),
            entry,
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

    fn draw_rowhouse_cornices(
        &mut self,
        building: &Building,
        context: &BuildingContext,
        style: BuildingStyle,
    ) {
        if context.kind != WorldBuildingKind::Rowhouse {
            return;
        }
        let color = palette::scale(style.facade, 0.72);
        for edge_index in 0..building.ring.points.len() {
            let Some(cornice) = rowhouse_cornice_segment(building, context, edge_index) else {
                continue;
            };
            let inner_left_bottom =
                Vertex::world(cornice.inner[0], cornice.bottom, self.projection);
            let inner_right_bottom =
                Vertex::world(cornice.inner[1], cornice.bottom, self.projection);
            let inner_left_top = Vertex::world(cornice.inner[0], cornice.top, self.projection);
            let inner_right_top = Vertex::world(cornice.inner[1], cornice.top, self.projection);
            let outer_left_bottom =
                Vertex::world(cornice.outer[0], cornice.bottom, self.projection);
            let outer_right_bottom =
                Vertex::world(cornice.outer[1], cornice.bottom, self.projection);
            let outer_left_top = Vertex::world(cornice.outer[0], cornice.top, self.projection);
            let outer_right_top = Vertex::world(cornice.outer[1], cornice.top, self.projection);

            self.draw_solid_quad(
                [
                    outer_left_bottom,
                    outer_right_bottom,
                    outer_right_top,
                    outer_left_top,
                ],
                color,
                wall_light((
                    cornice.inner[1].0 - cornice.inner[0].0,
                    cornice.inner[1].1 - cornice.inner[0].1,
                )),
            );
            self.draw_solid_quad(
                [
                    inner_left_top,
                    inner_right_top,
                    outer_right_top,
                    outer_left_top,
                ],
                color,
                1.02,
            );
            self.draw_solid_quad(
                [
                    inner_left_bottom,
                    outer_left_bottom,
                    outer_right_bottom,
                    inner_right_bottom,
                ],
                color,
                0.68,
            );
            self.draw_solid_quad(
                [
                    inner_left_bottom,
                    inner_left_top,
                    outer_left_top,
                    outer_left_bottom,
                ],
                color,
                0.78,
            );
            self.draw_solid_quad(
                [
                    inner_right_bottom,
                    outer_right_bottom,
                    outer_right_top,
                    inner_right_top,
                ],
                color,
                0.78,
            );
        }
    }

    fn draw_rowhouse_entry_stoop(&mut self, entry: RowhouseEntryLayout, style: BuildingStyle) {
        let stone = palette::mix(style.facade, [132, 126, 116], 0.46);
        for tier in rowhouse_entry_stoop(entry) {
            let inner_left = Vertex::world(tier.inner[0], tier.height, self.projection);
            let inner_right = Vertex::world(tier.inner[1], tier.height, self.projection);
            let outer_left = Vertex::world(tier.outer[0], tier.height, self.projection);
            let outer_right = Vertex::world(tier.outer[1], tier.height, self.projection);
            let outer_left_ground = Vertex::world(tier.outer[0], 0.0, self.projection);
            let outer_right_ground = Vertex::world(tier.outer[1], 0.0, self.projection);
            let inner_left_ground = Vertex::world(tier.inner[0], 0.0, self.projection);
            let inner_right_ground = Vertex::world(tier.inner[1], 0.0, self.projection);

            self.draw_solid_quad(
                [inner_left, inner_right, outer_right, outer_left],
                stone,
                1.0,
            );
            self.draw_solid_quad(
                [
                    outer_left_ground,
                    outer_right_ground,
                    outer_right,
                    outer_left,
                ],
                stone,
                wall_light((
                    tier.outer[1].0 - tier.outer[0].0,
                    tier.outer[1].1 - tier.outer[0].1,
                )),
            );
            self.draw_solid_quad(
                [inner_left_ground, outer_left_ground, outer_left, inner_left],
                stone,
                wall_light((
                    tier.outer[0].0 - tier.inner[0].0,
                    tier.outer[0].1 - tier.inner[0].1,
                )),
            );
            self.draw_solid_quad(
                [
                    outer_right_ground,
                    inner_right_ground,
                    inner_right,
                    outer_right,
                ],
                stone,
                wall_light((
                    tier.inner[1].0 - tier.outer[1].0,
                    tier.inner[1].1 - tier.outer[1].1,
                )),
            );
        }
    }

    fn draw_inferred_pitched_roof(
        &mut self,
        ring: &Ring,
        roof: InferredRoof,
        style: BuildingStyle,
    ) {
        match roof.form {
            InferredRoofForm::Hipped { apex } => {
                let apex = Vertex::world(apex, roof.roof_top, self.projection);
                for index in 0..ring.points.len() {
                    let left = ring.points[index];
                    let right = ring.points[(index + 1) % ring.points.len()];
                    self.draw_aerial_roof_triangle(
                        [
                            Vertex::world(left, roof.wall_top, self.projection),
                            Vertex::world(right, roof.wall_top, self.projection),
                            apex,
                        ],
                        ring,
                        style,
                        roof_light((right.0 - left.0, right.1 - left.1)),
                    );
                }
            }
            InferredRoofForm::Gabled { ridge, gable_edges } => {
                let low = |point| Vertex::world(point, roof.wall_top, self.projection);
                let high = |point| Vertex::world(point, roof.roof_top, self.projection);
                let points = &ring.points;
                let first_gable = gable_edges.0;
                let second_gable = gable_edges.1;
                let first_long = (first_gable + ring.points.len() - 1) % ring.points.len();
                let second_long = (first_gable + 1) % ring.points.len();
                let first_low_edge = (points[first_long], points[first_gable]);
                let second_low_edge = (points[second_gable], points[second_long]);
                self.draw_aerial_roof_triangle(
                    [low(first_low_edge.0), low(first_low_edge.1), high(ridge[0])],
                    ring,
                    style,
                    roof_light((
                        first_low_edge.1.0 - first_low_edge.0.0,
                        first_low_edge.1.1 - first_low_edge.0.1,
                    )),
                );
                self.draw_aerial_roof_triangle(
                    [low(first_low_edge.0), high(ridge[0]), high(ridge[1])],
                    ring,
                    style,
                    roof_light((
                        first_low_edge.1.0 - first_low_edge.0.0,
                        first_low_edge.1.1 - first_low_edge.0.1,
                    )),
                );
                self.draw_aerial_roof_triangle(
                    [
                        low(second_low_edge.0),
                        low(second_low_edge.1),
                        high(ridge[1]),
                    ],
                    ring,
                    style,
                    roof_light((
                        second_low_edge.1.0 - second_low_edge.0.0,
                        second_low_edge.1.1 - second_low_edge.0.1,
                    )),
                );
                self.draw_aerial_roof_triangle(
                    [low(second_low_edge.0), high(ridge[1]), high(ridge[0])],
                    ring,
                    style,
                    roof_light((
                        second_low_edge.1.0 - second_low_edge.0.0,
                        second_low_edge.1.1 - second_low_edge.0.1,
                    )),
                );
                for (edge, ridge_point) in [(first_gable, ridge[0]), (second_gable, ridge[1])] {
                    let left = points[edge];
                    let right = points[(edge + 1) % points.len()];
                    self.draw_solid_triangle(
                        [low(left), low(right), high(ridge_point)],
                        style.facade,
                        wall_light((right.0 - left.0, right.1 - left.1)),
                    );
                }
            }
        }
    }

    fn draw_roof_furniture(&mut self, ring: &Ring, roof_height: f32, style: BuildingStyle) {
        let area = polygon_area(&ring.points);
        let count = match style.kind {
            FacadeKind::Rowhouse
                if style.seed.is_multiple_of(4)
                    && area >= ROWHOUSE_ROOF_FEATURE_AREA_SQUARE_METERS =>
            {
                1
            }
            FacadeKind::Industrial if area >= LARGE_INDUSTRIAL_ROOF_AREA_SQUARE_METERS => 3,
            FacadeKind::Industrial if area >= INDUSTRIAL_AREA_SQUARE_METERS => 2,
            FacadeKind::MidRise | FacadeKind::Tower
                if area >= MIDRISE_ROOF_FEATURE_AREA_SQUARE_METERS =>
            {
                1
            }
            FacadeKind::LowRise if area >= LOW_RISE_ROOF_FEATURE_AREA_SQUARE_METERS => 1,
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
                frontage_edge: None,
            };
            for edge_index in 0..equipment.points.len() {
                let left = equipment.points[edge_index];
                let right = equipment.points[(edge_index + 1) % equipment.points.len()];
                self.draw_wall(
                    left,
                    right,
                    (roof_height, roof_height + height),
                    edge_index,
                    equipment_style,
                    None,
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

    fn draw_solid_quad(&mut self, quad: [Vertex; 4], color: [u8; 3], light: f32) {
        self.draw_solid_triangle([quad[0], quad[1], quad[2]], color, light);
        self.draw_solid_triangle([quad[0], quad[2], quad[3]], color, light);
    }

    fn draw_aerial_roof_triangle(
        &mut self,
        triangle: [Vertex; 3],
        ring: &Ring,
        style: BuildingStyle,
        light: f32,
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
                let source = (
                    interpolate(&weights, &triangle, |vertex| vertex.source_x),
                    interpolate(&weights, &triangle, |vertex| vertex.source_y),
                );
                let z = interpolate(&weights, &triangle, |vertex| vertex.z);
                let offset = y * TILE_SIZE + x;
                let depth = self.projection.depth(source, z);
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                let aerial = self
                    .aerial
                    .sample(source.0, source.1, self.block_size)
                    .unwrap_or(style.facade);
                let color = roof_material(aerial, source, ring, style);
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
    entry: Option<RowhouseEntryLayout>,
}

#[derive(Clone, Copy)]
struct BuildingStyle {
    kind: FacadeKind,
    facade: [u8; 3],
    seed: u64,
    short_side: f32,
    party_edge_mask: u64,
    frontage_edge: Option<usize>,
}

fn rowhouse_frontage_edge(style: BuildingStyle, edge_index: usize, edge_length: f32) -> bool {
    style.kind == FacadeKind::Rowhouse
        && style
            .frontage_edge
            .is_none_or(|selected| selected == edge_index)
        && (edge_index >= u64::BITS as usize || style.party_edge_mask & (1_u64 << edge_index) == 0)
        && edge_length <= (style.short_side * 1.35).max(4.5)
}

fn rowhouse_entry_layout(
    building: &Building,
    context: &BuildingContext,
    style: BuildingStyle,
) -> Option<RowhouseEntryLayout> {
    if context.kind != WorldBuildingKind::Rowhouse
        || building.height < 5.5
        || building.ring.points.len() < 3
    {
        return None;
    }
    let edge_index = usize::from(building.frontage_edge?);
    if edge_index >= building.ring.points.len()
        || edge_index >= u64::BITS as usize
        || context.party_edge_mask & (1_u64 << edge_index) != 0
    {
        return None;
    }
    let left = building.ring.points[edge_index];
    let right = building.ring.points[(edge_index + 1) % building.ring.points.len()];
    let (origin, end) = canonical_edge(left, right);
    let edge = (end.0 - origin.0, end.1 - origin.1);
    let length = edge.0.hypot(edge.1);
    if !ROWHOUSE_CORNICE_EDGE_METERS.contains(&length)
        || !rowhouse_frontage_edge(style, edge_index, length)
    {
        return None;
    }
    let outward = outward_normal(&building.ring, origin, end)?;
    let bays = if length < 6.4 { 2.0 } else { 3.0 };
    let bay_width = length / bays;
    let door_start = bay_width * 0.2;
    let door_end = bay_width * 0.68;
    let maximum_width = length - ROWHOUSE_STOOP_SIDE_CLEARANCE_METERS * 2.0;
    if maximum_width < ROWHOUSE_STOOP_MINIMUM_WIDTH_METERS {
        return None;
    }
    Some(RowhouseEntryLayout {
        edge_index,
        origin,
        unit: (edge.0 / length, edge.1 / length),
        outward,
        length,
        bay_width,
        door_start,
        door_end,
    })
}

fn canonical_edge(left: (f32, f32), right: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let order = left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1));
    if order.is_gt() {
        (right, left)
    } else {
        (left, right)
    }
}

fn outward_normal(ring: &Ring, left: (f32, f32), right: (f32, f32)) -> Option<(f32, f32)> {
    let edge = (right.0 - left.0, right.1 - left.1);
    let length = edge.0.hypot(edge.1);
    if length <= f32::EPSILON {
        return None;
    }
    let normal = (-edge.1 / length, edge.0 / length);
    let midpoint = ((left.0 + right.0) * 0.5, (left.1 + right.1) * 0.5);
    let probe = |direction: f32| {
        (
            midpoint.0 + normal.0 * ROWHOUSE_CORNICE_NORMAL_PROBE_METERS * direction,
            midpoint.1 + normal.1 * ROWHOUSE_CORNICE_NORMAL_PROBE_METERS * direction,
        )
    };
    let positive_inside = ring.contains(probe(1.0));
    let negative_inside = ring.contains(probe(-1.0));
    (positive_inside != negative_inside).then_some(if positive_inside {
        (-normal.0, -normal.1)
    } else {
        normal
    })
}

fn rowhouse_entry_stoop(entry: RowhouseEntryLayout) -> [StoopTier; 2] {
    let maximum_width = entry.length - ROWHOUSE_STOOP_SIDE_CLEARANCE_METERS * 2.0;
    let lower_width = ROWHOUSE_STOOP_LOWER_WIDTH_METERS.min(maximum_width);
    let upper_width = ROWHOUSE_STOOP_UPPER_WIDTH_METERS.min(lower_width);
    let center = (entry.door_start + entry.door_end) * 0.5;
    let tier = |width: f32, depth: f32, height: f32| {
        let left = center - width * 0.5;
        let right = center + width * 0.5;
        let point = |along: f32, outward: f32| {
            (
                entry.origin.0 + entry.unit.0 * along + entry.outward.0 * outward,
                entry.origin.1 + entry.unit.1 * along + entry.outward.1 * outward,
            )
        };
        StoopTier {
            inner: [point(left, 0.0), point(right, 0.0)],
            outer: [point(left, depth), point(right, depth)],
            height,
        }
    };
    [
        tier(
            lower_width,
            ROWHOUSE_STOOP_LOWER_DEPTH_METERS,
            ROWHOUSE_STOOP_LOWER_HEIGHT_METERS,
        ),
        tier(
            upper_width,
            ROWHOUSE_STOOP_UPPER_DEPTH_METERS,
            ROWHOUSE_STOOP_UPPER_HEIGHT_METERS,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InferredRoof {
    wall_top: f32,
    roof_top: f32,
    form: InferredRoofForm,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CorniceSegment {
    inner: [(f32, f32); 2],
    outer: [(f32, f32); 2],
    bottom: f32,
    top: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RowhouseEntryLayout {
    edge_index: usize,
    origin: (f32, f32),
    unit: (f32, f32),
    outward: (f32, f32),
    length: f32,
    bay_width: f32,
    door_start: f32,
    door_end: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StoopTier {
    inner: [(f32, f32); 2],
    outer: [(f32, f32); 2],
    height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum InferredRoofForm {
    Hipped {
        apex: (f32, f32),
    },
    Gabled {
        ridge: [(f32, f32); 2],
        gable_edges: (usize, usize),
    },
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

// City footprints do not carry roof forms. Keep this inference deliberately
// narrow: only an unattached, simple, low detached house can receive one.
// In particular, a two-member row run remains flat because the `Twin` class
// often describes two rowhouses separated by a small mapping gap.
fn infer_pitched_roof(building: &Building, context: &BuildingContext) -> Option<InferredRoof> {
    if context.kind != WorldBuildingKind::Detached
        || !(5.0..18.0).contains(&building.height)
        || building.ring.points.len() != 4
    {
        return None;
    }
    let area = polygon_area(&building.ring.points);
    if !INFERRED_ROOF_AREA_SQUARE_METERS.contains(&area) {
        return None;
    }
    let edges: [(f32, f32); 4] = std::array::from_fn(|index| {
        let left = building.ring.points[index];
        let right = building.ring.points[(index + 1) % building.ring.points.len()];
        (right.0 - left.0, right.1 - left.1)
    });
    let lengths = edges.map(|edge| edge.0.hypot(edge.1));
    let short = lengths.iter().copied().fold(f32::INFINITY, f32::min);
    let long = lengths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !short.is_finite()
        || !INFERRED_ROOF_SHORT_SIDE_METERS.contains(&short)
        || !INFERRED_ROOF_LONG_SIDE_METERS.contains(&long)
    {
        return None;
    }
    let unit = |edge: (f32, f32), length: f32| (edge.0 / length, edge.1 / length);
    let units: [(f32, f32); 4] = std::array::from_fn(|index| unit(edges[index], lengths[index]));
    let dot = |left: (f32, f32), right: (f32, f32)| left.0.mul_add(right.0, left.1 * right.1);
    if (0..4).any(|index| dot(units[index], units[(index + 1) % 4]).abs() > 0.16)
        || dot(units[0], units[2]).abs() < 0.985
        || dot(units[1], units[3]).abs() < 0.985
        || (lengths[0] - lengths[2]).abs() > lengths[0].max(lengths[2]) * 0.12
        || (lengths[1] - lengths[3]).abs() > lengths[1].max(lengths[3]) * 0.12
    {
        return None;
    }

    let rise = (short * 0.28)
        .clamp(INFERRED_ROOF_MIN_RISE_METERS, INFERRED_ROOF_MAX_RISE_METERS)
        .min(building.height * 0.25)
        .min(building.height - 3.0);
    if rise < INFERRED_ROOF_MIN_RISE_METERS {
        return None;
    }
    let wall_top = building.height - rise;
    let edge_pair_lengths = [
        (lengths[0] + lengths[2]) * 0.5,
        (lengths[1] + lengths[3]) * 0.5,
    ];
    let ratio = edge_pair_lengths[0].max(edge_pair_lengths[1])
        / edge_pair_lengths[0].min(edge_pair_lengths[1]);
    let form = if ratio >= INFERRED_GABLE_RATIO {
        let long_pair = usize::from(edge_pair_lengths[1] > edge_pair_lengths[0]);
        let gable_edges = if long_pair == 0 { (1, 3) } else { (0, 2) };
        let midpoint = |edge: usize| {
            let left = building.ring.points[edge];
            let right = building.ring.points[(edge + 1) % building.ring.points.len()];
            ((left.0 + right.0) * 0.5, (left.1 + right.1) * 0.5)
        };
        InferredRoofForm::Gabled {
            ridge: [midpoint(gable_edges.0), midpoint(gable_edges.1)],
            gable_edges,
        }
    } else if ratio <= INFERRED_HIP_RATIO {
        InferredRoofForm::Hipped {
            apex: building.ring.center(),
        }
    } else {
        return None;
    };
    Some(InferredRoof {
        wall_top,
        roof_top: building.height,
        form,
    })
}

#[cfg(test)]
fn rowhouse_cornice_segments(
    building: &Building,
    context: &BuildingContext,
) -> Vec<CorniceSegment> {
    if context.kind != WorldBuildingKind::Rowhouse
        || building.height <= ROWHOUSE_CORNICE_HEIGHT_METERS
    {
        return Vec::new();
    }
    (0..building.ring.points.len())
        .filter_map(|edge_index| rowhouse_cornice_segment(building, context, edge_index))
        .collect()
}

fn rowhouse_cornice_segment(
    building: &Building,
    context: &BuildingContext,
    edge_index: usize,
) -> Option<CorniceSegment> {
    if context.kind != WorldBuildingKind::Rowhouse
        || edge_index >= u64::BITS as usize
        || context.party_edge_mask & (1_u64 << edge_index) != 0
        || building
            .frontage_edge
            .is_some_and(|frontage_edge| usize::from(frontage_edge) != edge_index)
        || building.ring.points.len() < 3
    {
        return None;
    }
    let left = building.ring.points[edge_index];
    let right = building.ring.points[(edge_index + 1) % building.ring.points.len()];
    let edge = (right.0 - left.0, right.1 - left.1);
    let length = edge.0.hypot(edge.1);
    if !ROWHOUSE_CORNICE_EDGE_METERS.contains(&length)
        || building.height <= ROWHOUSE_CORNICE_HEIGHT_METERS
    {
        return None;
    }
    let outward = outward_normal(&building.ring, left, right)?;
    let outer = [
        (
            left.0 + outward.0 * ROWHOUSE_CORNICE_OUTSET_METERS,
            left.1 + outward.1 * ROWHOUSE_CORNICE_OUTSET_METERS,
        ),
        (
            right.0 + outward.0 * ROWHOUSE_CORNICE_OUTSET_METERS,
            right.1 + outward.1 * ROWHOUSE_CORNICE_OUTSET_METERS,
        ),
    ];
    Some(CorniceSegment {
        inner: [left, right],
        outer,
        bottom: building.height - ROWHOUSE_CORNICE_HEIGHT_METERS,
        top: building.height,
    })
}

fn roof_light(edge: (f32, f32)) -> f32 {
    let length = edge.0.hypot(edge.1);
    if length <= f32::EPSILON {
        return 0.9;
    }
    let directional = (edge.0 - edge.1).abs() / length;
    (0.82 + directional * 0.18).clamp(0.78, 1.0)
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

fn wall_seed(building_seed: u64, left: (f32, f32), right: (f32, f32)) -> u64 {
    building_seed ^ facade_seed(((left.0 + right.0) * 0.5, (left.1 + right.1) * 0.5))
}

fn rowhouse_floor_height(seed: u64) -> f32 {
    2.85 + f32::from(((seed >> 7) & 3) as u8) * 0.1
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
        && ROWHOUSE_AREA_SQUARE_METERS.contains(&area)
        && ROWHOUSE_SHORT_SIDE_METERS.contains(&short)
        && ROWHOUSE_LONG_SIDE_METERS.contains(&long)
        && long / short.max(0.1) >= 1.25
        && compactness >= 0.5
    {
        FacadeKind::Rowhouse
    } else if height <= 18.0
        && (area >= INDUSTRIAL_AREA_SQUARE_METERS || short >= INDUSTRIAL_SHORT_SIDE_METERS)
    {
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
    let origin = points[0];
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(left, right)| {
            let left_x = f64::from(left.0 - origin.0);
            let left_y = f64::from(left.1 - origin.1);
            let right_x = f64::from(right.0 - origin.0);
            let right_y = f64::from(right.1 - origin.1);
            left_x.mul_add(right_y, -(right_x * left_y))
        })
        .sum::<f64>()
        .abs()
        .mul_add(0.5, 0.0) as f32
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
    if width <= MINIMUM_ROOF_FEATURE_SIDE_METERS || depth <= MINIMUM_ROOF_FEATURE_SIDE_METERS {
        return None;
    }
    let (feature_width, feature_depth, height) = match style.kind {
        FacadeKind::Rowhouse => (
            width.min(ROWHOUSE_ROOF_FEATURE_MAX_SIDE_METERS),
            depth.min(ROWHOUSE_ROOF_FEATURE_MAX_SIDE_METERS),
            1.15,
        ),
        FacadeKind::Industrial => (
            (width * 0.1).clamp(
                *INDUSTRIAL_ROOF_FEATURE_SIDE_METERS.start(),
                *INDUSTRIAL_ROOF_FEATURE_SIDE_METERS.end(),
            ),
            (depth * 0.12).clamp(
                *INDUSTRIAL_ROOF_FEATURE_DEPTH_METERS.start(),
                *INDUSTRIAL_ROOF_FEATURE_DEPTH_METERS.end(),
            ),
            1.4 + (style.seed & 3) as f32 * 0.35,
        ),
        FacadeKind::MidRise | FacadeKind::Tower => (
            (width * 0.2).clamp(
                *TALL_ROOF_FEATURE_SIDE_METERS.start(),
                *TALL_ROOF_FEATURE_SIDE_METERS.end(),
            ),
            (depth * 0.18).clamp(
                *TALL_ROOF_FEATURE_SIDE_METERS.start(),
                *TALL_ROOF_FEATURE_SIDE_METERS.end(),
            ),
            2.4 + (style.seed & 3) as f32 * 0.7,
        ),
        FacadeKind::LowRise => (
            (width * 0.12).clamp(
                *INDUSTRIAL_ROOF_FEATURE_SIDE_METERS.start(),
                *INDUSTRIAL_ROOF_FEATURE_SIDE_METERS.end(),
            ),
            (depth * 0.12).clamp(
                *INDUSTRIAL_ROOF_FEATURE_SIDE_METERS.start(),
                *INDUSTRIAL_ROOF_FEATURE_SIDE_METERS.end(),
            ),
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
    let along = style.entry.map_or_else(
        || ((point.0 - style.left.0) * edge.0 + (point.1 - style.left.1) * edge.1) / length,
        |entry| {
            (point.0 - entry.origin.0)
                .mul_add(entry.unit.0, (point.1 - entry.origin.1) * entry.unit.1)
        },
    );
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
    let along = style.entry.map_or_else(
        || ((point.0 - style.left.0) * edge.0 + (point.1 - style.left.1) * edge.1) / length,
        |entry| {
            (point.0 - entry.origin.0)
                .mul_add(entry.unit.0, (point.1 - entry.origin.1) * entry.unit.1)
        },
    );

    // A dark cornice and aligned floor courses make a run of simple boxes read
    // as Philadelphia rowhouses even when no facade photograph exists.
    if height - relative_z < 0.42 {
        return style.facade.map(|channel| scale_channel(channel, 0.7));
    }
    let floor_height = rowhouse_floor_height(style.seed);
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

    let bay_width = style.entry.map_or_else(
        || {
            let bays = if length < 6.4 { 2.0 } else { 3.0 };
            length / bays
        },
        |entry| entry.bay_width,
    );
    let column = along.rem_euclid(bay_width);
    let window_left = bay_width * 0.22;
    let window_right = bay_width * 0.78;
    let story = (relative_z / floor_height).floor() as usize;
    let upper_window = (0.76..=2.22).contains(&floor)
        && (window_left..=window_right).contains(&column)
        && relative_z >= floor_height * 0.72;
    let upper_window_trim = style.entry.is_some()
        && ROWHOUSE_WINDOW_TRIM_STORIES.contains(&story)
        && (0.76 - ROWHOUSE_WINDOW_TRIM_WIDTH_METERS..=2.22 + ROWHOUSE_WINDOW_TRIM_WIDTH_METERS)
            .contains(&floor)
        && (window_left - ROWHOUSE_WINDOW_TRIM_WIDTH_METERS
            ..=window_right + ROWHOUSE_WINDOW_TRIM_WIDTH_METERS)
            .contains(&column)
        && !upper_window;
    let ground_window = (0.92..=2.3).contains(&relative_z)
        && along >= bay_width * 1.12
        && (window_left..=window_right).contains(&column);
    let door = (0.12..=2.55).contains(&relative_z)
        && style.entry.map_or_else(
            || (bay_width * 0.2..=bay_width * 0.68).contains(&along),
            |entry| (entry.door_start..=entry.door_end).contains(&along),
        );
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
    if upper_window_trim {
        return palette::mix(style.facade, [190, 181, 165], 0.52);
    }
    style.facade
}

#[cfg(test)]
mod tests {
    use super::{
        FacadeKind, InferredRoofForm, ROWHOUSE_CORNICE_HEIGHT_METERS,
        ROWHOUSE_CORNICE_OUTSET_METERS, ROWHOUSE_WINDOW_TRIM_WIDTH_METERS, WallStyle, block_seed,
        classify_building, facade_detail, infer_pitched_roof, palette, point_in_polygon,
        polygon_area, roof_feature, rowhouse_cornice_segment, rowhouse_cornice_segments,
        rowhouse_entry_layout, rowhouse_entry_stoop, rowhouse_floor_height, rowhouse_frontage_edge,
        shade, wall_light, wall_material, wall_seed, wall_surface_light,
    };
    use crate::{
        projection::Projection,
        texture::AerialTile,
        world::{Bounds, Building, BuildingContext, BuildingKind, Ring, View},
    };
    use sha2::{Digest, Sha256};
    use tiny_skia::Pixmap;

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

    fn context(kind: BuildingKind) -> BuildingContext {
        BuildingContext {
            kind,
            material_group_seed: 0,
            party_edge_mask: 0,
        }
    }

    fn building(width: f32, depth: f32, height: f32) -> Building {
        Building {
            height,
            frontage_edge: None,
            ring: ring(width, depth),
        }
    }

    fn entry_style(building: &Building, context: &BuildingContext) -> super::BuildingStyle {
        super::BuildingStyle {
            kind: FacadeKind::Rowhouse,
            facade: [152, 91, 68],
            seed: 0,
            short_side: building
                .ring
                .bounds
                .width()
                .min(building.ring.bounds.height()),
            party_edge_mask: context.party_edge_mask,
            frontage_edge: (context.kind == BuildingKind::Rowhouse)
                .then(|| building.frontage_edge.map(usize::from))
                .flatten(),
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
            entry: None,
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
    fn morphology_uses_metre_scale_footprints_and_metre_heights() {
        assert_eq!(
            classify_building(&ring(5.2, 15.0), 9.2),
            FacadeKind::Rowhouse
        );
        assert_eq!(
            classify_building(&ring(43.0, 61.0), 10.0),
            FacadeKind::Industrial
        );
        assert_eq!(
            classify_building(&ring(72.0, 80.0), 90.0),
            FacadeKind::Tower
        );
        assert!((polygon_area(&ring(5.2, 15.0).points) - 78.0).abs() < 0.01);
    }

    #[test]
    fn small_metre_footprints_keep_their_area_and_classification_near_city_coordinates() {
        let local = ring(5.0, 16.0);
        let translated = Ring {
            bounds: Bounds {
                min_x: 820_000.0,
                min_y: 72_000.0,
                max_x: 820_005.0,
                max_y: 72_016.0,
            },
            points: vec![
                (820_000.0, 72_000.0),
                (820_005.0, 72_000.0),
                (820_005.0, 72_016.0),
                (820_000.0, 72_016.0),
            ],
        };

        assert!((polygon_area(&translated.points) - 80.0).abs() < f32::EPSILON);
        assert_eq!(
            classify_building(&local, 9.0),
            classify_building(&translated, 9.0)
        );
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
            entry: None,
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
    fn known_rowhouse_frontages_get_trim_on_two_upper_stories_only() -> Result<(), &'static str> {
        let mut house = building(5.0, 16.0, 12.0);
        house.frontage_edge = Some(0);
        let context = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let building_style = entry_style(&house, &context);
        let entry = rowhouse_entry_layout(&house, &context, building_style).ok_or("entry")?;
        let edge_end = (
            entry.origin.0 + entry.unit.0 * entry.length,
            entry.origin.1 + entry.unit.1 * entry.length,
        );
        let seed = wall_seed(building_style.seed, entry.origin, edge_end);
        let floor_height = rowhouse_floor_height(seed);
        let trim_along = entry.bay_width * 0.22 - ROWHOUSE_WINDOW_TRIM_WIDTH_METERS * 0.5;
        let trim_point = (
            entry.origin.0 + entry.unit.0 * trim_along,
            entry.origin.1 + entry.unit.1 * trim_along,
        );
        let style = WallStyle {
            facade: building_style.facade,
            kind: FacadeKind::Rowhouse,
            frontage: true,
            light: 1.0,
            bottom: 0.0,
            top: house.height,
            left: entry.origin,
            right: edge_end,
            seed,
            entry: Some(entry),
        };
        let trim = palette::mix(style.facade, [190, 181, 165], 0.52);

        assert_eq!(facade_detail(trim_point, floor_height + 1.0, style), trim);
        assert_eq!(
            facade_detail(trim_point, floor_height * 2.0 + 1.0, style),
            trim
        );
        assert_eq!(
            facade_detail(trim_point, floor_height * 3.0 + 1.0, style),
            style.facade
        );
        assert_eq!(
            facade_detail(
                trim_point,
                floor_height + 1.0,
                WallStyle {
                    entry: None,
                    ..style
                }
            ),
            style.facade
        );

        let glass_along = entry.bay_width * 0.5;
        let glass_point = (
            entry.origin.0 + entry.unit.0 * glass_along,
            entry.origin.1 + entry.unit.1 * glass_along,
        );
        assert_ne!(facade_detail(glass_point, floor_height + 1.0, style), trim);
        Ok(())
    }

    #[test]
    fn rowhouse_cornices_select_only_non_party_standard_frontage_edges() {
        let house = building(5.0, 16.0, 9.0);
        let rowhouse = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let cornices = rowhouse_cornice_segments(&house, &rowhouse);

        assert_eq!(cornices.len(), 2);
        assert!(rowhouse_cornice_segment(&house, &rowhouse, 0).is_some());
        assert!(rowhouse_cornice_segment(&house, &rowhouse, 2).is_some());
        assert!(rowhouse_cornice_segment(&house, &rowhouse, 1).is_none());
        assert!(rowhouse_cornice_segment(&house, &rowhouse, 3).is_none());
        assert!(rowhouse_cornice_segments(&house, &context(BuildingKind::RowhouseLike)).is_empty());
        assert!(rowhouse_cornice_segments(&house, &context(BuildingKind::Twin)).is_empty());
        assert!(rowhouse_cornice_segments(&house, &context(BuildingKind::Detached)).is_empty());
        assert!(rowhouse_cornice_segments(&house, &context(BuildingKind::Warehouse)).is_empty());
        assert!(rowhouse_cornice_segments(&house, &context(BuildingKind::Generic)).is_empty());
        assert!(rowhouse_cornice_segments(&building(3.0, 16.0, 9.0), &rowhouse).is_empty());
        assert!(rowhouse_cornice_segments(&building(9.2, 16.0, 9.0), &rowhouse).is_empty());
    }

    #[test]
    fn known_frontage_limits_openings_and_cornices_to_one_non_party_edge() {
        let mut house = building(5.0, 16.0, 9.0);
        house.frontage_edge = Some(0);
        let rowhouse = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        assert!(rowhouse_cornice_segment(&house, &rowhouse, 0).is_some());
        assert!(rowhouse_cornice_segment(&house, &rowhouse, 2).is_none());
        let known = super::BuildingStyle {
            kind: FacadeKind::Rowhouse,
            facade: [152, 91, 68],
            seed: 0,
            short_side: 5.0,
            party_edge_mask: rowhouse.party_edge_mask,
            frontage_edge: Some(0),
        };
        assert!(rowhouse_frontage_edge(known, 0, 5.0));
        assert!(!rowhouse_frontage_edge(known, 2, 5.0));
        assert!(!rowhouse_frontage_edge(known, 1, 16.0));
        let unknown = super::BuildingStyle {
            frontage_edge: None,
            ..known
        };
        assert!(rowhouse_frontage_edge(unknown, 0, 5.0));
        assert!(rowhouse_frontage_edge(unknown, 2, 5.0));
    }

    #[test]
    fn rowhouse_stoops_require_a_known_eligible_nonparty_frontage() {
        let mut house = building(5.0, 16.0, 9.0);
        house.frontage_edge = Some(0);
        let rowhouse = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let style = entry_style(&house, &rowhouse);
        assert!(rowhouse_entry_layout(&house, &rowhouse, style).is_some());

        house.frontage_edge = None;
        let style = entry_style(&house, &rowhouse);
        assert!(rowhouse_entry_layout(&house, &rowhouse, style).is_none());

        house.frontage_edge = Some(0);
        let style = entry_style(&house, &rowhouse);
        let party_front = BuildingContext {
            party_edge_mask: rowhouse.party_edge_mask | 1,
            ..rowhouse
        };
        assert!(rowhouse_entry_layout(&house, &party_front, style).is_none());
        assert!(
            rowhouse_entry_layout(&house, &context(BuildingKind::RowhouseLike), style).is_none()
        );
        assert!(rowhouse_entry_layout(&house, &context(BuildingKind::Twin), style).is_none());

        let mut short = building(3.0, 16.0, 9.0);
        short.frontage_edge = Some(0);
        let style = entry_style(&short, &rowhouse);
        assert!(rowhouse_entry_layout(&short, &rowhouse, style).is_none());

        let mut long_side = building(8.0, 5.0, 9.0);
        long_side.frontage_edge = Some(0);
        let style = entry_style(&long_side, &rowhouse);
        assert!(rowhouse_entry_layout(&long_side, &rowhouse, style).is_none());
    }

    #[test]
    fn rowhouse_stoop_geometry_is_outward_and_aligns_to_the_painted_door()
    -> Result<(), &'static str> {
        let mut house = building(5.0, 16.0, 9.0);
        house.frontage_edge = Some(0);
        let context = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let style = entry_style(&house, &context);
        let layout = rowhouse_entry_layout(&house, &context, style).ok_or("entry layout")?;
        let [lower, upper] = rowhouse_entry_stoop(layout);

        assert_eq!(lower.height, 0.18);
        assert_eq!(upper.height, 0.36);
        assert!(lower.outer.iter().all(|&point| !house.ring.contains(point)));
        assert!(upper.outer.iter().all(|&point| !house.ring.contains(point)));
        assert!(lower.outer[0].1 < 0.0 && lower.outer[1].1 < 0.0);

        let door_center = (layout.door_start + layout.door_end) * 0.5;
        let door_point = (
            layout.origin.0 + layout.unit.0 * door_center,
            layout.origin.1 + layout.unit.1 * door_center,
        );
        let base = [152, 91, 68];
        let style = WallStyle {
            facade: base,
            kind: FacadeKind::Rowhouse,
            frontage: true,
            light: 1.0,
            bottom: 0.0,
            top: house.height,
            left: house.ring.points[0],
            right: house.ring.points[1],
            seed: 0,
            entry: Some(layout),
        };
        assert_eq!(
            facade_detail(door_point, 1.0, style),
            crate::palette::mix(base, [66, 58, 52], 0.68)
        );
        Ok(())
    }

    #[test]
    fn rowhouse_stoop_layout_is_winding_stable() -> Result<(), &'static str> {
        let mut original = building(5.0, 16.0, 9.0);
        original.frontage_edge = Some(0);
        let context = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let style = entry_style(&original, &context);
        let first = rowhouse_entry_layout(&original, &context, style).ok_or("original entry")?;

        let mut reversed = original.clone();
        reversed.ring.points.reverse();
        reversed.frontage_edge = Some(2);
        let style = entry_style(&reversed, &context);
        let second = rowhouse_entry_layout(&reversed, &context, style).ok_or("reversed entry")?;

        assert_eq!(first.origin, second.origin);
        assert_eq!(first.unit, second.unit);
        assert_eq!(first.outward, second.outward);
        assert_eq!(first.door_start, second.door_start);
        assert_eq!(first.door_end, second.door_end);
        assert_eq!(rowhouse_entry_stoop(first), rowhouse_entry_stoop(second));

        let point = (
            first.origin.0 + first.unit.0 * (first.door_start + first.door_end) * 0.5,
            first.origin.1 + first.unit.1 * (first.door_start + first.door_end) * 0.5,
        );
        let paint = |left, right, entry| WallStyle {
            facade: [152, 91, 68],
            kind: FacadeKind::Rowhouse,
            frontage: true,
            light: 1.0,
            bottom: 0.0,
            top: 9.0,
            left,
            right,
            seed: 0,
            entry: Some(entry),
        };
        assert_eq!(
            facade_detail(
                point,
                1.0,
                paint(original.ring.points[0], original.ring.points[1], first)
            ),
            facade_detail(
                point,
                1.0,
                paint(reversed.ring.points[2], reversed.ring.points[3], second)
            )
        );
        Ok(())
    }

    #[test]
    fn rowhouse_cornice_geometry_is_winding_translation_and_height_stable() {
        let house = building(5.0, 16.0, 9.0);
        let context = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let original = rowhouse_cornice_segments(&house, &context);
        assert_eq!(original.len(), 2);
        for cornice in &original {
            assert_eq!(cornice.top, house.height);
            assert_eq!(
                cornice.bottom,
                house.height - ROWHOUSE_CORNICE_HEIGHT_METERS
            );
            for index in 0..2 {
                let offset = (
                    cornice.outer[index].0 - cornice.inner[index].0,
                    cornice.outer[index].1 - cornice.inner[index].1,
                );
                assert!((offset.0.hypot(offset.1) - ROWHOUSE_CORNICE_OUTSET_METERS).abs() < 1e-5);
            }
            let outer_midpoint = (
                (cornice.outer[0].0 + cornice.outer[1].0) * 0.5,
                (cornice.outer[0].1 + cornice.outer[1].1) * 0.5,
            );
            assert!(!house.ring.contains(outer_midpoint));
        }

        let mut reversed = house.clone();
        reversed.ring.points.reverse();
        let reversed = rowhouse_cornice_segments(&reversed, &context);
        let centers = |cornices: &[super::CorniceSegment]| {
            let mut centers: Vec<_> = cornices
                .iter()
                .map(|cornice| {
                    (
                        (cornice.outer[0].0 + cornice.outer[1].0) * 0.5,
                        (cornice.outer[0].1 + cornice.outer[1].1) * 0.5,
                    )
                })
                .collect();
            centers.sort_by(|left, right| left.1.total_cmp(&right.1));
            centers
        };
        assert_eq!(centers(&original), centers(&reversed));

        let offset = (820_000.0, 72_000.0);
        let mut translated = house.clone();
        translated.ring.points = house
            .ring
            .points
            .iter()
            .map(|&(x, y)| (x + offset.0, y + offset.1))
            .collect();
        translated.ring.bounds = Bounds {
            min_x: house.ring.bounds.min_x + offset.0,
            min_y: house.ring.bounds.min_y + offset.1,
            max_x: house.ring.bounds.max_x + offset.0,
            max_y: house.ring.bounds.max_y + offset.1,
        };
        let translated = rowhouse_cornice_segments(&translated, &context);
        for (local, translated) in original.iter().zip(&translated) {
            for index in 0..2 {
                assert!((translated.inner[index].0 - local.inner[index].0 - offset.0).abs() < 0.03);
                assert!((translated.inner[index].1 - local.inner[index].1 - offset.1).abs() < 0.03);
                assert!((translated.outer[index].0 - local.outer[index].0 - offset.0).abs() < 0.03);
                assert!((translated.outer[index].1 - local.outer[index].1 - offset.1).abs() < 0.03);
            }
            assert_eq!(translated.bottom, local.bottom);
            assert_eq!(translated.top, local.top);
        }
    }

    #[test]
    fn rowhouse_cornice_raster_and_depth_are_deterministic() -> Result<(), &'static str> {
        let house = building(5.0, 16.0, 9.0);
        let bounds = house
            .ring
            .bounds
            .projected(house.height, View::SouthEast)
            .pad(2.0);
        let projection = Projection {
            bounds,
            scale: 256.0 / bounds.width().max(bounds.height()),
            view: View::SouthEast,
        };
        let aerial = AerialTile::solid_for_tests([144, 136, 120]);
        let rowhouse = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let rowhouse_like = BuildingContext {
            party_edge_mask: rowhouse.party_edge_mask,
            ..context(BuildingKind::RowhouseLike)
        };
        let render = |context: &BuildingContext| {
            let mut pixmap = Pixmap::new(256, 256).ok_or("pixmap")?;
            let mut depth = vec![f32::NEG_INFINITY; 256 * 256];
            super::draw_city_buildings(
                &mut pixmap,
                [(&house, context)],
                &projection,
                &aerial,
                1.0,
                &mut depth,
            );
            Ok::<_, &'static str>((pixmap.data().to_vec(), depth))
        };

        let (first_pixels, first_depth) = render(&rowhouse)?;
        let (second_pixels, second_depth) = render(&rowhouse)?;
        let (without_cornice_pixels, without_cornice_depth) = render(&rowhouse_like)?;

        assert_eq!(first_pixels, second_pixels);
        assert_eq!(first_depth, second_depth);
        assert_ne!(first_pixels, without_cornice_pixels);
        assert_ne!(first_depth, without_cornice_depth);
        assert!(first_depth.iter().any(|depth| depth.is_finite()));
        Ok(())
    }

    #[test]
    fn rowhouse_stoop_raster_and_depth_are_deterministic_in_all_views() -> Result<(), &'static str>
    {
        let mut house = building(5.0, 16.0, 9.0);
        house.frontage_edge = Some(0);
        let context = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let aerial = AerialTile::solid_for_tests([144, 136, 120]);

        for view in View::ALL {
            let bounds = house.ring.bounds.projected(house.height, view).pad(3.0);
            let projection = Projection {
                bounds,
                scale: 256.0 / bounds.width().max(bounds.height()),
                view,
            };
            let render = || {
                let mut pixmap = Pixmap::new(256, 256).ok_or("pixmap")?;
                let mut depth = vec![f32::NEG_INFINITY; 256 * 256];
                super::draw_city_buildings(
                    &mut pixmap,
                    [(&house, &context)],
                    &projection,
                    &aerial,
                    1.0,
                    &mut depth,
                );
                Ok::<_, &'static str>((pixmap.data().to_vec(), depth))
            };
            let (first_pixels, first_depth) = render()?;
            let (second_pixels, second_depth) = render()?;
            assert_eq!(first_pixels, second_pixels, "{view:?}");
            assert_eq!(first_depth, second_depth, "{view:?}");
            assert!(
                first_depth.iter().any(|depth| depth.is_finite()),
                "{view:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_rowhouse_frontage_keeps_the_frozen_v62_raster_in_all_views()
    -> Result<(), &'static str> {
        let house = building(5.0, 16.0, 9.0);
        let context = BuildingContext {
            party_edge_mask: (1 << 1) | (1 << 3),
            ..context(BuildingKind::Rowhouse)
        };
        let aerial = AerialTile::solid_for_tests([144, 136, 120]);
        let expected = [
            [
                24, 252, 24, 29, 52, 90, 174, 131, 130, 161, 129, 254, 51, 26, 54, 185, 13, 114,
                104, 255, 254, 187, 219, 26, 145, 33, 130, 127, 97, 98, 46, 100,
            ],
            [
                36, 211, 187, 82, 162, 149, 185, 187, 222, 50, 191, 114, 222, 124, 215, 64, 107,
                68, 55, 136, 253, 238, 12, 112, 74, 68, 204, 97, 93, 226, 16, 189,
            ],
            [
                218, 14, 142, 243, 91, 60, 240, 129, 168, 19, 254, 141, 241, 100, 110, 82, 224,
                185, 129, 78, 151, 237, 120, 192, 114, 30, 209, 50, 222, 87, 239, 163,
            ],
            [
                119, 141, 191, 110, 85, 172, 211, 208, 82, 253, 209, 190, 127, 7, 186, 9, 10, 173,
                105, 150, 167, 152, 231, 85, 229, 108, 247, 73, 192, 103, 75, 83,
            ],
        ];

        for (view, expected) in View::ALL.into_iter().zip(expected) {
            let bounds = house.ring.bounds.projected(house.height, view).pad(3.0);
            let projection = Projection {
                bounds,
                scale: 256.0 / bounds.width().max(bounds.height()),
                view,
            };
            let mut pixmap = Pixmap::new(256, 256).ok_or("pixmap")?;
            let mut depth = vec![f32::NEG_INFINITY; 256 * 256];
            super::draw_city_buildings(
                &mut pixmap,
                [(&house, &context)],
                &projection,
                &aerial,
                1.0,
                &mut depth,
            );
            let mut digest = Sha256::new();
            digest.update(pixmap.data());
            for value in depth {
                digest.update(value.to_bits().to_le_bytes());
            }
            assert_eq!(<[u8; 32]>::from(digest.finalize()), expected, "{view:?}");
        }
        Ok(())
    }

    #[test]
    fn synthesized_roof_feature_is_deterministic_and_inside_its_roof() -> Result<(), &'static str> {
        let roof = ring(12.2, 9.1);
        let style = super::BuildingStyle {
            kind: FacadeKind::Industrial,
            facade: [160, 150, 140],
            seed: 42,
            short_side: 9.1,
            party_edge_mask: 0,
            frontage_edge: None,
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
    fn roof_feature_uses_metre_scale_minimum_side() {
        let roof = ring(12.2, 9.1);
        let style = super::BuildingStyle {
            kind: FacadeKind::Industrial,
            facade: [160, 150, 140],
            seed: 42,
            short_side: 9.1,
            party_edge_mask: 0,
            frontage_edge: None,
        };

        assert!(roof_feature(&roof, style, 0, 1).is_some());
        assert!(roof_feature(&ring(0.60, 3.0), style, 0, 1).is_none());
    }

    #[test]
    fn inferred_roofs_only_select_simple_detached_houses() {
        let detached = building(10.0, 10.0, 9.0);
        assert!(matches!(
            infer_pitched_roof(&detached, &context(BuildingKind::Detached)),
            Some(super::InferredRoof {
                form: InferredRoofForm::Hipped { .. },
                ..
            })
        ));

        for kind in [
            BuildingKind::Rowhouse,
            BuildingKind::RowhouseLike,
            BuildingKind::Twin,
            BuildingKind::Warehouse,
            BuildingKind::Generic,
        ] {
            assert!(
                infer_pitched_roof(&detached, &context(kind)).is_none(),
                "{kind:?} must remain flat"
            );
        }
        assert!(
            infer_pitched_roof(&building(4.0, 10.0, 9.0), &context(BuildingKind::Detached))
                .is_none()
        );
    }

    #[test]
    fn elongated_detached_house_has_a_long_axis_ridge_and_bounded_wall_top()
    -> Result<(), &'static str> {
        let house = building(16.0, 8.0, 10.0);
        let roof = infer_pitched_roof(&house, &context(BuildingKind::Detached))
            .ok_or("detached rectangle roof")?;
        let ridge = match roof.form {
            InferredRoofForm::Gabled { ridge, .. } => ridge,
            _ => return Err("elongated rectangle should have a gable ridge"),
        };
        let ridge_direction = (ridge[1].0 - ridge[0].0, ridge[1].1 - ridge[0].1);

        assert!(ridge_direction.0.abs() > ridge_direction.1.abs() * 10.0);
        assert_eq!(roof.roof_top, house.height);
        assert!((1.0..=2.8).contains(&(roof.roof_top - roof.wall_top)));
        assert!(roof.wall_top >= 3.0);
        Ok(())
    }

    #[test]
    fn inferred_roof_geometry_survives_large_coordinate_translation() -> Result<(), &'static str> {
        let local = building(16.0, 8.0, 10.0);
        let mut translated = local.clone();
        let offset = (820_000.0, 72_000.0);
        translated.ring.points = local
            .ring
            .points
            .iter()
            .map(|&(x, y)| (x + offset.0, y + offset.1))
            .collect();
        translated.ring.bounds = Bounds {
            min_x: offset.0,
            min_y: offset.1,
            max_x: offset.0 + 16.0,
            max_y: offset.1 + 8.0,
        };

        let local_roof =
            infer_pitched_roof(&local, &context(BuildingKind::Detached)).ok_or("local roof")?;
        let translated_roof = infer_pitched_roof(&translated, &context(BuildingKind::Detached))
            .ok_or("translated roof")?;
        assert_eq!(local_roof.wall_top, translated_roof.wall_top);
        match (local_roof.form, translated_roof.form) {
            (
                InferredRoofForm::Gabled { ridge: local, .. },
                InferredRoofForm::Gabled {
                    ridge: translated, ..
                },
            ) => {
                for index in 0..2 {
                    assert!((translated[index].0 - local[index].0 - offset.0).abs() < 0.02);
                    assert!((translated[index].1 - local[index].1 - offset.1).abs() < 0.02);
                }
            }
            _ => return Err("both roofs should stay gabled"),
        }
        Ok(())
    }

    #[test]
    fn inferred_roof_is_invariant_to_rectangle_start_and_winding() -> Result<(), &'static str> {
        let original = building(16.0, 8.0, 10.0);
        let mut reordered = original.clone();
        reordered.ring.points = vec![(16.0, 8.0), (16.0, 0.0), (0.0, 0.0), (0.0, 8.0)];
        let original = infer_pitched_roof(&original, &context(BuildingKind::Detached))
            .ok_or("original roof")?;
        let reordered = infer_pitched_roof(&reordered, &context(BuildingKind::Detached))
            .ok_or("reordered roof")?;
        let (
            InferredRoofForm::Gabled { ridge: first, .. },
            InferredRoofForm::Gabled { ridge: second, .. },
        ) = (original.form, reordered.form)
        else {
            return Err("rectangle roof changed form");
        };
        let mut first = first;
        let mut second = second;
        first.sort_by(|left, right| left.0.total_cmp(&right.0));
        second.sort_by(|left, right| left.0.total_cmp(&right.0));

        assert_eq!(original.wall_top, reordered.wall_top);
        assert_eq!(first, second);
        Ok(())
    }

    #[test]
    fn non_rectangular_or_complex_detached_footprints_remain_flat() {
        let cases = [
            vec![(0.0, 0.0), (16.0, 0.0), (16.0, 8.0)],
            vec![
                (0.0, 0.0),
                (16.0, 0.0),
                (16.0, 3.0),
                (6.0, 3.0),
                (6.0, 8.0),
                (0.0, 8.0),
            ],
            vec![(0.0, 0.0), (16.0, 0.0), (14.0, 8.0), (0.0, 8.0)],
        ];
        for points in cases {
            let min_x = points
                .iter()
                .map(|point| point.0)
                .fold(f32::INFINITY, f32::min);
            let min_y = points
                .iter()
                .map(|point| point.1)
                .fold(f32::INFINITY, f32::min);
            let max_x = points
                .iter()
                .map(|point| point.0)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y = points
                .iter()
                .map(|point| point.1)
                .fold(f32::NEG_INFINITY, f32::max);
            let house = Building {
                height: 10.0,
                frontage_edge: None,
                ring: Ring {
                    bounds: Bounds {
                        min_x,
                        min_y,
                        max_x,
                        max_y,
                    },
                    points,
                },
            };
            assert!(infer_pitched_roof(&house, &context(BuildingKind::Detached)).is_none());
        }
    }

    #[test]
    fn inferred_roof_render_and_depth_are_deterministic() -> Result<(), &'static str> {
        let house = building(16.0, 8.0, 10.0);
        let roof =
            infer_pitched_roof(&house, &context(BuildingKind::Detached)).ok_or("inferred roof")?;
        let bounds = house
            .ring
            .bounds
            .projected(roof.roof_top, View::SouthEast)
            .pad(3.0);
        let projection = Projection {
            bounds,
            scale: 256.0 / bounds.width().max(bounds.height()),
            view: View::SouthEast,
        };
        let aerial = AerialTile::solid_for_tests([144, 136, 120]);
        let context = context(BuildingKind::Detached);
        let render = || {
            let mut pixmap = Pixmap::new(256, 256).ok_or("pixmap")?;
            let mut depth = vec![f32::NEG_INFINITY; 256 * 256];
            super::draw_city_buildings(
                &mut pixmap,
                [(&house, &context)],
                &projection,
                &aerial,
                1.0,
                &mut depth,
            );
            Ok::<_, &'static str>((pixmap.data().to_vec(), depth))
        };
        let (first_pixels, first_depth) = render()?;
        let (second_pixels, second_depth) = render()?;

        assert_eq!(first_pixels, second_pixels);
        assert_eq!(first_depth, second_depth);
        assert!(first_depth.iter().any(|depth| depth.is_finite()));
        Ok(())
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
            entry: None,
        };
        assert!(wall_surface_light(0.0, style) < wall_surface_light(10.0, style));
        assert!(wall_surface_light(19.9, style) < wall_surface_light(10.0, style));
    }
}
