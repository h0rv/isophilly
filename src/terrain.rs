use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"ISOTERN1";
const SCHEMA_VERSION: u32 = 1;
const PREFIX_BYTES: usize = 16;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_GRID_DIMENSION: usize = 512;
const MAX_GRID_CELLS: usize = MAX_GRID_DIMENSION * MAX_GRID_DIMENSION;
const MAX_ARTIFACT_BYTES: usize = PREFIX_BYTES + MAX_HEADER_BYTES + MAX_GRID_CELLS * 3;
const CELL_SIZE_METERS: f64 = 256.0;
const EPSG: u64 = 32129;
const GROUND_ELEVATION_MIN_METERS: f64 = -5.0;
const GROUND_ELEVATION_MAX_METERS: f64 = 150.0;
const GROUND_POINT_COUNT_MINIMUM: u64 = 20;
const LIGHT_EAST: f32 = -0.55;
const LIGHT_NORTH: f32 = -0.72;
const LIGHT_UP: f32 = 1.0;
const RELIEF_EXAGGERATION: f32 = 3.0;
const RELIEF_STRENGTH: f32 = 0.45;
const MIN_SHADE: f32 = 0.92;
const MAX_SHADE: f32 = 1.08;
const REJECTED_SOURCE_TILES: [&str; 8] = [
    "26822E227832N.las",
    "26848E238392N.las",
    "26954E227832N.las",
    "27086E256872N.las",
    "27086E259512N.las",
    "27086E262152N.las",
    "27086E264792N.las",
    "27086E267432N.las",
];

#[derive(Clone, Copy)]
struct Grid {
    width: usize,
    height: usize,
    min_x: f64,
    min_y: f64,
}

pub struct Terrain {
    grid: Grid,
    elevations_cm: Vec<i16>,
    coverage: Vec<u8>,
    artifact_sha256: [u8; 32],
}

