//! The small, shared color contract used by every rendered tile layer.
//!
//! The source imagery is intentionally left continuous: it carries most of the
//! useful local detail.  This module only supplies stable anchors for the
//! procedural layers and the light, display-ready finish that previously lived
//! in a browser-only canvas filter.  Applying that finish while rendering keeps
//! PNG tiles, derived parents, screenshots, and the live canvas in agreement.

pub type Rgb = [u8; 3];

pub const GROUND: Rgb = [217, 209, 195];
pub const DISPLAY_GROUND: Rgb = [222, 212, 196];
pub const WATER: Rgb = [42, 132, 172];
pub const WATER_HIGHLIGHT: Rgb = [126, 196, 210];
pub const WATER_SHADOW: Rgb = [27, 101, 147];
pub const PARK: Rgb = [67, 151, 65];
pub const CANOPY: Rgb = [56, 126, 61];
pub const GRASS: Rgb = [86, 148, 73];
pub const AERIAL_VEGETATION: Rgb = [72, 142, 68];
pub const TREE_TRUNK: Rgb = [76, 61, 43];

pub const ROWHOUSE_FAMILIES: [[Rgb; 4]; 4] = [
    [[143, 78, 58], [156, 88, 65], [169, 101, 72], [132, 73, 59]],
    [[139, 92, 68], [153, 101, 75], [164, 113, 84], [128, 87, 72]],
    [
        [178, 151, 113],
        [188, 164, 128],
        [163, 140, 109],
        [194, 174, 143],
    ],
    [
        [137, 137, 132],
        [153, 148, 139],
        [167, 158, 145],
        [126, 130, 128],
    ],
];
pub const LOW_RISE_FACADES: [Rgb; 4] = [
    [151, 96, 72],
    [167, 112, 82],
    [143, 145, 139],
    [188, 169, 141],
];
pub const INDUSTRIAL_FACADES: [Rgb; 4] = [
    [169, 161, 145],
    [151, 153, 148],
    [181, 171, 150],
    [139, 147, 148],
];
pub const HIGH_RISE_FACADES: [Rgb; 4] = [
    [151, 137, 121],
    [132, 145, 150],
    [174, 164, 145],
    [146, 142, 136],
];

pub fn mix(left: Rgb, right: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    std::array::from_fn(|index| {
        (f32::from(left[index]) * (1.0 - amount) + f32::from(right[index]) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

pub fn scale(color: Rgb, amount: f32) -> Rgb {
    color.map(|channel| (f32::from(channel) * amount).round().clamp(0.0, 255.0) as u8)
}

pub fn soften(color: Rgb) -> Rgb {
    let luminance = (u16::from(color[0]) * 3 + u16::from(color[1]) * 6 + u16::from(color[2])) / 10;
    std::array::from_fn(|index| {
        let mixed = (u16::from(color[index]) * 3 + luminance * 2) / 5;
        mixed.clamp(56, 208) as u8
    })
}

pub fn tree_foliage(variant: u64) -> Rgb {
    match variant % 4 {
        0 => [48, 99, 49],
        1 => [55, 108, 52],
        2 => [61, 113, 55],
        _ => [44, 93, 47],
    }
}

/// Bake the former `saturate(1.14) contrast(1.04)` canvas treatment into a
/// pixel.  It deliberately has no quantization, so photographed mesh texture
/// detail remains intact.  The order matches the former CSS filter list.
pub fn display_finish(color: Rgb) -> Rgb {
    let channels = color.map(f32::from);
    let luminance = channels[0] * 0.213 + channels[1] * 0.715 + channels[2] * 0.072;
    let saturated = channels.map(|channel| luminance + (channel - luminance) * 1.14);
    saturated.map(|channel| ((channel - 127.5) * 1.04 + 127.5).round().clamp(0.0, 255.0) as u8)
}

pub fn bake_display_finish(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let styled = display_finish([pixel[0], pixel[1], pixel[2]]);
        pixel[..3].copy_from_slice(&styled);
    }
}

#[cfg(test)]
mod tests {
    use super::{DISPLAY_GROUND, GROUND, bake_display_finish, display_finish, mix};

    #[test]
    fn display_finish_matches_the_baked_blank_ground() {
        assert_eq!(display_finish(GROUND), DISPLAY_GROUND);
    }

    #[test]
    fn display_finish_is_bounded_and_keeps_mesh_texture_detail_continuous() {
        let input = [[0, 0, 0], [120, 121, 122], [121, 122, 123], [255, 255, 255]];
        let output: Vec<_> = input.into_iter().map(display_finish).collect();
        assert_eq!(output.first(), Some(&[0, 0, 0]));
        assert_eq!(output.last(), Some(&[255, 255, 255]));
        assert_ne!(output[1], output[2]);
    }

    #[test]
    fn display_finish_keeps_alpha_and_is_deterministic() {
        let mut pixels = [120, 121, 122, 37, 121, 122, 123, 255];
        bake_display_finish(&mut pixels);
        assert_eq!(pixels[3], 37);
        assert_eq!(pixels[7], 255);
        assert_ne!(&pixels[..3], &pixels[4..7]);
        assert_eq!(mix([10, 20, 30], [30, 40, 50], 0.5), [20, 30, 40]);
    }
}
