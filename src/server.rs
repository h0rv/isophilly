use std::{io, path::PathBuf, sync::Arc, time::Instant};

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
    world: Arc<World>,
    tile_dir: PathBuf,
    tile_version: String,
    blank_tile: Arc<Vec<u8>>,
    pub(crate) live_city: Arc<LiveCity>,
}

#[derive(Serialize)]
struct Meta {
    iso_bounds: [f32; 4],
    city_hall: Option<[f32; 2]>,
    landmarks: Vec<Landmark>,
    counts: Counts,
    tile_version: String,
    max_tile_zoom: u8,
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

const PYRAMID_VERSION: &str = "v34-stable-aerial-facades";
const MAX_VIEW_ZOOM: u8 = 10;
const HOME_ZOOM: u8 = 3;
const ROCKY_SOURCE: (f32, f32, f32) = (819_514.06, 73_343.64, 15.0);

pub async fn serve(world: Arc<World>, port: u16) -> io::Result<()> {
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
        tile_dir,
        tile_version,
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
    let tile_dir = tile_cache_dir(&tile_version(world));
    pyramid::build(world, aerial, mesh_textures, &tile_dir)
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
        max_tile_zoom: ART_ZOOM,
        max_zoom: MAX_VIEW_ZOOM,
        home_zoom: HOME_ZOOM,
    })
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