impl Terrain {
    pub fn open_optional(path: &Path) -> io::Result<Option<Self>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let limit = u64::try_from(MAX_ARTIFACT_BYTES)
            .map_err(|_| invalid("terrain artifact size bound is unsupported"))?;
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("terrain artifact exceeds its size bound"));
        }
        Self::parse(&bytes).map(Some)
    }

    pub fn artifact_sha256(&self) -> [u8; 32] {
        self.artifact_sha256
    }

    /// Returns a subtle fixed-sun tone multiplier. Missing support, rejected
    /// source-gap cells, and every outer interpolation halo deliberately stay
    /// neutral so incomplete LiDAR evidence cannot make an edge seam.
    pub fn hillshade(&self, point: (f32, f32)) -> f32 {
        let step = CELL_SIZE_METERS as f32;
        let Some(left) = self.sample((point.0 - step, point.1)) else {
            return 1.0;
        };
        let Some(right) = self.sample((point.0 + step, point.1)) else {
            return 1.0;
        };
        let Some(south) = self.sample((point.0, point.1 - step)) else {
            return 1.0;
        };
        let Some(north) = self.sample((point.0, point.1 + step)) else {
            return 1.0;
        };
        // Philadelphia's broad slopes are gentle at the 256 metre evidence
        // scale. Exaggerate their normal only for this tonal cue; geometry and
        // recorded elevations remain unchanged.
        let dz_dx = (right - left) / (2.0 * step) * RELIEF_EXAGGERATION;
        let dz_dy = (north - south) / (2.0 * step) * RELIEF_EXAGGERATION;
        if dz_dx.abs() <= f32::EPSILON && dz_dy.abs() <= f32::EPSILON {
            return 1.0;
        }
        let normal_length = (-dz_dx).hypot(-dz_dy).hypot(1.0);
        let light_length = LIGHT_EAST.hypot(LIGHT_NORTH).hypot(LIGHT_UP);
        let illumination = ((-dz_dx * LIGHT_EAST) + (-dz_dy * LIGHT_NORTH) + LIGHT_UP)
            / (normal_length * light_length);
        let flat_illumination = LIGHT_UP / light_length;
        (1.0 + (illumination / flat_illumination - 1.0) * RELIEF_STRENGTH)
            .clamp(MIN_SHADE, MAX_SHADE)
    }

    fn sample(&self, point: (f32, f32)) -> Option<f32> {
        let x = (f64::from(point.0) - self.grid.min_x) / CELL_SIZE_METERS - 0.5;
        let y = (f64::from(point.1) - self.grid.min_y) / CELL_SIZE_METERS - 0.5;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let column = x.floor() as isize;
        let row = y.floor() as isize;
        if column < 0
            || row < 0
            || column + 1 >= self.grid.width as isize
            || row + 1 >= self.grid.height as isize
        {
            return None;
        }
        let fraction_x = (x - column as f64) as f32;
        let fraction_y = (y - row as f64) as f32;
        let cells = [
            self.cell(column as usize, row as usize),
            self.cell(column as usize + 1, row as usize),
            self.cell(column as usize, row as usize + 1),
            self.cell(column as usize + 1, row as usize + 1),
        ];
        if cells
            .iter()
            .any(|(_, coverage)| !matches!(*coverage, 1 | 2))
        {
            return None;
        }
        let south = cells[0].0 * (1.0 - fraction_x) + cells[1].0 * fraction_x;
        let north = cells[2].0 * (1.0 - fraction_x) + cells[3].0 * fraction_x;
        Some(south * (1.0 - fraction_y) + north * fraction_y)
    }

    fn cell(&self, column: usize, row: usize) -> (f32, u8) {
        let index = row * self.grid.width + column;
        (
            f32::from(self.elevations_cm[index]) * 0.01,
            self.coverage[index],
        )
    }

    fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("terrain artifact exceeds its size bound"));
        }
        if bytes.len() < PREFIX_BYTES || &bytes[..8] != MAGIC {
            return Err(invalid("terrain artifact prefix is invalid"));
        }
        let schema_version = u32::from_le_bytes(bytes[8..12].try_into().map_err(invalid_bytes)?);
        let header_bytes =
            u32::from_le_bytes(bytes[12..16].try_into().map_err(invalid_bytes)?) as usize;
        if schema_version != SCHEMA_VERSION || header_bytes > MAX_HEADER_BYTES {
            return Err(invalid("terrain artifact prefix is invalid"));
        }
        let payload_offset = PREFIX_BYTES
            .checked_add(header_bytes)
            .ok_or_else(|| invalid("terrain header length overflows"))?;
        let header = bytes
            .get(PREFIX_BYTES..payload_offset)
            .ok_or_else(|| invalid("terrain artifact header is truncated"))?;
        let value: Value = serde_json::from_slice(header)
            .map_err(|_| invalid("terrain artifact header is invalid JSON"))?;
        let canonical = serde_json::to_vec(&value).map_err(io::Error::other)?;
        if canonical != header {
            return Err(invalid("terrain artifact header is not canonical JSON"));
        }
        let object = value
            .as_object()
            .ok_or_else(|| invalid("terrain header must be a JSON object"))?;
        let (grid, minimum_cm, maximum_cm) = parse_header(object)?;
        let cells = grid
            .width
            .checked_mul(grid.height)
            .ok_or_else(|| invalid("terrain grid size overflows"))?;
        let payload_bytes = cells
            .checked_mul(3)
            .ok_or_else(|| invalid("terrain payload size overflows"))?;
        let expected_bytes = payload_offset
            .checked_add(payload_bytes)
            .ok_or_else(|| invalid("terrain artifact size overflows"))?;
        if cells > MAX_GRID_CELLS || bytes.len() != expected_bytes {
            return Err(invalid(
                "terrain artifact payload size does not match its grid",
            ));
        }
        let payload = &bytes[payload_offset..];
        if digest_hex(&Sha256::digest(payload)) != string(object, "payload_sha256")? {
            return Err(invalid("terrain artifact payload SHA-256 does not match"));
        }
        let elevation_bytes = &payload[..cells * 2];
        let elevations_cm = elevation_bytes
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let coverage = payload[cells * 2..].to_vec();
        for (&elevation, &coverage) in elevations_cm.iter().zip(&coverage) {
            if coverage > 3 {
                return Err(invalid(
                    "terrain payload contains an unsupported coverage class",
                ));
            }
            if coverage == 0 && elevation != 0 {
                return Err(invalid("terrain unsupported cell has an elevation"));
            }
            if coverage != 0 && (elevation < minimum_cm || elevation > maximum_cm) {
                return Err(invalid("terrain elevation is outside its accepted range"));
            }
        }
        Ok(Self {
            grid,
            elevations_cm,
            coverage,
            artifact_sha256: Sha256::digest(bytes).into(),
        })
    }
}

