use std::{fs::OpenOptions, io, path::PathBuf, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::{
    live_city::{LiveCity, vehicles},
    mesh_texture::MeshTextureSource,
    pyramid::{self, ART_ZOOM, TileInventory, tile_path},
    render::render_blank_tile,
    scene::Scene,
    texture::AerialSource,
    tile_codec::{EXTENSION, MEDIA_TYPE},
    world::World,
};

#[derive(Clone)]
pub(crate) struct AppState {
    scene: Arc<Scene>,
    tile_dir: PathBuf,
    tile_inventory: Arc<TileInventory>,
    blank_tile: Arc<Vec<u8>>,
    pub(crate) live_city: Arc<LiveCity>,
}

const PYRAMID_VERSION: &str = "v39-texture-first-footprints";

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
    let state = AppState {
        scene,
        tile_dir,
        tile_inventory,
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
    let base_version = tile_version(world);
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
    if prebuild_is_complete(&world.world_sha256) {
        return Ok(());
    }

    let base_dir = tile_cache_dir(&base_version);
    if pyramid::validate_complete(&base_dir).is_ok() {
        Scene::from_world(world, base_version)?.write_current()?;
        println!("published existing tile pyramid");
        return Ok(());
    }

    let tile_version = available_tile_version(&base_version)?;
    let tile_dir = tile_cache_dir(&tile_version);
    let staging = tile_root.join(format!(".{tile_version}.building"));
    pyramid::build(world, aerial, mesh_textures, &staging)?;
    std::fs::rename(&staging, &tile_dir)?;
    let scene = Scene::from_world(world, tile_version)?;
    scene.write_current()
}

pub fn prebuild_is_complete(world_sha256: &[u8; 32]) -> bool {
    let base_version = tile_version_for_digest(world_sha256);
    let Ok(current) = Scene::read_current() else {
        return false;
    };
    if !current.matches_world(world_sha256)
        || !is_generation_of(&current.tile_version, &base_version)
        || pyramid::validate_complete(&tile_cache_dir(&current.tile_version)).is_err()
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
    let expected_bytes = state.tile_inventory.expected_bytes(z, x, y);
    if expected_bytes.is_none() {
        return logged_image(state.blank_tile.as_ref().clone(), "empty", z, x, y, started);
    }
    match tokio::fs::read(&path).await {
        Ok(image) if state.tile_inventory.matches(z, x, y, &image) => {
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
    tile_version_for_digest(&world.world_sha256)
}

fn tile_version_for_digest(world_sha256: &[u8; 32]) -> String {
    let suffix = world_sha256[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{PYRAMID_VERSION}-{suffix}")
}

fn is_generation_of(candidate: &str, base: &str) -> bool {
    candidate == base
        || candidate
            .strip_prefix(base)
            .and_then(|suffix| suffix.strip_prefix("-r"))
            .is_some_and(|revision| {
                !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit())
            })
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

#[cfg(test)]
mod tests {
    use super::is_generation_of;

    #[test]
    fn tile_generations_are_exact() {
        assert!(is_generation_of("v1-abc", "v1-abc"));
        assert!(is_generation_of("v1-abc-r2", "v1-abc"));
        assert!(!is_generation_of("v1-abc-r", "v1-abc"));
        assert!(!is_generation_of("v1-abcd", "v1-abc"));
    }
}
