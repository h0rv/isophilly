use crate::world::{Bounds, View};

pub struct Projection {
    pub bounds: Bounds,
    pub scale: f32,
    pub view: View,
}

impl Projection {
    pub fn point(&self, point: (f32, f32), height: f32) -> (f32, f32) {
        let projected = self.view.project(point.0, point.1, height);
        (
            (projected.0 - self.bounds.min_x) * self.scale,
            (projected.1 - self.bounds.min_y) * self.scale,
        )
    }

    pub fn inverse(&self, point: (f32, f32)) -> (f32, f32) {
        self.view.inverse(point.0, point.1)
    }

    pub fn depth(&self, point: (f32, f32), height: f32) -> f32 {
        self.view.depth(point.0, point.1, height)
    }
}

#[cfg(test)]
mod tests {
    use super::Projection;
    use crate::world::{Bounds, View};

    #[test]
    fn projection_inverse_and_depth_follow_every_view() {
        let source = (820_983.0, 71_996.0);
        for view in View::ALL {
            let projection = Projection {
                bounds: Bounds {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 1.0,
                    max_y: 1.0,
                },
                scale: 1.0,
                view,
            };
            let ground = view.project(source.0, source.1, 0.0);
            let round_trip = projection.inverse(ground);

            assert!((round_trip.0 - source.0).abs() < 0.1, "{}", view.id());
            assert!((round_trip.1 - source.1).abs() < 0.1, "{}", view.id());
            assert_eq!(projection.depth(source, 18.0), ground.1 + 18.0);
        }
    }
}