fn parse_header(object: &Map<String, Value>) -> io::Result<(Grid, i16, i16)> {
    exact_keys(
        object,
        &[
            "acceptance",
            "coverage",
            "evidence",
            "grid",
            "interpolation",
            "payload_sha256",
            "schema_version",
            "vertical_reference",
        ],
        "terrain header",
    )?;
    if integer(object, "schema_version")? != u64::from(SCHEMA_VERSION)
        || string(object, "vertical_reference")? != "relative_visual_relief_only"
        || !is_sha256(string(object, "payload_sha256")?)
    {
        return Err(invalid("terrain artifact schema or policy changed"));
    }
    parse_coverage(object)?;
    parse_evidence(object)?;
    parse_interpolation(object)?;
    let (minimum_cm, maximum_cm) = parse_acceptance(object)?;
    Ok((parse_grid(object)?, minimum_cm, maximum_cm))
}

fn parse_acceptance(object: &Map<String, Value>) -> io::Result<(i16, i16)> {
    let acceptance = nested(object, "acceptance", "terrain acceptance")?;
    exact_keys(
        acceptance,
        &[
            "ground_elevation_max_m",
            "ground_elevation_min_m",
            "ground_point_count_min",
        ],
        "terrain acceptance",
    )?;
    let minimum = number(acceptance, "ground_elevation_min_m")?;
    let maximum = number(acceptance, "ground_elevation_max_m")?;
    if integer(acceptance, "ground_point_count_min")? != GROUND_POINT_COUNT_MINIMUM
        || minimum != GROUND_ELEVATION_MIN_METERS
        || maximum != GROUND_ELEVATION_MAX_METERS
    {
        return Err(invalid("terrain acceptance contract changed"));
    }
    if minimum > maximum {
        return Err(invalid("terrain acceptance range is invalid"));
    }
    let minimum_cm = (minimum * 100.0).round();
    let maximum_cm = (maximum * 100.0).round();
    if minimum_cm < f64::from(i16::MIN)
        || maximum_cm > f64::from(i16::MAX)
        || (minimum * 100.0 - minimum_cm).abs() > f64::EPSILON
        || (maximum * 100.0 - maximum_cm).abs() > f64::EPSILON
    {
        return Err(invalid(
            "terrain acceptance range cannot encode centimetres",
        ));
    }
    Ok((minimum_cm as i16, maximum_cm as i16))
}

fn parse_coverage(object: &Map<String, Value>) -> io::Result<()> {
    let coverage = nested(object, "coverage", "terrain coverage")?;
    exact_keys(coverage, &["0", "1", "2", "3"], "terrain coverage")?;
    if coverage.get("0").and_then(Value::as_str) != Some("unsupported")
        || coverage.get("1").and_then(Value::as_str) != Some("direct")
        || coverage.get("2").and_then(Value::as_str) != Some("interpolated")
        || coverage.get("3").and_then(Value::as_str) != Some("rejected_gap_interpolated")
    {
        return Err(invalid("terrain coverage table changed"));
    }
    Ok(())
}

fn parse_evidence(object: &Map<String, Value>) -> io::Result<()> {
    let evidence = nested(object, "evidence", "terrain evidence")?;
    exact_keys(
        evidence,
        &[
            "manifest_sha256",
            "parquet_sha256",
            "rejected_source_count",
            "rejected_source_gaps_sha256",
            "rejected_source_tiles",
            "source_coverage_complete",
            "source_footprints_sha256",
        ],
        "terrain evidence",
    )?;
    for key in [
        "manifest_sha256",
        "parquet_sha256",
        "rejected_source_gaps_sha256",
        "source_footprints_sha256",
    ] {
        if !is_sha256(string(evidence, key)?) {
            return Err(invalid("terrain evidence contains an invalid SHA-256"));
        }
    }
    if boolean(evidence, "source_coverage_complete")? {
        return Err(invalid(
            "terrain evidence must record incomplete source coverage",
        ));
    }
    if integer(evidence, "rejected_source_count")? != REJECTED_SOURCE_TILES.len() as u64 {
        return Err(invalid("terrain evidence rejected source count changed"));
    }
    let tiles = evidence
        .get("rejected_source_tiles")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("terrain rejected source tiles must be an array"))?;
    if tiles.len() != REJECTED_SOURCE_TILES.len()
        || tiles
            .iter()
            .zip(REJECTED_SOURCE_TILES)
            .any(|(tile, expected)| tile.as_str() != Some(expected))
    {
        return Err(invalid("terrain rejected source tiles changed"));
    }
    Ok(())
}

