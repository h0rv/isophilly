use std::{fs, io, path::Path};

use rstar::{AABB, RTree, RTreeObject};

const MAGIC: &[u8; 8] = b"GEOPHILY";
const STREET_MAGIC: &[u8; 8] = b"GEOSTRPH";

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
    pub fn source_envelope(self) -> AABB<[f32; 2]> {
        let corners = [
            (
                (self.min_x + 2.0 * self.min_y) * 0.5,
                (2.0 * self.min_y - self.min_x) * 0.5,
            ),
            (
                (self.max_x + 2.0 * self.min_y) * 0.5,
                (2.0 * self.min_y - self.max_x) * 0.5,
            ),
            (
                (self.max_x + 2.0 * self.max_y) * 0.5,
                (2.0 * self.max_y - self.max_x) * 0.5,
            ),
            (
                (self.min_x + 2.0 * self.max_y) * 0.5,
                (2.0 * self.max_y - self.min_x) * 0.5,
            ),
        ];
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for (x, y) in corners {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        AABB::from_corners(
            [min_x - 350.0, min_y - 350.0],
            [max_x + 350.0, max_y + 350.0],
        )
    }
}

#[derive(Clone)]
pub struct Ring {
    pub points: Vec<(f32, f32)>,
    pub bounds: Bounds,
}
#[derive(Clone)]
pub struct Building {
    pub height: f32,
    pub ring: Ring,
    pub center: (f32, f32),
}
#[derive(Clone)]
pub struct Street {
    pub class: u8,
    pub points: Vec<(f32, f32)>,
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
    pub water: Vec<Ring>,
    pub parks: Vec<Ring>,
    pub streets: Vec<Street>,
    pub building_tree: RTree<Indexed>,
    pub water_tree: RTree<Indexed>,
    pub park_tree: RTree<Indexed>,
    pub street_tree: RTree<Indexed>,
    pub iso_bounds: Bounds,
    pub data_version: u64,
}

impl World {
    pub fn has_content(&self, query: &AABB<[f32; 2]>) -> bool {
        self.building_tree
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
    if cursor.u32()? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported data version",
        ));
    }
    let _epsg = cursor.u32()?;
    let building_count = cursor.u32()? as usize;
    let water_count = cursor.u32()? as usize;
    let park_count = cursor.u32()? as usize;
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
        let center = (
            (ring.bounds.min_x + ring.bounds.max_x) * 0.5,
            (ring.bounds.min_y + ring.bounds.max_y) * 0.5,
        );
        buildings.push(Building {
            height,
            ring,
            center,
        });
    }
    let water = (0..water_count)
        .map(|_| cursor.ring())
        .collect::<io::Result<Vec<_>>>()?;
    let parks = (0..park_count)
        .map(|_| cursor.ring())
        .collect::<io::Result<Vec<_>>>()?;
    let streets = street_bytes
        .as_deref()
        .map(parse_streets)
        .transpose()?
        .unwrap_or_default();
    let max_height = buildings.iter().map(|b| b.height).fold(0.0, f32::max);
    Ok(World {
        building_tree: index_buildings(&buildings),
        water_tree: index_rings(&water),
        park_tree: index_rings(&parks),
        street_tree: index_streets(&streets),
        buildings,
        water,
        parks,
        streets,
        iso_bounds: bounds.isometric(max_height),
        data_version,
    })
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
    let _epsg = cursor.u32()?;
    let count = cursor.u32()? as usize;
    for _ in 0..4 {
        let _bound = cursor.f64()?;
    }
    (0..count)
        .map(|_| {
            let class = cursor.u8()?;
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
            Ok(Street {
                class,
                points,
                bounds,
            })
        })
        .collect()
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
    fn u8(&mut self) -> io::Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated u8"))
    }
    fn f32(&mut self) -> io::Result<f32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid f32"))?;
        Ok(f32::from_le_bytes(bytes))
    }
    fn f64(&mut self) -> io::Result<f64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid f64"))?;
        Ok(f64::from_le_bytes(bytes))
    }
    fn ring(&mut self) -> io::Result<Ring> {
        let count = self.u32()? as usize;
        let mut points = Vec::with_capacity(count);
        let mut bounds = Bounds {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        for _ in 0..count {
            let x = self.f32()?;
            let y = self.f32()?;
            bounds.min_x = bounds.min_x.min(x);
            bounds.min_y = bounds.min_y.min(y);
            bounds.max_x = bounds.max_x.max(x);
            bounds.max_y = bounds.max_y.max(y);
            points.push((x, y));
        }
        Ok(Ring { points, bounds })
    }
}

pub fn isometric(x: f32, y: f32, height: f32) -> (f32, f32) {
    (x - y, (x + y) * 0.5 - height)
}

pub fn inverse_isometric(x: f32, y: f32) -> (f32, f32) {
    ((x + 2.0 * y) * 0.5, (2.0 * y - x) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::{Bounds, Cursor, fingerprint, fingerprint_pair, inverse_isometric, isometric};

    #[test]
    fn isometric_bounds_cover_ground_and_height() {
        let source = Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 10.0,
            max_y: 20.0,
        };
        let projected = source.isometric(5.0);

        assert_eq!(projected.min_x, -20.0);
        assert_eq!(projected.max_x, 10.0);
        assert_eq!(projected.min_y, -5.0);
        assert_eq!(projected.max_y, 15.0);
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

        assert_eq!(
            inverse_isometric(projected.0, projected.1),
            (820_983.0, 71_996.0)
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
    fn fingerprint_changes_with_data() {
        assert_ne!(fingerprint(b"city-a"), fingerprint(b"city-b"));
        assert_eq!(fingerprint(b"city-a"), fingerprint(b"city-a"));
        assert_ne!(
            fingerprint_pair(b"city", b"roads-a"),
            fingerprint_pair(b"city", b"roads-b")
        );
    }
}
