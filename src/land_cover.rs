use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, Read},
    path::Path,
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"ISOLULC1";
const SCHEMA_VERSION: u32 = 1;
const PREFIX_BYTES: usize = 16;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_GRID_PIXELS: usize = 100_000_000;
const MAX_ARTIFACT_BYTES: usize = PREFIX_BYTES + MAX_HEADER_BYTES + MAX_GRID_PIXELS;
const EPSG: u64 = 32129;
const ARCHIVE_BYTES: u64 = 521_373_667;
const ARCHIVE_URL: &str =
    "https://www.pasda.psu.edu/download/philacity/data/PhiladelphiaLandCoverRaster2018.zip";
const SOURCE_IDENTITY_SHA256: &str =
    "72cdba0fd90b5a7e880e5ce51c3cb5cfba26382a1617efa2149ce612d791d5b8";
// Keep this identical to AUDITED_SOURCE_ARCHIVE_SHA256 in land_cover.py. A
// present mask fails closed until both readers name the reviewed archive.
const AUDITED_SOURCE_ARCHIVE_SHA256: Option<&str> =
    Some("555ab81428c239dd4d1a1f162fdd072f4ff1b0b2ab15a2e96a3f241e2823bb3f");
const RIGHTS_NOTICE: &str = "The City reserves rights in this dataset and provides it as is. Confirm current City and PASDA terms before publishing source pixels or derived raster tiles.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LandCoverClass {
    TreeCanopy,
    GrassShrub,
    BareEarth,
    Water,
    Building,
    RoadRailroad,
    OtherPaved,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Grid {
    width: usize,
    height: usize,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Grid {
    fn pixel_width(self) -> f64 {
        (self.max_x - self.min_x) / self.width as f64
    }

    fn pixel_height(self) -> f64 {
        (self.max_y - self.min_y) / self.height as f64
    }
}

const PRODUCTION_GRID: Grid = Grid {
    width: 9_098,
    height: 10_174,
    min_x: 810_942.0,
    min_y: 62_427.0,
    max_x: 838_236.0,
    max_y: 92_949.0,
};

pub struct LandCoverMask {
    grid: Grid,
    classes: Vec<u8>,
    artifact_sha256: [u8; 32],
}

impl LandCoverMask {
    pub fn open_optional(path: &Path) -> io::Result<Option<Self>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let limit = u64::try_from(MAX_ARTIFACT_BYTES)
            .map_err(|_| invalid("land cover artifact size bound is unsupported"))?;
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("land cover artifact exceeds its size bound"));
        }
        Self::parse(&bytes, AUDITED_SOURCE_ARCHIVE_SHA256).map(Some)
    }

    pub fn artifact_sha256(&self) -> [u8; 32] {
        self.artifact_sha256
    }

    pub fn sample(&self, x: f64, y: f64) -> Option<LandCoverClass> {
        if !x.is_finite()
            || !y.is_finite()
            || x < self.grid.min_x
            || x > self.grid.max_x
            || y < self.grid.min_y
            || y > self.grid.max_y
        {
            return None;
        }
        let column = if x == self.grid.max_x {
            self.grid.width - 1
        } else {
            ((x - self.grid.min_x) / self.grid.pixel_width()).floor() as usize
        };
        let row = if y == self.grid.min_y {
            self.grid.height - 1
        } else {
            ((self.grid.max_y - y) / self.grid.pixel_height()).floor() as usize
        };
        match self.classes[row * self.grid.width + column] {
            0 => None,
            1 => Some(LandCoverClass::TreeCanopy),
            2 => Some(LandCoverClass::GrassShrub),
            3 => Some(LandCoverClass::BareEarth),
            4 => Some(LandCoverClass::Water),
            5 => Some(LandCoverClass::Building),
            6 => Some(LandCoverClass::RoadRailroad),
            7 => Some(LandCoverClass::OtherPaved),
            _ => None,
        }
    }

    fn parse(bytes: &[u8], audited_source_sha256: Option<&str>) -> io::Result<Self> {
        Self::parse_for_grid(bytes, audited_source_sha256, PRODUCTION_GRID)
    }

    fn parse_for_grid(
        bytes: &[u8],
        audited_source_sha256: Option<&str>,
        expected_grid: Grid,
    ) -> io::Result<Self> {
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("land cover artifact exceeds its size bound"));
        }
        if bytes.len() < PREFIX_BYTES || &bytes[..8] != MAGIC {
            return Err(invalid("land cover artifact prefix is invalid"));
        }
        let schema_version = u32::from_le_bytes(bytes[8..12].try_into().map_err(invalid_bytes)?);
        let header_bytes =
            u32::from_le_bytes(bytes[12..16].try_into().map_err(invalid_bytes)?) as usize;
        if schema_version != SCHEMA_VERSION || header_bytes > MAX_HEADER_BYTES {
            return Err(invalid("land cover artifact prefix is invalid"));
        }
        let payload_offset = PREFIX_BYTES
            .checked_add(header_bytes)
            .ok_or_else(|| invalid("land cover header length overflows"))?;
        let header_slice = bytes
            .get(PREFIX_BYTES..payload_offset)
            .ok_or_else(|| invalid("land cover artifact header is truncated"))?;
        let value: Value = serde_json::from_slice(header_slice)
            .map_err(|_| invalid("land cover artifact header is invalid JSON"))?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("land cover header must be a JSON object"))?;
        let grid = parse_header(object, audited_source_sha256, expected_grid)?;
        let pixels = grid
            .width
            .checked_mul(grid.height)
            .ok_or_else(|| invalid("land cover grid size overflows"))?;
        let expected_bytes = payload_offset
            .checked_add(pixels)
            .ok_or_else(|| invalid("land cover artifact size overflows"))?;
        if pixels > MAX_GRID_PIXELS || bytes.len() != expected_bytes {
            return Err(invalid(
                "land cover artifact payload size does not match its grid",
            ));
        }
        let classes = bytes[payload_offset..].to_vec();
        if classes.iter().any(|class| *class > 7) {
            return Err(invalid("land cover payload contains an unsupported class"));
        }
        let payload_sha256 = string(object, "payload_sha256")?;
        if digest_hex(&Sha256::digest(&classes)) != payload_sha256 {
            return Err(invalid(
                "land cover artifact payload SHA-256 does not match",
            ));
        }
        Ok(Self {
            grid,
            classes,
            artifact_sha256: Sha256::digest(bytes).into(),
        })
    }
}

