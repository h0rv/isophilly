use std::{collections::HashMap, io};

use rstar::AABB;
use tiny_skia::{Color, Pixmap};

use crate::{
    building_render::{draw_city_building_parts, draw_city_buildings},
    land_cover::{LandCoverClass, LandCoverMask},
    mesh_render::draw_textured_faces,
    mesh_texture::MeshTextureSource,
    palette::{self, GROUND},
    projection::Projection,
    shadow_render::draw_cast_shadows,
    texture::{AerialTile, missing_imagery},
    tile_codec::encode_rgba,
    transport_render::draw_transport,
    tree_render::{draw_canopy_mass, draw_street_trees},
    world::{Bounds, PRIMARY_MESH_TEXTURE_LIMIT, Ring, View, World},
};

const TILE_SIZE: u32 = 256;
const PROCEDURAL_ROOF_MARGIN_METERS: f32 = 8.0;
const SHORELINE_DISTANCE_METERS: f32 = 4.5;
const SHORELINE_PROBE_METERS: f32 = 1.5;

pub fn render_tile(
    world: &World,
    aerial: &AerialTile,
    mesh_textures: &MeshTextureSource,
    land_cover: Option<&LandCoverMask>,
    z: u8,
    x: u32,
    y: u32,
) -> io::Result<Vec<u8>> {
    let bounds = world.iso_bounds.tile(z, x, y);
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    let sampling_block = block_size(bounds);
    let projection = Projection {
        bounds,
        scale,
        view: View::SouthEast,
    };
    draw_ground(
        &mut pixmap,
        world,
        &projection,
        sampling_block,
        aerial,
        land_cover,
    );
    let transport_query = transport_query(bounds, scale);
    draw_transport(
        &mut pixmap,
        world
            .transport_iso_tree
            .locate_in_envelope_intersecting(transport_query)
            .map(|item| &world.transport[item.index]),
        &projection,
    );
    let mut depth = vec![f32::NEG_INFINITY; (TILE_SIZE * TILE_SIZE) as usize];
    let margin = PROCEDURAL_ROOF_MARGIN_METERS + 1.0 / scale;
    let query = AABB::from_corners(
        [bounds.min_x - margin, bounds.min_y - margin],
        [bounds.max_x + margin, bounds.max_y + margin],
    );
    let shadow_margin = world.max_height * 1.5;
    let shadow_query = AABB::from_corners(
        [bounds.min_x - shadow_margin, bounds.min_y - shadow_margin],
        [bounds.max_x + shadow_margin, bounds.max_y + shadow_margin],
    );
    draw_cast_shadows(
        &mut pixmap,
        world
            .building_iso_tree
            .locate_in_envelope_intersecting(shadow_query)
            .filter_map(|item| {
                (!world.building_detailed_by_parts[item.index])
                    .then_some(&world.buildings[item.index])
            }),
        world
            .building_part_iso_tree
            .locate_in_envelope_intersecting(shadow_query)
            .map(|item| &world.building_parts[item.index]),
        world
            .street_tree_iso_tree
            .locate_in_envelope_intersecting(shadow_query)
            .map(|item| &world.street_trees[item.index]),
        &projection,
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
                    .then_some((building, &world.building_contexts[item.index]))
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
    if let Some(land_cover) = land_cover {
        draw_canopy_mass(
            &mut pixmap,
            land_cover,
            &projection,
            aerial,
            sampling_block,
            &mut depth,
        );
    }
    draw_street_trees(
        &mut pixmap,
        world
            .street_tree_iso_tree
            .locate_in_envelope_intersecting(query)
            .map(|item| &world.street_trees[item.index]),
        &projection,
        &mut depth,
    );
    encode_display_pixmap(&mut pixmap)
}

pub fn render_rich_tile(
    world: &World,
    aerial: &AerialTile,
    mesh_textures: &MeshTextureSource,
    land_cover: Option<&LandCoverMask>,
    view: View,
    bounds: Bounds,
) -> io::Result<Vec<u8>> {
    let scale = TILE_SIZE as f32 / bounds.width();
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    let sampling_block = rich_block_size(bounds);
    let projection = Projection {
        bounds,
        scale,
        view,
    };
    draw_ground(
        &mut pixmap,
        world,
        &projection,
        sampling_block,
        aerial,
        land_cover,
    );
    let transport_query = transport_query(bounds.ground_source_bounds_for(view), scale);
    draw_transport(
        &mut pixmap,
        world
            .transport_source_tree
            .locate_in_envelope_intersecting(transport_query)
            .map(|item| &world.transport[item.index]),
        &projection,
    );
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
                    .then_some((building, &world.building_contexts[item.index]))
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
    draw_street_trees(
        &mut pixmap,
        world
            .street_tree_source_tree
            .locate_in_envelope_intersecting(query)
            .map(|item| &world.street_trees[item.index]),
        &projection,
        &mut depth,
    );
    encode_display_pixmap(&mut pixmap)
}

pub fn render_blank_tile() -> io::Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(TILE_SIZE, TILE_SIZE).ok_or_else(|| io::Error::other("pixmap"))?;
    pixmap.fill(ground());
    encode_display_pixmap(&mut pixmap)
}

