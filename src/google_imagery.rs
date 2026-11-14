//! Opt-in, hash-bound raw captures. No network access or capture generation.
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use image::{ImageFormat, ImageReader, RgbaImage};
use rstar::AABB;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{palette, pyramid::ART_ZOOM, tile_codec::encode_image, world::World};

const TILE_SIZE: u32 = 256;
const STYLE: &str = "isophilly-google-v1";
const MAX_ENTRIES: usize = 65_536;
const MAX_PNG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;

type TileKey = (u32, u32);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document<T> {
    schema_version: u8,
    world_sha256: String,
    iso_bounds: [f64; 4],
    zoom: u8,
    tile_size: u32,
    style: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<CaptureMetadata>,
    tiles: Vec<T>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureMetadata {
    height_origin: f64,
    renderer: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tile {
    x: u32,
    y: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    x: u32,
    y: u32,
    sha256: String,
    attributions: Vec<String>,
    captured_at: String,
}

pub(crate) struct GoogleImagery {
    root: PathBuf,
    entries: HashMap<TileKey, [u8; 32]>,
    world_sha256: [u8; 32],
    sha256: [u8; 32],
    attributions: Vec<String>,
}

pub(crate) fn plan(world: &World) -> Document<Tile> {
    let count = 1_u32 << ART_ZOOM;
    let tiles = (0..count)
        .flat_map(|y| (0..count).map(move |x| (x, y)))
        .filter(|&(x, y)| allowed(world, x, y))
        .map(|(x, y)| Tile { x, y })
        .collect();
    Document {
        schema_version: 1,
        world_sha256: digest_hex(&world.world_sha256),
        iso_bounds: world_bounds(world),
        zoom: ART_ZOOM,
        tile_size: TILE_SIZE,
        style: STYLE.to_owned(),
        capture: None,
        tiles,
    }
}

// The mesh index is already projected in View::SouthEast. Deliberately reject
// even bounding-box intersections, including legacy stadium/downtown textures.
// Two output pixels (at least two projected metres) guard filtering at edges.
fn allowed(world: &World, x: u32, y: u32) -> bool {
    let count = 1_u32 << ART_ZOOM;
    if x >= count || y >= count {
        return false;
    }
    let bounds = world.iso_bounds.tile(ART_ZOOM, x, y);
    let padded = bounds.pad((bounds.width() / TILE_SIZE as f32 * 2.0).max(2.0));
    let query = AABB::from_corners([padded.min_x, padded.min_y], [padded.max_x, padded.max_y]);
    world.has_content(&world.source_envelope(bounds))
        && world
            .mesh_face_tree
            .locate_in_envelope_intersecting(query)
            .next()
            .is_none()
}

impl GoogleImagery {
    /// Validate every PNG, not just the manifest, before any build-cache lookup.
    pub(crate) fn open(root: &Path, world: &World) -> io::Result<Self> {
        let bytes = read_bounded(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
        let manifest: Document<Capture> =
            serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
        if manifest.schema_version != 1
            || manifest.world_sha256 != digest_hex(&world.world_sha256)
            || manifest.iso_bounds != world_bounds(world)
            || manifest.zoom != ART_ZOOM
            || manifest.tile_size != TILE_SIZE
            || manifest.style != STYLE
            || manifest.capture.as_ref().is_some_and(|capture| {
                !capture.height_origin.is_finite() || capture.renderer != "state-plane-unlit-v1"
            })
        {
            return Err(invalid(
                "Google manifest schema, world, grid or style mismatch",
            ));
        }
        if manifest.tiles.is_empty() || manifest.tiles.len() > MAX_ENTRIES {
            return Err(invalid("Google manifest must contain 1..=65536 captures"));
        }
        let mut entries = HashMap::new();
        let mut attributions = BTreeSet::new();
        for capture in manifest.tiles {
            let key = (capture.x, capture.y);
            if !allowed(world, key.0, key.1) {
                return Err(invalid(format!(
                    "Google tile {}/{} is out of range, empty or overlaps photographed mesh",
                    key.0, key.1
                )));
            }
            if capture.attributions.len() < 2
                || !capture
                    .attributions
                    .iter()
                    .any(|value| value == "Google Maps")
                || capture
                    .attributions
                    .iter()
                    .any(|value| value.trim().is_empty())
                || !valid_timestamp(&capture.captured_at)
            {
                return Err(invalid(
                    "Google capture needs attribution and an ISO timestamp",
                ));
            }
            let digest = parse_digest(&capture.sha256)?;
            if entries.insert(key, digest).is_some() {
                return Err(invalid("duplicate Google capture tile"));
            }
            read_image(root, key, &digest)?;
            attributions.extend(capture.attributions);
        }
        Ok(Self {
            root: root.to_owned(),
            entries,
            world_sha256: world.world_sha256,
            sha256: Sha256::digest(&bytes).into(),
            attributions: attributions.into_iter().collect(),
        })
    }

    pub(crate) fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    pub(crate) fn attributions(&self) -> &[String] {
        &self.attributions
    }

    /// Revalidate on public build entry points, including completed-build reuse.
    /// This also catches a file changed after `open` but before building.
    pub(crate) fn validate(&self, world: &World) -> io::Result<()> {
        if self.world_sha256 != world.world_sha256 {
            return Err(invalid("Google imagery belongs to another world"));
        }
        let current = Self::open(&self.root, world)?;
        if current.sha256 != self.sha256 {
            return Err(invalid("Google manifest changed after validation"));
        }
        Ok(())
    }

    pub(crate) fn render_tile(&self, world: &World, x: u32, y: u32) -> io::Result<Option<Vec<u8>>> {
        let Some(digest) = self.entries.get(&(x, y)) else {
            return Ok(None);
        };
        if self.world_sha256 != world.world_sha256 || !allowed(world, x, y) {
            return Err(invalid("Google capture is not allowed in this world"));
        }
        // Reread and hash at use time. A changed/missing capture must never be
        // silently replaced by procedural rendering or written under an old ID.
        let image = read_image(&self.root, (x, y), digest)?;
        encode_image(&finish_capture(image)).map(Some)
    }
}

fn finish_capture(mut image: RgbaImage) -> RgbaImage {
    // Exactly the finish used by the existing renderer, once, on raw pixels.
    // Do not clip this finished image with the ground-plane City mask: an
    // elevated building pixel does not inverse-project to its ground location.
    // Boundary tiles may include a small amount of neighboring geography.
    palette::bake_display_finish(image.as_mut());
    image
}

fn read_image(root: &Path, (x, y): TileKey, digest: &[u8; 32]) -> io::Result<RgbaImage> {
    let path = root
        .join(ART_ZOOM.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"));
    let bytes = read_bounded(&path, MAX_PNG_BYTES)?;
    if <[u8; 32]>::from(Sha256::digest(&bytes)) != *digest {
        return Err(invalid(format!(
            "Google PNG hash mismatch: {}",
            path.display()
        )));
    }
    // Check dimensions before decoding to bound allocation for untrusted PNGs.
    let reader = ImageReader::with_format(io::Cursor::new(&bytes), ImageFormat::Png);
    if reader
        .into_dimensions()
        .map_err(|error| invalid(error.to_string()))?
        != (TILE_SIZE, TILE_SIZE)
    {
        return Err(invalid("Google PNG must be exactly 256x256"));
    }
    let mut reader = ImageReader::with_format(io::Cursor::new(&bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TILE_SIZE);
    limits.max_image_height = Some(TILE_SIZE);
    limits.max_alloc = Some(MAX_PNG_BYTES);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| invalid(error.to_string()))?;
    let image = image
        .as_rgba8()
        .ok_or_else(|| invalid("Google PNG must be 8-bit RGBA"))?;
    if image.pixels().any(|pixel| pixel[3] != 255) {
        return Err(invalid("Google PNG must be fully opaque"));
    }
    Ok(image.clone())
}

fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid(format!(
            "Google input exceeds size limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn world_bounds(world: &World) -> [f64; 4] {
    let bounds = world.iso_bounds;
    [bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y].map(f64::from)
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_digest(value: &str) -> io::Result<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("Google digest must be lowercase SHA256 hex"));
    }
    let mut digest = [0; 32];
    for (output, pair) in digest.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let nibble = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        *output = nibble(pair[0]) * 16 + nibble(pair[1]);
    }
    Ok(digest)
}

// ISO 8601 extended timestamp (RFC3339), without an extra date dependency.
fn valid_timestamp(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        b.get(range)?.iter().try_fold(0, |n, c| {
            c.is_ascii_digit().then(|| n * 10 + u32::from(c - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    if year == 0 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let mut end = 19;
    if b.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if start == end {
            return false;
        }
    }
    if b.get(end..) == Some(b"Z") {
        return true;
    }
    b.len() == end + 6
        && matches!(b[end], b'+' | b'-')
        && b[end + 3] == b':'
        && number(end + 1..end + 3).is_some_and(|n| n <= 23)
        && number(end + 4..end + 6).is_some_and(|n| n <= 59)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{View, load_world};
    use image::Rgba;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Fixture {
        root: PathBuf,
        world: World,
        key: TileKey,
    }

    impl Fixture {
        fn new() -> io::Result<Self> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "isophilly-google-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root)?;
            let hex = include_str!("../tests/fixtures/world-v9.hex").trim();
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(io::Error::other))
                .collect::<io::Result<Vec<_>>>()?;
            fs::write(root.join("world.bin"), bytes)?;
            let mut world = load_world(&root.join("world.bin"))?;
            // Use the production parser/indexes but a generous test extent so
            // there are both protected mesh tiles and mesh-free city tiles.
            world.iso_bounds = world.iso_bounds.pad(128.0);
            let key = plan(&world)
                .tiles
                .first()
                .map(|t| (t.x, t.y))
                .ok_or_else(|| invalid("fixture has no mesh-free tile"))?;
            let fixture = Self { root, world, key };
            fixture.write_png(RgbaImage::from_pixel(256, 256, Rgba([120, 80, 40, 255])))?;
            fixture.write_manifest(fixture.manifest()?)?;
            Ok(fixture)
        }

        fn png_path(&self) -> PathBuf {
            self.root
                .join("8")
                .join(self.key.0.to_string())
                .join(format!("{}.png", self.key.1))
        }

        fn write_png(&self, image: RgbaImage) -> io::Result<()> {
            let path = self.png_path();
            fs::create_dir_all(path.parent().ok_or_else(|| invalid("missing parent"))?)?;
            image
                .save_with_format(path, ImageFormat::Png)
                .map_err(io::Error::other)
        }

        fn manifest(&self) -> io::Result<serde_json::Value> {
            let mut document = serde_json::to_value(plan(&self.world)).map_err(io::Error::other)?;
            document["capture"] = serde_json::json!({
                "height_origin": 32.125,
                "renderer": "state-plane-unlit-v1"
            });
            let hash: [u8; 32] = Sha256::digest(fs::read(self.png_path())?).into();
            document["tiles"] = serde_json::json!([{
                "x": self.key.0, "y": self.key.1, "sha256": digest_hex(&hash),
                "attributions": ["Google Maps", "Provider", "Google Maps"],
                "captured_at": "2026-05-01T12:00:00.000Z"
            }]);
            Ok(document)
        }

        fn write_manifest(&self, document: serde_json::Value) -> io::Result<()> {
            fs::write(
                self.root.join("manifest.json"),
                serde_json::to_vec(&document).map_err(io::Error::other)?,
            )
        }

        fn open(&self) -> io::Result<GoogleImagery> {
            GoogleImagery::open(&self.root, &self.world)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn canonical_plan_excludes_every_projected_mesh_envelope_and_empty_tile() -> io::Result<()> {
        let f = Fixture::new()?;
        let document = plan(&f.world);
        assert_eq!(document.schema_version, 1);
        assert_eq!(document.iso_bounds, world_bounds(&f.world));
        assert_eq!(document.world_sha256, digest_hex(&f.world.world_sha256));
        assert_eq!(document.zoom, 8);
        assert_eq!(document.tile_size, 256);
        assert_eq!(document.style, STYLE);
        let planned: BTreeSet<_> = document.tiles.iter().map(|t| (t.x, t.y)).collect();
        let mut protected = 0;
        let mut empty = 0;
        for y in 0..256 {
            for x in 0..256 {
                let bounds = f.world.iso_bounds.tile(8, x, y);
                let query =
                    AABB::from_corners([bounds.min_x, bounds.min_y], [bounds.max_x, bounds.max_y]);
                if f.world
                    .mesh_face_tree
                    .locate_in_envelope_intersecting(query)
                    .next()
                    .is_some()
                {
                    protected += 1;
                    assert!(!planned.contains(&(x, y)));
                }
                if !f.world.has_content(&f.world.source_envelope(bounds)) {
                    empty += 1;
                    assert!(!planned.contains(&(x, y)));
                }
                assert_eq!(planned.contains(&(x, y)), allowed(&f.world, x, y));
            }
        }
        assert!(protected > 0);
        assert!(empty > 0);
        assert!(!allowed(&f.world, 256, 0));
        Ok(())
    }

    #[test]
    fn full_manifest_digest_and_sorted_attributions_bind_identity() -> io::Result<()> {
        let f = Fixture::new()?;
        let first = f.open()?;
        let expected: [u8; 32] = Sha256::digest(fs::read(f.root.join("manifest.json"))?).into();
        assert_eq!(first.sha256(), &expected);
        assert_eq!(first.attributions(), ["Google Maps", "Provider"]);
        let mut changed = f.manifest()?;
        changed["tiles"][0]["captured_at"] = "2026-05-02T12:00:00Z".into();
        f.write_manifest(changed)?;
        assert_ne!(first.sha256(), f.open()?.sha256());
        assert!(first.validate(&f.world).is_err());
        let mut changed = f.manifest()?;
        changed["capture"]["height_origin"] = 64.25.into();
        f.write_manifest(changed)?;
        assert_ne!(first.sha256(), f.open()?.sha256());
        // Earlier manifests without the optional capture metadata remain valid.
        let mut legacy = f.manifest()?;
        if let Some(object) = legacy.as_object_mut() {
            object.remove("capture");
        }
        f.write_manifest(legacy)?;
        assert!(f.open().is_ok());
        Ok(())
    }

    #[test]
    fn changed_or_missing_pixels_cannot_reuse_a_completed_build() -> io::Result<()> {
        let f = Fixture::new()?;
        let validated = f.open()?;
        f.write_png(RgbaImage::from_pixel(256, 256, Rgba([11, 22, 33, 255])))?;
        // The manifest is unchanged, so these failures specifically test PNG
        // verification rather than a changed manifest digest.
        assert!(f.open().is_err());
        assert!(
            crate::server::prebuild_is_complete_with_google(&f.world, None, None, &validated)
                .is_err()
        );
        assert!(validated.render_tile(&f.world, f.key.0, f.key.1).is_err());
        f.write_manifest(f.manifest()?)?;
        let updated = f.open()?;
        assert_ne!(validated.sha256(), updated.sha256());
        fs::remove_file(f.png_path())?;
        assert!(
            crate::server::prebuild_is_complete_with_google(&f.world, None, None, &updated)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn validated_captures_reject_a_different_world_at_use_time() -> io::Result<()> {
        let mut f = Fixture::new()?;
        let validated = f.open()?;
        f.world.world_sha256[0] ^= 1;
        assert!(f.open().is_err());
        assert!(validated.validate(&f.world).is_err());
        assert!(validated.render_tile(&f.world, f.key.0, f.key.1).is_err());
        Ok(())
    }

    #[test]
    fn rejects_schema_world_grid_style_duplicates_and_unknown_paths() -> io::Result<()> {
        let f = Fixture::new()?;
        for (field, value) in [
            ("schema_version", serde_json::json!(2)),
            ("world_sha256", serde_json::json!("0".repeat(64))),
            ("world_sha256", serde_json::json!("A".repeat(64))),
            ("iso_bounds", serde_json::json!([0, 0, 1, 1])),
            ("zoom", serde_json::json!(7)),
            ("tile_size", serde_json::json!(512)),
            ("style", serde_json::json!("invented-style")),
        ] {
            let mut document = f.manifest()?;
            document[field] = value;
            f.write_manifest(document)?;
            assert!(f.open().is_err(), "accepted invalid {field}");
        }
        let mut duplicate = f.manifest()?;
        duplicate["tiles"] = serde_json::json!([duplicate["tiles"][0], duplicate["tiles"][0]]);
        f.write_manifest(duplicate)?;
        assert!(f.open().is_err());
        let mut document = f.manifest()?;
        document["tiles"][0]["path"] = "../../capture.png".into();
        f.write_manifest(document)?;
        assert!(f.open().is_err());
        Ok(())
    }

    #[test]
    fn rejects_forbidden_mesh_overlap_and_out_of_range_tiles() -> io::Result<()> {
        let f = Fixture::new()?;
        let forbidden = (0..256)
            .flat_map(|y| (0..256).map(move |x| (x, y)))
            .find(|&(x, y)| {
                let b = f.world.iso_bounds.tile(8, x, y);
                f.world
                    .mesh_face_tree
                    .locate_in_envelope_intersecting(AABB::from_corners(
                        [b.min_x, b.min_y],
                        [b.max_x, b.max_y],
                    ))
                    .next()
                    .is_some()
            })
            .ok_or_else(|| invalid("no protected fixture tile"))?;
        for (x, y) in [forbidden, (256, 0), (0, 256), (u32::MAX, u32::MAX)] {
            let mut document = f.manifest()?;
            document["tiles"][0]["x"] = x.into();
            document["tiles"][0]["y"] = y.into();
            f.write_manifest(document)?;
            assert!(f.open().is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_missing_bad_hash_wrong_dimensions_rgb_and_transparent_pngs() -> io::Result<()> {
        let f = Fixture::new()?;
        let validated = f.open()?;
        for (w, h, alpha) in [
            (255, 256, 255),
            (256, 257, 255),
            (256, 256, 0),
            (256, 256, 254),
        ] {
            f.write_png(RgbaImage::from_pixel(w, h, Rgba([1, 2, 3, alpha])))?;
            f.write_manifest(f.manifest()?)?;
            assert!(f.open().is_err());
        }
        let rgb = image::RgbImage::from_pixel(256, 256, image::Rgb([1, 2, 3]));
        rgb.save_with_format(f.png_path(), ImageFormat::Png)
            .map_err(io::Error::other)?;
        f.write_manifest(f.manifest()?)?;
        assert!(f.open().is_err());
        f.write_png(RgbaImage::from_pixel(256, 256, Rgba([1, 2, 3, 255])))?;
        let mut document = f.manifest()?;
        document["tiles"][0]["sha256"] = "0".repeat(64).into();
        f.write_manifest(document)?;
        assert!(f.open().is_err());
        // Completed-build checks must reject captures changed since open.
        assert!(validated.validate(&f.world).is_err());
        assert!(validated.render_tile(&f.world, f.key.0, f.key.1).is_err());
        fs::remove_file(f.png_path())?;
        assert!(f.open().is_err());
        assert!(validated.validate(&f.world).is_err());
        Ok(())
    }

    #[test]
    fn rejects_large_files_and_unbounded_or_empty_capture_lists() -> io::Result<()> {
        let f = Fixture::new()?;
        let file = File::options().write(true).open(f.png_path())?;
        file.set_len(MAX_PNG_BYTES + 1)?;
        assert!(f.open().is_err());
        assert!(read_bounded(&f.png_path(), MAX_PNG_BYTES).is_err());
        for count in [0, MAX_ENTRIES + 1] {
            let mut document = f.manifest()?;
            document["tiles"] = serde_json::Value::Array(vec![document["tiles"][0].clone(); count]);
            f.write_manifest(document)?;
            assert!(f.open().is_err());
        }
        Ok(())
    }

    #[test]
    fn validates_iso_dates_and_required_attribution() -> io::Result<()> {
        for value in ["2026-05-01T12:34:56Z", "2024-02-29T12:34:56.789+01:30"] {
            assert!(valid_timestamp(value));
        }
        for value in [
            "",
            "2026-05-01",
            "2026-02-29T12:34:56Z",
            "2026-05-01T25:00:00Z",
            "2026-05-01T00:00:00.Z",
            "2026-05-01T00:00:00+25:00",
            "2026-05-01T00:00:00Zextra",
            "ééééééééééééé",
        ] {
            assert!(!valid_timestamp(value), "accepted {value}");
        }
        let f = Fixture::new()?;
        for value in [
            serde_json::json!([]),
            serde_json::json!([""]),
            serde_json::json!([" "]),
            serde_json::json!(["Google Maps"]),
            serde_json::json!(["Google", "Provider"]),
        ] {
            let mut document = f.manifest()?;
            document["tiles"][0]["attributions"] = value;
            f.write_manifest(document)?;
            assert!(f.open().is_err());
        }
        for value in [
            "💥".repeat(16),
            "A".repeat(64),
            "g".repeat(64),
            "0".repeat(63),
        ] {
            assert!(parse_digest(&value).is_err());
        }
        Ok(())
    }

    #[test]
    fn raw_pixels_receive_existing_finish_once_without_ground_plane_clipping() -> io::Result<()> {
        let mut f = Fixture::new()?;
        f.world.mesh_face_tree = rstar::RTree::new();
        // A tile whose centre lies inside the city, away from the boundary.
        let tile = plan(&f.world)
            .tiles
            .into_iter()
            .find(|tile| {
                let b = f.world.iso_bounds.tile(8, tile.x, tile.y);
                let point =
                    View::SouthEast.inverse((b.min_x + b.max_x) * 0.5, (b.min_y + b.max_y) * 0.5);
                f.world.city.iter().any(|ring| ring.contains(point))
            })
            .ok_or_else(|| invalid("no interior fixture tile"))?;
        let raw = [120, 80, 40];
        let image = finish_capture(RgbaImage::from_pixel(
            256,
            256,
            Rgba([raw[0], raw[1], raw[2], 255]),
        ));
        let expected = palette::display_finish(raw);
        assert_ne!(expected, palette::display_finish(expected));
        let pixel = image.get_pixel(128, 128);
        assert_eq!(pixel.0, [expected[0], expected[1], expected[2], 255]);
        // Every raw pixel is retained, including displaced roofs at City edges.
        assert!(
            image
                .pixels()
                .all(|p| p.0 == [expected[0], expected[1], expected[2], 255])
        );
        f.key = (tile.x, tile.y);
        f.write_png(RgbaImage::from_pixel(
            256,
            256,
            Rgba([raw[0], raw[1], raw[2], 255]),
        ))?;
        f.write_manifest(f.manifest()?)?;
        let captures = f.open()?;
        let encoded = captures
            .render_tile(&f.world, tile.x, tile.y)?
            .ok_or_else(|| invalid("missing encoded capture"))?;
        let decoded = image::load_from_memory_with_format(&encoded, ImageFormat::WebP)
            .map_err(io::Error::other)?
            .into_rgba8();
        assert_eq!(decoded, image);
        assert!(captures.render_tile(&f.world, 255, 255)?.is_none());
        Ok(())
    }
}
