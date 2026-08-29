use std::{fs, io, path::Path};

use rstar::{AABB, RTree, RTreeObject};

const MAGIC: &[u8; 8] = b"GEOPHILY";
const STREET_MAGIC: &[u8; 8] = b"GEOSTRPH";
const EPSG: u32 = 32129;
const MESH_FACE_BYTES: usize = 3 * 5 * size_of::<f32>();

#[derive(Clone, Copy)]
pub struct Bounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Bounds {
    pub fn width(self) -> f32 {
        self.max_x - self.min_x
    }
    pub fn height(self) -> f32 {
        self.max_y - self.min_y
    }
    pub fn isometric(self, max_height: f32) -> Self {
        let corners = [
            isometric(self.min_x, self.min_y, 0.0),
            isometric(self.min_x, self.max_y, 0.0),
            isometric(self.max_x, self.min_y, 0.0),
            isometric(self.max_x, self.max_y, 0.0),
        ];
        Self {
            min_x: corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
            min_y: corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min) - max_height,
            max_x: corners
                .iter()
                .map(|p| p.0)
                .fold(f32::NEG_INFINITY, f32::max),
            max_y: corners
                .iter()
                .map(|p| p.1)
                .fold(f32::NEG_INFINITY, f32::max),
        }
    }
    pub fn tile(self, z: u8, x: u32, y: u32) -> Self {
        let side = self.width().max(self.height()) / (1_u32 << z) as f32;
        Self {
            min_x: self.min_x + x as f32 * side,
            min_y: self.min_y + y as f32 * side,
            max_x: self.min_x + (x + 1) as f32 * side,
            max_y: self.min_y + (y + 1) as f32 * side,
        }
    }
    pub fn pad(self, amount: f32) -> Self {
        Self {
            min_x: self.min_x - amount,
            min_y: self.min_y - amount,
            max_x: self.max_x + amount,
            max_y: self.max_y + amount,
        }
    }
    pub fn ground_source_bounds(self) -> Self {
        let corners = [
            inverse_isometric(self.min_x, self.min_y),
            inverse_isometric(self.max_x, self.min_y),
            inverse_isometric(self.max_x, self.max_y),
            inverse_isometric(self.min_x, self.max_y),
        ];
        Self {
            min_x: corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
            min_y: corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
            max_x: corners
                .iter()
                .map(|p| p.0)
                .fold(f32::NEG_INFINITY, f32::max),
            max_y: corners
                .iter()
                .map(|p| p.1)
                .fold(f32::NEG_INFINITY, f32::max),
        }
    }
    pub fn source_envelope(self, max_height: f32) -> AABB<[f32; 2]> {
        let ground = self.ground_source_bounds();
        let height_margin = max_height * 1.2;
        AABB::from_corners(
            [ground.min_x - height_margin, ground.min_y - height_margin],
            [ground.max_x + height_margin, ground.max_y + height_margin],
        )
    }
}

#[derive(Clone)]
pub struct Ring {
    pub bounds: Bounds,
    pub points: Vec<(f32, f32)>,
}

impl Ring {
    pub fn center(&self) -> (f32, f32) {
        (
            (self.bounds.min_x + self.bounds.max_x) * 0.5,
            (self.bounds.min_y + self.bounds.max_y) * 0.5,
        )
    }

