use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    pyramid::{ART_ZOOM, RICH_ART_ZOOM},
    tile_identity::{base_tile_version_hex, is_generation_of, rich_tile_version},
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    land_cover_sha256: Option<String>,
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
    #[serde(default)]
    street_trees: usize,
    #[serde(default)]
    transport_lines: usize,
}

impl Scene {
    pub(crate) fn rich_views(&self) -> &[RichView] {
        &self.rich.views
    }
    pub(crate) fn from_world(
        world: &World,
        land_cover_sha256: Option<&[u8; 32]>,
        tile_version: String,
        rich_versions: &[(View, String)],
    ) -> io::Result<Self> {
        let bounds = world.iso_bounds;
        let rocky = isometric(ROCKY_SOURCE.0, ROCKY_SOURCE.1, ROCKY_SOURCE.2);
        let world_sha256 = digest_hex(&world.world_sha256);
        let scene = Self {
            schema_version: SCHEMA_VERSION,
            world_sha256,
            land_cover_sha256: land_cover_sha256.map(digest_hex),
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
                street_trees: world.street_trees.len(),
                transport_lines: world.transport.len(),
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

    pub(crate) fn matches_inputs(
        &self,
        world_sha256: &[u8; 32],
        land_cover_sha256: Option<&[u8; 32]>,
    ) -> bool {
        self.world_sha256 == digest_hex(world_sha256)
            && self.land_cover_sha256.as_deref() == land_cover_sha256.map(digest_hex).as_deref()
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(invalid("unsupported tile manifest version"));
        }
        validate_sha256(&self.world_sha256)?;
        if let Some(digest) = &self.land_cover_sha256 {
            validate_sha256(digest)?;
        }
        let base_version =
            base_tile_version_hex(&self.world_sha256, self.land_cover_sha256.as_deref());
        if !is_generation_of(&self.tile_version, &base_version) {
            return Err(invalid("tile version does not match scene input digests"));
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
        for (view, orientation) in self.rich.views.iter().zip(View::ALL) {
            let [min_x, min_y, max_x, max_y] = view.iso_bounds;
            let rich_base = rich_tile_version(&self.tile_version, orientation);
            if !view.iso_bounds.iter().all(|value| value.is_finite())
                || min_x >= max_x
                || min_y >= max_y
                || !is_generation_of(&view.tile_version, &rich_base)
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
    use crate::{
        tile_identity::{base_tile_version_hex, rich_tile_version},
        world::View,
    };

    fn scene() -> Scene {
        let world_sha256 = "a".repeat(64);
        let tile_version = base_tile_version_hex(&world_sha256, None);
        Scene {
            schema_version: SCHEMA_VERSION,
            world_sha256,
            land_cover_sha256: None,
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
                street_trees: 1,
                transport_lines: 0,
            },
            tile_version: tile_version.clone(),
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
                .zip(View::ALL)
                .map(|((id, label), view)| RichView {
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
                    tile_version: rich_tile_version(&tile_version, view),
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
    fn rejects_tile_identity_that_disagrees_with_manifest_digests() {
        let mut invalid = scene();
        invalid.tile_version = base_tile_version_hex(&"b".repeat(64), None);
        assert!(invalid.validate().is_err());

        let mut invalid = scene();
        invalid.rich.views[0].tile_version =
            rich_tile_version(&invalid.tile_version, View::SouthWest);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn land_cover_identity_requires_matching_base_and_every_rich_view() {
        let mut current = scene();
        current.land_cover_sha256 = Some("c".repeat(64));
        assert!(current.validate().is_err());

        current.tile_version =
            base_tile_version_hex(&current.world_sha256, current.land_cover_sha256.as_deref());
        for (rich, view) in current.rich.views.iter_mut().zip(View::ALL) {
            rich.tile_version = rich_tile_version(&current.tile_version, view);
        }
        assert!(current.validate().is_ok());

        current.rich.views[3].tile_version =
            rich_tile_version(&current.tile_version, View::SouthEast);
        assert!(current.validate().is_err());
    }

    #[test]
    fn validates_lowercase_sha256() {
        assert!(validate_sha256(&"0".repeat(64)).is_ok());
        assert!(validate_sha256(&"A".repeat(64)).is_err());
        assert!(validate_sha256("short").is_err());
        assert_eq!(digest_hex(&[0xab; 32]), "ab".repeat(32));
    }

    #[test]
    fn land_cover_digest_is_optional_but_part_of_input_identity() {
        let mut current = scene();
        let world = [0xaa; 32];
        current.world_sha256 = digest_hex(&world);
        assert!(current.matches_inputs(&world, None));

        let land_cover = [0xbb; 32];
        assert!(!current.matches_inputs(&world, Some(&land_cover)));
        current.land_cover_sha256 = Some(digest_hex(&land_cover));
        assert!(current.matches_inputs(&world, Some(&land_cover)));
        assert!(!current.matches_inputs(&world, None));
    }

    #[test]
    fn legacy_scene_without_land_cover_digest_remains_readable() -> std::io::Result<()> {
        let serialized = serde_json::to_value(scene()).map_err(std::io::Error::other)?;
        let parsed: Scene = serde_json::from_value(serialized).map_err(std::io::Error::other)?;
        assert!(parsed.land_cover_sha256.is_none());
        parsed.validate()?;
        Ok(())
    }
}
