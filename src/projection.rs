use crate::world::{Bounds, isometric};

pub struct Projection {
    pub bounds: Bounds,
    pub scale: f32,
}

impl Projection {
    pub fn point(&self, point: (f32, f32), height: f32) -> (f32, f32) {
        let projected = isometric(point.0, point.1, height);
        (
            (projected.0 - self.bounds.min_x) * self.scale,
            (projected.1 - self.bounds.min_y) * self.scale,
        )
    }
}
