import { projectLonLat } from "../../static/city-overlay.js";
import { applyProjection, inverseProjection } from "../../static/map-projection.js";

const DEG = Math.PI / 180;
const A = 6378137;
const E2 = 6.6943799901413165e-3;
const EAST = 0.13555646;
const NORTH = 0.9907696;

export function geodeticToEcef(longitude, latitude, height = 0) {
  const lon = longitude * DEG;
  const lat = latitude * DEG;
  const n = A / Math.sqrt(1 - E2 * Math.sin(lat) ** 2);
  return [
    (n + height) * Math.cos(lat) * Math.cos(lon),
    (n + height) * Math.cos(lat) * Math.sin(lon),
    (n * (1 - E2) + height) * Math.sin(lat),
  ];
}

export function ecefToGeodetic(x, y, z) {
  const p = Math.hypot(x, y);
  let lat = Math.atan2(z, p * (1 - E2));
  for (let i = 0; i < 7; i++) {
    const n = A / Math.sqrt(1 - E2 * Math.sin(lat) ** 2);
    lat = Math.atan2(z + E2 * n * Math.sin(lat), p);
  }
  const n = A / Math.sqrt(1 - E2 * Math.sin(lat) ** 2);
  return [Math.atan2(y, x) / DEG, lat / DEG, p / Math.cos(lat) - n];
}

/** Exact same planar projection as Rust View::SouthEast, not a perspective approximation. */
export function projectStatePlane(x, y, height, angle = 0, projection) {
  const broadEast = NORTH * x - EAST * y;
  const broadNorth = EAST * x + NORTH * y;
  const radians = angle * DEG;
  const east = broadEast * Math.cos(radians) + broadNorth * Math.sin(radians);
  const north = -broadEast * Math.sin(radians) + broadNorth * Math.cos(radians);
  const groundY = (east - north) * 0.5;
  return applyProjection(east + north, groundY, height, projection);
}

export function projectEcef(x, y, z, heightOrigin, angle = 0, projection) {
  const [lon, lat, height] = ecefToGeodetic(x, y, z);
  const [east, north] = projectLonLat(lon, lat);
  return projectStatePlane(east, north, height - heightOrigin, angle, projection);
}

/** RockTree uses Google's spherical globe, not the Map Tiles API's WGS84 ECEF. */
export function projectEarth(x, y, z, radius, heightOrigin, angle = 0, projection) {
  const longitude = Math.atan2(y, x) / DEG;
  const latitude = Math.atan2(z, Math.hypot(x, y)) / DEG;
  const height = Math.hypot(x, y, z) - radius - heightOrigin;
  const [east, north] = projectLonLat(longitude, latitude);
  return projectStatePlane(east, north, height, angle, projection);
}

export function inverseIsometric(x, y, angle = 0, projection) {
  [x, y] = inverseProjection(x, y, projection);
  const viewEast = (x + 2 * y) * 0.5;
  const viewNorth = (x - 2 * y) * 0.5;
  const radians = angle * DEG;
  const east = viewEast * Math.cos(radians) - viewNorth * Math.sin(radians);
  const north = viewEast * Math.sin(radians) + viewNorth * Math.cos(radians);
  const norm = NORTH * NORTH + EAST * EAST;
  return [(NORTH * east + EAST * north) / norm, (-EAST * east + NORTH * north) / norm];
}
