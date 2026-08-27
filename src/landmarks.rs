use crate::world::isometric;

pub const CITY_HALL_SOURCE: (f32, f32) = (820_988.6, 72_011.0);
pub const CITY_HALL_FOCUS_HEIGHT: f32 = 80.0;
pub const WILLIAM_PENN_BASE: f32 = 155.8;
pub const WILLIAM_PENN_HEIGHT: f32 = 11.4;

pub fn city_hall_focus() -> [f32; 2] {
    let projected = isometric(
        CITY_HALL_SOURCE.0,
        CITY_HALL_SOURCE.1,
        CITY_HALL_FOCUS_HEIGHT,
    );
    [projected.0, projected.1]
}
