use std::{fs, io, path::PathBuf, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::{
    live_city::{LiveCity, vehicles},
    mesh_texture::MeshTextureSource,
    pyramid::{self, ART_ZOOM, tile_path},
    render::render_blank_tile,
    texture::AerialSource,
    tile_codec::{EXTENSION, MEDIA_TYPE},
    world::{World, isometric},
};

#[derive(Clone)]
pub(crate) struct AppState {
    scene: Arc<Scene>,
    tile_dir: PathBuf,
    blank_tile: Arc<Vec<u8>>,
    pub(crate) live_city: Arc<LiveCity>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Scene {
    schema_version: u8,
    world_sha256: String,
    iso_bounds: [f32; 4],
    city_hall: Option<[f32; 2]>,
    landmarks: Vec<Landmark>,
    counts: Counts,
    tile_version: String,
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

const PYRAMID_VERSION: &str = "v34-stable-aerial-facades";
const SCENE_SCHEMA_VERSION: u8 = 1;
const CURRENT_SCENE: &str = "data/tiles/current.json";
const CLEAN_METADATA: &str = "data/clean/meta.json";
const MAX_VIEW_ZOOM: u8 = 10;
const HOME_ZOOM: u8 = 3;
const ROCKY_SOURCE: (f32, f32, f32) = (819_514.06, 73_343.64, 15.0);

pub async fn serve(port: u16) -> io::Result<()> {
    let scene = Arc::new(read_scene()?);
    let clean_sha256 = read_world_sha256()?;
    if scene.world_sha256 != clean_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the tile pyramid is stale; run `uv run poe prebuild`",
        ));
    }
    let tile_dir = tile_cache_dir(&scene.tile_version);
    if !pyramid::is_complete(&tile_dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the textured tile pyramid is missing; run `uv run poe prebuild` first",
        ));
    }
    let state = AppState {
        scene,
        tile_dir,
        blank_tile: Arc::new(render_blank_tile()?),
        live_city: Arc::new(LiveCity::new()?),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/city-overlay.js", get(city_overlay_js))
        .route("/neighborhoods.json", get(neighborhoods))
        .route("/api/vehicles", get(vehicles))
        .route("/meta", get(meta))
        .route("/tiles/{z}/{x}/{y}", get(tile))
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    println!("geo-philly http://127.0.0.1:{port}");
    axum::serve(listener, app).await.map_err(io::Error::other)
}