fn parse_grid(object: &Map<String, Value>) -> io::Result<Grid> {
    let grid = nested(object, "grid", "terrain grid")?;
    exact_keys(
        grid,
        &[
            "cell_size_m",
            "epsg",
            "height",
            "min_x",
            "min_y",
            "row_order",
            "sample_location",
            "width",
        ],
        "terrain grid",
    )?;
    let width = usize::try_from(integer(grid, "width")?)
        .map_err(|_| invalid("terrain grid width is unsupported"))?;
    let height = usize::try_from(integer(grid, "height")?)
        .map_err(|_| invalid("terrain grid height is unsupported"))?;
    let min_x = number(grid, "min_x")?;
    let min_y = number(grid, "min_y")?;
    if width == 0
        || height == 0
        || width > MAX_GRID_DIMENSION
        || height > MAX_GRID_DIMENSION
        || width
            .checked_mul(height)
            .is_none_or(|cells| cells > MAX_GRID_CELLS)
        || integer(grid, "epsg")? != EPSG
        || number(grid, "cell_size_m")? != CELL_SIZE_METERS
        || string(grid, "row_order")? != "south_to_north"
        || string(grid, "sample_location")? != "cell_center"
        || (min_x / CELL_SIZE_METERS).fract().abs() > f64::EPSILON
        || (min_y / CELL_SIZE_METERS).fract().abs() > f64::EPSILON
    {
        return Err(invalid(
            "terrain grid does not match the production contract",
        ));
    }
    Ok(Grid {
        width,
        height,
        min_x,
        min_y,
    })
}

fn parse_interpolation(object: &Map<String, Value>) -> io::Result<()> {
    let interpolation = nested(object, "interpolation", "terrain interpolation")?;
    exact_keys(
        interpolation,
        &["direct_min_samples", "fill", "smoothing"],
        "terrain interpolation",
    )?;
    if integer(interpolation, "direct_min_samples")? != 3
        || string(interpolation, "fill")?
            != "inverse_distance_squared_5_nearest_direct_cells_within_1500m"
        || string(interpolation, "smoothing")? != "single_3x3_median"
    {
        return Err(invalid("terrain interpolation contract changed"));
    }
    Ok(())
}

fn exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    context: &'static str,
) -> io::Result<()> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid(context));
    }
    Ok(())
}

fn nested<'a>(
    object: &'a Map<String, Value>,
    key: &'static str,
    context: &'static str,
) -> io::Result<&'a Map<String, Value>> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(context))
}

fn string<'a>(object: &'a Map<String, Value>, key: &'static str) -> io::Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("terrain header contains an invalid string"))
}

fn integer(object: &Map<String, Value>, key: &'static str) -> io::Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("terrain header contains an invalid integer"))
}

fn number(object: &Map<String, Value>, key: &'static str) -> io::Result<f64> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid("terrain header contains an invalid number"))
}