    pub fn contains(&self, point: (f32, f32)) -> bool {
        let mut inside = false;
        for index in 0..self.points.len() {
            let left = self.points[index];
            let right = self.points[(index + 1) % self.points.len()];
            let crosses = (left.1 > point.1) != (right.1 > point.1);
            if crosses {
                let x = (right.0 - left.0).mul_add((point.1 - left.1) / (right.1 - left.1), left.0);
                if point.0 < x {
                    inside = !inside;
                }
            }
        }
        inside
    }
}
#[derive(Clone)]
pub struct Building {
    pub height: f32,
    pub ring: Ring,
}
#[derive(Clone)]
pub struct BuildingPart {
    pub height: f32,
    pub ring: Ring,
}
#[derive(Clone)]
pub struct MeshFace {
    pub points: [(f32, f32, f32); 3],
    pub uvs: [(f32, f32); 3],
}
#[derive(Clone)]
pub struct BuildingMesh {
    pub texture_id: u32,
    pub height: f32,
    pub footprint: Ring,
    pub faces: Vec<MeshFace>,
    pub center: (f32, f32),
}
#[derive(Clone)]
pub struct TexturedFace {
    pub texture_id: u32,
    pub face: MeshFace,
}
#[derive(Clone)]
pub struct Street {
    pub bounds: Bounds,
}
#[derive(Clone)]
pub struct Indexed {
    pub index: usize,
    envelope: AABB<[f32; 2]>,
}
impl RTreeObject for Indexed {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

pub struct World {
    pub buildings: Vec<Building>,
    pub building_parts: Vec<BuildingPart>,
    pub building_meshes: Vec<BuildingMesh>,
    pub mesh_faces: Vec<TexturedFace>,
    pub texture_ids: Vec<u32>,
    pub texture_sha256: [u8; 32],
    pub water: Vec<Ring>,
    pub parks: Vec<Ring>,
    pub streets: Vec<Street>,
    pub building_tree: RTree<Indexed>,
    pub building_part_tree: RTree<Indexed>,
    pub building_mesh_tree: RTree<Indexed>,
    pub mesh_face_tree: RTree<Indexed>,
    pub water_tree: RTree<Indexed>,
    pub park_tree: RTree<Indexed>,
    pub street_tree: RTree<Indexed>,
    pub iso_bounds: Bounds,
    pub max_height: f32,
    pub data_version: u64,
}

impl World {
    pub fn source_envelope(&self, bounds: Bounds) -> AABB<[f32; 2]> {
        bounds.source_envelope(self.max_height)
    }

    pub fn max_aerial_height(&self, bounds: Bounds) -> f32 {
        self.building_tree
            .locate_in_envelope_intersecting(&self.source_envelope(bounds))
            .map(|item| self.buildings[item.index].height)
            .fold(0.0, f32::max)
    }

    pub fn has_content(&self, query: &AABB<[f32; 2]>) -> bool {
        self.building_tree
            .locate_in_envelope_intersecting(query)
            .next()
            .is_some()
            || self
                .building_part_tree
                .locate_in_envelope_intersecting(query)
                .next()
                .is_some()
            || self
                .building_mesh_tree
                .locate_in_envelope_intersecting(query)
                .next()
                .is_some()
            || self
                .water_tree
                .locate_in_envelope_intersecting(query)
                .next()
                .is_some()
            || self
                .park_tree
                .locate_in_envelope_intersecting(query)
                .next()
                .is_some()
            || self
                .street_tree
                .locate_in_envelope_intersecting(query)
                .next()
                .is_some()
    }

