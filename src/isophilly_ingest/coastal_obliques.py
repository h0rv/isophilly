from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import shutil
import struct
import subprocess
from collections import Counter
from contextlib import suppress
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import BinaryIO
from urllib.parse import unquote, urljoin, urlparse

import httpx

from .config import ROOT
from .download import USER_AGENT

COASTAL_DIR = ROOT / "data" / "coastal-obliques"
COLLECTION_URLS = {
    "schuylkill-2014": (
        "https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/"
        "DelEstCZ/2014/DECZ/Obliques/DEP%20-%20Schuylkill/JPEG/"
    ),
    "delaware-2014": (
        "https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/"
        "DelEstCZ/2014/DECZ/Obliques/DEP%20-%20DECZ/JPEG/"
    ),
    "little-tinicum-2014": (
        "https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/"
        "DelEstCZ/2014/DECZ/Obliques/DEP-LTI/JPEG/"
    ),
}
EXPECTED_COUNTS = {
    "schuylkill-2014": 191,
    "delaware-2014": 502,
    "little-tinicum-2014": 51,
}
AUDITED_LISTING_SHA256 = {
    "schuylkill-2014": "ca3fe773fcc25077e2b5fd2d8a00d11b95ac9db5b363d6c81dedba24caac5b5c",
    "delaware-2014": "3f984c0f886765148991a436b54eb7590a3329c40e730009332654cec49b4c59",
    "little-tinicum-2014": "24e73e46e58f7b052eaba7d12cd18af07fd2b160e48f8942ed8bada2e72d7632",
}
AUDITED_FRAME_MANIFEST_SHA256 = {
    "schuylkill-2014": "df0a3d4d45f184c19bc87cf50854718179a96235d3a5bb8e6f35f375d807a605",
    "delaware-2014": "78037be7c94377a65c752865468cc4d4618cf281bc124a4dc2d38739be14f5d2",
    "little-tinicum-2014": "1f9139a9cb2f60cb3c5e0e00f0b6da8e8f17b1ebd13ae356bee93235e71662a2",
}
AUDITED_FRAME_BYTES = {
    "schuylkill-2014": 264_790_713,
    "delaware-2014": 508_827_973,
    "little-tinicum-2014": 33_224_886,
}
RIGHTS_WARNING = (
    "RIGHTS WARNING: PASDA metadata does not state a conventional open-data license. "
    "Keep this local; do not publish source images or derived textures until Penn State/PA DEP "
    "confirms notification and derivative-redistribution terms in writing."
)
LISTING_ROW = re.compile(
    rb"(?P<size>\d+)\s*<A\s+HREF=[\"'](?P<href>[^\"']+\.(?:jpe?g))[\"'][^>]*>"
    rb"(?P<label>[^<]+)</A>",
    re.IGNORECASE,
)
CONTENT_RANGE = re.compile(r"^bytes (?P<start>\d+)-(?P<end>\d+)/(?P<total>\d+)$")
DOWNLOAD_CHUNK_BYTES = 1024 * 1024
MIN_SFM_SEQUENCE = 20
CONTACT_SHEET_THUMBNAIL = "320x240"
CONTACT_SHEET_COLUMNS = 5
CONTACT_SHEET_GEOMETRY = "+8+24"
LABEL_FONT_FAMILY = "Noto Sans"
AUDITED_LABEL_FONT_SHA256 = "478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823"
SFM_PYCOLMAP_VERSION = "4.1.1"
SFM_PYCOLMAP_LINUX_X86_64_SHA256 = (
    "5fdc8638461a5f69d3ecae3dca339a7347967af9493b36e8113da4470ee90d78"
)
SFM_IMAGE_MANIFEST_SHA256 = "2d6dfc5fc583b2145b32df221a273f0aec5038d6c1bad209d50d1f2da877d40d"
SFM_MATCH_BREAK_AFTER = 92
SFM_EXCLUDED_SEQUENCE_NUMBERS = (191,)
SFM_LOCAL_MATCH_WINDOW = 10
SFM_FIRST_CAPTURED_AT = "2014-07-02T10:33:09+00:00"
SFM_LAST_CAPTURED_AT = "2014-07-02T10:47:18+00:00"
SFM_BREAK_SECONDS = 174
SFM_EXACT_CAMERA_GROUPS = 43
SFM_SINGLETON_CAMERA_GROUPS = 16
SFM_EXPECTED_DIMENSIONS = {
    (3607, 2404): 36,
    (3607, 2405): 150,
    (3607, 2406): 2,
    (3607, 5411): 1,
    (3607, 5412): 1,
    (3607, 5717): 1,
}
SFM_EXPECTED_FOCALS_MM = {
    70: 58,
    75: 5,
    80: 10,
    90: 9,
    95: 11,
    100: 9,
    110: 11,
    115: 12,
    120: 11,
    130: 7,
    135: 4,
    140: 2,
    150: 7,
    160: 4,
    170: 8,
    180: 4,
    185: 5,
    195: 1,
    200: 2,
    210: 5,
    220: 1,
    230: 1,
    240: 3,
    270: 1,
}


class CoastalObliqueError(RuntimeError):
    pass


@dataclass(frozen=True, slots=True)
class Frame:
    name: str
    url: str
    bytes: int


@dataclass(frozen=True, slots=True)
class Inventory:
    schema_version: int
    collection: str
    source_url: str
    listing_sha256: str
    fetched_at: str
    frames: tuple[Frame, ...]


@dataclass(frozen=True, slots=True)
class SfmFrame:
    sequence: int
    name: str
    staged_name: str
    sha256: str
    width: int
    height: int
    captured_at: datetime
    focal_mm: int

    @property
    def included(self) -> bool:
        return self.sequence not in SFM_EXCLUDED_SEQUENCE_NUMBERS

    @property
    def focal_pixels(self) -> float:
        # These files were resized after capture. The long pixel edge corresponds to the
        # 36 mm edge of the full-frame sensor in either landscape or portrait orientation.
        return self.focal_mm * max(self.width, self.height) / 36.0


def _collection_dir(collection: str) -> Path:
    return COASTAL_DIR / collection


def _write_json_atomic(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".part")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(DOWNLOAD_CHUNK_BYTES):
            digest.update(chunk)
    return digest.hexdigest()


def parse_listing(content: bytes, source_url: str) -> tuple[Frame, ...]:
    frames: list[Frame] = []
    for match in LISTING_ROW.finditer(content):
        href = match.group("href").decode("ascii")
        label = match.group("label").decode("utf-8").strip()
        name = unquote(Path(urlparse(href).path).name)
        if label != name:
            raise CoastalObliqueError(
                f"PASDA listing label does not match link: {label!r}, {name!r}"
            )
        size = int(match.group("size"))
        if size <= 0:
            raise CoastalObliqueError(f"PASDA lists an empty JPEG: {name}")
        frames.append(Frame(name, urljoin(source_url, href), size))
    frames.sort(key=lambda frame: frame.name)
    if not frames:
        raise CoastalObliqueError("PASDA listing contained no JPEG frames")
    if len({frame.name for frame in frames}) != len(frames):
        raise CoastalObliqueError("PASDA listing contains duplicate JPEG names")
    return tuple(frames)


