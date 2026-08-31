use std::{collections::HashMap, fs::OpenOptions, io, path::PathBuf, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::{
    mesh_texture::MeshTextureSource,
    pyramid::{self, ART_ZOOM, TileInventory, tile_path},
    render::render_blank_tile,
    scene::Scene,
    texture::AerialSource,
    tile_codec::{EXTENSION, MEDIA_TYPE},
    tile_identity::{base_tile_version, is_generation_of, rich_tile_version},
    world::World,
};

#[derive(Clone)]
pub(crate) struct AppState {
    scene: Arc<Scene>,
    tile_dir: PathBuf,
    tile_inventory: Arc<TileInventory>,
    coverage_json: Arc<Vec<u8>>,
    rich_tiles: Arc<HashMap<String, RichTiles>>,
    blank_tile: Arc<Vec<u8>>,
}

struct RichTiles {
    tile_dir: PathBuf,
    inventory: TileInventory,
    coverage_json: Vec<u8>,
}

#[derive(Serialize)]
struct TileCoverage {
    schema_version: u8,
    tile_version: String,
    tiles: Vec<String>,
}

pub async fn serve(port: u16) -> io::Result<()> {
    let scene = Arc::new(Scene::read_current()?);
    let tile_dir = tile_cache_dir(&scene.tile_version);
    let tile_inventory = Arc::new(pyramid::read_inventory(&tile_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "the textured tile pyramid is missing or incomplete; run `uv run --locked poe prebuild`: {error}"
            ),
        )
    })?);
    let coverage_json = serde_json::to_vec(&TileCoverage {
        schema_version: 1,
        tile_version: scene.tile_version.clone(),
        tiles: tile_inventory.tile_keys(),
    })
    .map_err(io::Error::other)?;
    let rich_tiles = scene
        .rich_views()
        .iter()
        .map(|view| {
            let tile_dir = tile_cache_dir(view.tile_version());
            let inventory = pyramid::read_inventory(&tile_dir)?;
            let coverage_json = serde_json::to_vec(&TileCoverage {
                schema_version: 1,
                tile_version: view.tile_version().to_owned(),
                tiles: inventory.tile_keys(),
            })
            .map_err(io::Error::other)?;
            Ok((
                view.id().to_owned(),
                RichTiles {
                    tile_dir,
                    inventory,
                    coverage_json,
                },
            ))
        })
        .collect::<io::Result<HashMap<_, _>>>()?;
    let state = AppState {
        scene,
        tile_dir,
        tile_inventory,
        coverage_json: Arc::new(coverage_json),
        rich_tiles: Arc::new(rich_tiles),
        blank_tile: Arc::new(render_blank_tile()?),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/city-overlay.js", get(city_overlay_js))
        .route("/neighborhoods.json", get(neighborhoods))
        .route("/meta", get(meta))
        .route("/coverage.json", get(coverage))
        .route("/rich/{view}/coverage.json", get(rich_coverage))
        .route("/tiles/{z}/{x}/{y}", get(tile))
        .route("/rich/{view}/tiles/{z}/{x}/{y}", get(rich_tile))
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    println!("IsoPhilly http://127.0.0.1:{port}");
    axum::serve(listener, app).await.map_err(io::Error::other)
}

pub fn prebuild(
    world: &World,
    aerial: &AerialSource,
    mesh_textures: &MeshTextureSource,
    land_cover_sha256: Option<&[u8; 32]>,
) -> io::Result<()> {
    let base_version = base_tile_version(&world.world_sha256, land_cover_sha256);
    let tile_root = PathBuf::from("data/tiles");
    std::fs::create_dir_all(&tile_root)?;
    let build_lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(tile_root.join(format!(".{base_version}.lock")))?;
    build_lock.try_lock().map_err(|error| {
        io::Error::other(format!("another prebuild is already running: {error}"))
    })?;
    let base_dir = tile_cache_dir(&base_version);
    let tile_version = if pyramid::validate_complete(&base_dir).is_ok() {
        base_version
    } else {
        let tile_version = available_tile_version(&base_version)?;
        let tile_dir = tile_cache_dir(&tile_version);
        let staging = tile_root.join(format!(".{tile_version}.building"));
        pyramid::build(world, aerial, mesh_textures, &staging)?;
        std::fs::rename(&staging, &tile_dir)?;
        tile_version
    };
    let mut rich_versions = Vec::with_capacity(crate::world::View::ALL.len());
    for view in crate::world::View::ALL {
        let rich_base = rich_tile_version(&tile_version, view);
        let rich_dir = tile_cache_dir(&rich_base);
        let rich_version = if pyramid::validate_complete(&rich_dir).is_ok() {
            rich_base
        } else {
            let rich_version = available_tile_version(&rich_base)?;
            let rich_dir = tile_cache_dir(&rich_version);
            let staging = tile_root.join(format!(".{rich_version}.building"));
            pyramid::build_rich(world, aerial, mesh_textures, view, &staging)?;
            std::fs::rename(&staging, &rich_dir)?;
            rich_version
        };
        rich_versions.push((view, rich_version));
    }
    let scene = Scene::from_world(world, land_cover_sha256, tile_version, &rich_versions)?;
    scene.write_current()
}