    pub fn city_hall_focus(&self) -> Option<[f32; 2]> {
        const CITY_HALL: (f32, f32) = (820_994.25, 71_994.46);
        self.building_meshes
            .iter()
            .min_by(|left, right| {
                squared_distance(left.center, CITY_HALL)
                    .total_cmp(&squared_distance(right.center, CITY_HALL))
            })
            .and_then(|mesh| {
                mesh.faces
                    .iter()
                    .flat_map(|face| &face.points)
                    .max_by(|left, right| left.2.total_cmp(&right.2))
            })
            .map(|&(x, y, z)| {
                let point = isometric(x, y, z);
                [point.0, point.1]
            })
    }
}

pub fn load_world(path: &Path) -> io::Result<World> {
    let bytes = fs::read(path)?;
    let streets_path = path.with_file_name("streets.bin");
    let street_bytes = match fs::read(&streets_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let data_version = street_bytes.as_deref().map_or_else(
        || fingerprint(&bytes),
        |streets| fingerprint_pair(&bytes, streets),
    );
    let mut cursor = Cursor {
        bytes: &bytes,
        offset: 0,
    };
    if cursor.take(8)? != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not geo-philly data",
        ));
    }
    if cursor.u32()? != 5 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported data version",
        ));
    }
    if cursor.u32()? != EPSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "world data uses an unsupported coordinate system",
        ));
    }
    let building_count = cursor.u32()? as usize;
    let building_part_count = cursor.u32()? as usize;
    let building_mesh_count = cursor.u32()? as usize;
    let water_count = cursor.u32()? as usize;
    let park_count = cursor.u32()? as usize;
    let texture_sha256 = cursor
        .take(32)?
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid texture digest"))?;
    let bounds = Bounds {
        min_x: cursor.f64()? as f32,
        min_y: cursor.f64()? as f32,
        max_x: cursor.f64()? as f32,
        max_y: cursor.f64()? as f32,
    };
    let mut buildings = Vec::with_capacity(building_count);
    for _ in 0..building_count {
        let height = cursor.f32()?;
        let ring = cursor.ring()?;
        buildings.push(Building { height, ring });
    }
    let mut building_parts = Vec::with_capacity(building_part_count);
    for _ in 0..building_part_count {
        let _osm_id = cursor.u64()?;
        let height = cursor.f32()?;
        let _min_height = cursor.f32()?;
        let _roof_height = cursor.f32()?;
        if cursor.u8()? > 6 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported roof shape",
            ));
        }
        let _facade_color = cursor.optional_rgb()?;
        let ring = cursor.ring()?;
        building_parts.push(BuildingPart { height, ring });
    }
    let mut building_meshes = Vec::with_capacity(building_mesh_count);
    for _ in 0..building_mesh_count {
        let _source_id = cursor.u32()?;
        let texture_id = cursor.u32()?;
        let height = cursor.f32()?;
        if !(0.0..=400.0).contains(&height) || height == 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mesh height is outside the supported range",
            ));
        }
        let face_count = cursor.u32()? as usize;
        cursor.ensure_items(face_count, MESH_FACE_BYTES)?;
        let footprint = cursor.ring()?;
        let center = (
            (footprint.bounds.min_x + footprint.bounds.max_x) * 0.5,
            (footprint.bounds.min_y + footprint.bounds.max_y) * 0.5,
        );
        let mut faces = Vec::with_capacity(face_count);
        for _ in 0..face_count {
            cursor.ensure_items(3, 20)?;
            let mut points = [(0.0, 0.0, 0.0); 3];
            let mut uvs = [(0.0, 0.0); 3];
            for index in 0..3 {
                points[index] = cursor.point3()?;
                uvs[index] = (cursor.f32()?, cursor.f32()?);
            }
            faces.push(MeshFace { points, uvs });
        }
        building_meshes.push(BuildingMesh {
            texture_id,
            height,
            footprint,
            faces,
            center,
        });
    }
    let water = (0..water_count)
        .map(|_| cursor.ring())
        .collect::<io::Result<Vec<_>>>()?;
    let parks = (0..park_count)
        .map(|_| cursor.ring())
        .collect::<io::Result<Vec<_>>>()?;
    if cursor.remaining() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "world data contains trailing bytes",
        ));
    }
    let streets = street_bytes
        .as_deref()
        .map(parse_streets)
        .transpose()?
        .unwrap_or_default();
    let max_height = buildings
        .iter()
        .map(|building| building.height)
        .chain(building_parts.iter().map(|part| part.height))
        .chain(building_meshes.iter().map(|mesh| mesh.height))
        .fold(0.0, f32::max);
    let building_tree = index_buildings(&buildings);
    let mesh_faces = textured_faces(&building_meshes);
    let mut texture_ids: Vec<_> = building_meshes.iter().map(|mesh| mesh.texture_id).collect();
    texture_ids.sort_unstable();
    texture_ids.dedup();
    Ok(World {
        building_tree,
        building_part_tree: index_building_parts(&building_parts),
        building_mesh_tree: index_building_meshes(&building_meshes),
        mesh_face_tree: index_mesh_faces(&mesh_faces),
        water_tree: index_rings(&water),
        park_tree: index_rings(&parks),
        street_tree: index_streets(&streets),
        buildings,
        building_parts,
        building_meshes,
        mesh_faces,
        texture_ids,
        texture_sha256,
        water,
        parks,
        streets,
        iso_bounds: bounds.isometric(max_height),
        max_height,
        data_version,
    })
}

fn squared_distance(left: (f32, f32), right: (f32, f32)) -> f32 {
    (left.0 - right.0).powi(2) + (left.1 - right.1).powi(2)
}

