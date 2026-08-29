use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;

use crate::server::AppState;

const SURFACE_URL: &str = "https://www3.septa.org/api/TransitViewAll/index.php";
const RAIL_URL: &str = "https://www3.septa.org/api/TrainView/index.php";
const CACHE_TTL: Duration = Duration::from_secs(15);

pub(crate) struct LiveCity {
    client: Client,
    cache: Mutex<Option<CachedSnapshot>>,
}

struct CachedSnapshot {
    fetched: Instant,
    snapshot: Arc<VehicleSnapshot>,
}

#[derive(Clone, Serialize)]
pub(crate) struct VehicleSnapshot {
    updated_at: u64,
    stale: bool,
    vehicles: Vec<Vehicle>,
}

#[derive(Clone, Serialize)]
struct Vehicle {
    id: String,
    mode: VehicleMode,
    route: String,
    label: String,
    destination: String,
    latitude: f64,
    longitude: f64,
    heading: f64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum VehicleMode {
    Surface,
    RegionalRail,
}

#[derive(Deserialize)]
struct SurfaceResponse {
    routes: Vec<HashMap<String, Vec<SurfaceVehicle>>>,
}

#[derive(Deserialize)]
struct SurfaceVehicle {
    #[serde(rename = "VehicleID")]
    vehicle_id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    destination: String,
    lat: String,
    lng: String,
    #[serde(default)]
    heading: f64,
}

#[derive(Deserialize)]
struct RailVehicle {
    trainno: String,
    #[serde(default)]
    line: String,
    #[serde(default)]
    dest: String,
    lat: String,
    lon: String,
    #[serde(default, deserialize_with = "number_from_string")]
    heading: f64,
}

fn number_from_string<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| D::Error::custom("heading is not finite")),
        serde_json::Value::String(number) => number.parse().map_err(D::Error::custom),
        _ => Err(D::Error::custom("heading is neither a number nor a string")),
    }
}