fn parse_header(
    object: &Map<String, Value>,
    audited_source_sha256: Option<&str>,
    expected_grid: Grid,
) -> io::Result<Grid> {
    exact_keys(
        object,
        &[
            "archive_url",
            "classes",
            "dataset_id",
            "grid",
            "layer_id",
            "nodata",
            "payload_sha256",
            "resampling_contract",
            "rights_notice",
            "schema_version",
            "source_archive_bytes",
            "source_archive_sha256",
            "source_identity_sha256",
        ],
        "land cover header",
    )?;
    if integer(object, "schema_version")? != u64::from(SCHEMA_VERSION)
        || integer(object, "dataset_id")? != 1587
        || integer(object, "layer_id")? != 2
        || integer(object, "nodata")? != 0
        || integer(object, "source_archive_bytes")? != ARCHIVE_BYTES
        || string(object, "archive_url")? != ARCHIVE_URL
        || string(object, "source_identity_sha256")? != SOURCE_IDENTITY_SHA256
        || string(object, "resampling_contract")? != "nearest"
        || string(object, "rights_notice")? != RIGHTS_NOTICE
    {
        return Err(invalid("land cover artifact source or policy changed"));
    }
    for key in [
        "payload_sha256",
        "source_archive_sha256",
        "source_identity_sha256",
    ] {
        if !is_sha256(string(object, key)?) {
            return Err(invalid("land cover header contains an invalid SHA-256"));
        }
    }
    let audited_source_sha256 = audited_source_sha256
        .ok_or_else(|| invalid("land cover source archive is not audited in the Rust reader"))?;
    if !is_sha256(audited_source_sha256)
        || string(object, "source_archive_sha256")? != audited_source_sha256
    {
        return Err(invalid(
            "land cover artifact does not use the audited source archive",
        ));
    }
    let classes = object
        .get("classes")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("land cover class table must be an object"))?;
    const CLASS_KEYS: [&str; 8] = ["0", "1", "2", "3", "4", "5", "6", "7"];
    let expected_classes = [
        ("0", "unknown"),
        ("1", "tree_canopy"),
        ("2", "grass_shrub"),
        ("3", "bare_earth"),
        ("4", "water"),
        ("5", "building"),
        ("6", "road_railroad"),
        ("7", "other_paved"),
    ];
    exact_keys(classes, &CLASS_KEYS, "land cover class table")?;
    if expected_classes
        .iter()
        .any(|(key, expected)| classes.get(*key).and_then(Value::as_str) != Some(*expected))
    {
        return Err(invalid("land cover artifact class table changed"));
    }
    let grid = parse_grid(
        object
            .get("grid")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("land cover grid must be an object"))?,
    )?;
    if grid != expected_grid {
        return Err(invalid(
            "land cover grid does not match the production grid",
        ));
    }
    Ok(grid)
}