fn fingerprint(bytes: &[u8]) -> u64 {
    fingerprint_from(0xcbf2_9ce4_8422_2325, bytes)
}
fn fingerprint_pair(first: &[u8], second: &[u8]) -> u64 {
    fingerprint_from(fingerprint(first), second)
}
fn fingerprint_from(initial: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(initial, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn parse_streets(bytes: &[u8]) -> io::Result<Vec<Street>> {
    let mut cursor = Cursor { bytes, offset: 0 };
    if cursor.take(8)? != STREET_MAGIC || cursor.u32()? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported street data",
        ));
    }
    if cursor.u32()? != EPSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "street data uses an unsupported coordinate system",
        ));
    }
    let count = cursor.u32()? as usize;
    for _ in 0..4 {
        let _bound = cursor.f64()?;
    }
    let streets = (0..count)
        .map(|_| {
            let class = cursor.u8()?;
            if !matches!(class, 1..=5 | 9 | 10) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "street data contains an unsupported class",
                ));
            }
            let point_count = cursor.u32()? as usize;
            let mut points = Vec::with_capacity(point_count);
            let mut bounds = Bounds {
                min_x: f32::INFINITY,
                min_y: f32::INFINITY,
                max_x: f32::NEG_INFINITY,
                max_y: f32::NEG_INFINITY,
            };
            for _ in 0..point_count {
                let x = cursor.f32()?;
                let y = cursor.f32()?;
                bounds.min_x = bounds.min_x.min(x);
                bounds.min_y = bounds.min_y.min(y);
                bounds.max_x = bounds.max_x.max(x);
                bounds.max_y = bounds.max_y.max(y);
                points.push((x, y));
            }
            if points.len() < 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "street needs at least two points",
                ));
            }
            Ok(Street { bounds })
        })
        .collect::<io::Result<Vec<_>>>()?;
    if cursor.remaining() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "street data contains trailing bytes",
        ));
    }
    Ok(streets)
}