impl LiveCity {
    pub(crate) fn new() -> io::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(6))
            .user_agent("geo-philly/0.1 (+https://github.com/; SEPTA live map)")
            .build()
            .map_err(io::Error::other)?;
        Ok(Self {
            client,
            cache: Mutex::new(None),
        })
    }

    async fn snapshot(&self) -> Result<(Arc<VehicleSnapshot>, &'static str), String> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.fetched.elapsed() < CACHE_TTL
        {
            let state = if cached.snapshot.stale {
                "stale"
            } else {
                "fresh"
            };
            return Ok((Arc::clone(&cached.snapshot), state));
        }

        let (surface, rail) = tokio::join!(self.fetch_surface(), self.fetch_rail());
        let mut vehicles = Vec::new();
        let mut failures = Vec::new();
        match surface {
            Ok(mut current) => vehicles.append(&mut current),
            Err(error) => failures.push(error),
        }
        match rail {
            Ok(mut current) => vehicles.append(&mut current),
            Err(error) => failures.push(error),
        }

        if vehicles.is_empty() && !failures.is_empty() {
            if let Some(cached) = cache.as_ref() {
                let mut stale = cached.snapshot.as_ref().clone();
                stale.stale = true;
                let snapshot = Arc::new(stale);
                *cache = Some(CachedSnapshot {
                    fetched: Instant::now(),
                    snapshot: Arc::clone(&snapshot),
                });
                return Ok((snapshot, "stale"));
            }
            return Err(failures.join("; "));
        }
        for failure in failures {
            warn!(%failure, "SEPTA feed partially unavailable");
        }
        vehicles.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        let snapshot = Arc::new(VehicleSnapshot {
            updated_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            stale: false,
            vehicles,
        });
        *cache = Some(CachedSnapshot {
            fetched: Instant::now(),
            snapshot: Arc::clone(&snapshot),
        });
        Ok((snapshot, "refresh"))
    }

    async fn fetch_surface(&self) -> Result<Vec<Vehicle>, String> {
        let response = self
            .client
            .get(SURFACE_URL)
            .send()
            .await
            .map_err(|error| format!("surface feed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("surface feed: {error}"))?
            .json::<SurfaceResponse>()
            .await
            .map_err(|error| format!("surface feed: {error}"))?;
        Ok(surface_vehicles(response))
    }

    async fn fetch_rail(&self) -> Result<Vec<Vehicle>, String> {
        let response = self
            .client
            .get(RAIL_URL)
            .send()
            .await
            .map_err(|error| format!("rail feed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("rail feed: {error}"))?
            .json::<Vec<RailVehicle>>()
            .await
            .map_err(|error| format!("rail feed: {error}"))?;
        Ok(rail_vehicles(response))
    }
}

pub(crate) async fn vehicles(State(state): State<AppState>) -> Response {
    match state.live_city.snapshot().await {
        Ok((snapshot, cache)) => (
            [
                (
                    header::CACHE_CONTROL,
                    "public, max-age=5, stale-while-revalidate=20",
                ),
                (header::HeaderName::from_static("x-live-cache"), cache),
            ],
            Json(snapshot.as_ref().clone()),
        )
            .into_response(),
        Err(error) => {
            warn!(%error, "SEPTA feeds unavailable");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

fn surface_vehicles(response: SurfaceResponse) -> Vec<Vehicle> {
    let mut seen = HashSet::new();
    response
        .routes
        .into_iter()
        .flat_map(HashMap::into_iter)
        .flat_map(|(route, vehicles)| {
            vehicles
                .into_iter()
                .map(move |vehicle| (route.clone(), vehicle))
        })
        .filter_map(|(route, vehicle)| {
            if !valid_vehicle_id(&vehicle.vehicle_id) {
                return None;
            }
            let latitude = vehicle.lat.parse().ok()?;
            let longitude = vehicle.lng.parse().ok()?;
            if !valid_position(latitude, longitude) || !seen.insert(vehicle.vehicle_id.clone()) {
                return None;
            }
            Some(Vehicle {
                id: format!("surface:{}", vehicle.vehicle_id),
                mode: VehicleMode::Surface,
                route,
                label: vehicle.label,
                destination: vehicle.destination,
                latitude,
                longitude,
                heading: vehicle.heading,
            })
        })
        .collect()
}

fn valid_vehicle_id(value: &str) -> bool {
    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("none")
}

fn rail_vehicles(response: Vec<RailVehicle>) -> Vec<Vehicle> {
    response
        .into_iter()
        .filter_map(|vehicle| {
            let latitude = vehicle.lat.parse().ok()?;
            let longitude = vehicle.lon.parse().ok()?;
            valid_position(latitude, longitude).then(|| Vehicle {
                id: format!("rail:{}", vehicle.trainno),
                mode: VehicleMode::RegionalRail,
                route: vehicle.line.clone(),
                label: vehicle.trainno,
                destination: vehicle.dest,
                latitude,
                longitude,
                heading: vehicle.heading,
            })
        })
        .collect()
}

fn valid_position(latitude: f64, longitude: f64) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && (39.65..=40.35).contains(&latitude)
        && (-75.65..=-74.75).contains(&longitude)
}

#[cfg(test)]
mod tests {
    use super::{RailVehicle, SurfaceResponse, rail_vehicles, surface_vehicles};

    #[test]
    fn parses_surface_feed_and_drops_zero_coordinates_and_duplicates()
    -> Result<(), serde_json::Error> {
        let response: SurfaceResponse = serde_json::from_str(
            r#"{"routes":[{"17":[{"VehicleID":"1","label":"17","destination":"Penn's Landing","lat":"39.95","lng":"-75.16","heading":90},{"VehicleID":"2","lat":"0.0","lng":"0.0"},{"VehicleID":"0","lat":"39.95","lng":"-75.16"}]},{"33":[{"VehicleID":"1","lat":"39.96","lng":"-75.17"}]}]}"#,
        )?;
        let vehicles = surface_vehicles(response);
        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].route, "17");
        Ok(())
    }

    #[test]
    fn parses_rail_string_heading() -> Result<(), serde_json::Error> {
        let response: Vec<RailVehicle> = serde_json::from_str(
            r#"[{"trainno":"421","line":"Warminster","dest":"Suburban","lat":"40.1","lon":"-75.2","heading":"180"}]"#,
        )?;
        let vehicles = rail_vehicles(response);
        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].heading, 180.0);
        Ok(())
    }

    #[test]
    fn parses_rail_numeric_heading() -> Result<(), serde_json::Error> {
        let response: Vec<RailVehicle> = serde_json::from_str(
            r#"[{"trainno":"3520","line":"Media/Wawa","dest":"Doylestown","lat":"39.953","lon":"-75.167","heading":0}]"#,
        )?;
        assert_eq!(rail_vehicles(response)[0].heading, 0.0);
        Ok(())
    }
}
