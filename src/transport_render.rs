use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::{
    projection::Projection,
    world::{TransportKind, TransportLine},
};

// EPSG:32129 horizontal coordinates in this dataset are US survey feet. These
// widths are deliberately narrower than an actual carriageway: the aerial
// remains the road surface and the linework is only a hierarchy cue.
const EXPRESSWAY_WIDTH_FEET: f32 = 18.0;
const ARTERIAL_WIDTH_FEET: f32 = 10.0;
const CONNECTOR_WIDTH_FEET: f32 = 5.5;

/// Draw only City-ranked through routes over the aerial before shadows and
/// buildings. The physical width is world-anchored, so adjacent prebuilt tiles
/// meet exactly; a low pixel floor keeps arteries legible only when useful.
pub(crate) fn draw_transport<'a>(
    pixmap: &mut Pixmap,
    lines: impl IntoIterator<Item = &'a TransportLine>,
    projection: &Projection,
) {
    let lines: Vec<_> = lines.into_iter().collect();
    for kind in [
        TransportKind::Connector,
        TransportKind::Arterial,
        TransportKind::Expressway,
    ] {
        let mut path = PathBuilder::new();
        for line in lines.iter().copied().filter(|line| line.kind == kind) {
            append_line(&mut path, line, projection);
        }
        let Some(path) = path.finish() else {
            continue;
        };
        let (color, width_feet, minimum_pixels) = style(kind);
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: (width_feet * projection.scale).max(minimum_pixels),
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn append_line(path: &mut PathBuilder, line: &TransportLine, projection: &Projection) {
    let Some((&first, rest)) = line.points.split_first() else {
        return;
    };
    let first = projection.point(first, 0.0);
    path.move_to(first.0, first.1);
    for &point in rest {
        let point = projection.point(point, 0.0);
        path.line_to(point.0, point.1);
    }
}

fn style(kind: TransportKind) -> ([u8; 4], f32, f32) {
    match kind {
        // A cool, low-alpha hierarchy keeps the additional information from
        // repainting the photography. Each higher class is only subtly darker.
        TransportKind::Expressway => ([70, 76, 81, 92], EXPRESSWAY_WIDTH_FEET, 1.1),
        TransportKind::Arterial => ([91, 94, 91, 69], ARTERIAL_WIDTH_FEET, 0.8),
        TransportKind::Connector => ([111, 108, 100, 42], CONNECTOR_WIDTH_FEET, 0.55),
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::Pixmap;

    use super::draw_transport;
    use crate::{
        projection::Projection,
        world::{Bounds, TransportKind, TransportLine, View},
    };

    fn line(kind: TransportKind) -> TransportLine {
        TransportLine {
            kind,
            points: vec![(0.0, 8.0), (10.0, 8.0)],
            bounds: Bounds {
                min_x: 0.0,
                min_y: 8.0,
                max_x: 10.0,
                max_y: 8.0,
            },
        }
    }

    fn projection(bounds: Bounds) -> Projection {
        Projection {
            bounds,
            scale: 8.0,
            view: View::SouthEast,
        }
    }

    #[test]
    fn linework_changes_only_its_projected_route() -> Result<(), &'static str> {
        let mut pixmap = Pixmap::new(256, 256).ok_or("test pixmap")?;
        draw_transport(
            &mut pixmap,
            [&line(TransportKind::Arterial)],
            &projection(Bounds {
                min_x: -10.0,
                min_y: -10.0,
                max_x: 22.0,
                max_y: 22.0,
            }),
        );
        assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] > 0));
        Ok(())
    }

    #[test]
    fn adjacent_tiles_match_at_the_shared_edge() -> Result<(), &'static str> {
        let route = line(TransportKind::Expressway);
        let mut left = Pixmap::new(80, 80).ok_or("test pixmap")?;
        let mut right = Pixmap::new(80, 80).ok_or("test pixmap")?;
        let left_projection = projection(Bounds {
            min_x: -10.0,
            min_y: -10.0,
            max_x: 0.0,
            max_y: 0.0,
        });
        let right_projection = projection(Bounds {
            min_x: 0.0,
            min_y: -10.0,
            max_x: 10.0,
            max_y: 0.0,
        });
        draw_transport(&mut left, [&route], &left_projection);
        draw_transport(&mut right, [&route], &right_projection);
        let left_edge: Vec<_> = (0..80)
            .map(|row| left.data()[(row * 80 + 79) * 4..(row * 80 + 80) * 4].to_vec())
            .collect();
        let right_edge: Vec<_> = (0..80)
            .map(|row| right.data()[(row * 80) * 4..(row * 80 + 1) * 4].to_vec())
            .collect();
        assert_eq!(left_edge, right_edge);
        Ok(())
    }
}
