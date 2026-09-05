use tiny_skia::Pixmap;

use crate::{projection::Projection, world::StreetTree};

const TILE_SIZE: usize = 256;
const MIN_CROWN_RADIUS_PIXELS: f32 = 0.55;
const SQRT_2: f32 = std::f32::consts::SQRT_2;
const SQRT_1_5: f32 = 1.224_744_9;
const TRUNK_COLOR: [u8; 3] = [76, 61, 43];
const CROWN_LOBE_COUNT: usize = 4;

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
        let crown_radius = tree.crown_radius();
        let style = crown_style(tree.point);
        let crown_radius = crown_radius * style.radius_scale;
        let radius_px = crown_radius * self.projection.scale;
        if radius_px < MIN_CROWN_RADIUS_PIXELS {
            return;
        }
        let height = tree.height();
        let crown_center_height = height - crown_radius * 0.75;
        let lobes = crown_lobes(tree.point, crown_center_height, crown_radius, style);
        let projected_lobes = lobes.map(|lobe| ProjectedCrownLobe {
            center: self.projection.point(lobe.point, lobe.height),
            base_depth: self.projection.depth(lobe.point, lobe.height),
            radius: lobe.radius,
            tone: lobe.tone,
        });
        self.draw_trunk(tree, crown_center_height - crown_radius * 0.52);
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
        let palette = tree_palette(tree.point);
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

fn crown_style(point: (f32, f32)) -> CrownStyle {
    let hash = tree_hash(point);
    let signed = |shift: u32| ((hash >> shift & 0xff) as f32 / 127.5) - 1.0;
    CrownStyle {
        // The inventory diameter is useful, but drawing its full inferred crown
        // made dense blocks overwhelm streets and parks.
        radius_scale: 0.82 + ((hash & 0xff) as f32 / 255.0) * 0.1,
        lean_x: signed(8) * 0.055,
        lean_y: signed(16) * 0.045,
        broadness: signed(24) * 0.045,
        diagonal: signed(32) * 0.035,
    }
}

fn crown_lobes(
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
    match tree_hash(point) % 4 {
        0 => [48, 99, 49],
        1 => [55, 108, 52],
        2 => [61, 113, 55],
        _ => [44, 93, 47],
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
        CROWN_LOBE_COUNT, MIN_CROWN_RADIUS_PIXELS, TILE_SIZE, TRUNK_COLOR, crown_lobes,
        crown_style, draw_street_trees, sphere_surface, tree_palette,
    };
    use crate::{
        projection::Projection,
        world::{Bounds, StreetTree, View},
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
    }

    #[test]
    fn crown_shape_is_stable_and_varies_by_location() {
        assert_eq!(crown_style((123.0, 456.0)), crown_style((123.0, 456.0)));
        let shapes: std::collections::BTreeSet<_> = (0..8)
            .flat_map(|x| {
                (0..8).map(move |y| {
                    let style = crown_style((x as f32, y as f32));
                    (
                        style.radius_scale.to_bits(),
                        style.lean_x.to_bits(),
                        style.lean_y.to_bits(),
                    )
                })
            })
            .collect();
        assert!(shapes.len() > 32);
    }

    #[test]
    fn clustered_crown_is_stable_varied_and_inside_inventory_extent() {
        let point = (123.0, 456.0);
        let inventory_radius = 4.0;
        let style = crown_style(point);
        let radius = inventory_radius * style.radius_scale;
        let first = crown_lobes(point, 10.0, radius, style);
        let second = crown_lobes(point, 10.0, radius, style);

        assert_eq!(first, second);
        assert_eq!(first.len(), CROWN_LOBE_COUNT);
        assert!(first.windows(2).any(|lobes| lobes[0].tone != lobes[1].tone));
        assert!(
            first
                .windows(2)
                .any(|lobes| lobes[0].point != lobes[1].point)
        );
        for lobe in first {
            let offset = (lobe.point.0 - point.0).hypot(lobe.point.1 - point.1);
            assert!(offset + lobe.radius <= inventory_radius);
            assert!(lobe.height + lobe.radius <= 10.0 + radius * 0.75 + f32::EPSILON);
        }
    }

    #[test]
    fn subpixel_trees_are_skipped() -> Result<(), &'static str> {
        let mut pixmap = Pixmap::new(TILE_SIZE as u32, TILE_SIZE as u32).ok_or("pixmap")?;
        let mut depth = vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE];
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 0.15,
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
                },
                StreetTree {
                    point: view.inverse(0.4, 0.7),
                    diameter: 0.45,
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