pub fn prebuild_is_complete(world_sha256: &[u8; 32], land_cover_sha256: Option<&[u8; 32]>) -> bool {
    let base_version = base_tile_version(world_sha256, land_cover_sha256);
    let Ok(current) = Scene::read_current() else {
        return false;
    };
    if !current.matches_inputs(world_sha256, land_cover_sha256)
        || !is_generation_of(&current.tile_version, &base_version)
        || pyramid::validate_complete(&tile_cache_dir(&current.tile_version)).is_err()
        || current
            .rich_views()
            .iter()
            .zip(crate::world::View::ALL)
            .any(|(rich, view)| {
                let expected = rich_tile_version(&current.tile_version, view);
                !is_generation_of(rich.tile_version(), &expected)
            })
        || current
            .rich_views()
            .iter()
            .any(|view| pyramid::validate_complete(&tile_cache_dir(view.tile_version())).is_err())
    {
        return false;
    }
    println!("tile pyramid already complete: {}", current.tile_version);
    true
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

async fn coverage(State(state): State<AppState>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        state.coverage_json.as_ref().clone(),
    )
}

async fn rich_coverage(
    State(state): State<AppState>,
    AxumPath(view): AxumPath<String>,
) -> Response {
    let Some(tiles) = state.rich_tiles.get(&view) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        tiles.coverage_json.clone(),
    )
        .into_response()
}

async fn tile(
    State(state): State<AppState>,
    AxumPath((z, x, y)): AxumPath<(u8, u32, String)>,
) -> Response {
    serve_tile(
        &state.tile_dir,
        &state.tile_inventory,
        &state.blank_tile,
        z,
        x,
        &y,
        Instant::now(),
    )
    .await
}

async fn serve_tile(
    tile_dir: &std::path::Path,
    tile_inventory: &TileInventory,
    blank_tile: &[u8],
    z: u8,
    x: u32,
    y: &str,
    started: Instant,
) -> Response {
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
    let path = tile_path(tile_dir, z, x, y);
    let expected_bytes = tile_inventory.expected_bytes(z, x, y);
    if expected_bytes.is_none() {
        return logged_image(blank_tile.to_vec(), "empty", z, x, y, started);
    }
    match tokio::fs::read(&path).await {
        Ok(image) if tile_inventory.matches(z, x, y, &image) => {
            logged_image(image, "disk", z, x, y, started)
        }
        Ok(image) => {
            warn!(
                z,
                x,
                y,
                expected_bytes,
                bytes = image.len(),
                "tile size does not match inventory"
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            warn!(z, x, y, expected_bytes, "expected tile is missing");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(error) => {
            warn!(?error, path = %path.display(), "tile cache read failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn rich_tile(
    State(state): State<AppState>,
    AxumPath((view, z, x, y)): AxumPath<(String, u8, u32, String)>,
) -> Response {
    let started = Instant::now();
    let Some(tiles) = state.rich_tiles.get(&view) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    serve_tile(
        &tiles.tile_dir,
        &tiles.inventory,
        &state.blank_tile,
        z,
        x,
        &y,
        started,
    )
    .await
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

fn available_tile_version(base: &str) -> io::Result<String> {
    let mut revision = 0_u32;
    loop {
        let candidate = if revision == 0 {
            base.to_owned()
        } else {
            format!("{base}-r{revision}")
        };
        let final_path = tile_cache_dir(&candidate);
        if !final_path.exists() {
            return Ok(candidate);
        }
        revision = revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("tile revision number exhausted"))?;
    }
}

fn tile_cache_dir(tile_version: &str) -> PathBuf {
    PathBuf::from("data/tiles").join(tile_version)
}
