use std::io;

use image::{ExtendedColorType, ImageEncoder, RgbaImage, codecs::webp::WebPEncoder};

pub const EXTENSION: &str = "webp";
pub const MEDIA_TYPE: &str = "image/webp";

pub fn encode_rgba(bytes: &[u8], width: u32, height: u32) -> io::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    WebPEncoder::new_lossless(&mut encoded)
        .write_image(bytes, width, height, ExtendedColorType::Rgba8)
        .map_err(io::Error::other)?;
    Ok(encoded)
}

pub fn encode_image(image: &RgbaImage) -> io::Result<Vec<u8>> {
    encode_rgba(image.as_raw(), image.width(), image.height())
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, Rgba, RgbaImage};

    use super::encode_image;

    #[test]
    fn lossless_tile_round_trips_exact_pixels() -> Result<(), Box<dyn std::error::Error>> {
        let source = RgbaImage::from_fn(16, 16, |x, y| {
            Rgba([(x * 16) as u8, (y * 16) as u8, ((x + y) * 8) as u8, 255])
        });
        let encoded = encode_image(&source)?;
        let decoded =
            image::load_from_memory_with_format(&encoded, ImageFormat::WebP)?.into_rgba8();

        assert_eq!(decoded, source);
        Ok(())
    }
}
