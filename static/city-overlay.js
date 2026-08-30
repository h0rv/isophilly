// @ts-check

const DEG = Math.PI / 180;
const A = 6_378_137;
const INVERSE_FLATTENING = 298.257222101;
const F = 1 / INVERSE_FLATTENING;
const E = Math.sqrt(F * (2 - F));
const FALSE_EASTING = 600_000;
const FALSE_NORTHING = 0;
const LATITUDE_ORIGIN = 39 + 20 / 60;
const CENTRAL_MERIDIAN = -(77 + 45 / 60);
const STANDARD_PARALLEL_1 = 39 + 56 / 60;
const STANDARD_PARALLEL_2 = 40 + 58 / 60;
const BROAD_NORTH_EAST = 0.13555646;
const BROAD_NORTH_NORTH = 0.9907696;

/** @param {number} latitude */
function m(latitude) {
  const phi = latitude * DEG;
  return Math.cos(phi) / Math.sqrt(1 - E * E * Math.sin(phi) ** 2);
}

/** @param {number} latitude */
function t(latitude) {
  const phi = latitude * DEG;
  const sinPhi = Math.sin(phi);
  return Math.tan(Math.PI / 4 - phi / 2) / ((1 - E * sinPhi) / (1 + E * sinPhi)) ** (E / 2);
}

const projectionN =
  (Math.log(m(STANDARD_PARALLEL_1)) - Math.log(m(STANDARD_PARALLEL_2))) /
  (Math.log(t(STANDARD_PARALLEL_1)) - Math.log(t(STANDARD_PARALLEL_2)));
const projectionF = m(STANDARD_PARALLEL_1) / (projectionN * t(STANDARD_PARALLEL_1) ** projectionN);
const rhoOrigin = A * projectionF * t(LATITUDE_ORIGIN) ** projectionN;

/** Convert WGS84 coordinates to Pennsylvania South State Plane metres (EPSG:32129). */
/** @param {number} longitude @param {number} latitude @returns {[number, number]} */
export function projectLonLat(longitude, latitude) {
  const rho = A * projectionF * t(latitude) ** projectionN;
  const theta = projectionN * (longitude - CENTRAL_MERIDIAN) * DEG;
  return [
    FALSE_EASTING + rho * Math.sin(theta),
    FALSE_NORTHING + rhoOrigin - rho * Math.cos(theta),
  ];
}

/** Project a WGS84 point onto the same Broad-Street-aligned isometric plane as the renderer. */
/** @param {number} longitude @param {number} latitude @param {number} [height] @returns {[number, number]} */
export function isometricLonLat(longitude, latitude, height = 0) {
  const [x, y] = projectLonLat(longitude, latitude);
  const broadEast = BROAD_NORTH_NORTH * x - BROAD_NORTH_EAST * y;
  const broadNorth = BROAD_NORTH_EAST * x + BROAD_NORTH_NORTH * y;
  return [broadEast + broadNorth, (broadEast - broadNorth) * 0.5 - height];
}

/** @typedef {{ altitude: number, azimuth: number }} SolarPosition */

/** Deterministic NOAA-style solar position, accurate enough for map lighting. */
/** @param {Date} date @param {number} latitude @param {number} longitude @returns {SolarPosition} */
export function solarPosition(date, latitude, longitude) {
  const julianDay = date.getTime() / 86_400_000 + 2_440_587.5;
  const century = (julianDay - 2_451_545) / 36_525;
  const meanLongitude = (280.46646 + century * (36_000.76983 + century * 0.0003032)) % 360;
  const meanAnomaly = 357.52911 + century * (35_999.05029 - 0.0001537 * century);
  const orbitEccentricity = 0.016708634 - century * (0.000042037 + 0.0000001267 * century);
  const center =
    Math.sin(meanAnomaly * DEG) * (1.914602 - century * (0.004817 + 0.000014 * century)) +
    Math.sin(2 * meanAnomaly * DEG) * (0.019993 - 0.000101 * century) +
    Math.sin(3 * meanAnomaly * DEG) * 0.000289;
  const apparentLongitude =
    meanLongitude + center - 0.00569 - 0.00478 * Math.sin((125.04 - 1934.136 * century) * DEG);
  const meanObliquity =
    23 + (26 + (21.448 - century * (46.815 + century * (0.00059 - century * 0.001813))) / 60) / 60;
  const obliquity = meanObliquity + 0.00256 * Math.cos((125.04 - 1934.136 * century) * DEG);
  const declination = Math.asin(Math.sin(obliquity * DEG) * Math.sin(apparentLongitude * DEG));
  const variable = Math.tan((obliquity * DEG) / 2) ** 2;
  const equationOfTime =
    4 *
    (180 / Math.PI) *
    (variable * Math.sin(2 * meanLongitude * DEG) -
      2 * orbitEccentricity * Math.sin(meanAnomaly * DEG) +
      4 *
        orbitEccentricity *
        variable *
        Math.sin(meanAnomaly * DEG) *
        Math.cos(2 * meanLongitude * DEG) -
      0.5 * variable * variable * Math.sin(4 * meanLongitude * DEG) -
      1.25 * orbitEccentricity ** 2 * Math.sin(2 * meanAnomaly * DEG));
  const utcMinutes = date.getUTCHours() * 60 + date.getUTCMinutes() + date.getUTCSeconds() / 60;
  const solarMinutes = (((utcMinutes + equationOfTime + 4 * longitude) % 1440) + 1440) % 1440;
  const hourAngle = (solarMinutes / 4 - 180) * DEG;
  const phi = latitude * DEG;
  const altitude = Math.asin(
    Math.sin(phi) * Math.sin(declination) +
      Math.cos(phi) * Math.cos(declination) * Math.cos(hourAngle),
  );
  const azimuth =
    (Math.atan2(
      Math.sin(hourAngle),
      Math.cos(hourAngle) * Math.sin(phi) - Math.tan(declination) * Math.cos(phi),
    ) /
      DEG +
      180 +
      360) %
    360;
  return { altitude: altitude / DEG, azimuth };
}

/** @typedef {{ phase: "day" | "golden" | "twilight" | "night", color: string, alpha: number }} LightingState */

/** @param {number} altitude @returns {LightingState} */
export function lightingState(altitude) {
  if (altitude >= 10) return { phase: "day", color: "#ffffff", alpha: 0 };
  if (altitude >= 0) return { phase: "golden", color: "#f09a62", alpha: (10 - altitude) / 100 };
  if (altitude >= -6)
    return { phase: "twilight", color: "#34446b", alpha: 0.12 + (-altitude / 6) * 0.16 };
  return { phase: "night", color: "#10172e", alpha: 0.42 };
}

/** A restrained color grade for the map tiles. UI and labels are drawn unfiltered. */
/** @param {boolean} vivid */
export function mapColorFilter(vivid) {
  return vivid ? "saturate(1.14) contrast(1.04)" : "none";
}
