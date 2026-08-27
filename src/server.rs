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
    texture::{AerialSource, AerialTile, TextureMode},
    world::World,
};

#[derive(Clone)]
struct AppState {
    world: Arc<World>,
    aerial: Option<Arc<AerialSource>>,
    texture: TextureMode,
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
    texture: TextureMode,
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

struct RenderedTile {
    png: Vec<u8>,
    cacheable: bool,
}

const PLAIN_WARM_ZOOM: u8 = 4;
const TEXTURED_WARM_ZOOM: u8 = 2;
const RENDER_VERSION: &str = "v16";
const MAX_ZOOM: u8 = 12;
const HOME_ZOOM: u8 = 3;
const PERSIST_MAX_ZOOM: u8 = 8;
const TEXTURED_CONTEXT_MAX_ZOOM: u8 = 5;

pub async fn serve(
    world: Arc<World>,
    aerial: Option<Arc<AerialSource>>,
    texture: TextureMode,
    port: u16,
) -> io::Result<()> {
    let warm_world = Arc::clone(&world);
    let warm_aerial = aerial.clone();
    let tile_version = tile_version(&world, texture);
    let tile_dir = tile_cache_dir(&tile_version);
    let warm_tile_dir = tile_dir.clone();
    let state = AppState {
        world,
        aerial,
        texture,
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
    println!(
        "geo-philly http://127.0.0.1:{port} texture={}",
        texture.slug()
    );
    let warm_task = tokio::task::spawn_blocking(move || {
        let warm_zoom = if texture == TextureMode::None {
            PLAIN_WARM_ZOOM
        } else {
            TEXTURED_WARM_ZOOM
        };
        warm_missing_tiles(
            &warm_world,
            warm_aerial.as_deref(),
            texture,
            &warm_tile_dir,
            warm_zoom,
        );
    });
    drop(warm_task);
    axum::serve(listener, app).await.map_err(io::Error::other)
}
pub fn prebuild(
    world: &World,
    aerial: Option<&AerialSource>,
    texture: TextureMode,
    max_zoom: u8,
) -> io::Result<()> {
    let tile_dir = tile_cache_dir(&tile_version(world, texture));
    for z in 0..=max_zoom {
        let count = 1_u32 << z;
        (0..count).into_par_iter().try_for_each(|y| {
            (0..count).try_for_each(|x| {
                cache_tile(
                    world,
                    aerial,
                    texture,
                    &tile_dir,
                    TileCoord { z, x, y },
                    true,
                )
                .map(|_| ())
            })
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
        texture: state.texture,
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
    let coord = TileCoord { z, x, y };
    let tile_bounds = state.world.iso_bounds.tile(z, x, y);
    let has_content = state
        .world
        .has_content(&state.world.source_envelope(tile_bounds));
    let render_context = should_render_context(state.texture, z, has_content);
    if !has_content && !render_context {
        return png_response(state.blank_tile.as_ref().clone(), "empty");
    }
    let path = state
        .tile_dir
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"));
    let persist = should_persist_tile(z, has_content, render_context);
    let cached = if persist {
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
            match render_requested_tile(&state, &path, coord, render_context, persist, started)
                .await
            {
                Ok(rendered) => rendered,
                Err(status) => return status.into_response(),
            }
        }
        None => match render_requested_tile(&state, &path, coord, render_context, persist, started)
            .await
        {
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
    coord: TileCoord,
    render_context: bool,
    persist: bool,
    started: Instant,
) -> Result<(Vec<u8>, &'static str), StatusCode> {
    let TileCoord { z, x, y } = coord;
    let queued = Instant::now();
    let render_slot = match state.render_slots.acquire().await {
        Ok(slot) => slot,
        Err(error) => {
            warn!(?error, z, x, y, "tile render queue closed");
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    let queue_ms = queued.elapsed().as_millis();
    let rendering = Instant::now();
    let world = Arc::clone(&state.world);
    let aerial = state.aerial.clone();
    let texture = state.texture;
    let rendered = match tokio::task::spawn_blocking(move || {
        render(
            world.as_ref(),
            aerial.as_deref(),
            texture,
            coord,
            render_context,
        )
    })
    .await
    {
        Ok(Ok(rendered)) => rendered,
        Ok(Err(error)) => {
            warn!(?error, z, x, y, "tile render failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(error) => {
            warn!(?error, z, x, y, "tile worker failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let render_ms = rendering.elapsed().as_millis();
    drop(render_slot);
    let persist = persist && rendered.cacheable;
    if persist && let Some(parent) = path.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            warn!(?error, path = %parent.display(), "tile cache directory failed");
        } else if let Err(error) = tokio::fs::write(path, &rendered.png).await {
            warn!(?error, path = %path.display(), "tile cache write failed");
        }
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
    let cache = if !rendered.cacheable {
        "degraded"
    } else if persist {
        "rendered"
    } else {
        "volatile"
    };
    Ok((rendered.png, cache))
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

fn warm_missing_tiles(
    world: &World,
    aerial: Option<&AerialSource>,
    texture: TextureMode,
    tile_dir: &std::path::Path,
    max_zoom: u8,
) {
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
                        .filter_map(|x| {
                            match cache_tile(
                                world,
                                aerial,
                                texture,
                                tile_dir,
                                TileCoord { z, x, y },
                                false,
                            ) {
                                Ok(rendered) => Some(u32::from(rendered)),
                                Err(error) => {
                                    warn!(?error, z, x, y, "tile warm failed");
                                    None
                                }
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
    aerial: Option<&AerialSource>,
    texture: TextureMode,
    tile_dir: &std::path::Path,
    coord: TileCoord,
    overwrite: bool,
) -> io::Result<bool> {
    let bounds = world.iso_bounds.tile(coord.z, coord.x, coord.y);
    let has_content = world.has_content(&world.source_envelope(bounds));
    let render_context = should_render_context(texture, coord.z, has_content);
    if !has_content && !render_context {
        return Ok(false);
    }
    let path = tile_path(tile_dir, coord.z, coord.x, coord.y);
    if !overwrite && path.exists() {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tile path has no parent"))?;
    fs::create_dir_all(parent)?;
    let rendered = render(world, aerial, texture, coord, render_context)?;
    if !rendered.cacheable {
        return Ok(false);
    }
    fs::write(path, rendered.png)?;
    Ok(true)
}

fn render(
    world: &World,
    aerial: Option<&AerialSource>,
    texture: TextureMode,
    coord: TileCoord,
    render_context: bool,
) -> io::Result<RenderedTile> {
    let TileCoord { z, x, y } = coord;
    let bounds = world.iso_bounds.tile(z, x, y);
    if !render_context && !world.has_content(&world.source_envelope(bounds)) {
        return render_blank_tile().map(|png| RenderedTile {
            png,
            cacheable: true,
        });
    }
    let aerial_tile = match aerial
        .map(|source| AerialTile::for_isometric_tile(source, bounds, z, x, y))
        .transpose()
    {
        Ok(tile) => tile,
        Err(error) => {
            warn!(
                ?error,
                z, x, y, "aerial tile unavailable; rendering geometry only"
            );
            None
        }
    };
    let cacheable = aerial.is_none() || aerial_tile.is_some();
    render_tile(world, aerial_tile.as_ref(), texture, z, x, y)
        .map(|png| RenderedTile { png, cacheable })
}

fn render_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |count| count.get().clamp(1, 8))
}

const fn should_persist(z: u8) -> bool {
    z <= PERSIST_MAX_ZOOM
}

const fn should_persist_tile(z: u8, has_content: bool, render_context: bool) -> bool {
    should_persist(z) && (has_content || render_context)
}

fn should_render_context(texture: TextureMode, z: u8, has_content: bool) -> bool {
    !has_content && texture != TextureMode::None && z <= TEXTURED_CONTEXT_MAX_ZOOM
}

fn tile_path(tile_dir: &std::path::Path, z: u8, x: u32, y: u32) -> PathBuf {
    tile_dir
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"))
}

fn tile_version(world: &World, texture: TextureMode) -> String {
    format!(
        "{RENDER_VERSION}-{}-{:016x}",
        texture.slug(),
        world.data_version
    )
}

fn tile_cache_dir(tile_version: &str) -> PathBuf {
    PathBuf::from("data/tiles").join(tile_version)
}

#[cfg(test)]
mod tests {
    use super::{
        PERSIST_MAX_ZOOM, TEXTURED_CONTEXT_MAX_ZOOM, should_persist, should_persist_tile,
        should_render_context,
    };
    use crate::texture::TextureMode;

    #[test]
    fn deep_zoom_tiles_do_not_fill_the_disk_cache() {
        assert!(should_persist(PERSIST_MAX_ZOOM));
        assert!(!should_persist(PERSIST_MAX_ZOOM + 1));
    }

    #[test]
    fn textured_context_tiles_are_prebuilt_and_persisted() {
        let z = TEXTURED_CONTEXT_MAX_ZOOM;
        let render_context = should_render_context(TextureMode::Pixel, z, false);

        assert!(render_context);
        assert!(should_persist_tile(z, false, render_context));
        assert!(!should_render_context(TextureMode::None, z, false));
        assert!(!should_render_context(TextureMode::Pixel, z + 1, false));
    }
}