fn parse_grid(object: &Map<String, Value>) -> io::Result<Grid> {
    exact_keys(
        object,
        &[
            "epsg",
            "height",
            "max_x",
            "max_y",
            "min_x",
            "min_y",
            "resampling",
            "row_order",
            "width",
        ],
        "land cover grid",
    )?;
    let width = usize::try_from(integer(object, "width")?)
        .map_err(|_| invalid("land cover width is too large"))?;
    let height = usize::try_from(integer(object, "height")?)
        .map_err(|_| invalid("land cover height is too large"))?;
    let grid = Grid {
        width,
        height,
        min_x: number(object, "min_x")?,
        min_y: number(object, "min_y")?,
        max_x: number(object, "max_x")?,
        max_y: number(object, "max_y")?,
    };
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| invalid("land cover grid size overflows"))?;
    let pixel_width = grid.pixel_width();
    let pixel_height = grid.pixel_height();
    if integer(object, "epsg")? != EPSG
        || width == 0
        || height == 0
        || pixels > MAX_GRID_PIXELS
        || !grid.min_x.is_finite()
        || !grid.min_y.is_finite()
        || !grid.max_x.is_finite()
        || !grid.max_y.is_finite()
        || grid.min_x >= grid.max_x
        || grid.min_y >= grid.max_y
        || !(0.25..=10.0).contains(&pixel_width)
        || !(0.25..=10.0).contains(&pixel_height)
        || (pixel_width - pixel_height).abs() > 1e-9
        || string(object, "row_order")? != "north_to_south"
        || string(object, "resampling")? != "nearest"
    {
        return Err(invalid("land cover grid is unsupported"));
    }
    Ok(grid)
}

fn exact_keys(object: &Map<String, Value>, expected: &[&str], name: &str) -> io::Result<()> {
    let actual: BTreeSet<_> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{name} has an unexpected schema"),
        ));
    }
    Ok(())
}

fn integer(object: &Map<String, Value>, key: &str) -> io::Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("land cover header field must be an unsigned integer"))
}

