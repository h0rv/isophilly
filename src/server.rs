use std::{
    fs, io,
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use tokio::sync::{Mutex, Semaphore};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::{
    mesh_texture::MeshTextureSource,
    pyramid::{self, ART_ZOOM, tile_path},
    render::{render_blank_tile, render_tile},
    texture::{AerialSource, AerialTile},
    world::{World, isometric},
};

#[derive(Clone)]
struct AppState {
    world: Arc<World>,
    aerial: Arc<AerialSource>,
    mesh_textures: Arc<MeshTextureSource>,
    tile_dir: PathBuf,
    tile_version: String,
    blank_tile: Arc<Vec<u8>>,
    render_slots: Arc<Semaphore>,
    render_locks: Arc<Vec<Arc<Mutex<()>>>>,
}

#[derive(Serialize)]
struct Meta {
    iso_bounds: [f32; 4],
    city_hall: Option<[f32; 2]>,
    landmarks: Vec<Landmark>,
    counts: Counts,
    tile_version: String,
    max_zoom: u8,
    home_zoom: u8,
}

#[derive(Serialize)]
struct Landmark {
    name: &'static str,
    point: [f32; 2],
    min_zoom: u8,
    color: &'static str,
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

const RENDER_VERSION: &str = "v22-real-facades";
const MAX_ZOOM: u8 = 12;
const HOME_ZOOM: u8 = 3;
const RENDER_LOCKS: usize = 64;
const ROCKY_SOURCE: (f32, f32, f32) = (819_514.06, 73_343.64, 15.0);
static TEMPORARY_TILE_ID: AtomicU64 = AtomicU64::new(0);

pub async fn serve(
    world: Arc<World>,
    aerial: Arc<AerialSource>,
    mesh_textures: Arc<MeshTextureSource>,
    port: u16,
) -> io::Result<()> {
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
        mesh_textures,
        tile_dir,
        tile_version,
        blank_tile: Arc::new(render_blank_tile()?),
        render_slots: Arc::new(Semaphore::new(render_workers())),
        render_locks: Arc::new(
            (0..RENDER_LOCKS)
                .map(|_| Arc::new(Mutex::new(())))
                .collect(),
        ),
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

pub fn prebuild(
    world: &World,
    aerial: &AerialSource,
    mesh_textures: &MeshTextureSource,
) -> io::Result<()> {
    let tile_dir = tile_cache_dir(&tile_version(world));
    pyramid::build(world, aerial, mesh_textures, &tile_dir)
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
    let rocky = isometric(ROCKY_SOURCE.0, ROCKY_SOURCE.1, ROCKY_SOURCE.2);
    Json(Meta {
        iso_bounds: [bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y],
        city_hall: state.world.city_hall_focus(),
        landmarks: vec![Landmark {
            name: "Rocky",
            point: [rocky.0, rocky.1],
            min_zoom: 7,
            color: "#8f5f3b",
        }],
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
        Ok((png, cache)) => logged_png(png, cache, z, x, y, started),
        Err(status) => status.into_response(),
    }
}

async fn render_requested_tile(
    state: &AppState,
    path: &std::path::Path,
    coord: TileCoord,
    started: Instant,
) -> Result<(Vec<u8>, &'static str), StatusCode> {
    let TileCoord { z, x, y } = coord;
    let queued = Instant::now();
    let lock = Arc::clone(&state.render_locks[tile_lock_index(coord)]);
    let tile_guard = lock.lock_owned().await;
    match tokio::fs::read(path).await {
        Ok(png) => return Ok((png, "disk")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(?error, path = %path.display(), "tile cache read failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    let render_slot = Arc::clone(&state.render_slots)
        .acquire_owned()
        .await
        .map_err(|error| {
            warn!(?error, z, x, y, "tile render queue closed");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    let queue_ms = queued.elapsed().as_millis();
    let rendering = Instant::now();
    let world = Arc::clone(&state.world);
    let aerial = Arc::clone(&state.aerial);
    let mesh_textures = Arc::clone(&state.mesh_textures);
    let path = path.to_owned();
    let rendered = tokio::task::spawn_blocking(move || {
        let _tile_guard = tile_guard;
        let _render_slot = render_slot;
        let rendered = render(&world, &aerial, &mesh_textures, coord)?;
        write_tile_atomically(&path, &rendered)?;
        Ok::<_, io::Error>(rendered)
    })
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
    info!(
        z,
        x,
        y,
        queue_ms,
        render_ms,
        elapsed_ms = started.elapsed().as_millis(),
        "tile rendered"
    );
    Ok((rendered, "rendered"))
}

fn tile_lock_index(coord: TileCoord) -> usize {
    let TileCoord { z, x, y } = coord;
    x.wrapping_mul(31)
        .wrapping_add(y.wrapping_mul(17))
        .wrapping_add(u32::from(z)) as usize
        % RENDER_LOCKS
}

fn write_tile_atomically(path: &FsPath, png: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tile cache path has no parent"))?;
    fs::create_dir_all(parent)?;
    let id = TEMPORARY_TILE_ID.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("png.part-{}-{id}", std::process::id()));
    fs::write(&temporary, png)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            let _removed = fs::remove_file(temporary);
            Ok(())
        }
        Err(error) => {
            let _removed = fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn render(
    world: &World,
    aerial: &AerialSource,
    mesh_textures: &MeshTextureSource,
    coord: TileCoord,
) -> io::Result<Vec<u8>> {
    let TileCoord { z, x, y } = coord;
    let bounds = world.iso_bounds.tile(z, x, y);
    let aerial = AerialTile::for_isometric_tile(aerial, bounds, z, x, y)?;
    render_tile(world, &aerial, mesh_textures, z, x, y)
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
