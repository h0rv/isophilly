use tiny_skia::Pixmap;

use crate::{projection::Projection, world::StreetTree};

const TILE_SIZE: usize = 256;
const MIN_CROWN_RADIUS_PIXELS: f32 = 0.55;
const SQRT_2: f32 = std::f32::consts::SQRT_2;
const SQRT_1_5: f32 = 1.224_744_9;

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
        let radius_px = crown_radius * self.projection.scale;
        if radius_px < MIN_CROWN_RADIUS_PIXELS {
            return;
        }
        let height = tree.height();
        let crown_center_height = height - crown_radius * 0.72;
        let center = self.projection.point(tree.point, crown_center_height);
        self.draw_trunk(tree, crown_center_height - crown_radius * 0.72);
        let extent_x = radius_px * SQRT_2;
        let extent_y = radius_px * SQRT_1_5;
        let min_x = (center.0 - extent_x).floor().max(0.0) as usize;
        let max_x = (center.0 + extent_x).ceil().min((TILE_SIZE - 1) as f32) as usize;
        let min_y = (center.1 - extent_y).floor().max(0.0) as usize;
        let max_y = (center.1 + extent_y).ceil().min((TILE_SIZE - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            return;
        }
        let palette = tree_palette(tree.point);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let projected_x = (x as f32 + 0.5 - center.0) / self.projection.scale;
                let projected_y = (y as f32 + 0.5 - center.1) / self.projection.scale;
                // In view space p=e+n and q=(e-n)/2. A sphere point projected
                // at screen offset (p, s) has q=s+t, where t is its vertical
                // offset from the crown center. Substitution into
                // e²+n²+t²=r² gives this quadratic. The larger root is the
                // camera-facing surface and its depth offset is s+2t.
                let Some(surface) = sphere_surface(projected_x, projected_y, crown_radius) else {
                    continue;
                };
                let pixel_depth =
                    self.projection.depth(tree.point, crown_center_height) + surface.depth_offset;
                let offset = y * TILE_SIZE + x;
                if pixel_depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = pixel_depth;
                let light = (0.82 + 0.17 * surface.vertical / crown_radius
                    - 0.06 * surface.east / crown_radius
                    + 0.04 * surface.north / crown_radius)
                    .clamp(0.65, 1.08);
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
        let half_width = (tree.diameter * self.projection.scale * 0.5).max(0.55);
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
                self.set_pixel(offset, [92, 66, 43]);
            }
        }
    }

    fn set_pixel(&mut self, offset: usize, color: [u8; 3]) {
        let start = offset * 4;
        self.pixmap.data_mut()[start..start + 4]
            .copy_from_slice(&[color[0], color[1], color[2], 255]);
    }
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
    let x = point.0.round() as i64 as u64;
    let y = point.1.round() as i64 as u64;
    match (x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77)) % 4 {
        0 => [54, 112, 51],
        1 => [63, 126, 56],
        2 => [70, 132, 60],
        _ => [49, 105, 48],
    }
}

fn shade(channel: u8, light: f32) -> u8 {
    (f32::from(channel) * light).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::{
        MIN_CROWN_RADIUS_PIXELS, TILE_SIZE, draw_street_trees, sphere_surface, tree_palette,
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
                .any(|pixel| pixel == [92, 66, 43, 255])
        );
        Ok(())
    }
}
