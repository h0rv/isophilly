use std::{fs, io, path::Path};

use rstar::{AABB, RTree, RTreeObject};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"GEOPHILY";
const EPSG: u32 = 32129;
const MESH_FACE_BYTES: usize = 3 * 5 * size_of::<f32>();
const MESH_COVERAGE_BUFFER_METERS: f32 = 12.0;

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
    fn include(&mut self, other: Self) {
        self.min_x = self.min_x.min(other.min_x);
        self.min_y = self.min_y.min(other.min_y);
        self.max_x = self.max_x.max(other.max_x);
        self.max_y = self.max_y.max(other.max_y);
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

    fn squared_distance_to_ring(&self, other: &Self) -> f32 {
        if self
            .points
            .iter()
            .copied()
            .any(|point| other.contains(point))
            || other
                .points
                .iter()
                .copied()
                .any(|point| self.contains(point))
        {
            return 0.0;
        }
        (0..self.points.len())
            .flat_map(|left| {
                (0..other.points.len()).map(move |right| {
                    squared_distance_between_segments(
                        self.points[left],
                        self.points[(left + 1) % self.points.len()],
                        other.points[right],
                        other.points[(right + 1) % other.points.len()],
                    )
                })
            })
            .fold(f32::INFINITY, f32::min)
    }

    fn intersects(&self, query: &AABB<[f32; 2]>) -> bool {
        let lower = query.lower();
        let upper = query.upper();
        if self.bounds.max_x < lower[0]
            || self.bounds.min_x > upper[0]
            || self.bounds.max_y < lower[1]
            || self.bounds.min_y > upper[1]
        {
            return false;
        }
        if self
            .points
            .iter()
            .any(|&(x, y)| (lower[0]..=upper[0]).contains(&x) && (lower[1]..=upper[1]).contains(&y))
        {
            return true;
        }
        if [
            (lower[0], lower[1]),
            (lower[0], upper[1]),
            (upper[0], lower[1]),
            (upper[0], upper[1]),
        ]
        .into_iter()
        .any(|corner| self.contains(corner))
        {
            return true;
        }
        (0..self.points.len()).any(|index| {
            segment_intersects_box(
                self.points[index],
                self.points[(index + 1) % self.points.len()],
                &lower,
                &upper,
            )
        })
    }
}

fn squared_distance_to_segment(point: (f32, f32), start: (f32, f32), end: (f32, f32)) -> f32 {
    let segment = (end.0 - start.0, end.1 - start.1);
    let length_squared = segment.0.mul_add(segment.0, segment.1 * segment.1);
    if length_squared == 0.0 {
        return squared_distance(point, start);
    }
    let offset = (point.0 - start.0, point.1 - start.1);
    let fraction =
        (offset.0.mul_add(segment.0, offset.1 * segment.1) / length_squared).clamp(0.0, 1.0);
    squared_distance(
        point,
        (
            segment.0.mul_add(fraction, start.0),
            segment.1.mul_add(fraction, start.1),
        ),
    )
}

fn squared_distance_between_segments(
    left_start: (f32, f32),
    left_end: (f32, f32),
    right_start: (f32, f32),
    right_end: (f32, f32),
) -> f32 {
    let cross = |start: (f32, f32), end: (f32, f32), point: (f32, f32)| {
        (end.0 - start.0).mul_add(point.1 - start.1, -(end.1 - start.1) * (point.0 - start.0))
    };
    let right_sides = (
        cross(left_start, left_end, right_start),
        cross(left_start, left_end, right_end),
    );
    let left_sides = (
        cross(right_start, right_end, left_start),
        cross(right_start, right_end, left_end),
    );
    if right_sides.0 * right_sides.1 < 0.0 && left_sides.0 * left_sides.1 < 0.0 {
        return 0.0;
    }
    [
        squared_distance_to_segment(left_start, right_start, right_end),
        squared_distance_to_segment(left_end, right_start, right_end),
        squared_distance_to_segment(right_start, left_start, left_end),
        squared_distance_to_segment(right_end, left_start, left_end),
    ]
    .into_iter()
    .fold(f32::INFINITY, f32::min)
}

