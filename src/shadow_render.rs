use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::{
    projection::Projection,
    world::{Building, BuildingPart, Ring, StreetTree},
};

// A fixed south-western sun keeps every prebuilt tile and repeatable visual test
// under identical lighting. These are ground-plane metres of north/east shadow
// per metre of height (roughly a 48 degree solar altitude).
const SHADOW_EAST_PER_HEIGHT: f32 = 0.55;
const SHADOW_NORTH_PER_HEIGHT: f32 = 0.72;
const SHADOW_COLOR: [u8; 4] = [48, 43, 38, 56];
const TREE_SHADOW_SIDES: usize = 12;

/// Paint fixed-sun cast shadows before the depth-tested city geometry.
///
/// All positions and curves are derived in world coordinates before projection.
/// The pixmap performs the only clipping, so neighboring 256-pixel tiles render
/// the same shadow at their shared boundary and panning cannot reset a pattern.
pub(crate) fn draw_cast_shadows<'a>(
    pixmap: &mut Pixmap,
    buildings: impl IntoIterator<Item = &'a Building>,
    parts: impl IntoIterator<Item = &'a BuildingPart>,
    trees: impl IntoIterator<Item = &'a StreetTree>,
    projection: &Projection,
) {
    let mut path = PathBuilder::new();
    for building in buildings {
        append_prism_shadow(&mut path, &building.ring, 0.0, building.height, projection);
    }
    for part in parts {
        append_prism_shadow(
            &mut path,
            &part.ring,
            part.min_height,
            part.height,
            projection,
        );
    }
    for tree in trees {
        append_tree_shadow(&mut path, tree, projection);
    }
    let Some(path) = path.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        SHADOW_COLOR[0],
        SHADOW_COLOR[1],
        SHADOW_COLOR[2],
        SHADOW_COLOR[3],
    );
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn append_prism_shadow(
    path: &mut PathBuilder,
    ring: &Ring,
    bottom: f32,
    top: f32,
    projection: &Projection,
) {
    if ring.points.len() < 3 || top <= bottom {
        return;
    }
    let bottom: Vec<_> = ring
        .points
        .iter()
        .map(|&point| projected_shadow_point(point, bottom, projection))
        .collect();
    let top: Vec<_> = ring
        .points
        .iter()
        .map(|&point| projected_shadow_point(point, top, projection))
        .collect();
    append_polygon(path, &top);
    for index in 0..ring.points.len() {
        let next = (index + 1) % ring.points.len();
        append_polygon(path, &[bottom[index], bottom[next], top[next], top[index]]);
    }
}

fn append_tree_shadow(path: &mut PathBuilder, tree: &StreetTree, projection: &Projection) {
    let radius = tree.crown_radius();
    let crown_height = tree.height() - radius * 0.64;
    let base = projection.point(tree.point, 0.0);
    let center_source = shadowed(tree.point, crown_height);
    let center = projection.point(center_source, 0.0);
    let direction = (center.0 - base.0, center.1 - base.1);
    let length = direction.0.hypot(direction.1);
    if length > f32::EPSILON {
        let half_width = (radius * projection.scale * 0.16).max(0.45);
        let normal = (
            -direction.1 / length * half_width,
            direction.0 / length * half_width,
        );
        append_polygon(
            path,
            &[
                (base.0 + normal.0, base.1 + normal.1),
                (base.0 - normal.0, base.1 - normal.1),
                (center.0 - normal.0, center.1 - normal.1),
                (center.0 + normal.0, center.1 + normal.1),
            ],
        );
    }
    let crown: Vec<_> = (0..TREE_SHADOW_SIDES)
        .map(|step| {
            let angle = step as f32 * std::f32::consts::TAU / TREE_SHADOW_SIDES as f32;
            projection.point(
                (
                    center_source.0 + angle.cos() * radius,
                    center_source.1 + angle.sin() * radius,
                ),
                0.0,
            )
        })
        .collect();
    append_polygon(path, &crown);
}

fn projected_shadow_point(point: (f32, f32), height: f32, projection: &Projection) -> (f32, f32) {
    projection.point(shadowed(point, height), 0.0)
}

fn shadowed(point: (f32, f32), height: f32) -> (f32, f32) {
    (
        SHADOW_EAST_PER_HEIGHT.mul_add(height, point.0),
        SHADOW_NORTH_PER_HEIGHT.mul_add(height, point.1),
    )
}

fn append_polygon(path: &mut PathBuilder, points: &[(f32, f32)]) {
    if points.len() < 3 {
        return;
    }
    let signed_area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(left, right)| left.0.mul_add(right.1, -(right.0 * left.1)))
        .sum::<f32>();
    if signed_area.abs() <= f32::EPSILON {
        return;
    }
    let mut ordered = points.to_vec();
    if signed_area < 0.0 {
        ordered.reverse();
    }
    path.move_to(ordered[0].0, ordered[0].1);
    for point in &ordered[1..] {
        path.line_to(point.0, point.1);
    }
    path.close();
}

#[cfg(test)]
mod tests {
    use tiny_skia::{Color, Pixmap};

    use super::{SHADOW_COLOR, draw_cast_shadows, shadowed};
    use crate::{
        projection::Projection,
        world::{Bounds, Building, Ring, StreetTree, TreeForm, View},
    };