pub fn prebuild(
    world: &World,
    aerial: &AerialSource,
    mesh_textures: &MeshTextureSource,
) -> io::Result<()> {
    let tile_version = tile_version(world);
    let tile_dir = tile_cache_dir(&tile_version);
    pyramid::build(world, aerial, mesh_textures, &tile_dir)?;
    let bounds = world.iso_bounds;
    let rocky = isometric(ROCKY_SOURCE.0, ROCKY_SOURCE.1, ROCKY_SOURCE.2);
    let scene = Scene {
        schema_version: SCENE_SCHEMA_VERSION,
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
    write_scene(&scene)
}

async fn index() -> Result<impl IntoResponse, StatusCode> {
    static_file("static/index.html", "text/html; charset=utf-8").await
}

async fn app_js() -> Result<impl IntoResponse, StatusCode> {
    static_file("static/app.js", "text/javascript; charset=utf-8").await
}

async fn city_overlay_js() -> Result<impl IntoResponse, StatusCode> {
    static_file("static/city-overlay.js", "text/javascript; charset=utf-8").await
}

async fn neighborhoods() -> Result<impl IntoResponse, StatusCode> {
    static_file(
        "static/neighborhoods.json",
        "application/json; charset=utf-8",
    )
    .await
}

async fn static_file(
    path: &str,
    content_type: &'static str,
) -> Result<impl IntoResponse, StatusCode> {
    tokio::fs::read(path)
        .await
        .map(|body| {
            (
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                body,
            )
        })
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn meta(State(state): State<AppState>) -> Json<Scene> {
    Json(state.scene.as_ref().clone())
}

async fn tile(
    State(state): State<AppState>,
    AxumPath((z, x, y)): AxumPath<(u8, u32, String)>,
) -> Response {
    let started = Instant::now();
    let Some(y) = y
        .strip_suffix(EXTENSION)
        .and_then(|value| value.strip_suffix('.'))
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if z > ART_ZOOM || x >= 1 << z || y >= 1 << z {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = tile_path(&state.tile_dir, z, x, y);
    match tokio::fs::read(&path).await {
        Ok(image) => logged_image(image, "disk", z, x, y, started),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            logged_image(state.blank_tile.as_ref().clone(), "empty", z, x, y, started)
        }
        Err(error) => {
            warn!(?error, path = %path.display(), "tile cache read failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn logged_image(
    image: Vec<u8>,
    cache: &'static str,
    z: u8,
    x: u32,
    y: u32,
    started: Instant,
) -> Response {
    info!(
        z,
        x,
        y,
        cache,
        elapsed_ms = started.elapsed().as_millis(),
        "tile served"
    );
    (
        [
            (header::CONTENT_TYPE, MEDIA_TYPE),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::HeaderName::from_static("x-tile-cache"), cache),
        ],
        image,
    )
        .into_response()
}

fn tile_version(world: &World) -> String {
    format!("{PYRAMID_VERSION}-{:016x}", world.data_version)
}

fn tile_cache_dir(tile_version: &str) -> PathBuf {
    PathBuf::from("data/tiles").join(tile_version)
}

fn read_world_sha256() -> io::Result<String> {
    let bytes = fs::read(CLEAN_METADATA)?;
    let metadata: CleanMetadata = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    validate_sha256(&metadata.artifacts.world.sha256)?;
    Ok(metadata.artifacts.world.sha256)
}

fn read_scene() -> io::Result<Scene> {
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
    let scene: Scene = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    validate_scene(&scene)?;
    Ok(scene)
}

fn write_scene(scene: &Scene) -> io::Result<()> {
    validate_scene(scene)?;
    let path = PathBuf::from(CURRENT_SCENE);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tile manifest path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.part");
    let mut bytes = serde_json::to_vec_pretty(scene).map_err(io::Error::other)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

fn validate_scene(scene: &Scene) -> io::Result<()> {
    if scene.schema_version != SCENE_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported tile manifest version",
        ));
    }
    validate_sha256(&scene.world_sha256)?;
    if scene.tile_version.is_empty()
        || !scene
            .tile_version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid tile version in manifest",
        ));
    }
    if scene.max_tile_zoom != ART_ZOOM || scene.max_zoom < scene.max_tile_zoom {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid zoom levels in tile manifest",
        ));
    }
    let [min_x, min_y, max_x, max_y] = scene.iso_bounds;
    if !scene.iso_bounds.iter().all(|value| value.is_finite()) || min_x >= max_x || min_y >= max_y {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid bounds in tile manifest",
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> io::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid world digest",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ART_ZOOM, Counts, Landmark, SCENE_SCHEMA_VERSION, Scene, validate_scene, validate_sha256,
    };

    fn scene() -> Scene {
        Scene {
            schema_version: SCENE_SCHEMA_VERSION,
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
        assert!(validate_scene(&scene()).is_ok());
    }

    #[test]
    fn rejects_path_in_tile_version() {
        let mut invalid = scene();
        invalid.tile_version = "../tiles".to_owned();
        assert!(validate_scene(&invalid).is_err());
    }

    #[test]
    fn validates_lowercase_sha256() {
        assert!(validate_sha256(&"0".repeat(64)).is_ok());
        assert!(validate_sha256(&"A".repeat(64)).is_err());
        assert!(validate_sha256("short").is_err());
    }
}
