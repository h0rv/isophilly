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
}
