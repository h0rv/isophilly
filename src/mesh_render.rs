use tiny_skia::{Color, Pixmap};

use crate::{
    building_render::{RenderContext, building_color, projected_point},
    render::{missing_imagery, mix_color, shade},
    texture::AerialTile,
    world::{BuildingMesh, MeshFace, view_depth},
};

const TILE_SIZE: usize = 256;
const ROOF_NORMAL: f32 = 0.55;
const MIN_TRIANGLE_AREA: f32 = 0.35;

pub fn draw_building_meshes<'a>(
    pixmap: &mut Pixmap,
    meshes: impl IntoIterator<Item = &'a BuildingMesh>,
    context: &RenderContext<'_>,
) {
    let mut rasterizer = Rasterizer {
        pixmap,
        context,
        depth: vec![f32::NEG_INFINITY; TILE_SIZE * TILE_SIZE],
    };
    for mesh in meshes {
        rasterizer.draw_mesh(mesh);
    }
}

struct Rasterizer<'a, 'b> {
    pixmap: &'a mut Pixmap,
    context: &'b RenderContext<'b>,
    depth: Vec<f32>,
}

impl Rasterizer<'_, '_> {
    fn draw_mesh(&mut self, mesh: &BuildingMesh) {
        let fallback = building_color(
            &mesh.footprint,
            mesh.center,
            mesh.height,
            None,
            self.context.texture,
            self.context.block_size,
        );
        let base = mesh.facade_color.map_or(fallback, |color| {
            mix_color(
                fallback,
                Color::from_rgba8(color[0], color[1], color[2], 255),
                0.72,
            )
        });
        for face in &mesh.faces {
            self.draw_face(face, base, self.context.aerial);
        }
    }

    fn draw_face(&mut self, face: &MeshFace, base: Color, aerial: Option<&AerialTile>) {
        let Some(normal) = normal(face) else {
            return;
        };
        let center = center(face);
        let color = face_color(normal, center, base, aerial, self.context);
        let Some(&first) = face.points.first() else {
            return;
        };
        for edge in face.points[1..].windows(2) {
            self.draw_triangle([first, edge[0], edge[1]], color);
        }
    }

    fn draw_triangle(&mut self, triangle: [(f32, f32, f32); 3], color: Color) {
        let projected = triangle.map(|(x, y, z)| {
            let screen = projected_point((x, y), z, self.context);
            Vertex {
                x: screen.0,
                y: screen.1,
                depth: view_depth(x, y, z),
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
        let rgba = color_bytes(color);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample = Vertex {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    depth: 0.0,
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
}

fn edge(left: Vertex, right: Vertex, point: Vertex) -> f32 {
    (point.x - left.x).mul_add(right.y - left.y, -((point.y - left.y) * (right.x - left.x)))
}

fn face_color(
    normal: (f32, f32, f32),
    center: (f32, f32, f32),
    base: Color,
    aerial: Option<&AerialTile>,
    context: &RenderContext<'_>,
) -> Color {
    let roof = normal.2.abs() >= ROOF_NORMAL;
    let color = if roof {
        aerial
            .filter(|aerial| aerial.contains(center.0, center.1))
            .map(|aerial| aerial.sample(center.0, center.1, context.texture, context.block_size))
            .filter(|sample| !missing_imagery(*sample))
            .map_or(base, |sample| {
                let photo = Color::from_rgba8(sample[0], sample[1], sample[2], 255);
                mix_color(base, photo, 0.9)
            })
    } else {
        base
    };
    let light = if roof {
        1.04
    } else {
        let directional = (normal.0 - normal.1).abs();
        0.68 + 0.2 * directional.clamp(0.0, 1.0)
    };
    shade(color, light)
}

fn center(face: &MeshFace) -> (f32, f32, f32) {
    let count = face.points.len() as f32;
    let sum = face.points.iter().fold((0.0, 0.0, 0.0), |sum, point| {
        (sum.0 + point.0, sum.1 + point.1, sum.2 + point.2)
    });
    (sum.0 / count, sum.1 / count, sum.2 / count)
}

fn normal(face: &MeshFace) -> Option<(f32, f32, f32)> {
    let mut result = (0.0, 0.0, 0.0);
    for (left, right) in face
        .points
        .iter()
        .zip(face.points.iter().cycle().skip(1))
        .take(face.points.len())
    {
        result.0 += (left.1 - right.1) * (left.2 + right.2);
        result.1 += (left.2 - right.2) * (left.0 + right.0);
        result.2 += (left.0 - right.0) * (left.1 + right.1);
    }
    let length = result.0.hypot(result.1).hypot(result.2);
    (length > f32::EPSILON).then(|| (result.0 / length, result.1 / length, result.2 / length))
}

fn color_bytes(color: Color) -> [u8; 4] {
    [
        (color.red() * 255.0).round() as u8,
        (color.green() * 255.0).round() as u8,
        (color.blue() * 255.0).round() as u8,
        255,
    ]
}