fn segment_intersects_box(
    start: (f32, f32),
    end: (f32, f32),
    lower: &[f32; 2],
    upper: &[f32; 2],
) -> bool {
    let delta = (end.0 - start.0, end.1 - start.1);
    let mut enter = 0.0_f32;
    let mut leave = 1.0_f32;
    for (direction, distance) in [
        (-delta.0, start.0 - lower[0]),
        (delta.0, upper[0] - start.0),
        (-delta.1, start.1 - lower[1]),
        (delta.1, upper[1] - start.1),
    ] {
        if direction == 0.0 {
            if distance < 0.0 {
                return false;
            }
            continue;
        }
        let ratio = distance / direction;
        if direction < 0.0 {
            enter = enter.max(ratio);
        } else {
            leave = leave.min(ratio);
        }
        if enter > leave {
            return false;
        }
    }
    true
}
#[derive(Clone)]
pub struct Building {
    pub height: f32,
    pub ring: Ring,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoofShape {
    Flat,
    Gabled,
    Hipped,
    Pyramidal,
    Dome,
    Cone,
    Mansard,
}
impl TryFrom<u8> for RoofShape {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Flat),
            1 => Ok(Self::Gabled),
            2 => Ok(Self::Hipped),
            3 => Ok(Self::Pyramidal),
            4 => Ok(Self::Dome),
            5 => Ok(Self::Cone),
            6 => Ok(Self::Mansard),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported roof shape",
            )),
        }
    }
}
#[derive(Clone)]
pub struct BuildingPart {
    pub osm_id: u64,
    pub height: f32,
    pub min_height: f32,
    pub roof_height: f32,
    pub roof_shape: RoofShape,
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
    pub center: (f32, f32),
    pub highest_point: (f32, f32, f32),
}
#[derive(Clone)]
pub struct TexturedFace {
    pub texture_id: u32,
    pub face: MeshFace,
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
    pub city: Vec<Ring>,
    pub water: Vec<Ring>,
    pub parks: Vec<Ring>,
    pub building_iso_tree: RTree<Indexed>,
    pub building_covered_by_mesh: Vec<bool>,
    pub building_detailed_by_parts: Vec<bool>,
    pub building_part_iso_tree: RTree<Indexed>,
    pub building_part_covered_by_mesh: Vec<bool>,
    pub mesh_face_tree: RTree<Indexed>,
    pub city_tree: RTree<Indexed>,
    pub water_tree: RTree<Indexed>,
    pub park_tree: RTree<Indexed>,
    pub iso_bounds: Bounds,
    pub max_height: f32,
    pub world_sha256: [u8; 32],
}

impl World {
    pub fn source_envelope(&self, bounds: Bounds) -> AABB<[f32; 2]> {
        bounds.source_envelope(self.max_height)
    }

    pub fn aerial_source_bounds(&self, bounds: Bounds) -> Bounds {
        let mut source = bounds.ground_source_bounds();
        let query = AABB::from_corners([bounds.min_x, bounds.min_y], [bounds.max_x, bounds.max_y]);
        for item in self
            .building_iso_tree
            .locate_in_envelope_intersecting(query)
        {
            source.include(self.buildings[item.index].ring.bounds);
        }
        for item in self
            .building_part_iso_tree
            .locate_in_envelope_intersecting(&query)
        {
            source.include(self.building_parts[item.index].ring.bounds);
        }
        source
    }

    pub fn has_content(&self, query: &AABB<[f32; 2]>) -> bool {
        self.city_tree
            .locate_in_envelope_intersecting(*query)
            .any(|item| self.city[item.index].intersects(query))
    }

    pub fn city_hall_focus(&self) -> Option<[f32; 2]> {
        const CITY_HALL: (f32, f32) = (820_994.25, 71_994.46);
        self.building_meshes
            .iter()
            .min_by(|left, right| {
                squared_distance(left.center, CITY_HALL)
                    .total_cmp(&squared_distance(right.center, CITY_HALL))
            })
            .map(|mesh| mesh.highest_point)
            .map(|(x, y, z)| {
                let point = isometric(x, y, z);
                [point.0, point.1]
            })
    }
}

pub fn load_world(path: &Path) -> io::Result<World> {
    let bytes = fs::read(path)?;
    let world_sha256 = Sha256::digest(&bytes).into();
    parse_world(&bytes, world_sha256)
}

pub fn world_digest(path: &Path) -> io::Result<[u8; 32]> {
    Ok(Sha256::digest(fs::read(path)?).into())
}