def frame_manifest_sha256(frames: tuple[Frame, ...]) -> str:
    """Hash only ordered source authority, excluding fetch time and JSON formatting."""
    payload = json.dumps(
        [asdict(frame) for frame in frames],
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def inventory_dict(inventory: Inventory) -> dict[str, object]:
    return {
        "schema_version": inventory.schema_version,
        "collection": inventory.collection,
        "source_url": inventory.source_url,
        "listing_sha256": inventory.listing_sha256,
        "frame_manifest_sha256": frame_manifest_sha256(inventory.frames),
        "fetched_at": inventory.fetched_at,
        "rights_warning": RIGHTS_WARNING,
        "counts": {"jpeg_frames": len(inventory.frames)},
        "bytes": {"jpeg_frames": sum(frame.bytes for frame in inventory.frames)},
        "frames": [asdict(frame) for frame in inventory.frames],
    }


def parse_inventory(value: object) -> Inventory:
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        raise CoastalObliqueError("unsupported coastal-oblique inventory schema")
    collection = value.get("collection")
    source_url = value.get("source_url")
    listing_sha256 = value.get("listing_sha256")
    fetched_at = value.get("fetched_at")
    if (
        not isinstance(collection, str)
        or collection not in COLLECTION_URLS
        or source_url != COLLECTION_URLS[collection]
        or not isinstance(listing_sha256, str)
        or len(listing_sha256) != 64
        or not isinstance(fetched_at, str)
    ):
        raise CoastalObliqueError("coastal-oblique inventory provenance is invalid")
    raw_frames = value.get("frames")
    if not isinstance(raw_frames, list):
        raise CoastalObliqueError("coastal-oblique inventory has no frame list")
    frames: list[Frame] = []
    for raw in raw_frames:
        if not isinstance(raw, dict):
            raise CoastalObliqueError("coastal-oblique inventory contains an invalid frame")
        name, url, size = raw.get("name"), raw.get("url"), raw.get("bytes")
        if (
            not isinstance(name, str)
            or Path(name).name != name
            or Path(name).suffix.lower() not in {".jpg", ".jpeg"}
            or not isinstance(url, str)
            or not url.startswith(source_url)
            or unquote(Path(urlparse(url).path).name) != name
            or not isinstance(size, int)
            or size <= 0
        ):
            raise CoastalObliqueError(f"invalid frame in coastal-oblique inventory: {name!r}")
        frames.append(Frame(name, url, size))
    if len(frames) != EXPECTED_COUNTS[collection]:
        raise CoastalObliqueError(
            f"{collection} inventory has {len(frames)} JPEGs; expected "
            f"{EXPECTED_COUNTS[collection]}"
        )
    names = [frame.name for frame in frames]
    if len(set(names)) != len(names):
        raise CoastalObliqueError(f"{collection} inventory has duplicate JPEG names")
    if names != sorted(names):
        raise CoastalObliqueError(f"{collection} inventory frames are not in canonical order")
    recorded_manifest = value.get("frame_manifest_sha256")
    if recorded_manifest is not None and recorded_manifest != frame_manifest_sha256(tuple(frames)):
        raise CoastalObliqueError(f"{collection} inventory frame-manifest checksum is invalid")
    return Inventory(1, collection, source_url, listing_sha256, fetched_at, tuple(frames))


def load_inventory(collection: str) -> Inventory:
    path = _collection_dir(collection) / "inventory.json"
    try:
        inventory = parse_inventory(json.loads(path.read_text()))
    except (FileNotFoundError, json.JSONDecodeError) as error:
        raise CoastalObliqueError(
            f"missing or corrupt inventory: {path}; run "
            f"`poe oblique-plan --collection {collection}`"
        ) from error
    audited_hash = AUDITED_LISTING_SHA256[collection]
    if inventory.listing_sha256 != audited_hash:
        raise CoastalObliqueError(
            f"inventory listing hash is not the audited pin for {collection}: "
            f"{inventory.listing_sha256} != {audited_hash}"
        )
    actual_bytes = sum(frame.bytes for frame in inventory.frames)
    if actual_bytes != AUDITED_FRAME_BYTES[collection]:
        raise CoastalObliqueError(
            f"inventory byte total is not the audited pin for {collection}: "
            f"{actual_bytes} != {AUDITED_FRAME_BYTES[collection]}"
        )
    actual_manifest = frame_manifest_sha256(inventory.frames)
    expected_manifest = AUDITED_FRAME_MANIFEST_SHA256[collection]
    if actual_manifest != expected_manifest:
        raise CoastalObliqueError(
            f"inventory frame manifest is not the audited pin for {collection}: "
            f"{actual_manifest} != {expected_manifest}"
        )
    return inventory


def create_inventory(
    collection: str, *, refresh: bool = False, client: httpx.Client | None = None
) -> Inventory:
    if collection not in COLLECTION_URLS:
        raise CoastalObliqueError(f"unsupported collection: {collection}")
    path = _collection_dir(collection) / "inventory.json"
    if path.exists() and not refresh:
        return load_inventory(collection)
    if not path.exists() and _cached_artifacts(collection):
        raise CoastalObliqueError(
            "refusing to create an inventory while collection artifacts exist without their "
            "matching inventory.json; restore it or archive the collection after review"
        )
    owns_client = client is None
    if client is None:
        client = httpx.Client(
            headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=120
        )
    try:
        response = client.get(COLLECTION_URLS[collection])
        response.raise_for_status()
    finally:
        if owns_client:
            client.close()
    frames = parse_listing(response.content, COLLECTION_URLS[collection])
    expected = EXPECTED_COUNTS[collection]
    if len(frames) != expected:
        raise CoastalObliqueError(
            f"PASDA now lists {len(frames)} JPEGs for {collection}; audited count is {expected}. "
            "Stop and review the changed source before accepting it."
        )
    inventory = Inventory(
        1,
        collection,
        COLLECTION_URLS[collection],
        hashlib.sha256(response.content).hexdigest(),
        datetime.now(UTC).isoformat(),
        frames,
    )
    if path.exists():
        try:
            old = load_inventory(collection)
        except CoastalObliqueError:
            if _cached_artifacts(collection):
                raise CoastalObliqueError(
                    "refusing to replace a corrupt inventory while cached JPEG or review "
                    "artifacts exist; restore the matching inventory or archive the collection"
                ) from None
        else:
            if old.listing_sha256 != inventory.listing_sha256 and _cached_artifacts(collection):
                raise CoastalObliqueError(
                    "refusing to replace a changed inventory while cached JPEG, partial, "
                    "progress, or review artifacts exist; archive the collection after review"
                )
            progress = _load_progress(collection, old)
            if progress["frames"] and old.listing_sha256 != inventory.listing_sha256:
                # Kept as a defense if the artifact detector is narrowed later.
                raise CoastalObliqueError(
                    "refusing to replace an inventory with download progress; archive or "
                    "remove the collection directory after reviewing the PASDA change"
                )
    audited_hash = AUDITED_LISTING_SHA256[collection]
    if inventory.listing_sha256 != audited_hash:
        raise CoastalObliqueError(
            f"PASDA listing SHA-256 changed for {collection}: "
            f"{inventory.listing_sha256} != audited {audited_hash}. `--refresh` cannot accept "
            "a new source. Re-audit the directory, rights, counts, and sizes, then update the "
            "reviewed AUDITED_LISTING_SHA256 constant and documentation."
        )
    actual_manifest = frame_manifest_sha256(inventory.frames)
    expected_manifest = AUDITED_FRAME_MANIFEST_SHA256[collection]
    if (
        sum(frame.bytes for frame in inventory.frames) != AUDITED_FRAME_BYTES[collection]
        or actual_manifest != expected_manifest
    ):
        raise CoastalObliqueError(
            f"PASDA parsed frame manifest changed for {collection}: {actual_manifest} != "
            f"audited {expected_manifest}. Re-audit every ordered URL and byte count before "
            "updating the reviewed semantic pin."
        )
    _write_json_atomic(path, inventory_dict(inventory))
    return inventory


def _empty_progress(inventory: Inventory) -> dict[str, object]:
    return {
        "schema_version": 2,
        "inventory_listing_sha256": inventory.listing_sha256,
        "frames": {},
    }


def _load_progress(collection: str, inventory: Inventory) -> dict[str, object]:
    path = _collection_dir(collection) / "progress.json"
    try:
        value = json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return _empty_progress(inventory)
    if (
        not isinstance(value, dict)
        or value.get("schema_version") != 2
        or value.get("inventory_listing_sha256") != inventory.listing_sha256
        or not isinstance(value.get("frames"), dict)
    ):
        return _empty_progress(inventory)
    return value


def _cached_artifacts(collection: str) -> tuple[Path, ...]:
    root = _collection_dir(collection)
    if not root.exists():
        return ()
    return tuple(
        sorted(
            path for path in root.rglob("*") if path.is_file() and path != root / "inventory.json"
        )
    )


def _downloaded_entry_valid(frame: Frame, entry: object, path: Path) -> bool:
    if not isinstance(entry, dict) or entry.get("status") != "downloaded" or not path.is_file():
        return False
    digest = entry.get("sha256")
    return (
        path.stat().st_size == frame.bytes
        and isinstance(digest, str)
        and len(digest) == 64
        and sha256_file(path) == digest
    )


def _validate_jpeg_file(path: Path) -> tuple[int, int]:
    with path.open("rb") as source:
        width, height = read_jpeg_dimensions(source)
        source.seek(-2, 2)
        if source.read(2) != b"\xff\xd9":
            raise CoastalObliqueError(f"JPEG has no final EOI marker: {path.name}")
    executable = shutil.which("magick")
    if executable is None:
        raise CoastalObliqueError(
            "ImageMagick is required to fully decode and validate downloaded JPEGs"
        )
    try:
        subprocess.run(
            [executable, str(path), "null:"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as error:
        detail = error.stderr.decode("utf-8", errors="replace").strip()
        raise CoastalObliqueError(f"ImageMagick rejected {path.name}: {detail}") from error
    return width, height


def _record_complete_frame(
    frame: Frame,
    path: Path,
    entries: dict[str, object],
    progress_path: Path,
    progress: dict[str, object],
) -> tuple[str, int, int]:
    width, height = _validate_jpeg_file(path)
    digest = sha256_file(path)
    entries[frame.name] = {
        "status": "downloaded",
        "bytes": frame.bytes,
        "sha256": digest,
        "width": width,
        "height": height,
    }
    _write_json_atomic(progress_path, progress)
    return digest, width, height


def download_frame(
    collection: str,
    frame: Frame,
    inventory: Inventory,
    client: httpx.Client,
) -> tuple[Path, str]:
    root = _collection_dir(collection)
    raw = root / "jpeg"
    raw.mkdir(parents=True, exist_ok=True)
    final = raw / frame.name
    partial = raw / f"{frame.name}.part"
    progress_path = root / "progress.json"
    progress = _load_progress(collection, inventory)
    entries = progress["frames"]
    assert isinstance(entries, dict)
    if _downloaded_entry_valid(frame, entries.get(frame.name), final):
        entry = entries[frame.name]
        assert isinstance(entry, dict)
        partial.unlink(missing_ok=True)
        return final, str(entry["sha256"])
    if final.is_file() and final.stat().st_size == frame.bytes:
        try:
            digest, _, _ = _record_complete_frame(frame, final, entries, progress_path, progress)
        except CoastalObliqueError:
            final.unlink()
        else:
            partial.unlink(missing_ok=True)
            return final, digest
    if partial.is_file() and partial.stat().st_size == frame.bytes:
        try:
            digest, _, _ = _record_complete_frame(frame, partial, entries, progress_path, progress)
        except CoastalObliqueError:
            partial.unlink()
        else:
            partial.replace(final)
            return final, digest
    if final.exists():
        if partial.exists():
            partial.unlink()
        final.replace(partial)
    offset = partial.stat().st_size if partial.exists() else 0
    if offset > frame.bytes:
        partial.unlink()
        offset = 0
    headers = {"Range": f"bytes={offset}-"} if offset else {}
    entries[frame.name] = {"status": "downloading", "downloaded_bytes": offset}
    _write_json_atomic(progress_path, progress)
    with client.stream("GET", frame.url, headers=headers) as response:
        response.raise_for_status()
        if offset:
            match = CONTENT_RANGE.fullmatch(response.headers.get("Content-Range", ""))
            if (
                response.status_code != 206
                or match is None
                or int(match.group("start")) != offset
                or int(match.group("total")) != frame.bytes
            ):
                raise CoastalObliqueError(f"invalid resume response for {frame.name}")
        elif response.status_code != 200:
            raise CoastalObliqueError(f"unexpected HTTP {response.status_code} for {frame.name}")
        mode = "ab" if offset else "wb"
        with partial.open(mode) as output:
            for chunk in response.iter_bytes(DOWNLOAD_CHUNK_BYTES):
                output.write(chunk)
    if partial.stat().st_size != frame.bytes:
        raise CoastalObliqueError(
            f"short JPEG for {frame.name}: {partial.stat().st_size} != {frame.bytes}"
        )
    digest, _, _ = _record_complete_frame(frame, partial, entries, progress_path, progress)
    partial.replace(final)
    return final, digest


def read_jpeg_dimensions(source: BinaryIO) -> tuple[int, int]:
    if source.read(2) != b"\xff\xd8":
        raise CoastalObliqueError("download is not a JPEG")
    while True:
        marker_start = source.read(1)
        if not marker_start:
            raise CoastalObliqueError("JPEG has no size marker")
        if marker_start != b"\xff":
            continue
        while (marker := source.read(1)) == b"\xff":
            pass
        if not marker:
            raise CoastalObliqueError("truncated JPEG marker")
        code = marker[0]
        if code in {0xD8, 0xD9} or 0xD0 <= code <= 0xD7:
            continue
        length_bytes = source.read(2)
        if len(length_bytes) != 2:
            raise CoastalObliqueError("truncated JPEG segment")
        length = struct.unpack(">H", length_bytes)[0]
        if length < 2:
            raise CoastalObliqueError("invalid JPEG segment length")
        if code in {0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF}:
            payload = source.read(5)
            if len(payload) != 5:
                raise CoastalObliqueError("truncated JPEG size marker")
            height, width = struct.unpack(">HH", payload[1:])
            if width <= 0 or height <= 0:
                raise CoastalObliqueError("invalid JPEG dimensions")
            return width, height
        source.seek(length - 2, 1)


def _exif_metadata(path: Path) -> dict[str, str | None]:
    fields = (
        "Make",
        "Model",
        "Orientation",
        "DateTimeOriginal",
        "LensModel",
        "FocalLength",
        "FocalLengthIn35mmFilm",
        "GPSLatitude",
        "GPSLatitudeRef",
        "GPSLongitude",
        "GPSLongitudeRef",
    )
    result = {field: None for field in fields}
    executable = shutil.which("magick") or shutil.which("identify")
    if executable is None:
        return result
    separator = "\x1f"
    format_string = separator.join(f"%[EXIF:{field}]" for field in fields)
    command = [executable]
    if Path(executable).name == "magick":
        command.append("identify")
    command.extend(["-quiet", "-ping", "-format", format_string, str(path)])
    completed = subprocess.run(command, check=True, capture_output=True, text=True)
    values = completed.stdout.split(separator)
    if len(values) != len(fields):
        raise CoastalObliqueError(f"unexpected ImageMagick EXIF output for {path.name}")
    return {field: value.strip() or None for field, value in zip(fields, values, strict=True)}


def fetch(collection: str, max_frames: int | None = None) -> int:
    inventory = load_inventory(collection)
    progress = _load_progress(collection, inventory)
    entries = progress["frames"]
    assert isinstance(entries, dict)
    pending = [
        frame
        for frame in inventory.frames
        if not _downloaded_entry_valid(
            frame, entries.get(frame.name), _collection_dir(collection) / "jpeg" / frame.name
        )
    ]
    if max_frames is not None:
        pending = pending[:max_frames]
    with httpx.Client(
        headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=120
    ) as client:
        for frame in pending:
            print(f"Downloading {frame.name} ({frame.bytes:,} bytes)")
            download_frame(collection, frame, inventory, client)
    return len(pending)


def write_metadata(collection: str) -> Path:
    inventory = load_inventory(collection)
    progress = _load_progress(collection, inventory)
    entries = progress["frames"]
    assert isinstance(entries, dict)
    rows: list[dict[str, object]] = []
    for frame in inventory.frames:
        path = _collection_dir(collection) / "jpeg" / frame.name
        entry = entries.get(frame.name)
        if not _downloaded_entry_valid(frame, entry, path):
            continue
        assert isinstance(entry, dict)
        with path.open("rb") as source:
            width, height = read_jpeg_dimensions(source)
        rows.append(
            {
                "name": frame.name,
                "url": frame.url,
                "bytes": frame.bytes,
                "sha256": entry["sha256"],
                "width": width,
                "height": height,
                "exif": _exif_metadata(path),
                "georeferencing": None,
                "camera_pose": None,
            }
        )
    output = _collection_dir(collection) / "frame-metadata.json"
    _write_json_atomic(
        output,
        {
            "schema_version": 1,
            "collection": collection,
            "inventory_listing_sha256": inventory.listing_sha256,
            "rights_warning": RIGHTS_WARNING,
            "note": (
                "Dimensions are read from JPEG structure; available EXIF fields are extracted "
                "with ImageMagick. PASDA publishes no pose or georeference."
            ),
            "frames": rows,
        },
    )
    return output


def _contact_sheet_toolchain() -> tuple[list[str], list[str], list[list[str]], str]:
    magick = shutil.which("magick")
    if magick is not None:
        version_commands = [[magick, "-version"]]
        version = subprocess.run(
            version_commands[0], check=True, capture_output=True, text=True
        ).stdout.strip()
        return [magick], [magick, "montage"], version_commands, version
    convert = shutil.which("convert")
    montage = shutil.which("montage")
    if convert is None or montage is None:
        raise CoastalObliqueError(
            "ImageMagick is required for the contact sheet (`magick` or both `convert` "
            "and `montage`)"
        )
    version_commands = [[convert, "-version"], [montage, "-version"]]
    versions = [
        subprocess.run(command, check=True, capture_output=True, text=True).stdout.strip()
        for command in version_commands
    ]
    return [convert], [montage], version_commands, "\n\n".join(versions)


def _label_font() -> tuple[Path, str, list[str]]:
    resolver = shutil.which("fc-match")
    if resolver is None:
        raise CoastalObliqueError(
            f"fontconfig is required to resolve the pinned {LABEL_FONT_FAMILY!r} label font"
        )
    command = [resolver, "-f", "%{file}\n", LABEL_FONT_FAMILY]
    completed = subprocess.run(command, check=True, capture_output=True, text=True)
    candidates = [Path(line) for line in completed.stdout.splitlines() if line]
    if len(candidates) != 1 or not candidates[0].is_file():
        raise CoastalObliqueError(
            f"fontconfig did not resolve one usable {LABEL_FONT_FAMILY!r} font file"
        )
    path = candidates[0].resolve()
    digest = sha256_file(path)
    if digest != AUDITED_LABEL_FONT_SHA256:
        raise CoastalObliqueError(
            f"resolved {LABEL_FONT_FAMILY!r} font is not the audited file: "
            f"{digest} != {AUDITED_LABEL_FONT_SHA256}; install the pinned Noto Sans font or "
            "review and update the contact-sheet font pin"
        )
    return path, digest, command


def _thumbnail_cache_valid(path: Path, metadata_path: Path, expected: dict[str, object]) -> bool:
    if not path.is_file() or not metadata_path.is_file():
        return False
    try:
        metadata = json.loads(metadata_path.read_text())
    except (OSError, json.JSONDecodeError):
        return False
    digest = metadata.get("thumbnail_sha256") if isinstance(metadata, dict) else None
    return (
        isinstance(metadata, dict)
        and all(metadata.get(key) == value for key, value in expected.items())
        and isinstance(digest, str)
        and len(digest) == 64
        and sha256_file(path) == digest
    )


def _prune_thumbnail_cache(cache_root: Path, expected_files: set[Path]) -> None:
    if not cache_root.exists():
        return
    for path in sorted(cache_root.rglob("*"), reverse=True):
        if path.is_file() and path not in expected_files:
            path.unlink()
        elif path.is_dir():
            with suppress(OSError):
                path.rmdir()


def contact_sheet(collection: str) -> Path:
    inventory = load_inventory(collection)
    progress = _load_progress(collection, inventory)
    entries = progress["frames"]
    assert isinstance(entries, dict)
    ordered: list[tuple[Frame, Path, str]] = []
    for frame in inventory.frames:
        path = _collection_dir(collection) / "jpeg" / frame.name
        entry = entries.get(frame.name)
        if _downloaded_entry_valid(frame, entry, path):
            assert isinstance(entry, dict)
            ordered.append((frame, path, str(entry["sha256"])))
    if not ordered:
        raise CoastalObliqueError("no verified JPEGs; run `poe oblique-next` first")
    convert_prefix, montage_prefix, version_commands, version = _contact_sheet_toolchain()
    label_font, label_font_sha256, font_resolver_command = _label_font()
    toolchain_input = {
        "convert_prefix": convert_prefix,
        "montage_prefix": montage_prefix,
        "versions": version,
        "thumbnail": CONTACT_SHEET_THUMBNAIL,
        "columns": CONTACT_SHEET_COLUMNS,
        "geometry": CONTACT_SHEET_GEOMETRY,
    }
    toolchain_key = hashlib.sha256(
        json.dumps(toolchain_input, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    cache_root = _collection_dir(collection) / ".contact-sheet-cache"
    thumbnails: list[Path] = []
    thumbnail_commands: list[list[str]] = []
    expected_cache_files: set[Path] = set()
    for index, (frame, source, digest) in enumerate(ordered, start=1):
        cache_key = hashlib.sha256(f"{toolchain_key}:{digest}".encode()).hexdigest()
        cache_dir = cache_root / cache_key
        thumbnail = cache_dir / frame.name
        metadata_path = thumbnail.with_suffix(thumbnail.suffix + ".json")
        expected: dict[str, object] = {
            "schema_version": 1,
            "source_name": frame.name,
            "source_sha256": digest,
            "toolchain_key": toolchain_key,
            "ordered_index": index,
            "transform": {"auto_orient": True, "thumbnail": CONTACT_SHEET_THUMBNAIL},
        }
        expected_cache_files.update((thumbnail, metadata_path))
        if not _thumbnail_cache_valid(thumbnail, metadata_path, expected):
            cache_dir.mkdir(parents=True, exist_ok=True)
            temporary = thumbnail.with_suffix(".part" + thumbnail.suffix)
            temporary.unlink(missing_ok=True)
            # This command receives exactly one full-resolution source. Keeping this loop
            # sequential places a hard bound on decoder memory, even for the full collection.
            command = [
                *convert_prefix,
                str(source),
                "-auto-orient",
                "-thumbnail",
                CONTACT_SHEET_THUMBNAIL,
                str(temporary),
            ]
            subprocess.run(command, check=True)
            temporary.replace(thumbnail)
            _write_json_atomic(
                metadata_path,
                {**expected, "thumbnail_sha256": sha256_file(thumbnail), "exact_command": command},
            )
            thumbnail_commands.append(command)
        thumbnails.append(thumbnail)
    output = _collection_dir(collection) / "contact-sheet.jpg"
    temporary = output.with_suffix(".part.jpg")
    temporary.unlink(missing_ok=True)
    command = [
        *montage_prefix,
        "-font",
        str(label_font),
        "-label",
        "%t",
        "-tile",
        f"{CONTACT_SHEET_COLUMNS}x",
    ]
    command.extend(["-geometry", CONTACT_SHEET_GEOMETRY])
    command.extend(str(thumbnail) for thumbnail in thumbnails)
    command.append(str(temporary))
    subprocess.run(command, check=True)
    temporary.replace(output)
    _write_json_atomic(
        output.with_suffix(".json"),
        {
            "schema_version": 1,
            "collection": collection,
            "inventory_listing_sha256": inventory.listing_sha256,
            "contact_sheet": output.name,
            "contact_sheet_sha256": sha256_file(output),
            "labels": "source filename",
            "label_font": {
                "family": LABEL_FONT_FAMILY,
                "path": str(label_font),
                "sha256": label_font_sha256,
                "resolver_command": font_resolver_command,
            },
            "layout": {
                "thumbnail": CONTACT_SHEET_THUMBNAIL,
                "columns": CONTACT_SHEET_COLUMNS,
                "geometry": CONTACT_SHEET_GEOMETRY,
            },
            "memory_model": (
                "Sequentially decode and thumbnail exactly one full-resolution source; "
                "montage receives only cached small thumbnails."
            ),
            "toolchain": {
                "toolchain_key": toolchain_key,
                "version_commands": version_commands,
                "version": version,
                "thumbnail_commands_run": thumbnail_commands,
                "montage_command": command,
            },
            "ordered_frames": [
                {"index": index, "name": frame.name, "sha256": digest}
                for index, (frame, _, digest) in enumerate(ordered, start=1)
            ],
            "rights_warning": RIGHTS_WARNING,
        },
    )
    _prune_thumbnail_cache(cache_root, expected_cache_files)
    return output


def _write_bytes_immutable(path: Path, content: bytes) -> None:
    if path.exists():
        if path.read_bytes() != content:
            raise CoastalObliqueError(
                f"refusing to replace immutable SfM plan artifact: {path}; archive the "
                "existing sfm directory after reviewing the input or policy change"
            )
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".part")
    temporary.write_bytes(content)
    temporary.replace(path)


def _publish_immutable_artifact_set(output_dir: Path, artifacts: dict[str, bytes]) -> None:
    expected_names = set(artifacts)

    def validate_existing(directory: Path, *, allow_partial: bool) -> None:
        if not directory.is_dir():
            raise CoastalObliqueError(f"SfM artifact-set path is not a directory: {directory}")
        actual_names = {path.name for path in directory.iterdir()}
        unexpected = actual_names - expected_names
        if unexpected:
            raise CoastalObliqueError(
                f"SfM artifact set contains unexpected entries: {sorted(unexpected)}"
            )
        if not allow_partial and actual_names != expected_names:
            raise CoastalObliqueError(
                f"SfM artifact set is incomplete: {sorted(actual_names)} != "
                f"{sorted(expected_names)}"
            )
        for name in actual_names:
            path = directory / name
            if not path.is_file() or path.read_bytes() != artifacts[name]:
                raise CoastalObliqueError(
                    f"SfM artifact set drifted at {path}; archive the entire {output_dir.name} "
                    "directory after review instead of mixing plan generations"
                )

    if output_dir.exists():
        validate_existing(output_dir, allow_partial=False)
        return
    output_dir.parent.mkdir(parents=True, exist_ok=True)
    staging = output_dir.parent / f".{output_dir.name}.part"
    if staging.exists():
        validate_existing(staging, allow_partial=True)
    else:
        staging.mkdir()
    for name, content in artifacts.items():
        _write_bytes_immutable(staging / name, content)
    validate_existing(staging, allow_partial=False)
    try:
        staging.replace(output_dir)
    except OSError as error:
        if output_dir.exists():
            validate_existing(output_dir, allow_partial=False)
            return
        raise CoastalObliqueError(f"could not publish atomic SfM artifact set: {error}") from error


def _sfm_frame_manifest_sha256(frames: tuple[SfmFrame, ...]) -> str:
    payload = json.dumps(
        [{"name": frame.name, "sha256": frame.sha256} for frame in frames],
        separators=(",", ":"),
        sort_keys=True,
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def _parse_sfm_frames(raw_frames: object) -> tuple[SfmFrame, ...]:
    if not isinstance(raw_frames, list):
        raise CoastalObliqueError("SfM metadata has no frame list")
    frames: list[SfmFrame] = []
    for index, raw in enumerate(raw_frames, start=1):
        if not isinstance(raw, dict):
            raise CoastalObliqueError(f"invalid SfM frame metadata at position {index}")
        name, digest, width, height, exif = (
            raw.get("name"),
            raw.get("sha256"),
            raw.get("width"),
            raw.get("height"),
            raw.get("exif"),
        )
        expected_name = f"Schuylkill   {index:03}.jpg"
        if name != expected_name:
            raise CoastalObliqueError(
                f"noncanonical SfM frame at position {index}: {name!r} != {expected_name!r}"
            )
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise CoastalObliqueError(f"invalid SHA-256 for {name}")
        if not isinstance(width, int) or not isinstance(height, int) or width < 1 or height < 1:
            raise CoastalObliqueError(f"invalid dimensions for {name}")
        if not isinstance(exif, dict):
            raise CoastalObliqueError(f"missing EXIF for {name}")
        expected_exif = {
            "Make": "Canon",
            "Model": "Canon EOS 5D Mark II",
            "LensModel": "EF70-300mm f/4.5-5.6 DO IS USM",
            "Orientation": "1",
        }
        for field, expected in expected_exif.items():
            if exif.get(field) != expected:
                raise CoastalObliqueError(
                    f"unexpected {field} for {name}: {exif.get(field)!r} != {expected!r}"
                )
        focal = exif.get("FocalLength")
        match = re.fullmatch(r"(\d+)/1", focal) if isinstance(focal, str) else None
        if match is None or int(match.group(1)) <= 0:
            raise CoastalObliqueError(f"invalid EXIF focal length for {name}: {focal!r}")
        captured = exif.get("DateTimeOriginal")
        if not isinstance(captured, str):
            raise CoastalObliqueError(f"missing EXIF capture time for {name}")
        try:
            captured_at = datetime.strptime(captured, "%Y:%m:%d %H:%M:%S").replace(tzinfo=UTC)
        except ValueError as error:
            raise CoastalObliqueError(
                f"invalid EXIF capture time for {name}: {captured!r}"
            ) from error
        frames.append(
            SfmFrame(
                index,
                name,
                f"frame-{index:03}.jpg",
                digest,
                width,
                height,
                captured_at,
                int(match.group(1)),
            )
        )
    return tuple(frames)


def _validate_sfm_profile(frames: tuple[SfmFrame, ...]) -> dict[str, object]:
    if len(frames) != EXPECTED_COUNTS["schuylkill-2014"]:
        raise CoastalObliqueError(f"SfM plan requires all 191 frames; found {len(frames)}")
    manifest = _sfm_frame_manifest_sha256(frames)
    if manifest != SFM_IMAGE_MANIFEST_SHA256:
        raise CoastalObliqueError(
            f"SfM image manifest changed: {manifest} != audited {SFM_IMAGE_MANIFEST_SHA256}"
        )
    dimensions = Counter((frame.width, frame.height) for frame in frames)
    if dimensions != Counter(SFM_EXPECTED_DIMENSIONS):
        raise CoastalObliqueError("SfM image dimensions differ from the audited flight")
    focals = Counter(frame.focal_mm for frame in frames)
    if focals != Counter(SFM_EXPECTED_FOCALS_MM):
        raise CoastalObliqueError("SfM EXIF focal distribution differs from the audited flight")
    groups = Counter((frame.width, frame.height, frame.focal_mm) for frame in frames)
    singleton_groups = sum(count == 1 for count in groups.values())
    if len(groups) != SFM_EXACT_CAMERA_GROUPS or singleton_groups != SFM_SINGLETON_CAMERA_GROUPS:
        raise CoastalObliqueError("SfM intrinsic groups differ from the audited flight")
    if any(
        later.captured_at <= earlier.captured_at
        for earlier, later in zip(frames, frames[1:], strict=False)
    ):
        raise CoastalObliqueError("SfM EXIF capture times are not strictly increasing")
    first = frames[0].captured_at.isoformat()
    last = frames[-1].captured_at.isoformat()
    gaps = [
        int((later.captured_at - earlier.captured_at).total_seconds())
        for earlier, later in zip(frames, frames[1:], strict=False)
    ]
    largest_gap = max(gaps)
    break_after = gaps.index(largest_gap) + 1
    if (
        first != SFM_FIRST_CAPTURED_AT
        or last != SFM_LAST_CAPTURED_AT
        or largest_gap != SFM_BREAK_SECONDS
        or break_after != SFM_MATCH_BREAK_AFTER
    ):
        raise CoastalObliqueError("SfM EXIF timeline differs from the audited flight")
    return {
        "ordered_image_manifest_sha256": manifest,
        "frame_count": len(frames),
        "distinct_focal_lengths": len(focals),
        "exact_focal_dimension_groups": len(groups),
        "singleton_groups": singleton_groups,
        "portrait_frames": [frame.name for frame in frames if frame.height > frame.width],
        "capture_start": first,
        "capture_end": last,
        "largest_gap": {"after_sequence": break_after, "seconds": largest_gap},
    }


def _sfm_pairs(frames: tuple[SfmFrame, ...]) -> tuple[tuple[str, str], ...]:
    included = [frame for frame in frames if frame.included]
    pairs: list[tuple[str, str]] = []
    for position, first in enumerate(included):
        for second in included[position + 1 :]:
            if first.sequence <= SFM_MATCH_BREAK_AFTER < second.sequence:
                break
            distance = second.sequence - first.sequence
            if distance > SFM_LOCAL_MATCH_WINDOW:
                break
            pairs.append((first.staged_name, second.staged_name))
    return tuple(pairs)


def _sfm_plan_dict(
    collection: str,
    listing_sha256: str,
    frames: tuple[SfmFrame, ...],
    audit: dict[str, object],
    pairs_sha256: str,
    pair_count: int,
) -> dict[str, object]:
    images = [
        {
            "sequence": frame.sequence,
            "source_name": frame.name,
            "staged_name": frame.staged_name,
            "sha256": frame.sha256,
            "captured_at": frame.captured_at.isoformat(),
            "included_in_baseline": frame.included,
            "exclusion_reason": (
                "non-3:2 portrait aspect outlier; retain for a later diagnostic"
                if not frame.included
                else None
            ),
            "camera": {
                "camera_id": frame.sequence,
                "sharing": "per-image",
                "model": "SIMPLE_RADIAL",
                "width": frame.width,
                "height": frame.height,
                "params": [
                    round(frame.focal_pixels, 9),
                    frame.width / 2.0,
                    frame.height / 2.0,
                    0.0,
                ],
                "has_prior_focal_length": True,
                "focal_source": "EXIF focal mm × longest pixel edge / 36 mm full-frame edge",
            },
        }
        for frame in frames
    ]
    return {
        "schema_version": 1,
        "kind": "deterministic-sfm-plan-not-execution-evidence",
        "collection": collection,
        "inventory_listing_sha256": listing_sha256,
        "rights_warning": RIGHTS_WARNING,
        "backend": {
            "package": "pycolmap",
            "version": SFM_PYCOLMAP_VERSION,
            "platform": "CPython 3.13 / manylinux_2_28_x86_64",
            "wheel_sha256": SFM_PYCOLMAP_LINUX_X86_64_SHA256,
            "installed_or_imported_by_plan": False,
        },
        "input_audit": audit,
        "images": images,
        "matching": {
            "mode": "explicit-imported-pairs",
            "staged_names_avoid_source_filename_spaces": True,
            "segments": [[1, SFM_MATCH_BREAK_AFTER], [SFM_MATCH_BREAK_AFTER + 1, 190]],
            "excluded_sequences": list(SFM_EXCLUDED_SEQUENCE_NUMBERS),
            "local_window": SFM_LOCAL_MATCH_WINDOW,
            "pairs_path": "pairs.txt",
            "pairs_sha256": pairs_sha256,
            "pair_count": pair_count,
            "cross_break_pairs": 0,
            "loop_detection": False,
        },
        "options": {
            "camera_mode": "PER_IMAGE",
            "feature_extraction": {
                "type": "SIFT",
                "max_image_size": 2400,
                "max_num_features": 8192,
                "num_threads": 2,
                "use_gpu": False,
            },
            "feature_matching": {
                "max_num_matches": 8192,
                "num_threads": 2,
                "use_gpu": False,
            },
            "incremental_mapping": {
                "num_threads": 2,
                "random_seed": 0,
                "multiple_models": True,
                "max_num_models": 4,
                "min_model_size": 20,
                "max_runtime_seconds": 10800,
                "ba_refine_focal_length": True,
                "ba_refine_principal_point": False,
                "ba_refine_extra_params": True,
            },
        },
        "resource_bounds": {
            "cpu_threads": 2,
            "address_space_bytes": 8 * 1024**3,
            "environment": {
                "OMP_NUM_THREADS": "1",
                "OPENBLAS_NUM_THREADS": "1",
                "MKL_NUM_THREADS": "1",
            },
            "launcher_prefix": [
                "prlimit",
                "--as=8589934592",
                "--",
                "nice",
                "-n",
                "10",
                "ionice",
                "-c",
                "3",
                "taskset",
                "-c",
                "0,1",
            ],
            "may_not_overlap_citywide_lidar_processing": True,
            "systemd_cgroup_claimed": False,
        },
        "promotion_gates": {
            "dominant_model_registered_images_min": 153,
            "median_track_length_min": 3,
            "median_reprojection_error_px_max": 2.0,
            "p95_reprojection_error_px_max": 4.0,
            "focal_drift_from_exif_max_fraction": 0.1,
            "require_no_temporal_camera_jump_or_reversal": True,
            "failure_result": "visual-reference-only",
        },
        "georegistration": {
            "status": None,
            "method": "deterministic RANSAC plus Umeyama Sim(3) from recorded GCPs",
            "crs": "EPSG:32129",
            "minimum_gcps": 8,
            "minimum_images_per_gcp": 2,
            "minimum_inliers": 6,
            "median_residual_m_max": 3.0,
            "p95_residual_m_max": 8.0,
            "withheld_checkpoints_min": 2,
            "withheld_checkpoint_residual_m_max": 10.0,
            "xy_source": "2025 PASDA orthophoto",
            "z_source": "classified 2025 Philadelphia LiDAR",
            "forbidden_controls": ["water", "trees", "changed structures"],
            "model_aligner_allowed_without_true_camera_centers": False,
        },
        "execution": {"status": "not-run", "reconstruction_evidence": False},
    }


def sfm_plan(collection: str) -> Path:
    if collection != "schuylkill-2014":
        raise CoastalObliqueError(
            "the deterministic SfM profile is currently audited only for schuylkill-2014"
        )
    metadata_path = write_metadata(collection)
    try:
        metadata = json.loads(metadata_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise CoastalObliqueError(f"cannot read SfM frame metadata: {metadata_path}") from error
    if not isinstance(metadata, dict):
        raise CoastalObliqueError("invalid SfM frame metadata document")
    listing_sha256 = metadata.get("inventory_listing_sha256")
    if listing_sha256 != AUDITED_LISTING_SHA256[collection]:
        raise CoastalObliqueError("SfM metadata does not reference the audited inventory")
    frames = _parse_sfm_frames(metadata.get("frames"))
    audit = _validate_sfm_profile(frames)
    pairs = _sfm_pairs(frames)
    pairs_content = "".join(f"{first} {second}\n" for first, second in pairs).encode()
    pairs_sha256 = hashlib.sha256(pairs_content).hexdigest()
    plan = _sfm_plan_dict(collection, listing_sha256, frames, audit, pairs_sha256, len(pairs))
    plan_content = (json.dumps(plan, indent=2, sort_keys=True) + "\n").encode()
    plan_sha256 = hashlib.sha256(plan_content).hexdigest()
    output_dir = _collection_dir(collection) / "sfm" / "plan"
    _publish_immutable_artifact_set(
        output_dir,
        {
            "pairs.txt": pairs_content,
            "plan.json": plan_content,
            "plan.sha256": f"{plan_sha256}  plan.json\n".encode(),
        },
    )
    output = output_dir / "plan.json"
    return output


def _frame_sequence(frames: list[object]) -> tuple[list[int], bool]:
    numbers: list[int] = []
    for raw in frames:
        if not isinstance(raw, dict) or not isinstance(raw.get("name"), str):
            raise CoastalObliqueError("invalid frame metadata for SfM handoff")
        match = re.search(r"(\d+)\.(?:jpe?g)$", raw["name"], re.IGNORECASE)
        if match is None:
            raise CoastalObliqueError(f"frame has no sequence number: {raw['name']}")
        numbers.append(int(match.group(1)))
    contiguous = bool(numbers) and numbers == list(range(numbers[0], numbers[0] + len(numbers)))
    return numbers, contiguous


def _sfm_camera_audit(frames: list[object]) -> dict[str, object]:
    focal_lengths: set[str] = set()
    intrinsic_groups: set[tuple[int, int, str, str]] = set()
    portrait_frames: list[str] = []
    missing_focal_frames: list[str] = []
    for raw in frames:
        if not isinstance(raw, dict):
            raise CoastalObliqueError("invalid frame metadata for camera audit")
        name, width, height, exif = (
            raw.get("name"),
            raw.get("width"),
            raw.get("height"),
            raw.get("exif"),
        )
        if (
            not isinstance(name, str)
            or not isinstance(width, int)
            or not isinstance(height, int)
            or not isinstance(exif, dict)
        ):
            raise CoastalObliqueError("incomplete frame metadata for camera audit")
        focal = exif.get("FocalLength")
        orientation = exif.get("Orientation")
        if not isinstance(focal, str) or not focal:
            missing_focal_frames.append(name)
            focal = "missing"
        else:
            focal_lengths.add(focal)
        orientation_key = orientation if isinstance(orientation, str) else "missing"
        intrinsic_groups.add((width, height, focal, orientation_key))
        if height > width:
            portrait_frames.append(name)
    return {
        "policy": (
            "Use one EXIF-seeded SIMPLE_RADIAL camera per image. The zoom changes during the "
            "flight, so a shared intrinsic is prohibited."
        ),
        "distinct_focal_lengths": len(focal_lengths),
        "exact_focal_dimension_orientation_groups": len(intrinsic_groups),
        "missing_focal_frames": missing_focal_frames,
        "portrait_frames": portrait_frames,
    }


def sfm_handoff(collection: str, *, allow_incomplete: bool = False) -> Path:
    metadata_path = write_metadata(collection)
    metadata = json.loads(metadata_path.read_text())
    frames = metadata["frames"]
    if not frames:
        raise CoastalObliqueError("no verified JPEGs available for SfM handoff")
    if not isinstance(frames, list):
        raise CoastalObliqueError("invalid frame metadata for SfM handoff")
    numbers, contiguous = _frame_sequence(frames)
    camera_audit = _sfm_camera_audit(frames)
    sufficiently_large = len(frames) >= MIN_SFM_SEQUENCE
    if not allow_incomplete and (not contiguous or not sufficiently_large):
        raise CoastalObliqueError(
            f"SfM requires at least {MIN_SFM_SEQUENCE} contiguous frames; found "
            f"{len(frames)} frame(s), contiguous={contiguous}. Use "
            "`sfm-handoff --allow-incomplete` only for an explicitly incomplete diagnostic."
        )
    backend = "colmap" if shutil.which("colmap") else None
    if backend is None and importlib.util.find_spec("pycolmap") is not None:
        backend = "pycolmap"
    output = _collection_dir(collection) / "sfm-handoff.json"
    _write_json_atomic(
        output,
        {
            "schema_version": 2,
            "collection": collection,
            "inventory_listing_sha256": metadata["inventory_listing_sha256"],
            "backend_detected": backend,
            "image_directory": "jpeg",
            "sequential_order": [frame["name"] for frame in frames],
            "images": frames,
            "completeness": {
                "downloaded_frames": len(frames),
                "listed_frames": EXPECTED_COUNTS[collection],
                "first_sequence_number": numbers[0],
                "last_sequence_number": numbers[-1],
                "contiguous": contiguous,
                "minimum_default_sequence": MIN_SFM_SEQUENCE,
                "sufficient_for_default_handoff": contiguous and sufficiently_large,
                "complete_collection": len(frames) == EXPECTED_COUNTS[collection],
                "incomplete_override": allow_incomplete,
            },
            "georeferencing": None,
            "camera_pose": None,
            "camera_intrinsics": camera_audit,
            "planning_policy": (
                {
                    "command": "poe oblique-sfm-plan",
                    "required_before_execution": True,
                    "match_segments": [[1, SFM_MATCH_BREAK_AFTER], [93, 190]],
                    "excluded_sequences": list(SFM_EXCLUDED_SEQUENCE_NUMBERS),
                    "missing_focal_exif_is_fatal": True,
                }
                if collection == "schuylkill-2014"
                else None
            ),
            "next_step": (
                "Run `poe oblique-sfm-plan` to validate and freeze the CPU-bounded, per-image "
                "EXIF-seeded SIMPLE_RADIAL policy. The plan quarantines frame 191 and splits "
                "matching between frames 92 and 93. It does not run reconstruction."
                if backend
                else (
                    "Run `poe oblique-sfm-plan` to validate per-image EXIF-seeded "
                    "SIMPLE_RADIAL intrinsics before installing the pinned free pycolmap "
                    "backend. Planning does not import pycolmap or run reconstruction."
                )
            ),
            "rights_warning": RIGHTS_WARNING,
        },
    )
    return output


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Acquire audited PASDA coastal oblique JPEGs")
    commands = parser.add_subparsers(dest="command", required=True)

    def add_collection(command: argparse.ArgumentParser) -> None:
        command.add_argument(
            "--collection", choices=tuple(COLLECTION_URLS), default="schuylkill-2014"
        )

    plan = commands.add_parser("plan")
    add_collection(plan)
    plan.add_argument("--refresh", action="store_true")
    fetch_command = commands.add_parser("fetch")
    add_collection(fetch_command)
    fetch_command.add_argument("--max-frames", type=int)
    for name in ("status", "metadata", "contact-sheet"):
        add_collection(commands.add_parser(name))
    sfm = commands.add_parser("sfm-handoff")
    add_collection(sfm)
    sfm.add_argument("--allow-incomplete", action="store_true")
    add_collection(commands.add_parser("sfm-plan"))
    return parser


def main() -> None:
    args = _parser().parse_args()
    print(RIGHTS_WARNING)
    if args.command == "plan":
        inventory = create_inventory(args.collection, refresh=args.refresh)
        print(
            f"Pinned {len(inventory.frames)} JPEGs / "
            f"{sum(frame.bytes for frame in inventory.frames):,} bytes at "
            f"{_collection_dir(args.collection) / 'inventory.json'}"
        )
    elif args.command == "fetch":
        if args.max_frames is not None and args.max_frames < 1:
            raise CoastalObliqueError("--max-frames must be positive")
        print(f"Downloaded {fetch(args.collection, args.max_frames)} frame(s)")
    elif args.command == "status":
        inventory = load_inventory(args.collection)
        progress = _load_progress(args.collection, inventory)
        entries = progress["frames"]
        assert isinstance(entries, dict)
        complete = sum(
            _downloaded_entry_valid(
                frame,
                entries.get(frame.name),
                _collection_dir(args.collection) / "jpeg" / frame.name,
            )
            for frame in inventory.frames
        )
        print(f"{args.collection}: {complete}/{len(inventory.frames)} verified JPEGs")
    elif args.command == "metadata":
        print(write_metadata(args.collection))
    elif args.command == "contact-sheet":
        print(contact_sheet(args.collection))
    elif args.command == "sfm-handoff":
        path = sfm_handoff(args.collection, allow_incomplete=args.allow_incomplete)
        handoff = json.loads(path.read_text())
        print(f"{path}: {handoff['next_step']}")
    else:
        path = sfm_plan(args.collection)
        print(f"{path}: immutable plan only; pycolmap was not imported or run")


if __name__ == "__main__":
    main()
