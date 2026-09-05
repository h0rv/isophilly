use tiny_skia::Pixmap;

use crate::{
    land_cover::{LandCoverClass, LandCoverMask},
    palette,
    projection::Projection,
    texture::{AerialTile, missing_imagery},
    world::{StreetTree, TreeForm},
};

const TILE_SIZE: usize = 256;
const MIN_CROWN_RADIUS_PIXELS: f32 = 0.55;
const SQRT_2: f32 = std::f32::consts::SQRT_2;
const SQRT_1_5: f32 = 1.224_744_9;
const TRUNK_COLOR: [u8; 3] = palette::TREE_TRUNK;
const CROWN_LOBE_COUNT: usize = 4;
// The PASDA mask is a continuous area classification, not a second tree
// inventory. Keep it as one low canopy surface so woods read as woods instead
// of materializing a point tree for every raster cell.
const CANOPY_MASS_HEIGHT: f32 = 4.5;
const CANOPY_TONE_PATCH_METERS: f32 = 12.0;
const CANOPY_FOLIAGE_MIX: f32 = 0.56;

/// Draw the PASDA tree-canopy class as a low, depth-tested foliage surface.
///
/// This is intentionally bounded to the output tile: it samples one source
/// coordinate per output pixel and allocates no citywide geometry. Source
/// coordinates, rather than tile-local indices, select both the mask and
/// foliage tone, keeping adjacent tiles and all views phase-stable.
pub fn draw_canopy_mass(
    pixmap: &mut Pixmap,
    land_cover: &LandCoverMask,
    projection: &Projection,
    aerial: &AerialTile,
    block_size: f32,
    depth: &mut [f32],
) {
    draw_canopy_mass_with_samples(
        pixmap,
        projection,
        depth,
        |point| land_cover.sample(f64::from(point.0), f64::from(point.1)),
        |point| aerial.sample(point.0, point.1, block_size),
    );
}