fn parse_world(bytes: &[u8], world_sha256: [u8; 32]) -> io::Result<World> {
    let mut cursor = Cursor { bytes, offset: 0 };
    if cursor.take(8)? != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not isophilly data",
        ));
    }
    if cursor.u32()? != 8 {
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
    let city_ring_count = cursor.u32()? as usize;
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
        let osm_id = cursor.u64()?;
        let height = cursor.f32()?;
        let min_height = cursor.f32()?;
        let roof_height = cursor.f32()?;
        let roof_shape = RoofShape::try_from(cursor.u8()?)?;
        if !(0.0..=400.0).contains(&height)
            || !(0.0..height).contains(&min_height)
            || !(0.0..=height - min_height).contains(&roof_height)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "building part heights are outside the supported range",
            ));
        }
        building_parts.push(BuildingPart {
            osm_id,
            height,
            min_height,
            roof_height,
            roof_shape,
            ring: cursor.ring()?,
        });
    }
    let mut building_meshes = Vec::with_capacity(building_mesh_count);
    let mut mesh_faces = Vec::new();
    for _ in 0..building_mesh_count {
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
        let mut highest_point: Option<(f32, f32, f32)> = None;
        for _ in 0..face_count {
            cursor.ensure_items(3, 20)?;
            let mut points = [(0.0, 0.0, 0.0); 3];
            let mut uvs = [(0.0, 0.0); 3];
            for index in 0..3 {
                points[index] = cursor.point3()?;
                uvs[index] = (cursor.f32()?, cursor.f32()?);
                if highest_point.is_none_or(|highest| points[index].2 > highest.2) {
                    highest_point = Some(points[index]);
                }
            }
            mesh_faces.push(TexturedFace {
                texture_id,
                face: MeshFace { points, uvs },
            });
        }
        let highest_point = highest_point.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "mesh must contain at least one face",
            )
        })?;
        building_meshes.push(BuildingMesh {
            texture_id,
            height,
            footprint,
            center,
            highest_point,
        });
    }
    let city = (0..city_ring_count)
        .map(|_| cursor.ring())
        .collect::<io::Result<Vec<_>>>()?;
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
    let max_height = buildings
        .iter()
        .map(|building| building.height)
        .chain(building_parts.iter().map(|part| part.height))
        .chain(building_meshes.iter().map(|mesh| mesh.height))
        .fold(0.0, f32::max);
    let building_iso_tree = index_buildings(&buildings);
    let building_source_tree = index_source_buildings(&buildings);
    let building_part_iso_tree = index_building_parts(&building_parts);
    let building_mesh_tree = index_building_meshes(&building_meshes);
    let building_covered_by_mesh = buildings
        .iter()
        .map(|building| {
            let bounds = building.ring.bounds.pad(MESH_COVERAGE_BUFFER_METERS);
            let query =
                AABB::from_corners([bounds.min_x, bounds.min_y], [bounds.max_x, bounds.max_y]);
            building_mesh_tree
                .locate_in_envelope_intersecting(query)
                .any(|item| mesh_covers_building(building, &building_meshes[item.index]))
        })
        .collect();
    let building_part_covered_by_mesh = building_parts
        .iter()
        .map(|part| {
            let bounds = part.ring.bounds.pad(MESH_COVERAGE_BUFFER_METERS);
            let query =
                AABB::from_corners([bounds.min_x, bounds.min_y], [bounds.max_x, bounds.max_y]);
            building_mesh_tree
                .locate_in_envelope_intersecting(&query)
                .any(|item| mesh_covers_part(part, &building_meshes[item.index]))
        })
        .collect();
    let building_detailed_by_parts =
        detailed_buildings(&buildings, &building_source_tree, &building_parts);
    let mut texture_ids: Vec<_> = building_meshes.iter().map(|mesh| mesh.texture_id).collect();
    texture_ids.sort_unstable();
    texture_ids.dedup();
    Ok(World {
        building_iso_tree,
        building_covered_by_mesh,
        building_detailed_by_parts,
        building_part_iso_tree,
        building_part_covered_by_mesh,
        mesh_face_tree: index_mesh_faces(&mesh_faces),
        city_tree: index_rings(&city),
        water_tree: index_rings(&water),
        park_tree: index_rings(&parks),
        buildings,
        building_parts,
        building_meshes,
        mesh_faces,
        texture_ids,
        texture_sha256,
        city,
        water,
        parks,
        iso_bounds: bounds.isometric(max_height),
        max_height,
        world_sha256,
    })
}

fn mesh_covers_building(building: &Building, mesh: &BuildingMesh) -> bool {
    mesh_covers_ring(&building.ring, building.height, mesh)
}

