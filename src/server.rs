use std::{io, path::PathBuf, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::{
    pyramid::{self, ART_ZOOM, tile_path},
    render::{render_blank_tile, render_tile},
    texture::{AerialSource, AerialTile},
    world::World,
};

#[derive(Clone)]
struct AppState {
    world: Arc<World>,
    aerial: Arc<AerialSource>,
    tile_dir: PathBuf,
    tile_version: String,
    blank_tile: Arc<Vec<u8>>,
    render_slots: Arc<Semaphore>,
}

#[derive(Serialize)]
struct Meta {
    iso_bounds: [f32; 4],
    city_hall: Option<[f32; 2]>,
    counts: Counts,
    tile_version: String,
    max_zoom: u8,
    home_zoom: u8,
}

#[derive(Serialize)]
struct Counts {
    buildings: usize,
    building_parts: usize,
    building_meshes: usize,
    water: usize,
    parks: usize,
    streets: usize,
}

#[derive(Clone, Copy)]
struct TileCoord {
    z: u8,
    x: u32,
    y: u32,
}

const RENDER_VERSION: &str = "v21-seamless-pyramid";
const MAX_ZOOM: u8 = 12;
const HOME_ZOOM: u8 = 3;

pub async fn serve(world: Arc<World>, aerial: Arc<AerialSource>, port: u16) -> io::Result<()> {
    let tile_version = tile_version(&world);
    let tile_dir = tile_cache_dir(&tile_version);
    if !pyramid::is_complete(&tile_dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the textured tile pyramid is missing; run `uv run poe prebuild` first",
        ));
    }
    let state = AppState {
        world,
        aerial,
        tile_dir,
        tile_version,
        blank_tile: Arc::new(render_blank_tile()?),
        render_slots: Arc::new(Semaphore::new(render_workers())),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
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

pub fn prebuild(world: &World, aerial: &AerialSource) -> io::Result<()> {
    let tile_dir = tile_cache_dir(&tile_version(world));
    pyramid::build(world, aerial, &tile_dir)
}

async fn index() -> Result<impl IntoResponse, StatusCode> {
    static_file("static/index.html", "text/html; charset=utf-8").await
}

async fn app_js() -> Result<impl IntoResponse, StatusCode> {
    static_file("static/app.js", "text/javascript; charset=utf-8").await
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

async fn meta(State(state): State<AppState>) -> Json<Meta> {
    let bounds = state.world.iso_bounds;
    Json(Meta {
        iso_bounds: [bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y],
        city_hall: state.world.city_hall_focus(),
        counts: Counts {
            buildings: state.world.buildings.len(),
            building_parts: state.world.building_parts.len(),
            building_meshes: state.world.building_meshes.len(),
            water: state.world.water.len(),
            parks: state.world.parks.len(),
            streets: state.world.streets.len(),
        },
        tile_version: state.tile_version.clone(),
        max_zoom: MAX_ZOOM,
        home_zoom: HOME_ZOOM,
    })
}

async fn tile(
    State(state): State<AppState>,
    AxumPath((z, x, y)): AxumPath<(u8, u32, String)>,
) -> Response {
    let started = Instant::now();
    let Ok(y) = y.trim_end_matches(".png").parse::<u32>() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if z > MAX_ZOOM || x >= 1 << z || y >= 1 << z {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = tile_path(&state.tile_dir, z, x, y);
    match tokio::fs::read(&path).await {
        Ok(png) => return logged_png(png, "disk", z, x, y, started),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(?error, path = %path.display(), "tile cache read failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if z <= ART_ZOOM {
        return logged_png(state.blank_tile.as_ref().clone(), "empty", z, x, y, started);
    }
    let bounds = state.world.iso_bounds.tile(z, x, y);
    if !state
        .world
        .has_content(&state.world.source_envelope(bounds))
    {
        return logged_png(state.blank_tile.as_ref().clone(), "empty", z, x, y, started);
    }
    match render_requested_tile(&state, &path, TileCoord { z, x, y }, started).await {
        Ok(png) => logged_png(png, "rendered", z, x, y, started),
        Err(status) => status.into_response(),
    }
}

async fn render_requested_tile(
    state: &AppState,
    path: &std::path::Path,
    coord: TileCoord,
    started: Instant,
) -> Result<Vec<u8>, StatusCode> {
    let TileCoord { z, x, y } = coord;
    let queued = Instant::now();
    let render_slot = state.render_slots.acquire().await.map_err(|error| {
        warn!(?error, z, x, y, "tile render queue closed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let queue_ms = queued.elapsed().as_millis();
    let rendering = Instant::now();
    let world = Arc::clone(&state.world);
    let aerial = Arc::clone(&state.aerial);
    let rendered = tokio::task::spawn_blocking(move || render(&world, &aerial, coord))
        .await
        .map_err(|error| {
            warn!(?error, z, x, y, "tile worker failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|error| {
            warn!(?error, z, x, y, "textured tile unavailable");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    let render_ms = rendering.elapsed().as_millis();
    drop(render_slot);
    if let Some(parent) = path.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent).await
    {
        warn!(?error, path = %parent.display(), "tile cache directory failed");
    } else if let Err(error) = tokio::fs::write(path, &rendered).await {
        warn!(?error, path = %path.display(), "tile cache write failed");
    }
    info!(
        z,
        x,
        y,
        queue_ms,
        render_ms,
        elapsed_ms = started.elapsed().as_millis(),
        "tile rendered"
    );
    Ok(rendered)
}

fn render(world: &World, aerial: &AerialSource, coord: TileCoord) -> io::Result<Vec<u8>> {
    let TileCoord { z, x, y } = coord;
    let bounds = world.iso_bounds.tile(z, x, y);
    let aerial = AerialTile::for_isometric_tile(aerial, bounds, z, x, y)?;
    render_tile(world, &aerial, z, x, y)
}

fn logged_png(
    png: Vec<u8>,
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
    png_response(png, cache)
}

fn png_response(png: Vec<u8>, cache: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::HeaderName::from_static("x-tile-cache"), cache),
        ],
        png,
    )
        .into_response()
}

fn render_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |count| count.get().clamp(1, 8))
}

fn tile_version(world: &World) -> String {
    format!("{RENDER_VERSION}-{:016x}", world.data_version)
}

fn tile_cache_dir(tile_version: &str) -> PathBuf {
    PathBuf::from("data/tiles").join(tile_version)
}
