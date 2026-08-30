use std::io;

use image::RgbImage;
use tiny_skia::Pixmap;

use crate::{
    mesh_texture::MeshTextureSource,
    projection::Projection,
    world::{MeshFace, TexturedFace},
};

const TILE_SIZE: usize = 256;
const ROOF_NORMAL: f32 = 0.55;
const MIN_TRIANGLE_AREA: f32 = 0.35;

pub fn draw_textured_faces<'a>(
    pixmap: &mut Pixmap,
    faces: impl IntoIterator<Item = &'a TexturedFace>,
    projection: &Projection,
    textures: &MeshTextureSource,
    depth: &mut [f32],
) -> io::Result<()> {
    let mut rasterizer = Rasterizer {
        pixmap,
        projection,
        depth,
    };
    let mut faces: Vec<_> = faces.into_iter().collect();
    faces.sort_unstable_by_key(|face| face.texture_id);
    let mut texture_id = None;
    let mut texture = None;
    for face in faces {
        if texture_id != Some(face.texture_id) {
            texture = Some(textures.load(face.texture_id)?);
            texture_id = Some(face.texture_id);
        }
        if let Some(texture) = &texture {
            rasterizer.draw_face(&face.face, texture);
        }
    }
    Ok(())
}

struct Rasterizer<'a, 'b> {
    pixmap: &'a mut Pixmap,
    projection: &'b Projection,
    depth: &'a mut [f32],
}

impl Rasterizer<'_, '_> {
    fn draw_face(&mut self, face: &MeshFace, texture: &RgbImage) {
        let Some(normal) = normal(face) else {
            return;
        };
        let light = if normal.2.abs() >= ROOF_NORMAL {
            1.0
        } else {
            let directional = (normal.0 - normal.1).abs();
            0.9 + 0.08 * directional.clamp(0.0, 1.0)
        };
        self.draw_triangle(face.points, face.uvs, texture, light);
    }

    fn draw_triangle(
        &mut self,
        triangle: [(f32, f32, f32); 3],
        uvs: [(f32, f32); 3],
        texture: &RgbImage,
        light: f32,
    ) {
        let projected: [Vertex; 3] = std::array::from_fn(|index| {
            let (x, y, z) = triangle[index];
            let screen = self.projection.point((x, y), z);
            Vertex {
                x: screen.0,
                y: screen.1,
                depth: self.projection.view.depth(x, y, z),
                u: uvs[index].0,
                v: uvs[index].1,
            }
        });
        let area = edge(projected[0], projected[1], projected[2]);
        if area.abs() < MIN_TRIANGLE_AREA {
            return;
        }
        let min_x = projected
            .iter()
            .map(|vertex| vertex.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_x = projected
            .iter()
            .map(|vertex| vertex.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((TILE_SIZE - 1) as f32) as usize;
        let min_y = projected
            .iter()
            .map(|vertex| vertex.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_y = projected
            .iter()
            .map(|vertex| vertex.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((TILE_SIZE - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            return;
        }
        let inverse_area = 1.0 / area;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample = Vertex {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    depth: 0.0,
                    u: 0.0,
                    v: 0.0,
                };
                let weights = [
                    edge(projected[1], projected[2], sample) * inverse_area,
                    edge(projected[2], projected[0], sample) * inverse_area,
                    edge(projected[0], projected[1], sample) * inverse_area,
                ];
                if weights.iter().any(|weight| *weight < -f32::EPSILON) {
                    continue;
                }
                let depth = weights[0].mul_add(
                    projected[0].depth,
                    weights[1].mul_add(projected[1].depth, weights[2] * projected[2].depth),
                );
                let offset = y * TILE_SIZE + x;
                if depth <= self.depth[offset] {
                    continue;
                }
                self.depth[offset] = depth;
                let u = weights[0].mul_add(
                    projected[0].u,
                    weights[1].mul_add(projected[1].u, weights[2] * projected[2].u),
                );
                let v = weights[0].mul_add(
                    projected[0].v,
                    weights[1].mul_add(projected[1].v, weights[2] * projected[2].v),
                );
                let rgba = sample_texture(texture, u, v, light);
                let byte_offset = offset * 4;
                self.pixmap.data_mut()[byte_offset..byte_offset + 4].copy_from_slice(&rgba);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Vertex {
    x: f32,
    y: f32,
    depth: f32,
    u: f32,
    v: f32,
}

fn edge(left: Vertex, right: Vertex, point: Vertex) -> f32 {
    (point.x - left.x).mul_add(right.y - left.y, -((point.y - left.y) * (right.x - left.x)))
}

fn normal(face: &MeshFace) -> Option<(f32, f32, f32)> {
    let left = face.points[0];
    let middle = face.points[1];
    let right = face.points[2];
    let first = (middle.0 - left.0, middle.1 - left.1, middle.2 - left.2);
    let second = (right.0 - left.0, right.1 - left.1, right.2 - left.2);
    let result = (
        first.1 * second.2 - first.2 * second.1,
        first.2 * second.0 - first.0 * second.2,
        first.0 * second.1 - first.1 * second.0,
    );
    let length = result.0.hypot(result.1).hypot(result.2);
    (length > f32::EPSILON).then(|| (result.0 / length, result.1 / length, result.2 / length))
}

fn sample_texture(texture: &RgbImage, u: f32, v: f32, light: f32) -> [u8; 4] {
    let x = (u.clamp(0.0, 1.0) * texture.width().saturating_sub(1) as f32).round() as u32;
    let y = (v.clamp(0.0, 1.0) * texture.height().saturating_sub(1) as f32).round() as u32;
    let color = texture.get_pixel(x, y).0;
    [
        posterize(color[0], light),
        posterize(color[1], light),
        posterize(color[2], light),
        255,
    ]
}

fn posterize(channel: u8, light: f32) -> u8 {
    let lit = (f32::from(channel) * light).round().clamp(0.0, 255.0) as u16;
    ((lit + 8) / 16 * 16).min(255) as u8
}

#[cfg(test)]
mod tests {
    use image::{Rgb, RgbImage};

    use super::sample_texture;

    #[test]
    fn texture_v_axis_matches_i3s_jpeg_rows() {
        let mut image = RgbImage::new(1, 2);
        image.put_pixel(0, 0, Rgb([32, 48, 64]));
        image.put_pixel(0, 1, Rgb([160, 176, 192]));

        assert_eq!(sample_texture(&image, 0.0, 0.0, 1.0), [32, 48, 64, 255]);
        assert_eq!(sample_texture(&image, 0.0, 1.0, 1.0), [160, 176, 192, 255]);
    }
}