fn mesh_covers_part(part: &BuildingPart, mesh: &BuildingMesh) -> bool {
    mesh.height >= part.height * 0.9
        && mesh.footprint.squared_distance_to_ring(&part.ring)
            <= MESH_COVERAGE_BUFFER_METERS.powi(2)
}

fn mesh_covers_ring(ring: &Ring, height: f32, mesh: &BuildingMesh) -> bool {
    mesh.height * 2.0 >= height
        && mesh.footprint.squared_distance_to_ring(ring) <= MESH_COVERAGE_BUFFER_METERS.powi(2)
}

fn squared_distance(left: (f32, f32), right: (f32, f32)) -> f32 {
    (left.0 - right.0).powi(2) + (left.1 - right.1).powi(2)
}

fn index_buildings(buildings: &[Building]) -> RTree<Indexed> {
    RTree::bulk_load(
        buildings
            .iter()
            .enumerate()
            .map(|(index, building)| indexed(index, building_iso_bounds(building)))
            .collect(),
    )
}

fn index_source_buildings(buildings: &[Building]) -> RTree<Indexed> {
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
            .map(|(index, part)| indexed(index, building_part_iso_bounds(part)))
            .collect(),
    )
}

fn building_iso_bounds(building: &Building) -> Bounds {
    elevated_ring_iso_bounds(&building.ring, 0.0, building.height)
}

fn building_part_iso_bounds(part: &BuildingPart) -> Bounds {
    elevated_ring_iso_bounds(&part.ring, part.min_height, part.height)
}

fn elevated_ring_iso_bounds(ring: &Ring, min_height: f32, height: f32) -> Bounds {
    let mut bounds = Bounds {
        min_x: f32::INFINITY,
        min_y: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        max_y: f32::NEG_INFINITY,
    };
    for &(x, y) in &ring.points {
        for z in [min_height, height] {
            let point = isometric(x, y, z);
            bounds.min_x = bounds.min_x.min(point.0);
            bounds.min_y = bounds.min_y.min(point.1);
            bounds.max_x = bounds.max_x.max(point.0);
            bounds.max_y = bounds.max_y.max(point.1);
        }
    }
    bounds
}

fn detailed_buildings(
    buildings: &[Building],
    building_tree: &RTree<Indexed>,
    parts: &[BuildingPart],
) -> Vec<bool> {
    let building_areas: Vec<f32> = buildings
        .iter()
        .map(|building| ring_area(&building.ring))
        .collect();
    let mut covered_areas = vec![0.0; buildings.len()];
    for part in parts {
        let center = part.ring.center();
        let point = AABB::from_point([center.0, center.1]);
        let parent = building_tree
            .locate_in_envelope_intersecting(&point)
            .filter(|item| buildings[item.index].ring.contains(center))
            .min_by(|left, right| {
                building_areas[left.index].total_cmp(&building_areas[right.index])
            });
        if let Some(parent) = parent {
            covered_areas[parent.index] += ring_area(&part.ring).min(building_areas[parent.index]);
        }
    }
    covered_areas
        .iter()
        .zip(building_areas)
        .map(|(covered, area)| area > 0.0 && *covered >= area * 0.65)
        .collect()
}