fn index_buildings(buildings: &[Building]) -> RTree<Indexed> {
    RTree::bulk_load(
        buildings
            .iter()
            .enumerate()
            .map(|(index, building)| indexed(index, building.ring.bounds))
            .collect(),
    )
}
fn index_building_parts(parts: &[BuildingPart]) -> RTree<Indexed> {
    RTree::bulk_load(
        parts
            .iter()
            .enumerate()
            .map(|(index, part)| indexed(index, part.ring.bounds))
            .collect(),
    )
}
fn index_building_meshes(meshes: &[BuildingMesh]) -> RTree<Indexed> {
    RTree::bulk_load(
        meshes
            .iter()
            .enumerate()
            .map(|(index, mesh)| indexed(index, mesh.footprint.bounds))
            .collect(),
    )
}
fn textured_faces(meshes: &[BuildingMesh]) -> Vec<TexturedFace> {
    meshes
        .iter()
        .flat_map(|mesh| {
            mesh.faces.iter().cloned().map(|face| TexturedFace {
                texture_id: mesh.texture_id,
                face,
            })
        })
        .collect()
}
fn index_mesh_faces(faces: &[TexturedFace]) -> RTree<Indexed> {
    RTree::bulk_load(
        faces
            .iter()
            .enumerate()
            .map(|(index, textured)| {
                let mut bounds = Bounds {
                    min_x: f32::INFINITY,
                    min_y: f32::INFINITY,
                    max_x: f32::NEG_INFINITY,
                    max_y: f32::NEG_INFINITY,
                };
                for &(x, y, z) in &textured.face.points {
                    let point = isometric(x, y, z);
                    bounds.min_x = bounds.min_x.min(point.0);
                    bounds.min_y = bounds.min_y.min(point.1);
                    bounds.max_x = bounds.max_x.max(point.0);
                    bounds.max_y = bounds.max_y.max(point.1);
                }
                indexed(index, bounds)
            })
            .collect(),
    )
}
fn index_rings(rings: &[Ring]) -> RTree<Indexed> {
    RTree::bulk_load(
        rings
            .iter()
            .enumerate()
            .map(|(index, ring)| indexed(index, ring.bounds))
            .collect(),
    )
}
fn index_streets(streets: &[Street]) -> RTree<Indexed> {
    RTree::bulk_load(
        streets
            .iter()
            .enumerate()
            .map(|(index, street)| indexed(index, street.bounds))
            .collect(),
    )
}
fn indexed(index: usize, bounds: Bounds) -> Indexed {
    Indexed {
        index,
        envelope: AABB::from_corners([bounds.min_x, bounds.min_y], [bounds.max_x, bounds.max_y]),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
    fn ensure_items(&self, count: usize, bytes_per_item: usize) -> io::Result<()> {
        let required = count
            .checked_mul(bytes_per_item)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "item count overflow"))?;
        if required > self.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "item count exceeds the remaining data",
            ));
        }
        Ok(())
    }
    fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "overflow"))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated data"))?;
        self.offset = end;
        Ok(slice)
    }
    fn u32(&mut self) -> io::Result<u32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid u32"))?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn u64(&mut self) -> io::Result<u64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid u64"))?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn u8(&mut self) -> io::Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated u8"))
    }
    fn optional_rgb(&mut self) -> io::Result<Option<[u8; 3]>> {
        let bytes = self.take(4)?;
        match bytes[3] {
            0 if bytes[..3] == [0, 0, 0] => Ok(None),
            255 => Ok(Some([bytes[0], bytes[1], bytes[2]])),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid optional RGB value",
            )),
        }
    }
    fn f32(&mut self) -> io::Result<f32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid f32"))?;
        let value = f32::from_le_bytes(bytes);
        if !value.is_finite() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "non-finite f32"));
        }
        Ok(value)
    }
    fn point3(&mut self) -> io::Result<(f32, f32, f32)> {
        Ok((self.f32()?, self.f32()?, self.f32()?))
    }
    fn f64(&mut self) -> io::Result<f64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid f64"))?;
        let value = f64::from_le_bytes(bytes);
        if !value.is_finite() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "non-finite f64"));
        }
        Ok(value)
    }
    fn ring(&mut self) -> io::Result<Ring> {
        let count = self.u32()? as usize;
        if count < 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ring needs at least three points",
            ));
        }
        self.ensure_items(count, 8)?;
        let mut bounds = Bounds {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            let x = self.f32()?;
            let y = self.f32()?;
            bounds.min_x = bounds.min_x.min(x);
            bounds.min_y = bounds.min_y.min(y);
            bounds.max_x = bounds.max_x.max(x);
            bounds.max_y = bounds.max_y.max(y);
            points.push((x, y));
        }
        Ok(Ring { bounds, points })
    }
}

// Broad Street runs 7.79 degrees east of grid north in EPSG:32129. Treat it as
// Philadelphia's local north axis, then use the classic two-axis isometric view:
// north runs up-right, south runs down-left, and both main facade directions are
// visible instead of looking straight down the street grid.
const BROAD_NORTH_EAST: f32 = 0.135_556_46;
const BROAD_NORTH_NORTH: f32 = 0.990_769_6;

pub fn isometric(x: f32, y: f32, height: f32) -> (f32, f32) {
    let broad_east = BROAD_NORTH_NORTH.mul_add(x, -(BROAD_NORTH_EAST * y));
    let broad_north = BROAD_NORTH_EAST.mul_add(x, BROAD_NORTH_NORTH * y);
    (
        broad_east + broad_north,
        (broad_east - broad_north).mul_add(0.5, -height),
    )
}

pub fn inverse_isometric(x: f32, y: f32) -> (f32, f32) {
    let broad_east = (x + 2.0 * y) * 0.5;
    let broad_north = (x - 2.0 * y) * 0.5;
    (
        BROAD_NORTH_NORTH.mul_add(broad_east, BROAD_NORTH_EAST * broad_north),
        (-BROAD_NORTH_EAST).mul_add(broad_east, BROAD_NORTH_NORTH * broad_north),
    )
}

pub fn view_depth(x: f32, y: f32, height: f32) -> f32 {
    let projected = isometric(x, y, 0.0);
    projected.1 + height
}

#[cfg(test)]
mod tests {
    use super::{
        BROAD_NORTH_EAST, BROAD_NORTH_NORTH, Bounds, Cursor, MESH_FACE_BYTES, fingerprint,
        fingerprint_pair, inverse_isometric, isometric,
    };

