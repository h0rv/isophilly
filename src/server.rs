use std::{fs, io, path::PathBuf, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rayon::prelude::*;
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::{
    render::{render_blank_tile, render_tile},
    world::World,
};

#[derive(Clone)]
struct AppState {
    world: Arc<World>,
    tile_dir: PathBuf,
    tile_version: String,
    blank_tile: Arc<Vec<u8>>,
    render_slots: Arc<Semaphore>,
}
#[derive(Serialize)]
struct Meta {
    iso_bounds: [f32; 4],
    city_hall: [f32; 2],
    counts: Counts,
    tile_version: String,
    max_zoom: u8,
    home_zoom: u8,
}
#[derive(Serialize)]
struct Counts {
    buildings: usize,
    water: usize,
    parks: usize,
    streets: usize,
}

const WARM_ZOOM: u8 = 4;
const CITY_HALL: [f32; 2] = [748_854.06, 446_419.38];
const RENDER_VERSION: &str = "v4";
const MAX_ZOOM: u8 = 12;
const HOME_ZOOM: u8 = 3;
const PERSIST_MAX_ZOOM: u8 = 8;

pub async fn serve(world: Arc<World>, port: u16) -> io::Result<()> {
    let warm_world = Arc::clone(&world);
    let tile_version = tile_version(&world);
    let tile_dir = tile_cache_dir(&tile_version);
    let warm_tile_dir = tile_dir.clone();
    let state = AppState {
        world,
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
    let warm_task = tokio::task::spawn_blocking(move || {
        warm_missing_tiles(&warm_world, &warm_tile_dir, WARM_ZOOM);
    });
    drop(warm_task);
    axum::serve(listener, app).await.map_err(io::Error::other)
}
pub fn prebuild(world: &World, max_zoom: u8) -> io::Result<()> {
    let tile_dir = tile_cache_dir(&tile_version(world));
    for z in 0..=max_zoom {
        let count = 1_u32 << z;
        (0..count).into_par_iter().try_for_each(|y| {
            (0..count).try_for_each(|x| cache_tile(world, &tile_dir, z, x, y, true).map(|_| ()))
        })?;
        println!("prebuilt z{z}");
    }
    Ok(())
}
async fn index() -> Result<impl IntoResponse, StatusCode> {
    tokio::fs::read("static/index.html")
        .await
        .map(|body| {
            (
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                body,
            )
        })
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn app_js() -> Result<impl IntoResponse, StatusCode> {
    tokio::fs::read("static/app.js")
        .await
        .map(|body| {
            (
                [
                    (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
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
        city_hall: CITY_HALL,
        counts: Counts {
            buildings: state.world.buildings.len(),
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
    let tile_bounds = state.world.iso_bounds.tile(z, x, y);
    if !state.world.has_content(&tile_bounds.source_envelope()) {
        return png_response(state.blank_tile.as_ref().clone(), "empty");
    }
    let path = state
        .tile_dir
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"));
    let cached = if should_persist(z) {
        Some(tokio::fs::read(&path).await)
    } else {
        None
    };
    let (png, cache) = match cached {
        Some(Ok(png)) => (png, "disk"),
        Some(Err(error)) => {
            if error.kind() != io::ErrorKind::NotFound {
                warn!(?error, path = %path.display(), "tile cache read failed");
            }
            match render_requested_tile(&state, &path, z, x, y, started).await {
                Ok(rendered) => rendered,
                Err(status) => return status.into_response(),
            }
        }
        None => match render_requested_tile(&state, &path, z, x, y, started).await {
            Ok(rendered) => rendered,
            Err(status) => return status.into_response(),
        },
    };
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

async fn render_requested_tile(
    state: &AppState,
    path: &std::path::Path,
    z: u8,
    x: u32,
    y: u32,
    started: Instant,
) -> Result<(Vec<u8>, &'static str), StatusCode> {
    let queued = Instant::now();
    let render_slot = match state.render_slots.acquire().await {
        Ok(slot) => slot,
        Err(error) => {
            warn!(?error, z, x, y, "tile render queue closed");
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    let world = Arc::clone(&state.world);
    let png = match tokio::task::spawn_blocking(move || render_tile(&world, z, x, y)).await {
        Ok(Ok(png)) => png,
        Ok(Err(error)) => {
            warn!(?error, z, x, y, "tile render failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(error) => {
            warn!(?error, z, x, y, "tile worker failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let queue_ms = queued.elapsed().as_millis();
    drop(render_slot);
    if should_persist(z)
        && let Some(parent) = path.parent()
    {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            warn!(?error, path = %parent.display(), "tile cache directory failed");
        } else if let Err(error) = tokio::fs::write(path, &png).await {
            warn!(?error, path = %path.display(), "tile cache write failed");
        }
    }
    info!(
        z,
        x,
        y,
        queue_ms,
        elapsed_ms = started.elapsed().as_millis(),
        "tile rendered"
    );
    Ok((
        png,
        if should_persist(z) {
            "rendered"
        } else {
            "volatile"
        },
    ))
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

fn warm_missing_tiles(world: &World, tile_dir: &std::path::Path, max_zoom: u8) {
    let started = Instant::now();
    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(render_workers().min(2))
        .build()
    {
        Ok(pool) => pool,
        Err(error) => {
            warn!(?error, "tile cache warmer could not start");
            return;
        }
    };
    let mut rendered = 0_u32;
    for z in 0..=max_zoom {
        let count = 1_u32 << z;
        let level_rendered: u32 = pool.install(|| {
            (0..count)
                .into_par_iter()
                .map(|y| {
                    (0..count)
                        .filter_map(|x| match cache_tile(world, tile_dir, z, x, y, false) {
                            Ok(rendered) => Some(u32::from(rendered)),
                            Err(error) => {
                                warn!(?error, z, x, y, "tile warm failed");
                                None
                            }
                        })
                        .sum::<u32>()
                })
                .sum()
        });
        rendered += level_rendered;
    }
    info!(
        rendered,
        elapsed_ms = started.elapsed().as_millis(),
        "tile cache warm complete"
    );
}

fn cache_tile(
    world: &World,
    tile_dir: &std::path::Path,
    z: u8,
    x: u32,
    y: u32,
    overwrite: bool,
) -> io::Result<bool> {
    let path = tile_path(tile_dir, z, x, y);
    if !overwrite && path.exists() {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tile path has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::write(path, render_tile(world, z, x, y)?)?;
    Ok(true)
}

fn render_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |count| count.get().clamp(1, 8))
}

const fn should_persist(z: u8) -> bool {
    z <= PERSIST_MAX_ZOOM
}

fn tile_path(tile_dir: &std::path::Path, z: u8, x: u32, y: u32) -> PathBuf {
    tile_dir
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"))
}

fn tile_version(world: &World) -> String {
    format!("{RENDER_VERSION}-{:016x}", world.data_version)
}

fn tile_cache_dir(tile_version: &str) -> PathBuf {
    PathBuf::from("data/tiles").join(tile_version)
}

#[cfg(test)]
mod tests {
    use super::{PERSIST_MAX_ZOOM, should_persist};

    #[test]
    fn deep_zoom_tiles_do_not_fill_the_disk_cache() {
        assert!(should_persist(PERSIST_MAX_ZOOM));
        assert!(!should_persist(PERSIST_MAX_ZOOM + 1));
    }
}
