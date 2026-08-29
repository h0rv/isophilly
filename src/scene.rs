use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    pyramid::ART_ZOOM,
    world::{World, isometric},
};

const SCHEMA_VERSION: u8 = 1;
const CURRENT_SCENE: &str = "data/tiles/current.json";
const CLEAN_METADATA: &str = "data/clean/meta.json";
const MAX_VIEW_ZOOM: u8 = 10;
const HOME_ZOOM: u8 = 3;
const ROCKY_SOURCE: (f32, f32, f32) = (819_514.06, 73_343.64, 15.0);

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Scene {
    schema_version: u8,
    world_sha256: String,
    iso_bounds: [f32; 4],
    city_hall: Option<[f32; 2]>,
    landmarks: Vec<Landmark>,
    counts: Counts,
    pub(crate) tile_version: String,
    max_tile_zoom: u8,
    max_zoom: u8,
    home_zoom: u8,
}

#[derive(Clone, Deserialize, Serialize)]
struct Landmark {
    name: String,
    point: [f32; 2],
    min_zoom: u8,
    color: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct Counts {
    buildings: usize,
    building_parts: usize,
    building_meshes: usize,
    water: usize,
    parks: usize,
    streets: usize,
}

#[derive(Deserialize)]
struct CleanMetadata {
    artifacts: CleanArtifacts,
}

#[derive(Deserialize)]
struct CleanArtifacts {
    #[serde(rename = "philly.bin")]
    world: CleanArtifact,
}

#[derive(Deserialize)]
struct CleanArtifact {
    sha256: String,
}

impl Scene {
    pub(crate) fn from_world(world: &World, tile_version: String) -> io::Result<Self> {
        let bounds = world.iso_bounds;
        let rocky = isometric(ROCKY_SOURCE.0, ROCKY_SOURCE.1, ROCKY_SOURCE.2);
        let scene = Self {
            schema_version: SCHEMA_VERSION,
            world_sha256: read_world_sha256()?,
            iso_bounds: [bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y],
            city_hall: world.city_hall_focus(),
            landmarks: vec![Landmark {
                name: "Rocky".to_owned(),
                point: [rocky.0, rocky.1],
                min_zoom: 7,
                color: "#8f5f3b".to_owned(),
            }],
            counts: Counts {
                buildings: world.buildings.len(),
                building_parts: world.building_parts.len(),
                building_meshes: world.building_meshes.len(),
                water: world.water.len(),
                parks: world.parks.len(),
                streets: world.streets.len(),
            },
            tile_version,
            max_tile_zoom: ART_ZOOM,
            max_zoom: MAX_VIEW_ZOOM,
            home_zoom: HOME_ZOOM,
        };
        scene.validate()?;
        Ok(scene)
    }

    pub(crate) fn read_current() -> io::Result<Self> {
        let bytes = fs::read(CURRENT_SCENE).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "the tile manifest is missing; run `uv run poe prebuild` first",
                )
            } else {
                error
            }
        })?;
        let scene: Self = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        scene.validate()?;
        if scene.world_sha256 != read_world_sha256()? {
            return Err(invalid(
                "the tile pyramid is stale; run `uv run poe prebuild`",
            ));
        }
        Ok(scene)
    }

    pub(crate) fn write_current(&self) -> io::Result<()> {
        self.validate()?;
        let path = PathBuf::from(CURRENT_SCENE);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("tile manifest path has no parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = path.with_extension("json.part");
        let mut bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        bytes.push(b'\n');
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(invalid("unsupported tile manifest version"));
        }
        validate_sha256(&self.world_sha256)?;
        if self.tile_version.is_empty()
            || !self
                .tile_version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(invalid("invalid tile version in manifest"));
        }
        if self.max_tile_zoom != ART_ZOOM || self.max_zoom < self.max_tile_zoom {
            return Err(invalid("invalid zoom levels in tile manifest"));
        }
        let [min_x, min_y, max_x, max_y] = self.iso_bounds;
        if !self.iso_bounds.iter().all(|value| value.is_finite())
            || min_x >= max_x
            || min_y >= max_y
        {
            return Err(invalid("invalid bounds in tile manifest"));
        }
        Ok(())
    }
}

fn read_world_sha256() -> io::Result<String> {
    let bytes = fs::read(CLEAN_METADATA)?;
    let metadata: CleanMetadata = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    validate_sha256(&metadata.artifacts.world.sha256)?;
    Ok(metadata.artifacts.world.sha256)
}

fn validate_sha256(value: &str) -> io::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("invalid world digest"));
    }
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::{ART_ZOOM, Counts, Landmark, SCHEMA_VERSION, Scene, validate_sha256};

    fn scene() -> Scene {
        Scene {
            schema_version: SCHEMA_VERSION,
            world_sha256: "a".repeat(64),
            iso_bounds: [1.0, 2.0, 3.0, 4.0],
            city_hall: Some([2.0, 3.0]),
            landmarks: vec![Landmark {
                name: "Rocky".to_owned(),
                point: [2.0, 3.0],
                min_zoom: 7,
                color: "#8f5f3b".to_owned(),
            }],
            counts: Counts {
                buildings: 1,
                building_parts: 0,
                building_meshes: 1,
                water: 0,
                parks: 0,
                streets: 0,
            },
            tile_version: "v1-test".to_owned(),
            max_tile_zoom: ART_ZOOM,
            max_zoom: 10,
            home_zoom: 3,
        }
    }

    #[test]
    fn accepts_valid_scene() {
        assert!(scene().validate().is_ok());
    }

    #[test]
    fn rejects_path_in_tile_version() {
        let mut invalid = scene();
        invalid.tile_version = "../tiles".to_owned();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn validates_lowercase_sha256() {
        assert!(validate_sha256(&"0".repeat(64)).is_ok());
        assert!(validate_sha256(&"A".repeat(64)).is_err());
        assert!(validate_sha256("short").is_err());
    }
}