    #[test]
    fn isometric_bounds_cover_ground_and_height() {
        let source = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 10.0,
            max_y: 20.0,
        };
        let projected = source.isometric(5.0);

        for (x, y) in [(0.0, 0.0), (0.0, 20.0), (10.0, 0.0), (10.0, 20.0)] {
            let ground = isometric(x, y, 0.0);
            let roof = isometric(x, y, 5.0);
            assert!((projected.min_x..=projected.max_x).contains(&ground.0));
            assert!((projected.min_y..=projected.max_y).contains(&ground.1));
            assert!((projected.min_y..=projected.max_y).contains(&roof.1));
        }
    }

    #[test]
    fn tiles_form_a_square_grid() {
        let bounds = Bounds {
            min_x: -20.0,
            min_y: -5.0,
            max_x: 10.0,
            max_y: 15.0,
        };
        let tile = bounds.tile(1, 1, 0);

        assert_eq!(tile.min_x, -5.0);
        assert_eq!(tile.max_x, 10.0);
        assert_eq!(tile.min_y, -5.0);
        assert_eq!(tile.max_y, 10.0);
    }

    #[test]
    fn isometric_projection_round_trips_on_the_ground() {
        let projected = isometric(820_983.0, 71_996.0, 0.0);

        let source = inverse_isometric(projected.0, projected.1);
        assert!((source.0 - 820_983.0).abs() < 0.1);
        assert!((source.1 - 71_996.0).abs() < 0.1);
    }

    #[test]
    fn ring_contains_points_without_treating_its_bounding_box_as_geometry() {
        let ring = super::Ring {
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 4.0,
                max_y: 4.0,
            },
            points: vec![(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)],
        };

        assert!(ring.contains((1.0, 1.0)));
        assert!(!ring.contains((3.0, 3.0)));
    }

    #[test]
    fn local_north_uses_the_classic_isometric_angle() {
        let city_hall = (821_700.0, 75_000.0);
        let north = (
            city_hall.0 + BROAD_NORTH_EAST * 1_000.0,
            city_hall.1 + BROAD_NORTH_NORTH * 1_000.0,
        );
        let hall_screen = isometric(city_hall.0, city_hall.1, 0.0);
        let north_screen = isometric(north.0, north.1, 0.0);

        let delta_x = north_screen.0 - hall_screen.0;
        let delta_y = north_screen.1 - hall_screen.1;
        assert!(delta_x > 0.0);
        assert!(north_screen.1 < hall_screen.1);
        assert!((delta_y / delta_x + 0.5).abs() < 0.01);
    }

    #[test]
    fn known_landmarks_have_a_familiar_orientation() {
        let city_hall = isometric(820_983.06, 71_996.36, 0.0);
        let rittenhouse = isometric(820_283.8, 71_642.66, 0.0);
        let lincoln_financial_field = isometric(820_818.1, 66_237.54, 0.0);

        assert!(
            rittenhouse.0 < city_hall.0,
            "Rittenhouse is west of City Hall"
        );
        assert!(
            lincoln_financial_field.1 > city_hall.1,
            "the stadium complex is south of Center City"
        );
        assert!(
            lincoln_financial_field.0 < city_hall.0,
            "south points down-left in the isometric view"
        );
    }

    #[test]
    fn cursor_rejects_truncated_values() {
        let mut cursor = Cursor {
            bytes: &[1, 2, 3],
            offset: 0,
        };

        assert!(cursor.u32().is_err());
    }

    #[test]
    fn cursor_rejects_face_count_larger_than_remaining_data() {
        let cursor = Cursor {
            bytes: &[0; MESH_FACE_BYTES - 1],
            offset: 0,
        };

        assert!(cursor.ensure_items(1, MESH_FACE_BYTES).is_err());
        assert!(cursor.ensure_items(usize::MAX, MESH_FACE_BYTES).is_err());
    }

    #[test]
    fn fingerprint_changes_with_data() {
        assert_ne!(fingerprint(b"city-a"), fingerprint(b"city-b"));
        assert_eq!(fingerprint(b"city-a"), fingerprint(b"city-a"));
        assert_ne!(
            fingerprint_pair(b"city", b"roads-a"),
            fingerprint_pair(b"city", b"roads-b")
        );
    }
}