fn ground() -> Color {
    Color::from_rgba8(GROUND[0], GROUND[1], GROUND[2], 255)
}

fn encode_display_pixmap(pixmap: &mut Pixmap) -> io::Result<Vec<u8>> {
    palette::bake_display_finish(pixmap.data_mut());
    encode_rgba(pixmap.data(), TILE_SIZE, TILE_SIZE)
}

fn transport_query(bounds: Bounds, scale: f32) -> AABB<[f32; 2]> {
    // Centerline widths are in metres, while their scale floor is in pixels.
    // Include both in the query so a route just outside a tile still paints
    // its visible stroke at the edge. This keeps prebuilt neighbors seamless.
    let margin = 3.048 + 2.0 / scale.max(f32::EPSILON);
    AABB::from_corners(
        [bounds.min_x - margin, bounds.min_y - margin],
        [bounds.max_x + margin, bounds.max_y + margin],
    )
}

fn draw_ground(
    pixmap: &mut Pixmap,
    world: &World,
    projection: &Projection,
    block_size: f32,
    aerial: &AerialTile,
    land_cover: Option<&LandCoverMask>,
) {
    let bounds = projection.bounds;
    let scale = projection.scale;
    let view = projection.view;
    let fallback = GROUND;
    let source_bounds = bounds.ground_source_bounds_for(view);
    let water_margin = water_query_margin(block_size);
    let water_query = AABB::from_corners(
        [
            source_bounds.min_x - water_margin,
            source_bounds.min_y - water_margin,
        ],
        [
            source_bounds.max_x + water_margin,
            source_bounds.max_y + water_margin,
        ],
    );
    let park_query = AABB::from_corners(
        [
            source_bounds.min_x - block_size * 0.5,
            source_bounds.min_y - block_size * 0.5,
        ],
        [
            source_bounds.max_x + block_size * 0.5,
            source_bounds.max_y + block_size * 0.5,
        ],
    );
    let water: Vec<_> = world
        .water_tree
        .locate_in_envelope_intersecting(water_query)
        .map(|item| &world.water[item.index])
        .collect();
    let parks: Vec<_> = world
        .park_tree
        .locate_in_envelope_intersecting(park_query)
        .map(|item| &world.parks[item.index])
        .collect();
    let mut colors = HashMap::new();
    for py in 0..TILE_SIZE {
        for px in 0..TILE_SIZE {
            let iso_x = (f32::from(px as u16) + 0.5).mul_add(1.0 / scale, bounds.min_x);
            let iso_y = (f32::from(py as u16) + 0.5).mul_add(1.0 / scale, bounds.min_y);
            let (source_x, source_y) = view.inverse(iso_x, iso_y);
            let (key, sample_point) = canonical_block_sample((source_x, source_y), block_size);
            let color = *colors.entry(key).or_insert_with(|| {
                let sampled = aerial.sample(sample_point.0, sample_point.1, block_size);
                let aerial_color = if missing_imagery(sampled) {
                    fallback
                } else {
                    palette::mix(fallback, sampled.unwrap_or(fallback), 0.9)
                };
                let land_cover_class = land_cover.and_then(|mask| {
                    mask.sample(f64::from(sample_point.0), f64::from(sample_point.1))
                });
                let nearby_water = land_cover.is_some_and(|mask| nearby_water(mask, sample_point));
                grade_ground_with_context(
                    aerial_color,
                    sample_point,
                    &water,
                    &parks,
                    land_cover_class,
                    nearby_water,
                )
            });
            let offset = ((py * TILE_SIZE + px) * 4) as usize;
            pixmap.data_mut()[offset..offset + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

fn canonical_block_sample(point: (f32, f32), block_size: f32) -> ((i32, i32), (f32, f32)) {
    let key = (
        (point.0 / block_size).floor() as i32,
        (point.1 / block_size).floor() as i32,
    );
    let center = (
        (key.0 as f32 + 0.5) * block_size,
        (key.1 as f32 + 0.5) * block_size,
    );
    (key, center)
}

#[cfg(test)]
fn grade_ground(
    color: [u8; 3],
    point: (f32, f32),
    water: &[&Ring],
    parks: &[&Ring],
    land_cover: Option<LandCoverClass>,
) -> [u8; 3] {
    grade_ground_with_context(color, point, water, parks, land_cover, false)
}

fn grade_ground_with_context(
    color: [u8; 3],
    point: (f32, f32),
    water: &[&Ring],
    parks: &[&Ring],
    land_cover: Option<LandCoverClass>,
    nearby_water: bool,
) -> [u8; 3] {
    if let Some(tone) = water_tone(point, water, land_cover, nearby_water) {
        return grade_water_with_tone(color, point, tone);
    }
    let aerial_vegetation =
        color[1].saturating_add(12) >= color[0] && color[1] > color[2].saturating_add(4);
    let vegetation = match land_cover {
        Some(class) => matches!(
            class,
            LandCoverClass::TreeCanopy | LandCoverClass::GrassShrub
        ),
        None => aerial_vegetation,
    };
    if parks.iter().any(|ring| ring.contains(point)) && vegetation {
        return grade_park(color);
    }
    match land_cover {
        Some(LandCoverClass::TreeCanopy) => palette::mix(color, palette::CANOPY, 0.32),
        Some(LandCoverClass::GrassShrub) => palette::mix(color, palette::GRASS, 0.24),
        Some(_) => color,
        None if aerial_vegetation => palette::mix(color, palette::AERIAL_VEGETATION, 0.24),
        None => color,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaterTone {
    Open,
    Shoreline,
}

fn water_query_margin(block_size: f32) -> f32 {
    block_size * 0.5 + SHORELINE_DISTANCE_METERS + SHORELINE_PROBE_METERS
}

fn nearby_water(mask: &LandCoverMask, point: (f32, f32)) -> bool {
    if mask.sample(f64::from(point.0), f64::from(point.1)) == Some(LandCoverClass::Water) {
        return true;
    }
    SHORELINE_PROBES.iter().any(|offset| {
        mask.sample(f64::from(point.0 + offset.0), f64::from(point.1 + offset.1))
            == Some(LandCoverClass::Water)
    })
}

fn water_tone(
    point: (f32, f32),
    water: &[&Ring],
    land_cover: Option<LandCoverClass>,
    nearby_water: bool,
) -> Option<WaterTone> {
    if water.iter().any(|ring| ring.contains(point)) || land_cover == Some(LandCoverClass::Water) {
        return Some(WaterTone::Open);
    }
    if !nearby_water {
        return None;
    }
    nearest_water_distance(point, water)
        .filter(|distance| *distance <= SHORELINE_DISTANCE_METERS)
        .map(|_| WaterTone::Shoreline)
}

#[cfg(test)]
fn grade_water(color: [u8; 3], point: (f32, f32)) -> [u8; 3] {
    grade_water_with_tone(color, point, WaterTone::Open)
}

fn grade_water_with_tone(color: [u8; 3], point: (f32, f32), tone: WaterTone) -> [u8; 3] {
    let (base_mix, highlight_mix, shadow_mix) = match tone {
        WaterTone::Open => (0.56, 0.16, 0.12),
        WaterTone::Shoreline => (0.42, 0.10, 0.0),
    };
    let base = palette::mix(color, palette::WATER, base_mix);
    // Sparse diagonal bands use source coordinates, so a tile or view boundary
    // cannot reset the texture phase.
    let band = point.0.mul_add(0.075, point.1 * 0.035).rem_euclid(29.0);
    if band < 1.4 {
        palette::mix(base, palette::WATER_HIGHLIGHT, highlight_mix)
    } else if shadow_mix > 0.0 && (14.5..15.3).contains(&band) {
        palette::mix(base, palette::WATER_SHADOW, shadow_mix)
    } else {
        base
    }
}

fn grade_park(color: [u8; 3]) -> [u8; 3] {
    palette::mix(color, palette::PARK, 0.48)
}

fn block_size(bounds: Bounds) -> f32 {
    bounds.width() / 128.0
}

fn rich_block_size(bounds: Bounds) -> f32 {
    bounds.width() / TILE_SIZE as f32
}

const SHORELINE_PROBES: [(f32, f32); 4] = [
    (-SHORELINE_PROBE_METERS, 0.0),
    (SHORELINE_PROBE_METERS, 0.0),
    (0.0, -SHORELINE_PROBE_METERS),
    (0.0, SHORELINE_PROBE_METERS),
];

fn nearest_water_distance(point: (f32, f32), water: &[&Ring]) -> Option<f32> {
    let mut best = f32::INFINITY;
    for ring in water {
        if !distance_could_be_within(point, ring, SHORELINE_DISTANCE_METERS, best) {
            continue;
        }
        let distance = distance_to_ring(point, ring);
        if distance < best {
            best = distance;
            if best <= f32::EPSILON {
                return Some(0.0);
            }
        }
    }
    best.is_finite().then_some(best)
}

fn distance_could_be_within(point: (f32, f32), ring: &Ring, limit: f32, best: f32) -> bool {
    let limit = limit.min(best);
    point.0 >= ring.bounds.min_x - limit
        && point.0 <= ring.bounds.max_x + limit
        && point.1 >= ring.bounds.min_y - limit
        && point.1 <= ring.bounds.max_y + limit
}

fn distance_to_ring(point: (f32, f32), ring: &Ring) -> f32 {
    if ring.contains(point) {
        return 0.0;
    }
    ring.points
        .iter()
        .copied()
        .zip(ring.points.iter().copied().cycle().skip(1))
        .take(ring.points.len())
        .map(|(start, end)| distance_to_segment_squared(point, start, end))
        .fold(f32::INFINITY, f32::min)
        .sqrt()
}

fn distance_to_segment_squared(point: (f32, f32), start: (f32, f32), end: (f32, f32)) -> f32 {
    let edge = (end.0 - start.0, end.1 - start.1);
    let length_squared = edge.0.mul_add(edge.0, edge.1 * edge.1);
    if length_squared <= f32::EPSILON {
        let dx = point.0 - start.0;
        let dy = point.1 - start.1;
        return dx.mul_add(dx, dy * dy);
    }
    let amount = (((point.0 - start.0) * edge.0) + ((point.1 - start.1) * edge.1)) / length_squared;
    let amount = amount.clamp(0.0, 1.0);
    let closest = (start.0 + edge.0 * amount, start.1 + edge.1 * amount);
    let dx = point.0 - closest.0;
    let dy = point.1 - closest.1;
    dx.mul_add(dx, dy * dy)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rstar::Envelope;

    use super::{
        SHORELINE_DISTANCE_METERS, SHORELINE_PROBE_METERS, TILE_SIZE, block_size,
        canonical_block_sample, grade_ground, grade_ground_with_context, grade_water,
        rich_block_size, transport_query, water_query_margin,
    };
    use crate::land_cover::LandCoverClass;
    use crate::pyramid::ART_ZOOM;
    use crate::world::{Bounds, Ring, View};

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
    fn transport_query_keeps_a_near_edge_stroke_in_both_neighbor_tiles() {
        let left = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 100.0,
        };
        let right = Bounds {
            min_x: 100.0,
            min_y: 0.0,
            max_x: 200.0,
            max_y: 100.0,
        };
        // This line is just beyond the left tile's source bounds, but an
        // expressway stroke can still reach its rightmost output pixels.
        let line = rstar::AABB::from_corners([102.0, 40.0], [103.0, 60.0]);

        assert!(transport_query(left, 1.0).intersects(&line));
        assert!(transport_query(right, 1.0).intersects(&line));
    }

    #[test]
    fn water_mask_shifts_aerial_color_toward_blue() {
        let water = square();
        let source = [112, 104, 88];
        let result = grade_ground(source, (5.0, 5.0), &[&water], &[], None);

        assert!(result[2] > source[2]);
        assert!(result[2] > result[0]);
    }

    #[test]
    fn shoreline_water_is_only_promoted_when_nearby_mask_water_exists() {
        let water = square();
        let source = [144, 128, 120];
        let shoreline = grade_ground_with_context(source, (12.0, 5.0), &[&water], &[], None, true);
        let background =
            grade_ground_with_context(source, (12.0, 5.0), &[&water], &[], None, false);

        assert!(shoreline[2] > background[2]);
        assert!(shoreline[2] > shoreline[0]);
        assert_eq!(background, source);
    }

    #[test]
    fn water_takes_precedence_over_overlapping_park() {
        let water = square();
        let park = square();
        let source = [112, 112, 80];
        let water_only = grade_ground(source, (5.0, 5.0), &[&water], &[], None);
        let overlap = grade_ground(source, (5.0, 5.0), &[&water], &[&park], None);

        assert_eq!(overlap, water_only);
        assert!(overlap[2] > overlap[1]);
    }

    #[test]
    fn directional_water_texture_is_view_stable_and_bounded() {
        let source = [112, 104, 88];
        let colors: BTreeSet<_> = (0..500)
            .map(|step| grade_water(source, (step as f32 * 3.0, 100.0)))
            .collect();
        let source_point = (819_514.0, 73_344.0);
        let expected = grade_water(source, source_point);

        assert!((2..=3).contains(&colors.len()));
        for view in View::ALL {
            let projected = view.project(source_point.0, source_point.1, 0.0);
            let recovered = view.inverse(projected.0, projected.1);
            assert_eq!(grade_water(source, recovered), expected);
        }
        assert!(colors.iter().all(|color| color[2] > color[0]));
    }

    #[test]
    fn directional_water_texture_does_not_reset_at_tile_seam() {
        let world_bounds = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 65_536.0,
            max_y: 65_536.0,
        };
        let left = world_bounds.tile(ART_ZOOM, 100, 100);
        let right = world_bounds.tile(ART_ZOOM, 101, 100);
        let sample_block = block_size(left);

        for view in View::ALL {
            let mut left_samples = std::collections::BTreeMap::new();
            let mut right_samples = std::collections::BTreeMap::new();
            for pixel in 0..TILE_SIZE {
                let left_iso = (
                    left.max_x - 0.5 / (TILE_SIZE as f32 / left.width()),
                    (pixel as f32 + 0.5)
                        .mul_add(1.0 / (TILE_SIZE as f32 / left.width()), left.min_y),
                );
                let right_iso = (
                    right.min_x + 0.5 / (TILE_SIZE as f32 / right.width()),
                    (pixel as f32 + 0.5)
                        .mul_add(1.0 / (TILE_SIZE as f32 / right.width()), right.min_y),
                );
                let left_block =
                    canonical_block_sample(view.inverse(left_iso.0, left_iso.1), sample_block);
                let right_block =
                    canonical_block_sample(view.inverse(right_iso.0, right_iso.1), sample_block);
                left_samples.insert(left_block.0, left_block.1);
                right_samples.insert(right_block.0, right_block.1);
            }
            let shared: Vec<_> = left_samples
                .keys()
                .filter(|key| right_samples.contains_key(key))
                .collect();
            assert!(
                !shared.is_empty(),
                "{} view has no shared seam block",
                view.id()
            );
            for key in shared {
                assert_eq!(left_samples[key], right_samples[key]);
            }
        }
    }

    #[test]
    fn shoreline_query_margin_includes_the_water_buffer_and_probe() {
        assert_eq!(water_query_margin(128.0), 70.0);
        assert_eq!(
            water_query_margin(0.0),
            SHORELINE_DISTANCE_METERS + SHORELINE_PROBE_METERS
        );
    }

    #[test]
    fn canonical_block_representative_does_not_depend_on_first_pixel() {
        let left = canonical_block_sample((1000.01, 417.01), 2.0);
        let right = canonical_block_sample((1001.99, 417.99), 2.0);

        assert_eq!(left, right);
    }

    #[test]
    fn park_mask_enriches_grass_without_painting_pavement() {
        let park = square();
        let vegetation = grade_ground([112, 112, 80], (5.0, 5.0), &[], &[&park], None);
        let pavement = grade_ground([144, 128, 120], (5.0, 5.0), &[], &[&park], None);

        assert!(vegetation[1] > vegetation[0]);
        assert_eq!(pavement, [144, 128, 120]);
        assert_eq!(vegetation, [90, 131, 73]);
        assert_eq!(
            grade_ground([112, 112, 80], (15.0, 15.0), &[], &[], None),
            [102, 119, 77]
        );
    }

    #[test]
    fn classified_canopy_and_grass_preserve_distinct_aerial_detail() {
        let source = [118, 112, 88];
        let canopy = grade_ground(
            source,
            (5.0, 5.0),
            &[],
            &[],
            Some(LandCoverClass::TreeCanopy),
        );
        let grass = grade_ground(
            source,
            (5.0, 5.0),
            &[],
            &[],
            Some(LandCoverClass::GrassShrub),
        );

        assert_ne!(canopy, grass);
        assert!(canopy[1] > canopy[0] && grass[1] > grass[0]);
        assert_ne!(canopy, [56, 126, 61]);
        assert_ne!(grass, [86, 148, 73]);
    }

    #[test]
    fn classified_nonvegetation_prevents_false_green_park_pixels() {
        let park = square();
        let greenish_aerial = [112, 112, 80];
        for class in [
            LandCoverClass::BareEarth,
            LandCoverClass::Building,
            LandCoverClass::RoadRailroad,
            LandCoverClass::OtherPaved,
        ] {
            assert_eq!(
                grade_ground(greenish_aerial, (5.0, 5.0), &[], &[&park], Some(class),),
                greenish_aerial
            );
        }
    }

    #[test]
    fn hydrology_and_classified_water_take_water_precedence() {
        let water = square();
        let source = [112, 112, 80];
        let expected = grade_ground(source, (5.0, 5.0), &[&water], &[], None);
        assert_eq!(
            grade_ground(
                source,
                (5.0, 5.0),
                &[&water],
                &[],
                Some(LandCoverClass::TreeCanopy),
            ),
            expected
        );
        assert_eq!(
            grade_ground(source, (5.0, 5.0), &[], &[], Some(LandCoverClass::Water),),
            expected
        );
    }

    #[test]
    fn classified_grading_is_stable_in_every_view() {
        let source = (819_514.0, 73_344.0);
        let color = [118, 112, 88];
        let expected = grade_ground(color, source, &[], &[], Some(LandCoverClass::TreeCanopy));
        for view in View::ALL {
            let projected = view.project(source.0, source.1, 0.0);
            let recovered = view.inverse(projected.0, projected.1);
            assert_eq!(
                grade_ground(color, recovered, &[], &[], Some(LandCoverClass::TreeCanopy),),
                expected,
                "{}",
                view.id()
            );
        }
    }
}