fn ring_area(ring: &Ring) -> f32 {
    ring.points
        .iter()
        .zip(ring.points.iter().cycle().skip(1))
        .take(ring.points.len())
        .map(|(left, right)| left.0.mul_add(right.1, -(right.0 * left.1)))
        .sum::<f32>()
        .abs()
        * 0.5
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
    use sha2::{Digest, Sha256};

    use super::{
        BROAD_NORTH_EAST, BROAD_NORTH_NORTH, Bounds, Building, BuildingMesh, BuildingPart, Cursor,
        MESH_FACE_BYTES, Ring, RoofShape, detailed_buildings, index_source_buildings,
        inverse_isometric, isometric, mesh_covers_building, mesh_covers_part, parse_world,
    };

    fn square(size: f32) -> Ring {
        Ring {
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: size,
                max_y: size,
            },
            points: vec![(0.0, 0.0), (size, 0.0), (size, size), (0.0, size)],
        }
    }

    fn golden_world() -> std::io::Result<Vec<u8>> {
        let hex = include_str!("../tests/fixtures/world-v8.hex")
            .trim()
            .as_bytes();
        if !hex.len().is_multiple_of(2) {
            return Err(std::io::Error::other("golden world has odd-length hex"));
        }
        hex.chunks_exact(2)
            .map(|pair| {
                let high = hex_nibble(pair[0])
                    .ok_or_else(|| std::io::Error::other("golden world contains invalid hex"))?;
                let low = hex_nibble(pair[1])
                    .ok_or_else(|| std::io::Error::other("golden world contains invalid hex"))?;
                Ok(high * 16 + low)
            })
            .collect()
    }

    fn hex_nibble(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            _ => None,
        }
    }

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
        let nearby = super::Ring {
            bounds: Bounds {
                min_x: 5.0,
                min_y: 0.0,
                max_x: 6.0,
                max_y: 1.0,
            },
            points: vec![(5.0, 0.0), (6.0, 0.0), (6.0, 1.0), (5.0, 1.0)],
        };
        assert!((ring.squared_distance_to_ring(&nearby) - 1.0).abs() < f32::EPSILON);

        let crossing = super::Ring {
            bounds: Bounds {
                min_x: -1.0,
                min_y: 1.5,
                max_x: 5.0,
                max_y: 2.5,
            },
            points: vec![(-1.0, 1.5), (5.0, 1.5), (5.0, 2.5), (-1.0, 2.5)],
        };
        assert_eq!(ring.squared_distance_to_ring(&crossing), 0.0);
    }

    #[test]
    fn ring_intersection_handles_containment_crossing_and_disjoint_boxes() {
        let ring = super::Ring {
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 4.0,
                max_y: 4.0,
            },
            points: vec![(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)],
        };

        assert!(ring.intersects(&rstar::AABB::from_corners([0.5, 0.5], [1.0, 1.0])));
        assert!(ring.intersects(&rstar::AABB::from_corners([1.5, 1.5], [3.0, 3.0])));
        assert!(!ring.intersects(&rstar::AABB::from_corners([3.0, 3.0], [4.0, 4.0])));
        assert!(!ring.intersects(&rstar::AABB::from_corners([5.0, 5.0], [6.0, 6.0])));
    }

    #[test]
    fn stale_short_mesh_does_not_hide_current_tower() {
        let building = Building {
            height: 100.0,
            ring: square(10.0),
        };
        let mesh = BuildingMesh {
            texture_id: 1,
            height: 49.0,
            footprint: square(10.0),
            center: (5.0, 5.0),
            highest_point: (5.0, 5.0, 49.0),
        };

        assert!(!mesh_covers_building(&building, &mesh));
    }

    #[test]
    fn shorter_photo_mesh_does_not_hide_a_newer_building_part() {
        let part = BuildingPart {
            osm_id: 1,
            height: 341.0,
            min_height: 277.0,
            roof_height: 0.0,
            roof_shape: RoofShape::Flat,
            ring: square(10.0),
        };
        let old_mesh = BuildingMesh {
            texture_id: 1,
            height: 277.0,
            footprint: square(10.0),
            center: (5.0, 5.0),
            highest_point: (5.0, 5.0, 277.0),
        };

        assert!(!mesh_covers_part(&part, &old_mesh));
    }

    #[test]
    fn building_parts_replace_a_parent_only_when_they_cover_most_of_it() {
        let buildings = vec![Building {
            height: 20.0,
            ring: square(10.0),
        }];
        let tree = index_source_buildings(&buildings);
        let part = |size| BuildingPart {
            osm_id: 1,
            height: 30.0,
            min_height: 0.0,
            roof_height: 0.0,
            roof_shape: RoofShape::Flat,
            ring: square(size),
        };

        assert_eq!(detailed_buildings(&buildings, &tree, &[part(9.0)]), [true]);
        assert_eq!(detailed_buildings(&buildings, &tree, &[part(4.0)]), [false]);
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
    fn parses_python_v8_golden_world() -> std::io::Result<()> {
        let bytes = golden_world()?;
        let digest = Sha256::digest(&bytes).into();
        let world = parse_world(&bytes, digest)?;

        assert_eq!(world.buildings.len(), 1);
        assert_eq!(world.building_parts.len(), 1);
        assert_eq!(world.building_parts[0].osm_id, 42);
        assert_eq!(world.building_parts[0].roof_shape, RoofShape::Pyramidal);
        assert_eq!(world.building_meshes.len(), 1);
        assert_eq!(world.mesh_faces.len(), 1);
        assert_eq!(world.city.len(), 1);
        assert_eq!(world.water.len(), 1);
        assert_eq!(world.parks.len(), 1);
        assert_eq!(world.texture_ids, vec![7]);
        assert_eq!(
            world.texture_sha256,
            std::array::from_fn(|index| index as u8)
        );
        assert_eq!(world.world_sha256, digest);
        Ok(())
    }
}