fn number(object: &Map<String, Value>, key: &str) -> io::Result<f64> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid("land cover grid field must be a finite number"))
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> io::Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("land cover header field must be a string"))
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
    invalid("land cover artifact prefix is invalid")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::{
        Grid, LandCoverClass, LandCoverMask, PREFIX_BYTES, PRODUCTION_GRID, digest_hex,
        parse_header,
    };

    const SOURCE_SHA256: &str = "1212121212121212121212121212121212121212121212121212121212121212";
    const SYNTHETIC_GRID: Grid = Grid {
        width: 3,
        height: 2,
        min_x: 10.0,
        min_y: 20.0,
        max_x: 13.0,
        max_y: 22.0,
    };

    fn header(classes: &[u8]) -> Value {
        json!({
            "archive_url": super::ARCHIVE_URL,
            "classes": {
                "0": "unknown", "1": "tree_canopy", "2": "grass_shrub",
                "3": "bare_earth", "4": "water", "5": "building",
                "6": "road_railroad", "7": "other_paved"
            },
            "dataset_id": 1587,
            "grid": {
                "epsg": 32129, "width": 3, "height": 2,
                "min_x": 10.0, "min_y": 20.0, "max_x": 13.0, "max_y": 22.0,
                "row_order": "north_to_south", "resampling": "nearest"
            },
            "layer_id": 2,
            "nodata": 0,
            "payload_sha256": digest_hex(&Sha256::digest(classes)),
            "resampling_contract": "nearest",
            "rights_notice": super::RIGHTS_NOTICE,
            "schema_version": 1,
            "source_archive_bytes": super::ARCHIVE_BYTES,
            "source_archive_sha256": SOURCE_SHA256,
            "source_identity_sha256": super::SOURCE_IDENTITY_SHA256
        })
    }

    fn artifact(classes: &[u8]) -> Vec<u8> {
        let header = header(classes);
        let header = serde_json::to_vec(&header).unwrap_or_else(|_| std::process::abort());
        let mut bytes = Vec::with_capacity(PREFIX_BYTES + header.len() + classes.len());
        bytes.extend_from_slice(super::MAGIC);
        bytes.extend_from_slice(&super::SCHEMA_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(classes);
        bytes
    }

    #[test]
    fn samples_north_to_south_and_exact_outer_bounds() -> std::io::Result<()> {
        let mask = LandCoverMask::parse_for_grid(
            &artifact(&[1, 2, 3, 4, 0, 7]),
            Some(SOURCE_SHA256),
            SYNTHETIC_GRID,
        )?;

        assert_eq!(mask.sample(10.1, 21.9), Some(LandCoverClass::TreeCanopy));
        assert_eq!(mask.sample(11.9, 21.1), Some(LandCoverClass::GrassShrub));
        assert_eq!(mask.sample(10.1, 20.1), Some(LandCoverClass::Water));
        assert_eq!(mask.sample(11.1, 20.1), None);
        assert_eq!(mask.sample(13.0, 21.9), Some(LandCoverClass::BareEarth));
        assert_eq!(mask.sample(10.0, 20.0), Some(LandCoverClass::Water));
        assert_eq!(mask.sample(10.0, 22.0), Some(LandCoverClass::TreeCanopy));
        assert_eq!(mask.sample(13.0, 22.0), Some(LandCoverClass::BareEarth));
        assert_eq!(mask.sample(13.0, 20.0), Some(LandCoverClass::OtherPaved));
        assert_eq!(mask.sample(9.99, 21.0), None);
        assert_eq!(mask.sample(13.01, 21.0), None);
        assert_eq!(mask.sample(11.0, 19.99), None);
        assert_eq!(mask.sample(11.0, 22.01), None);
        assert_eq!(mask.sample(f64::NAN, 21.0), None);
        Ok(())
    }

    #[test]
    fn rejects_schema_payload_and_size_drift() {
        let original = artifact(&[1, 2, 3, 4, 0, 7]);
        let mut tampered = original.clone();
        let last = tampered.len() - 1;
        tampered[last] = 6;
        assert!(
            LandCoverMask::parse_for_grid(&tampered, Some(SOURCE_SHA256), SYNTHETIC_GRID).is_err()
        );
        assert!(
            LandCoverMask::parse_for_grid(
                &original[..original.len() - 1],
                Some(SOURCE_SHA256),
                SYNTHETIC_GRID,
            )
            .is_err()
        );
        assert!(LandCoverMask::parse_for_grid(&original, None, SYNTHETIC_GRID).is_err());
        assert!(
            LandCoverMask::parse_for_grid(&original, Some(&"34".repeat(32)), SYNTHETIC_GRID,)
                .is_err()
        );

        let mut extra = original;
        let header_length = u32::from_le_bytes(extra[12..16].try_into().unwrap_or([0; 4]));
        let header_end = PREFIX_BYTES + header_length as usize;
        let mut header: serde_json::Value =
            serde_json::from_slice(&extra[PREFIX_BYTES..header_end]).unwrap_or(Value::Null);
        header["extra"] = json!(true);
        let header = serde_json::to_vec(&header).unwrap_or_default();
        extra.splice(PREFIX_BYTES..header_end, header.iter().copied());
        extra[12..16].copy_from_slice(&(header.len() as u32).to_le_bytes());
        assert!(
            LandCoverMask::parse_for_grid(&extra, Some(SOURCE_SHA256), SYNTHETIC_GRID).is_err()
        );
    }

    #[test]
    fn artifact_digest_binds_header_and_payload() -> std::io::Result<()> {
        let bytes = artifact(&[1, 2, 3, 4, 0, 7]);
        let mask = LandCoverMask::parse_for_grid(&bytes, Some(SOURCE_SHA256), SYNTHETIC_GRID)?;
        let expected: [u8; 32] = Sha256::digest(&bytes).into();

        assert_eq!(mask.artifact_sha256(), expected);
        Ok(())
    }

    #[test]
    fn production_grid_is_exact_not_just_plausible() -> std::io::Result<()> {
        let mut value = header(&[1, 2, 3, 4, 0, 7]);
        value["grid"] = json!({
            "epsg": 32129,
            "width": 9098,
            "height": 10174,
            "min_x": 810942.0,
            "min_y": 62427.0,
            "max_x": 838236.0,
            "max_y": 92949.0,
            "row_order": "north_to_south",
            "resampling": "nearest"
        });
        let object = value
            .as_object()
            .ok_or_else(|| std::io::Error::other("test header is not an object"))?;
        assert_eq!(
            parse_header(object, Some(SOURCE_SHA256), PRODUCTION_GRID)?,
            PRODUCTION_GRID
        );

        for (field, changed) in [
            ("width", json!(9099)),
            ("height", json!(10175)),
            ("max_x", json!(838237.0)),
        ] {
            let mut changed_header = value.clone();
            changed_header["grid"][field] = changed;
            let changed_object = changed_header
                .as_object()
                .ok_or_else(|| std::io::Error::other("test header is not an object"))?;
            assert!(parse_header(changed_object, Some(SOURCE_SHA256), PRODUCTION_GRID).is_err());
        }
        let mut shifted = value;
        shifted["grid"]["min_x"] = json!(810945.0);
        shifted["grid"]["max_x"] = json!(838239.0);
        let shifted_object = shifted
            .as_object()
            .ok_or_else(|| std::io::Error::other("test header is not an object"))?;
        assert!(parse_header(shifted_object, Some(SOURCE_SHA256), PRODUCTION_GRID).is_err());
        Ok(())
    }
}