fn draw_canopy_mass_with_samples(
    pixmap: &mut Pixmap,
    projection: &Projection,
    depth: &mut [f32],
    sample_land_cover: impl Fn((f32, f32)) -> Option<LandCoverClass>,
    sample_aerial: impl Fn((f32, f32)) -> Option<[u8; 3]>,
) {
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let (source, pixel_depth) = canopy_surface_at_pixel(projection, x, y);
            if sample_land_cover(source) != Some(LandCoverClass::TreeCanopy) {
                continue;
            }
            let offset = y * TILE_SIZE + x;
            if pixel_depth <= depth[offset] {
                continue;
            }
            depth[offset] = pixel_depth;
            let color = grade_canopy_color(source, sample_aerial(source));
            let start = offset * 4;
            pixmap.data_mut()[start..start + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

fn canopy_surface_at_pixel(
    projection: &Projection,
    pixel_x: usize,
    pixel_y: usize,
) -> ((f32, f32), f32) {
    let projected = (
        (pixel_x as f32 + 0.5).mul_add(1.0 / projection.scale, projection.bounds.min_x),
        (pixel_y as f32 + 0.5).mul_add(1.0 / projection.scale, projection.bounds.min_y),
    );
    // Raising a horizontal surface by h shifts its screen y by -h. Recover the
    // source location from the corresponding ground-coordinate projection.
    let source = projection.inverse((projected.0, projected.1 + CANOPY_MASS_HEIGHT));
    (source, projection.depth(source, CANOPY_MASS_HEIGHT))
}

fn canopy_mass_color(point: (f32, f32)) -> [u8; 3] {
    let patch = (
        (point.0 / CANOPY_TONE_PATCH_METERS).floor(),
        (point.1 / CANOPY_TONE_PATCH_METERS).floor(),
    );
    // Broad, fixed patches distinguish foliage mass from a flat green overlay
    // without creating a noisy per-pixel texture or a tile-local phase.
    palette::mix(palette::CANOPY, tree_palette(patch), 0.48)
}

fn grade_canopy_color(point: (f32, f32), sampled_aerial: Option<[u8; 3]>) -> [u8; 3] {
    let aerial_color = if missing_imagery(sampled_aerial) {
        palette::GROUND
    } else {
        // Match ground's source treatment before layering foliage over it, so
        // the canopy remains tied to the same bounded aerial-color contract.
        palette::mix(
            palette::GROUND,
            sampled_aerial.unwrap_or(palette::GROUND),
            0.9,
        )
    };
    palette::mix(aerial_color, canopy_mass_color(point), CANOPY_FOLIAGE_MIX)
}

pub fn draw_street_trees<'a>(
    pixmap: &mut Pixmap,
    trees: impl IntoIterator<Item = &'a StreetTree>,
    projection: &Projection,
    depth: &mut [f32],
) {
    let mut rasterizer = TreeRasterizer {
        pixmap,
        projection,
        depth,
    };
    for tree in trees {
        rasterizer.draw(tree);
    }
}

struct TreeRasterizer<'a, 'b> {
    pixmap: &'a mut Pixmap,
    projection: &'b Projection,
    depth: &'a mut [f32],
}

impl TreeRasterizer<'_, '_> {
    fn draw(&mut self, tree: &StreetTree) {
        let crown_radius = match tree.form {
            // Retain the v59 fallback silhouette exactly.  New form-specific
            // layouts are deliberately opt-in for explicit source labels.
            TreeForm::Default => {
                tree.crown_radius() * crown_style(tree.point, tree.form).radius_scale
            }
            TreeForm::Conifer | TreeForm::Columnar | TreeForm::Weeping | TreeForm::Shrub => {
                tree.crown_radius()
            }
        };
        let radius_px = crown_radius * self.projection.scale;
        if radius_px < MIN_CROWN_RADIUS_PIXELS {
            return;
        }
        let lobes = crown_lobes(tree);
        let projected_lobes = lobes.map(|lobe| ProjectedCrownLobe {
            center: self.projection.point(lobe.point, lobe.height),
            base_depth: self.projection.depth(lobe.point, lobe.height),
            radius: lobe.radius,
            tone: lobe.tone,
        });
        self.draw_trunk(tree, trunk_top(tree));
        let (min_x, max_x, min_y, max_y) = projected_lobes.iter().fold(
            (
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
            ),
            |(min_x, max_x, min_y, max_y), lobe| {
                let radius_px = lobe.radius * self.projection.scale;
                (
                    min_x.min(lobe.center.0 - radius_px * SQRT_2),
                    max_x.max(lobe.center.0 + radius_px * SQRT_2),
                    min_y.min(lobe.center.1 - radius_px * SQRT_1_5),
                    max_y.max(lobe.center.1 + radius_px * SQRT_1_5),
                )
            },
        );
        let min_x = min_x.floor().max(0.0) as usize;
        let max_x = max_x.ceil().min((TILE_SIZE - 1) as f32) as usize;
        let min_y = min_y.floor().max(0.0) as usize;
        let max_y = max_y.ceil().min((TILE_SIZE - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            return;
        }
        let palette = street_tree_palette(tree.point, tree.form);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let mut closest = None;
                for lobe in projected_lobes {
                    let projected_x = (x as f32 + 0.5 - lobe.center.0) / self.projection.scale;
                    let projected_y = (y as f32 + 0.5 - lobe.center.1) / self.projection.scale;
                    let Some(surface) = sphere_surface(projected_x, projected_y, lobe.radius)
                    else {
                        continue;
                    };
                    let pixel_depth = lobe.base_depth + surface.depth_offset;
                    if closest.is_none_or(|(depth, _, _, _)| pixel_depth > depth) {
                        closest = Some((pixel_depth, surface, lobe.radius, lobe.tone));
                    }
                }
                let Some((pixel_depth, surface, surface_radius, tone)) = closest else {
                    continue;
                };
                let offset = y * TILE_SIZE + x;
                if pixel_depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = pixel_depth;
                let light = (0.8 + 0.15 * surface.vertical / surface_radius
                    - 0.05 * surface.east / surface_radius
                    + 0.03 * surface.north / surface_radius)
                    .clamp(0.66, 1.02);
                // A few broad tone steps read as foliage at isometric scale;
                // separate overlapping lobes keep the crown from reading as a
                // green balloon while avoiding noisy per-pixel stippling.
                let light = ((light * tone) * 8.0).round() / 8.0;
                self.set_pixel(offset, palette.map(|channel| shade(channel, light)));
            }
        }
    }

    fn draw_trunk(&mut self, tree: &StreetTree, top: f32) {
        if top <= 0.0 {
            return;
        }
        let ground = self.projection.point(tree.point, 0.0);
        let trunk_top = self.projection.point(tree.point, top);
        let half_width = (tree.diameter * self.projection.scale * 0.45).max(0.51);
        let min_x = (ground.0 - half_width).floor().max(0.0) as usize;
        let max_x = (ground.0 + half_width).ceil().min((TILE_SIZE - 1) as f32) as usize;
        let min_y = trunk_top.1.floor().max(0.0) as usize;
        let max_y = ground.1.ceil().min((TILE_SIZE - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            return;
        }
        for y in min_y..=max_y {
            let height = (ground.1 - (y as f32 + 0.5)) / self.projection.scale;
            if !(0.0..=top).contains(&height) {
                continue;
            }
            for x in min_x..=max_x {
                if (x as f32 + 0.5 - ground.0).abs() > half_width {
                    continue;
                }
                let offset = y * TILE_SIZE + x;
                let pixel_depth = self.projection.depth(tree.point, height);
                if pixel_depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = pixel_depth;
                self.set_pixel(offset, TRUNK_COLOR);
            }
        }
    }

    fn set_pixel(&mut self, offset: usize, color: [u8; 3]) {
        let start = offset * 4;
        self.pixmap.data_mut()[start..start + 4]
            .copy_from_slice(&[color[0], color[1], color[2], 255]);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CrownStyle {
    radius_scale: f32,
    lean_x: f32,
    lean_y: f32,
    broadness: f32,
    diagonal: f32,
    turn: f32,
    tone_shift: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CrownLobe {
    point: (f32, f32),
    height: f32,
    radius: f32,
    tone: f32,
}

#[derive(Clone, Copy)]
struct ProjectedCrownLobe {
    center: (f32, f32),
    base_depth: f32,
    radius: f32,
    tone: f32,
}

fn crown_style(point: (f32, f32), form: TreeForm) -> CrownStyle {
    let hash = tree_hash(point);
    if form == TreeForm::Default {
        let signed = |shift: u32| ((hash >> shift & 0xff) as f32 / 127.5) - 1.0;
        return CrownStyle {
            radius_scale: 0.82 + ((hash & 0xff) as f32 / 255.0) * 0.1,
            lean_x: signed(8) * 0.055,
            lean_y: signed(16) * 0.045,
            broadness: signed(24) * 0.045,
            diagonal: signed(32) * 0.035,
            turn: 0.0,
            tone_shift: 0.0,
        };
    }
    let hash = hash ^ (form as u64).wrapping_mul(0x9e37_79b9);
    CrownStyle {
        radius_scale: 1.0,
        lean_x: 0.0,
        lean_y: 0.0,
        broadness: 0.0,
        diagonal: 0.0,
        turn: (hash >> 40) as u16 as f32 / u16::MAX as f32 * std::f32::consts::TAU,
        tone_shift: ((hash >> 24 & 0xff) as f32 / 255.0 - 0.5) * 0.06,
    }
}

fn crown_lobes(tree: &StreetTree) -> [CrownLobe; CROWN_LOBE_COUNT] {
    let point = tree.point;
    let radius = tree.crown_radius();
    let height = tree.height();
    let style = crown_style(point, tree.form);
    if tree.form == TreeForm::Default {
        let radius = radius * style.radius_scale;
        return default_crown_lobes(point, height - radius * 0.75, radius, style);
    }
    let lobe = |angle: f32, distance: f32, radius_scale: f32, top_clearance: f32, tone: f32| {
        let lobe_radius = radius * radius_scale;
        CrownLobe {
            point: (
                point.0 + (style.turn + angle).cos() * radius * distance,
                point.1 + (style.turn + angle).sin() * radius * distance,
            ),
            // A lobe's analytic sphere is bounded by this radius, so retaining
            // at least one radius of clearance keeps every crown top at or
            // below the existing DBH-derived visual height.
            height: height - radius * top_clearance,
            radius: lobe_radius,
            tone: tone + style.tone_shift,
        }
    };
    match tree.form {
        TreeForm::Default => unreachable!("default crowns returned above"),
        TreeForm::Conifer => [
            lobe(0.0, 0.0, 0.25, 0.25, 1.10),
            lobe(0.3, 0.12, 0.34, 0.46, 1.04),
            lobe(2.5, 0.21, 0.40, 0.58, 0.96),
            lobe(4.3, 0.24, 0.43, 0.70, 0.88),
        ],
        TreeForm::Columnar => [
            lobe(0.0, 0.0, 0.30, 0.30, 1.08),
            lobe(0.2, 0.07, 0.31, 0.43, 1.02),
            lobe(3.2, 0.11, 0.34, 0.55, 0.94),
            lobe(1.7, 0.08, 0.28, 0.60, 0.98),
        ],
        TreeForm::Weeping => [
            lobe(0.0, 0.0, 0.35, 0.35, 1.08),
            lobe(0.2, 0.25, 0.40, 0.58, 1.00),
            lobe(3.1, 0.28, 0.39, 0.65, 0.91),
            lobe(4.7, 0.18, 0.36, 0.56, 0.97),
        ],
        TreeForm::Shrub => [
            lobe(0.0, 0.20, 0.42, 0.85, 1.02),
            lobe(1.8, 0.30, 0.38, 0.80, 0.96),
            lobe(3.8, 0.28, 0.39, 0.82, 0.91),
            lobe(5.7, 0.24, 0.40, 0.78, 1.06),
        ],
    }
}

fn default_crown_lobes(
    point: (f32, f32),
    height: f32,
    radius: f32,
    style: CrownStyle,
) -> [CrownLobe; CROWN_LOBE_COUNT] {
    let turn = (tree_hash(point) >> 40) as u16;
    let angle = f32::from(turn) / f32::from(u16::MAX) * std::f32::consts::TAU;
    let direction = |offset: f32, distance: f32| {
        let angle = angle + offset;
        (angle.cos() * distance, angle.sin() * distance)
    };
    let offset = |(x, y): (f32, f32)| {
        (
            point.0 + x + style.lean_x * radius,
            point.1 + y + style.lean_y * radius,
        )
    };
    let broad = style.broadness * radius;
    let diagonal = style.diagonal * radius;
    let first = direction(0.0, radius * 0.27);
    let second = direction(2.12, radius * 0.28);
    let third = direction(4.24, radius * 0.26);

    [
        CrownLobe {
            point: offset((broad, diagonal)),
            height: height + radius * 0.08,
            radius: radius * 0.66,
            tone: 1.03,
        },
        CrownLobe {
            point: offset(first),
            height: height + radius * 0.14,
            radius: radius * 0.58,
            tone: 1.08,
        },
        CrownLobe {
            point: offset(second),
            height: height - radius * 0.12,
            radius: radius * 0.62,
            tone: 0.92,
        },
        CrownLobe {
            point: offset(third),
            height: height - radius * 0.04,
            radius: radius * 0.6,
            tone: 0.98,
        },
    ]
}

fn trunk_top(tree: &StreetTree) -> f32 {
    let radius = tree.crown_radius();
    if tree.form == TreeForm::Default {
        let radius = radius * crown_style(tree.point, tree.form).radius_scale;
        return tree.height() - radius * (0.75 + 0.52);
    }
    let clearance = match tree.form {
        TreeForm::Shrub => 1.15,
        TreeForm::Weeping => 0.96,
        TreeForm::Conifer => 0.82,
        TreeForm::Columnar => 0.88,
        TreeForm::Default => unreachable!("default trunk returned above"),
    };
    tree.height() - radius * clearance
}

#[derive(Clone, Copy)]
struct SphereSurface {
    east: f32,
    north: f32,
    vertical: f32,
    depth_offset: f32,
}

fn sphere_surface(projected_x: f32, projected_y: f32, radius: f32) -> Option<SphereSurface> {
    let discriminant =
        12.0 * radius * radius - 8.0 * projected_y * projected_y - 6.0 * projected_x * projected_x;
    if discriminant < 0.0 {
        return None;
    }
    let vertical = (-4.0 * projected_y + discriminant.sqrt()) / 6.0;
    let q = projected_y + vertical;
    let east = (projected_x + 2.0 * q) * 0.5;
    let north = (projected_x - 2.0 * q) * 0.5;
    Some(SphereSurface {
        east,
        north,
        vertical,
        depth_offset: projected_y + 2.0 * vertical,
    })
}

fn tree_palette(point: (f32, f32)) -> [u8; 3] {
    palette::tree_foliage(tree_hash(point))
}

fn street_tree_palette(point: (f32, f32), form: TreeForm) -> [u8; 3] {
    match form {
        TreeForm::Default => tree_palette(point),
        TreeForm::Conifer | TreeForm::Columnar | TreeForm::Weeping | TreeForm::Shrub => {
            palette::tree_foliage(
                tree_hash(point) ^ (form as u64).wrapping_mul(0xd6e8_feb8_6659_fd93),
            )
        }
    }
}

fn tree_hash(point: (f32, f32)) -> u64 {
    let x = point.0.round() as i64 as u64;
    let y = point.1.round() as i64 as u64;
    let mut hash = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77);
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^ (hash >> 27)
}

fn shade(channel: u8, light: f32) -> u8 {
    (f32::from(channel) * light).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::{
        CANOPY_MASS_HEIGHT, CANOPY_TONE_PATCH_METERS, CROWN_LOBE_COUNT, MIN_CROWN_RADIUS_PIXELS,
        TILE_SIZE, TRUNK_COLOR, canopy_mass_color, canopy_surface_at_pixel, crown_lobes,
        crown_style, draw_canopy_mass_with_samples, draw_street_trees, grade_canopy_color,
        sphere_surface, street_tree_palette, tree_palette, trunk_top,
    };
    use crate::{
        land_cover::LandCoverClass,
        palette,
        projection::Projection,
        world::{Bounds, StreetTree, TreeForm, View},
    };

    fn projection_for(scale: f32, view: View) -> Projection {
        Projection {
            bounds: Bounds {
                min_x: -128.0 / scale,
                min_y: -128.0 / scale,
                max_x: 128.0 / scale,
                max_y: 128.0 / scale,
            },
            scale,
            view,
        }
    }

    fn projection(scale: f32) -> Projection {
        projection_for(scale, View::SouthEast)
    }

    #[test]
    fn palette_is_stable_and_varies_by_location() {
        assert_eq!(tree_palette((123.0, 456.0)), tree_palette((123.0, 456.0)));
        let colors: std::collections::BTreeSet<_> = (0..8)
            .flat_map(|x| (0..8).map(move |y| tree_palette((x as f32, y as f32))))
            .collect();
        assert_eq!(colors.len(), 4);
        assert_eq!(
            street_tree_palette((123.0, 456.0), TreeForm::Default),
            tree_palette((123.0, 456.0))
        );
        assert_eq!(
            street_tree_palette((123.0, 456.0), TreeForm::Conifer),
            street_tree_palette((123.0, 456.0), TreeForm::Conifer)
        );
    }

    #[test]
    fn crown_style_is_stable_for_a_form_and_changes_with_the_form() {
        assert_eq!(
            crown_style((123.0, 456.0), TreeForm::Conifer),
            crown_style((123.0, 456.0), TreeForm::Conifer)
        );
        assert_ne!(
            crown_style((123.0, 456.0), TreeForm::Conifer),
            crown_style((123.0, 456.0), TreeForm::Shrub)
        );
    }

    #[test]
    fn default_crown_keeps_the_v59_scale_and_trunk_placement() {
        let tree = StreetTree {
            point: (123.0, 456.0),
            diameter: 1.0,
            form: TreeForm::Default,
        };
        let style = crown_style(tree.point, tree.form);
        let scaled_radius = tree.crown_radius() * style.radius_scale;
        let lobes = crown_lobes(&tree);

        assert!((0.82..=0.92).contains(&style.radius_scale));
        assert_eq!(lobes[0].radius.to_bits(), (scaled_radius * 0.66).to_bits());
        assert_eq!(
            trunk_top(&tree).to_bits(),
            (tree.height() - scaled_radius * (0.75 + 0.52)).to_bits()
        );
    }

    #[test]
    fn canopy_surface_recovers_source_coordinates_and_depth_in_every_view() {
        let source = (823_456.25, 74_321.75);
        for view in View::ALL {
            let projected = view.project(source.0, source.1, CANOPY_MASS_HEIGHT);
            let projection = Projection {
                bounds: Bounds {
                    min_x: projected.0 - 0.5,
                    min_y: projected.1 - 0.5,
                    max_x: projected.0 + 255.5,
                    max_y: projected.1 + 255.5,
                },
                scale: 1.0,
                view,
            };
            let (recovered, depth) = canopy_surface_at_pixel(&projection, 0, 0);

            assert!((recovered.0 - source.0).abs() < 0.1, "{}", view.id());
            assert!((recovered.1 - source.1).abs() < 0.1, "{}", view.id());
            assert!((depth - projection.depth(source, CANOPY_MASS_HEIGHT)).abs() < 0.1);
        }
    }

    #[test]
    fn canopy_mass_is_deterministic_and_respects_existing_depth() -> Result<(), &'static str> {
        let mut first = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut second = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut first_depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
        let mut second_depth = first_depth.clone();
        let sample = |_| Some(LandCoverClass::TreeCanopy);

        draw_canopy_mass_with_samples(
            &mut first,
            &projection(1.0),
            &mut first_depth,
            sample,
            |_| Some([92, 128, 72]),
        );
        draw_canopy_mass_with_samples(
            &mut second,
            &projection(1.0),
            &mut second_depth,
            sample,
            |_| Some([92, 128, 72]),
        );

        assert_eq!(first.data(), second.data());
        assert_eq!(first_depth, second_depth);
        assert!(first_depth.iter().all(|value| value.is_finite()));
        assert!(first.data().chunks_exact(4).all(|pixel| pixel[3] == 255));

        let mut blocked = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut blocked_depth = vec![f32::INFINITY; TILE_SIZE * TILE_SIZE];
        draw_canopy_mass_with_samples(
            &mut blocked,
            &projection(1.0),
            &mut blocked_depth,
            sample,
            |_| Some([92, 128, 72]),
        );
        assert!(blocked.data().iter().all(|channel| *channel == 0));
        assert!(blocked_depth.iter().all(|value| *value == f32::INFINITY));
        Ok(())
    }

    #[test]
    fn explicit_street_trees_stay_above_the_canopy_mass() -> Result<(), &'static str> {
        let mut pixmap = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
        let projection = projection(3.0);
        draw_canopy_mass_with_samples(
            &mut pixmap,
            &projection,
            &mut depth,
            |_| Some(LandCoverClass::TreeCanopy),
            |_| Some([92, 128, 72]),
        );
        let before = pixmap.data().to_vec();
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 1.0,
            form: TreeForm::Default,
        };

        draw_street_trees(&mut pixmap, [&tree], &projection, &mut depth);

        assert_ne!(pixmap.data(), before);
        Ok(())
    }

    #[test]
    fn canopy_grading_preserves_aerial_detail_and_uses_the_missing_imagery_fallback() {
        let point = (819_516.25, 72_998.75);
        let nearby = (point.0 + 0.1, point.1);
        let first = grade_canopy_color(point, Some([80, 120, 70]));
        let second = grade_canopy_color(nearby, Some([96, 120, 70]));

        assert_ne!(first, second);
        assert!(second[0] > first[0]);

        let fallback = grade_canopy_color(point, None);
        assert_eq!(fallback, grade_canopy_color(point, Some([250, 250, 250])));
        assert_eq!(
            fallback,
            palette::mix(palette::GROUND, canopy_mass_color(point), 0.56)
        );
    }

    #[test]
    fn canopy_tone_uses_source_patches_not_tile_phase() {
        let point = (819_516.25, 72_998.75);

        assert_eq!(canopy_mass_color(point), canopy_mass_color(point));
        assert_eq!(
            canopy_mass_color(point),
            canopy_mass_color((point.0 + CANOPY_TONE_PATCH_METERS * 0.01, point.1))
        );
        let tones: std::collections::BTreeSet<_> = (0..16)
            .map(|offset| {
                canopy_mass_color((point.0 + offset as f32 * CANOPY_TONE_PATCH_METERS, point.1))
            })
            .collect();
        assert!(tones.len() > 1);
    }

    #[test]
    fn crown_forms_are_stable_and_bounded_by_the_raw_inventory_crown() {
        let point = (123.0, 456.0);
        let mut layouts = Vec::new();
        for form in [
            TreeForm::Default,
            TreeForm::Conifer,
            TreeForm::Columnar,
            TreeForm::Weeping,
            TreeForm::Shrub,
        ] {
            let tree = StreetTree {
                point,
                diameter: 1.0,
                form,
            };
            let first = crown_lobes(&tree);
            assert_eq!(first, crown_lobes(&tree));
            assert_eq!(first.len(), CROWN_LOBE_COUNT);
            assert!(first.windows(2).any(|lobes| lobes[0].tone != lobes[1].tone));
            for lobe in first {
                let offset = (lobe.point.0 - point.0).hypot(lobe.point.1 - point.1);
                assert!(offset + lobe.radius <= tree.crown_radius() + f32::EPSILON);
                assert!(lobe.height + lobe.radius <= tree.height() + f32::EPSILON);
            }
            layouts.push(crown_lobes(&tree).map(|lobe| {
                (
                    (lobe.point.0 - point.0).to_bits(),
                    (lobe.point.1 - point.1).to_bits(),
                    lobe.height.to_bits(),
                    lobe.radius.to_bits(),
                )
            }));
        }
        assert!(layouts.windows(2).all(|layouts| layouts[0] != layouts[1]));
    }

    #[test]
    fn subpixel_trees_are_skipped() -> Result<(), &'static str> {
        let mut pixmap = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 0.15,
            form: TreeForm::Default,
        };
        let scale = (MIN_CROWN_RADIUS_PIXELS / tree.crown_radius()) * 0.9;

        draw_street_trees(&mut pixmap, [&tree], &projection(scale), &mut depth);

        assert!(pixmap.data().iter().all(|channel| *channel == 0));
        assert!(depth.iter().all(|value| *value == f32::NEG_INFINITY));
        Ok(())
    }

    #[test]
    fn visible_tree_respects_existing_depth() -> Result<(), &'static str> {
        let mut pixmap = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut depth = vec![f32::INFINITY; TILE_SIZE * TILE_SIZE];
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 0.5,
            form: TreeForm::Default,
        };

        draw_street_trees(&mut pixmap, [&tree], &projection(2.0), &mut depth);

        assert!(pixmap.data().iter().all(|channel| *channel == 0));
        Ok(())
    }

    #[test]
    fn visible_tree_draw_is_deterministic() -> Result<(), &'static str> {
        let mut first = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut second = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut first_depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
        let mut second_depth = first_depth.clone();
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 0.5,
            form: TreeForm::Default,
        };

        draw_street_trees(&mut first, [&tree], &projection(2.0), &mut first_depth);
        draw_street_trees(&mut second, [&tree], &projection(2.0), &mut second_depth);

        assert!(first.data().iter().any(|channel| *channel != 0));
        assert!(first_depth.iter().any(|value| value.is_finite()));
        assert_eq!(first.data(), second.data());
        assert_eq!(first_depth, second_depth);
        Ok(())
    }

    #[test]
    fn crown_surface_lies_on_the_sphere_and_uses_matching_depth() -> Result<(), &'static str> {
        let projected_x = 0.7;
        let projected_y = -0.4;
        let radius = 2.5;
        let surface = sphere_surface(projected_x, projected_y, radius).ok_or("surface")?;
        let squared_radius = surface.east.mul_add(
            surface.east,
            surface
                .north
                .mul_add(surface.north, surface.vertical * surface.vertical),
        );

        assert!((squared_radius - radius * radius).abs() < 0.000_01);
        assert!(
            (surface.depth_offset - (projected_y + 2.0 * surface.vertical)).abs() < f32::EPSILON
        );
        assert!(sphere_surface(radius * 2.0, 0.0, radius).is_none());
        Ok(())
    }

    #[test]
    fn partial_building_edge_occludes_only_covered_crown_pixels() -> Result<(), &'static str> {
        for view in View::ALL {
            let mut pixmap = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
            let mut depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
            for row in depth.chunks_exact_mut(TILE_SIZE) {
                row[..TILE_SIZE / 2].fill(f32::INFINITY);
            }
            let tree = StreetTree {
                point: view.inverse(0.0, 0.0),
                diameter: 0.5,
                form: TreeForm::Default,
            };

            draw_street_trees(&mut pixmap, [&tree], &projection_for(3.0, view), &mut depth);

            assert!(
                pixmap
                    .data()
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(offset, _)| offset % TILE_SIZE < TILE_SIZE / 2)
                    .all(|(_, pixel)| pixel == [0, 0, 0, 0]),
                "{}",
                view.id()
            );
            assert!(
                pixmap
                    .data()
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(offset, _)| offset % TILE_SIZE >= TILE_SIZE / 2)
                    .any(|(_, pixel)| pixel != [0, 0, 0, 0]),
                "{}",
                view.id()
            );
        }
        Ok(())
    }

    #[test]
    fn overlapping_crowns_are_order_independent_in_every_view() -> Result<(), &'static str> {
        for view in View::ALL {
            let trees = [
                StreetTree {
                    point: view.inverse(0.0, 0.0),
                    diameter: 0.55,
                    form: TreeForm::Default,
                },
                StreetTree {
                    point: view.inverse(0.4, 0.7),
                    diameter: 0.45,
                    form: TreeForm::Conifer,
                },
            ];
            let mut forward = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
            let mut reverse = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
            let mut forward_depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
            let mut reverse_depth = forward_depth.clone();
            let projection = projection_for(3.0, view);

            draw_street_trees(&mut forward, trees.iter(), &projection, &mut forward_depth);
            draw_street_trees(
                &mut reverse,
                trees.iter().rev(),
                &projection,
                &mut reverse_depth,
            );

            assert_eq!(forward.data(), reverse.data(), "{}", view.id());
            assert_eq!(forward_depth, reverse_depth, "{}", view.id());
        }
        Ok(())
    }

    #[test]
    fn crowns_cross_adjacent_tile_seams_in_every_view() -> Result<(), &'static str> {
        for view in View::ALL {
            let tree = StreetTree {
                point: view.inverse(0.0, 0.0),
                diameter: 0.5,
                form: TreeForm::Weeping,
            };
            let mut left = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
            let mut right = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
            let mut left_depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
            let mut right_depth = left_depth.clone();
            let left_projection = Projection {
                bounds: Bounds {
                    min_x: -(TILE_SIZE as f32),
                    min_y: -128.0,
                    max_x: 0.0,
                    max_y: 128.0,
                },
                scale: 1.0,
                view,
            };
            let right_projection = Projection {
                bounds: Bounds {
                    min_x: 0.0,
                    min_y: -128.0,
                    max_x: TILE_SIZE as f32,
                    max_y: 128.0,
                },
                scale: 1.0,
                view,
            };

            draw_street_trees(&mut left, [&tree], &left_projection, &mut left_depth);
            draw_street_trees(&mut right, [&tree], &right_projection, &mut right_depth);

            assert!(
                left.data()
                    .chunks_exact(4)
                    .skip(TILE_SIZE - 1)
                    .step_by(TILE_SIZE)
                    .any(|pixel| pixel != [0, 0, 0, 0]),
                "{} left",
                view.id()
            );
            assert!(
                right
                    .data()
                    .chunks_exact(4)
                    .step_by(TILE_SIZE)
                    .any(|pixel| pixel != [0, 0, 0, 0]),
                "{} right",
                view.id()
            );
        }
        Ok(())
    }

    #[test]
    fn default_tree_has_a_visible_trunk() -> Result<(), &'static str> {
        let mut pixmap = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 0.15,
            form: TreeForm::Default,
        };

        draw_street_trees(&mut pixmap, [&tree], &projection(3.0), &mut depth);

        assert!(
            pixmap
                .data()
                .chunks_exact(4)
                .any(|pixel| pixel == [TRUNK_COLOR[0], TRUNK_COLOR[1], TRUNK_COLOR[2], 255])
        );
        Ok(())
    }
}