fn boolean(object: &Map<String, Value>, key: &'static str) -> io::Result<bool> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("terrain header contains an invalid boolean"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn digest_hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid_bytes(_: std::array::TryFromSliceError) -> io::Error {
    invalid("terrain artifact prefix is invalid")
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::{CELL_SIZE_METERS, MAX_SHADE, MIN_SHADE, PREFIX_BYTES, Terrain, digest_hex};

    fn artifact(width: usize, height: usize, elevations: &[i16], coverage: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        for elevation in elevations {
            payload.extend_from_slice(&elevation.to_le_bytes());
        }
        payload.extend_from_slice(coverage);
        let digest = digest_hex(&Sha256::digest(&payload));
        let header = json!({
            "acceptance": {"ground_elevation_max_m": 150.0, "ground_elevation_min_m": -5.0, "ground_point_count_min": 20},
            "coverage": {"0":"unsupported", "1":"direct", "2":"interpolated", "3":"rejected_gap_interpolated"},
            "evidence": {"manifest_sha256":"a".repeat(64), "parquet_sha256":"b".repeat(64), "source_footprints_sha256":"c".repeat(64), "source_coverage_complete":false, "rejected_source_count":8, "rejected_source_gaps_sha256":"d".repeat(64), "rejected_source_tiles":super::REJECTED_SOURCE_TILES},
            "grid": {"cell_size_m": CELL_SIZE_METERS, "epsg":32129, "height":height, "min_x":0.0, "min_y":0.0, "row_order":"south_to_north", "sample_location":"cell_center", "width":width},
            "interpolation": {"direct_min_samples":3, "fill":"inverse_distance_squared_5_nearest_direct_cells_within_1500m", "smoothing":"single_3x3_median"},
            "payload_sha256":digest,
            "schema_version":1,
            "vertical_reference":"relative_visual_relief_only"
        });
        let header = serde_json::to_vec(&header).unwrap_or_else(|_| std::process::abort());
        let mut bytes = Vec::with_capacity(PREFIX_BYTES + header.len() + payload.len());
        bytes.extend_from_slice(b"ISOTERN1");
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn parses_strict_canonical_artifact_and_binds_digest() -> std::io::Result<()> {
        let bytes = artifact(4, 4, &[100; 16], &[1; 16]);
        let terrain = Terrain::parse(&bytes)?;
        assert_eq!(
            terrain.artifact_sha256(),
            <[u8; 32]>::from(Sha256::digest(&bytes))
        );
        assert_eq!(terrain.hillshade((512.0, 512.0)), 1.0);
        Ok(())
    }

    #[test]
    fn slopes_are_lit_and_darkened_by_the_fixed_southwest_sun() -> std::io::Result<()> {
        let mut east_rising = Vec::new();
        let mut west_rising = Vec::new();
        for _row in 0..5 {
            for column in 0..5 {
                east_rising.push((column as i16) * 200);
                west_rising.push((4 - column as i16) * 200);
            }
        }
        let light = Terrain::parse(&artifact(5, 5, &east_rising, &[1; 25]))?;
        let dark = Terrain::parse(&artifact(5, 5, &west_rising, &[1; 25]))?;
        assert!(light.hillshade((640.0, 640.0)) > 1.0);
        assert!(dark.hillshade((640.0, 640.0)) < 1.0);
        Ok(())
    }

    #[test]
    fn unsupported_and_rejected_gap_halos_are_neutral() -> std::io::Result<()> {
        let elevations: Vec<_> = (0..25).map(|index| index as i16 * 100).collect();
        let mut unsupported = [1; 25];
        unsupported[12] = 0;
        let mut unsupported_elevations = elevations.clone();
        unsupported_elevations[12] = 0;
        let mut rejected = [1; 25];
        rejected[12] = 3;
        assert_eq!(
            Terrain::parse(&artifact(5, 5, &unsupported_elevations, &unsupported))?
                .hillshade((640.0, 640.0)),
            1.0
        );
        assert_eq!(
            Terrain::parse(&artifact(5, 5, &elevations, &rejected))?.hillshade((640.0, 640.0)),
            1.0
        );
        Ok(())
    }

    #[test]
    fn hillshade_is_source_anchored_and_deterministic_across_tile_seams() -> std::io::Result<()> {
        let mut elevations = Vec::new();
        for row in 0..6 {
            for column in 0..6 {
                elevations.push((row * 90 + column * 160) as i16);
            }
        }
        let terrain = Terrain::parse(&artifact(6, 6, &elevations, &[1; 36]))?;
        let shared_source_sample = (768.0, 768.0);
        let first = terrain.hillshade(shared_source_sample);
        let second = terrain.hillshade(shared_source_sample);

        assert_eq!(first, second);
        assert!((MIN_SHADE..=MAX_SHADE).contains(&first));
        Ok(())
    }

    #[test]
    fn rejects_noncanonical_or_tampered_payload() {
        let bytes = artifact(4, 4, &[100; 16], &[1; 16]);
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] = 2;
        assert!(Terrain::parse(&tampered).is_err());

        let mut noncanonical = bytes;
        noncanonical.insert(PREFIX_BYTES, b' ');
        let size = u32::from_le_bytes(noncanonical[12..16].try_into().unwrap_or([0; 4])) + 1;
        noncanonical[12..16].copy_from_slice(&size.to_le_bytes());
        assert!(Terrain::parse(&noncanonical).is_err());
    }
}