    fn ring(points: &[(f32, f32)]) -> Ring {
        Ring {
            bounds: Bounds {
                min_x: points
                    .iter()
                    .map(|point| point.0)
                    .fold(f32::INFINITY, f32::min),
                min_y: points
                    .iter()
                    .map(|point| point.1)
                    .fold(f32::INFINITY, f32::min),
                max_x: points
                    .iter()
                    .map(|point| point.0)
                    .fold(f32::NEG_INFINITY, f32::max),
                max_y: points
                    .iter()
                    .map(|point| point.1)
                    .fold(f32::NEG_INFINITY, f32::max),
            },
            points: points.to_vec(),
        }
    }

    fn projection(bounds: Bounds) -> Projection {
        Projection {
            bounds,
            scale: 1.0,
            view: View::SouthEast,
        }
    }

    fn white(width: u32, height: u32) -> Result<Pixmap, &'static str> {
        let mut pixmap = Pixmap::new(width, height).ok_or("test pixmap")?;
        pixmap.fill(Color::WHITE);
        Ok(pixmap)
    }

    #[test]
    fn shadow_vector_is_fixed_in_world_coordinates() {
        let first = shadowed((100.0, 200.0), 10.0);
        let second = shadowed((112.0, 207.0), 10.0);

        assert_eq!((second.0 - first.0, second.1 - first.1), (12.0, 7.0));
        assert_eq!(shadowed((100.0, 200.0), 0.0), (100.0, 200.0));
    }

    #[test]
    fn building_shadow_is_translucent() -> Result<(), &'static str> {
        let building = Building {
            height: 18.0,
            frontage_edge: None,
            ring: ring(&[(0.0, 0.0), (12.0, 0.0), (12.0, 12.0), (0.0, 12.0)]),
        };
        let center = View::SouthEast.project(6.0, 6.0, 0.0);
        let bounds = Bounds {
            min_x: center.0 - 64.0,
            min_y: center.1 - 64.0,
            max_x: center.0 + 64.0,
            max_y: center.1 + 64.0,
        };
        let mut pixmap = white(128, 128)?;

        draw_cast_shadows(
            &mut pixmap,
            [&building],
            std::iter::empty(),
            std::iter::empty(),
            &projection(bounds),
        );

        assert!(pixmap.data().chunks_exact(4).any(|pixel| {
            pixel[0] < 255
                && pixel[0] > SHADOW_COLOR[0]
                && pixel[1] < 255
                && pixel[2] < 255
                && pixel[3] == 255
        }));
        Ok(())
    }

    #[test]
    fn tree_shadow_is_visible_without_tile_local_noise() -> Result<(), &'static str> {
        let tree = StreetTree {
            point: (0.0, 0.0),
            diameter: 0.4,
            form: TreeForm::Default,
        };
        let center = View::SouthEast.project(0.0, 0.0, 0.0);
        let bounds = Bounds {
            min_x: center.0 - 64.0,
            min_y: center.1 - 64.0,
            max_x: center.0 + 64.0,
            max_y: center.1 + 64.0,
        };
        let mut pixmap = white(128, 128)?;

        draw_cast_shadows(
            &mut pixmap,
            std::iter::empty(),
            std::iter::empty(),
            [&tree],
            &projection(bounds),
        );

        assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[0] < 255));
        Ok(())
    }

    #[test]
    fn adjacent_tiles_match_a_single_world_anchored_render() -> Result<(), &'static str> {
        let building = Building {
            height: 30.0,
            frontage_edge: None,
            ring: ring(&[(95.0, 95.0), (110.0, 95.0), (110.0, 110.0), (95.0, 110.0)]),
        };
        let center = View::SouthEast.project(102.5, 102.5, 0.0);
        let min_y = center.1 - 128.0;
        let left_bounds = Bounds {
            min_x: center.0 - 256.0,
            min_y,
            max_x: center.0,
            max_y: min_y + 256.0,
        };
        let right_bounds = Bounds {
            min_x: center.0,
            min_y,
            max_x: center.0 + 256.0,
            max_y: min_y + 256.0,
        };
        let whole_bounds = Bounds {
            min_x: left_bounds.min_x,
            min_y,
            max_x: right_bounds.max_x,
            max_y: min_y + 256.0,
        };
        let mut left = white(256, 256)?;
        let mut right = white(256, 256)?;
        let mut whole = white(512, 256)?;

        for (pixmap, bounds) in [
            (&mut left, left_bounds),
            (&mut right, right_bounds),
            (&mut whole, whole_bounds),
        ] {
            draw_cast_shadows(
                pixmap,
                [&building],
                std::iter::empty(),
                std::iter::empty(),
                &projection(bounds),
            );
        }

        for y in 0..256_usize {
            let left_row = &left.data()[y * 256 * 4..(y + 1) * 256 * 4];
            let right_row = &right.data()[y * 256 * 4..(y + 1) * 256 * 4];
            let whole_row = &whole.data()[y * 512 * 4..(y + 1) * 512 * 4];
            assert_eq!(left_row, &whole_row[..256 * 4]);
            assert_eq!(right_row, &whole_row[256 * 4..]);
        }
        Ok(())
    }
}
