use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    pyramid::{ART_ZOOM, RICH_ART_ZOOM},
    world::{View, World, isometric},
};

const SCHEMA_VERSION: u8 = 2;
const CURRENT_SCENE: &str = "data/tiles/current.json";
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
    rich: RichScene,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct RichScene {
    views: Vec<RichView>,
    home_zoom: u8,
    max_tile_zoom: u8,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct RichView {
    id: String,
    label: String,
    iso_bounds: [f32; 4],
    city_hall: Option<[f32; 2]>,
    landmarks: Vec<Landmark>,
    tile_version: String,
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
    building_meshes: usize,
}

impl Scene {
    pub(crate) fn rich_views(&self) -> &[RichView] {
        &self.rich.views
    }
    pub(crate) fn from_world(
        world: &World,
        tile_version: String,
        rich_versions: &[(View, String)],
    ) -> io::Result<Self> {
        let bounds = world.iso_bounds;
        let rocky = isometric(ROCKY_SOURCE.0, ROCKY_SOURCE.1, ROCKY_SOURCE.2);
        let world_sha256 = digest_hex(&world.world_sha256);
        let scene = Self {
            schema_version: SCHEMA_VERSION,
            world_sha256,
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
                building_meshes: world.building_meshes.len(),
            },
            tile_version,
            max_tile_zoom: ART_ZOOM,
            max_zoom: MAX_VIEW_ZOOM,
            home_zoom: HOME_ZOOM,
            rich: RichScene {
                views: rich_versions
                    .iter()
                    .map(|(view, version)| {
                        let bounds = world.rich_bounds(*view).ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidData, "world has no rich mesh")
                        })?;
                        Ok(RichView {
                            id: view.id().to_owned(),
                            label: view_label(*view).to_owned(),
                            iso_bounds: [bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y],
                            city_hall: world.city_hall_focus_for(*view),
                            landmarks: vec![Landmark {
                                name: "Rocky".to_owned(),
                                point: {
                                    let point = view.project(
                                        ROCKY_SOURCE.0,
                                        ROCKY_SOURCE.1,
                                        ROCKY_SOURCE.2,
                                    );
                                    [point.0, point.1]
                                },
                                min_zoom: 4,
                                color: "#8f5f3b".to_owned(),
                            }],
                            tile_version: version.clone(),
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?,
                home_zoom: 4,
                max_tile_zoom: RICH_ART_ZOOM,
            },
        };
        scene.validate()?;
        Ok(scene)
    }

    pub(crate) fn read_current() -> io::Result<Self> {
        let bytes = fs::read(CURRENT_SCENE).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "the tile manifest is missing; run `uv run --locked poe prebuild` first",
                )
            } else {
                error
            }
        })?;
        let scene: Self = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        scene.validate()?;
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

    pub(crate) fn matches_world(&self, digest: &[u8; 32]) -> bool {
        self.world_sha256 == digest_hex(digest)
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
        if self.rich.views.len() != View::ALL.len()
            || self
                .rich
                .views
                .iter()
                .zip(View::ALL)
                .any(|(candidate, expected)| candidate.id != expected.id())
        {
            return Err(invalid(
                "rich views must contain all four orientations in order",
            ));
        }
        for view in &self.rich.views {
            let [min_x, min_y, max_x, max_y] = view.iso_bounds;
            if !view.iso_bounds.iter().all(|value| value.is_finite())
                || min_x >= max_x
                || min_y >= max_y
                || view.tile_version.is_empty()
            {
                return Err(invalid("invalid rich view"));
            }
        }
        if self.rich.max_tile_zoom != RICH_ART_ZOOM || self.rich.home_zoom > self.max_zoom {
            return Err(invalid("invalid rich zoom levels"));
        }
        Ok(())
    }
}

impl RichView {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn tile_version(&self) -> &str {
        &self.tile_version
    }
}

fn view_label(view: View) -> &'static str {
    match view {
        View::SouthEast => "Southeast",
        View::SouthWest => "Southwest",
        View::NorthWest => "Northwest",
        View::NorthEast => "Northeast",
    }
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
    use super::{
        ART_ZOOM, Counts, Landmark, RICH_ART_ZOOM, RichScene, RichView, SCHEMA_VERSION, Scene,
        digest_hex, validate_sha256,
    };

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
                building_meshes: 1,
            },
            tile_version: "v1-test".to_owned(),
            max_tile_zoom: ART_ZOOM,
            max_zoom: 10,
            home_zoom: 3,
            rich: RichScene {
                views: [
                    ("se", "Southeast"),
                    ("sw", "Southwest"),
                    ("nw", "Northwest"),
                    ("ne", "Northeast"),
                ]
                .into_iter()
                .map(|(id, label)| RichView {
                    id: id.to_owned(),
                    label: label.to_owned(),
                    iso_bounds: [1.0, 2.0, 3.0, 4.0],
                    city_hall: Some([2.0, 3.0]),
                    landmarks: vec![Landmark {
                        name: "Rocky".to_owned(),
                        point: [2.0, 3.0],
                        min_zoom: 4,
                        color: "#8f5f3b".to_owned(),
                    }],
                    tile_version: format!("v1-rich-{id}"),
                })
                .collect(),
                home_zoom: 4,
                max_tile_zoom: RICH_ART_ZOOM,
            },
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
        assert_eq!(digest_hex(&[0xab; 32]), "ab".repeat(32));
    }
}
