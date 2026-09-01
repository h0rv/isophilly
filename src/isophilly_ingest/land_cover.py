from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import shutil
import socket
import stat
import struct
import subprocess
import tempfile
import time
import zipfile
from collections.abc import Callable, Iterator
from dataclasses import asdict, dataclass, fields
from enum import IntEnum
from pathlib import Path

import httpx
import numpy as np
from numpy.typing import NDArray

from .config import ROOT

SCHEMA_VERSION = 1
MAGIC = b"ISOLULC1"
HEADER_PREFIX = struct.Struct("<8sII")
CHUNK_BYTES = 1024 * 1024
MAX_GRID_PIXELS = 100_000_000
TARGET_RESOLUTION_METERS = 3.0
TARGET_GRID = (810_942.0, 62_427.0, 838_236.0, 92_949.0, 9_098, 10_174)
GDAL_VERSION = "3.12.4"
CONVERSION_SCHEMA_VERSION = 1

DATASET_ID = 1587
DATASET_NAME = "Philadelphia Land Cover Raster 2018"
DATASET_YEAR = 2018
SERVICE_URL = "https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhillyLULC/MapServer"
LAYER_ID = 2
LAYER_NAME = "Philadelphia Land Cover 2018"
ARCHIVE_URL = (
    "https://www.pasda.psu.edu/download/philacity/data/PhiladelphiaLandCoverRaster2018.zip"
)
ARCHIVE_BYTES = 521_373_667
ARCHIVE_ETAG = '"054a6b74dbd51:0"'
SOURCE_CREDIT = (
    "University of Vermont Spatial Analysis Laboratory in collaboration with "
    "Philadelphia 2018 Tree Canopy Assessment."
)
SOURCE_EXTENT_US_SURVEY_FEET = (
    2_645_347.999999997,
    186_454.00000000006,
    2_753_588.0000000014,
    307_894.0000000009,
)
SOURCE_IDENTITY_SHA256 = "72cdba0fd90b5a7e880e5ce51c3cb5cfba26382a1617efa2149ce612d791d5b8"
AUDITED_SOURCE_ARCHIVE_SHA256: str | None = (
    "555ab81428c239dd4d1a1f162fdd072f4ff1b0b2ab15a2e96a3f241e2823bb3f"
)
AUDITED_GDB_ROOT: str | None = "PPR_LandCover_2018.gdb"
AUDITED_RASTER_NAME: str | None = "landcover_2018_philadelphia"
DEFAULT_MASK_PATH = ROOT / "data" / "clean" / "land-cover-2018.isomask"
DEFAULT_CONVERSION_ROOT = ROOT / "data" / "land-cover-2018" / "converted"
DEFAULT_ARCHIVE_PATH = ROOT / "data" / "raw" / "PhiladelphiaLandCoverRaster2018.zip"
ARCHIVE_DOWNLOAD_CHUNK_BYTES = 4 * 1024 * 1024
ARCHIVE_DOWNLOAD_MAX_ATTEMPTS = 4
ARCHIVE_DOWNLOAD_BACKOFF_SECONDS = 2.0
ARCHIVE_DOWNLOAD_MAX_BACKOFF_SECONDS = 30.0
ARCHIVE_TIMEOUT = httpx.Timeout(connect=30.0, read=120.0, write=30.0, pool=30.0)
CONTENT_RANGE = re.compile(r"^bytes (?P<start>\d+)-(?P<end>\d+)/(?P<total>\d+)$")
STRONG_ETAG = re.compile(r'^"[\x21\x23-\x7e\x80-\xff]*"$')

RIGHTS_NOTICE = (
    "The City reserves rights in this dataset and provides it as is. Confirm current "
    "City and PASDA terms before publishing source pixels or derived raster tiles."
)


class LandCoverError(RuntimeError):
    pass


class TransientLandCoverTransferError(LandCoverError):
    pass


@dataclass(frozen=True, slots=True)
class ArchiveTransferSpec:
    url: str
    expected_bytes: int
    etag: str


@dataclass(frozen=True, slots=True)
class ArchiveCheckpoint:
    downloaded_bytes: int
    etag: str


class LandCoverClass(IntEnum):
    UNKNOWN = 0
    TREE_CANOPY = 1
    GRASS_SHRUB = 2
    BARE_EARTH = 3
    WATER = 4
    BUILDING = 5
    ROAD_RAILROAD = 6
    OTHER_PAVED = 7


CLASS_NAMES = {value.value: value.name.lower() for value in LandCoverClass}
SOURCE_CLASS_NAMES = {code: name for code, name in CLASS_NAMES.items() if code != 0}


@dataclass(frozen=True, slots=True)
class GridSpec:
    epsg: int
    width: int
    height: int
    min_x: float
    min_y: float
    max_x: float
    max_y: float
    row_order: str = "north_to_south"
    resampling: str = "nearest"

    def __post_init__(self) -> None:
        if self.epsg != 32129:
            raise LandCoverError("land cover grid must use EPSG:32129 metres")
        if self.width <= 0 or self.height <= 0:
            raise LandCoverError("land cover grid dimensions must be positive")
        if self.width * self.height > MAX_GRID_PIXELS:
            raise LandCoverError("land cover grid exceeds the 100 million pixel limit")
        if not self.min_x < self.max_x or not self.min_y < self.max_y:
            raise LandCoverError("land cover grid bounds must be finite and ordered")
        if not all(
            np.isfinite(value) for value in (self.min_x, self.min_y, self.max_x, self.max_y)
        ):
            raise LandCoverError("land cover grid bounds must be finite and ordered")
        if self.row_order != "north_to_south":
            raise LandCoverError("land cover rows must run north to south")
        if self.resampling != "nearest":
            raise LandCoverError("categorical land cover must use nearest-neighbor resampling")
        if not 0.25 <= self.pixel_width <= 10.0 or not 0.25 <= self.pixel_height <= 10.0:
            raise LandCoverError("land cover resolution must be between 0.25 and 10 metres")
        if not math.isclose(self.pixel_width, self.pixel_height, rel_tol=0.0, abs_tol=1e-9):
            raise LandCoverError("land cover pixels must be square")

    @property
    def pixel_width(self) -> float:
        return (self.max_x - self.min_x) / self.width

    @property
    def pixel_height(self) -> float:
        return (self.max_y - self.min_y) / self.height


@dataclass(frozen=True, slots=True)
class MaskHeader:
    schema_version: int
    source_identity_sha256: str
    source_archive_sha256: str
    source_archive_bytes: int
    payload_sha256: str
    grid: GridSpec


@dataclass(frozen=True, slots=True)
class RasterEvidence:
    driver: str
    description: str
    files: tuple[str, ...]
    width: int
    height: int
    data_type: str
    nodata: int | None
    geotransform: tuple[float, float, float, float, float, float]
    crs_wkt_sha256: str


@dataclass(frozen=True, slots=True)
class ToolchainEvidence:
    gdal_version: str
    gdalinfo_version_sha256: str
    gdalinfo_build_sha256: str
    gdalinfo_formats_sha256: str
    gdalinfo_help_sha256: str
    gdalwarp_version_sha256: str
    gdalwarp_build_sha256: str
    gdalwarp_help_sha256: str
    proj_version: str
    proj_version_sha256: str


# Keep typed evidence pins after their class declarations so replacing None with
# executable constructor literals never depends on a forward declaration.
AUDITED_RASTER_EVIDENCE: RasterEvidence | None = RasterEvidence(
    driver="OpenFileGDB",
    description='OpenFileGDB:"/vsizip/{{archive}}/PPR_LandCover_2018.gdb":landcover_2018_philadelphia',
    files=(
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000001.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000001.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000001.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000001.TablesByName.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000002.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000002.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000003.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000003.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000003.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.CatItemsByPhysicalName.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.CatItemsByType.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.FDO_UUID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000004.spx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000005.CatItemTypesByName.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000005.CatItemTypesByParentTypeID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000005.CatItemTypesByUUID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000005.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000005.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000005.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.CatRelsByDestinationID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.CatRelsByOriginID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.CatRelsByType.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.FDO_UUID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000006.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.CatRelTypesByBackwardLabel.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.CatRelTypesByDestItemTypeID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.CatRelTypesByForwardLabel.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.CatRelTypesByName.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.CatRelTypesByOriginItemTypeID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.CatRelTypesByUUID.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000007.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000009.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000009.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000009.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000009.spx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000a.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000a.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000a.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000b.freelist",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000b.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000b.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000b.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.band_index.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.blk_key_index.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.col_index.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000c.row_index.atx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000d.freelist",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000d.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000d.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000d.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000e.gdbindexes",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000e.gdbtable",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a0000000e.gdbtablx",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/gdb",
        "/vsizip/{{archive}}/PPR_LandCover_2018.gdb/timestamps",
    ),
    width=216480,
    height=242880,
    data_type="Byte",
    nodata=None,
    geotransform=(2645348, 0.5, 0, 307894, 0, -0.5),
    crs_wkt_sha256="205e384cdb58874490456cf42a2425d7db7029e22ea99fdbc795da51d0f8c710",
)
AUDITED_TOOLCHAIN: ToolchainEvidence | None = ToolchainEvidence(
    gdal_version="3.12.4",
    gdalinfo_build_sha256="03a97d02b0b86b186c5acf98126ced74156c4816ebd3bc0c658a4712f64e99e8",
    gdalinfo_formats_sha256="2329e21907b4a0856ae922989e3df864f9d06d2bc1bb95eeddc4e504a9d26360",
    gdalinfo_help_sha256="d485909c92b1305def9f5a0dd6841f4019cef40e1e193031cd64623d793db7c6",
    gdalinfo_version_sha256="8a07b4ef485a519447d24b0390092bcbee693cb206dbaf5068abcb271b3cedf2",
    gdalwarp_build_sha256="03a97d02b0b86b186c5acf98126ced74156c4816ebd3bc0c658a4712f64e99e8",
    gdalwarp_help_sha256="d485909c92b1305def9f5a0dd6841f4019cef40e1e193031cd64623d793db7c6",
    gdalwarp_version_sha256="8a07b4ef485a519447d24b0390092bcbee693cb206dbaf5068abcb271b3cedf2",
    proj_version="9.8.1",
    proj_version_sha256="79d0d2e9855e38182014faa8cca6b736eb0641b0a8c591b19754aeef18c787b9",
)


@dataclass(frozen=True, slots=True)
class EnviHeader:
    samples: int
    lines: int
    bands: int
    header_offset: int
    data_type: int
    interleave: str
    byte_order: int


@dataclass(frozen=True, slots=True)
class ArchiveMember:
    name: str
    bytes: int
    sha256: str


@dataclass(frozen=True, slots=True)
class ConversionManifest:
    schema_version: int
    source_identity_sha256: str
    source_archive_sha256: str
    archive_members_sha256: str
    archive_members: tuple[ArchiveMember, ...]
    source_dataset: str
    toolchain: ToolchainEvidence
    resampling: str
    source: RasterEvidence
    envi: EnviHeader
    grid: GridSpec
    classes_npy_sha256: str
    class_counts: tuple[int, int, int, int, int, int, int, int]


@dataclass(frozen=True, slots=True)
class LandCoverMask:
    header: MaskHeader
    classes: NDArray[np.uint8]

    def sample(self, x: float, y: float) -> LandCoverClass | None:
        grid = self.header.grid
        if not (grid.min_x <= x <= grid.max_x and grid.min_y <= y <= grid.max_y):
            return None
        x = min(x, math.nextafter(grid.max_x, grid.min_x))
        y = max(y, math.nextafter(grid.min_y, grid.max_y))
        column = int((x - grid.min_x) // grid.pixel_width)
        row = int((grid.max_y - y) // grid.pixel_height)
        value = LandCoverClass(int(self.classes[row, column]))
        return None if value is LandCoverClass.UNKNOWN else value


def source_identity() -> dict[str, object]:
    return {
        "archive_bytes": ARCHIVE_BYTES,
        "archive_etag": ARCHIVE_ETAG,
        "archive_url": ARCHIVE_URL,
        "classes": {str(code): name for code, name in SOURCE_CLASS_NAMES.items()},
        "credit": SOURCE_CREDIT,
        "dataset_id": DATASET_ID,
        "dataset_name": DATASET_NAME,
        "extent_us_survey_feet": list(SOURCE_EXTENT_US_SURVEY_FEET),
        "layer_id": LAYER_ID,
        "layer_name": LAYER_NAME,
        "layer_type": "Raster Layer",
        "service_url": SERVICE_URL,
        "year": DATASET_YEAR,
    }


def canonical_sha256(value: object) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _is_sha256(value: str) -> bool:
    return len(value) == 64 and all(character in "0123456789abcdef" for character in value)


def verify_source_pin() -> None:
    actual = canonical_sha256(source_identity())
    if actual != SOURCE_IDENTITY_SHA256:
        raise LandCoverError(
            "land cover source identity changed; review PASDA dataset 1587 and layer 2 before "
            "updating SOURCE_IDENTITY_SHA256"
        )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_BYTES):
            digest.update(chunk)
    return digest.hexdigest()


def _strong_etag(value: str | None) -> str | None:
    if value is None or value.lower().startswith("w/") or STRONG_ETAG.fullmatch(value) is None:
        return None
    return value


def _require_regular_or_missing(path: Path, label: str) -> None:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        return
    if not stat.S_ISREG(mode):
        raise LandCoverError(f"land cover archive {label} is not a regular file: {path}")


def _verify_path_identity(path: Path, opened: os.stat_result, label: str) -> None:
    try:
        current = path.lstat()
    except FileNotFoundError as error:
        raise LandCoverError(f"land cover archive {label} changed during use: {path}") from error
    if (
        not stat.S_ISREG(current.st_mode)
        or current.st_dev != opened.st_dev
        or current.st_ino != opened.st_ino
    ):
        raise LandCoverError(f"land cover archive {label} changed during use: {path}")


def _open_regular_nofollow(path: Path, label: str) -> tuple[int, os.stat_result]:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        raise
    except OSError as error:
        raise LandCoverError(f"cannot safely open land cover archive {label}: {path}") from error
    opened = os.fstat(descriptor)
    if not stat.S_ISREG(opened.st_mode):
        os.close(descriptor)
        raise LandCoverError(f"land cover archive {label} is not a regular file: {path}")
    return descriptor, opened


def _sha256_fd(descriptor: int) -> str:
    digest = hashlib.sha256()
    while chunk := os.read(descriptor, CHUNK_BYTES):
        digest.update(chunk)
    return digest.hexdigest()


def _sha256_regular_nofollow(path: Path, label: str) -> tuple[str, os.stat_result]:
    try:
        descriptor, opened = _open_regular_nofollow(path, label)
    except FileNotFoundError as error:
        raise LandCoverError(f"land cover archive {label} changed during use: {path}") from error
    try:
        digest = _sha256_fd(descriptor)
        _verify_path_identity(path, opened, label)
        return digest, opened
    finally:
        os.close(descriptor)


def _read_fd(descriptor: int, maximum_bytes: int) -> bytes:
    value = bytearray()
    while chunk := os.read(descriptor, min(CHUNK_BYTES, maximum_bytes + 1 - len(value))):
        value.extend(chunk)
        if len(value) > maximum_bytes:
            raise LandCoverError("land cover archive checkpoint exceeds its size limit")
    return bytes(value)


def _read_regular_nofollow(path: Path, label: str, maximum_bytes: int) -> bytes:
    descriptor, opened = _open_regular_nofollow(path, label)
    try:
        value = _read_fd(descriptor, maximum_bytes)
        _verify_path_identity(path, opened, label)
        return value
    finally:
        os.close(descriptor)


def _regular_size_if_exists(path: Path, label: str) -> int:
    try:
        path.lstat()
    except FileNotFoundError:
        return 0
    try:
        descriptor, opened = _open_regular_nofollow(path, label)
    except FileNotFoundError as error:
        raise LandCoverError(f"land cover archive {label} changed during use: {path}") from error
    try:
        _verify_path_identity(path, opened, label)
        return opened.st_size
    finally:
        os.close(descriptor)


def _fsync_directory(directory: Path) -> None:
    descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _unlink_and_fsync(path: Path) -> None:
    try:
        path.unlink()
    except FileNotFoundError:
        return
    _fsync_directory(path.parent)


def _replace_and_fsync(source: Path, destination: Path) -> None:
    source.replace(destination)
    _fsync_directory(destination.parent)


def _write_checkpoint(path: Path, checkpoint: ArchiveCheckpoint, spec: ArchiveTransferSpec) -> None:
    _require_regular_or_missing(path, "checkpoint")
    value = {
        "schema_version": 1,
        "url": spec.url,
        "expected_bytes": spec.expected_bytes,
        "downloaded_bytes": checkpoint.downloaded_bytes,
        "etag": checkpoint.etag,
    }
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".part", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, sort_keys=True, separators=(",", ":"))
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        _replace_and_fsync(temporary, path)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def _load_checkpoint(
    path: Path, partial: Path, spec: ArchiveTransferSpec
) -> ArchiveCheckpoint | None:
    try:
        partial.lstat()
    except FileNotFoundError:
        _unlink_and_fsync(path)
        return None
    try:
        value = json.loads(_read_regular_nofollow(path, "checkpoint", 64 * 1024))
    except FileNotFoundError:
        _unlink_and_fsync(partial)
        return None
    except json.JSONDecodeError:
        _unlink_and_fsync(partial)
        _unlink_and_fsync(path)
        return None
    expected_keys = {"schema_version", "url", "expected_bytes", "downloaded_bytes", "etag"}
    if not isinstance(value, dict) or set(value) != expected_keys:
        _unlink_and_fsync(partial)
        _unlink_and_fsync(path)
        return None
    downloaded = value["downloaded_bytes"]
    etag = value["etag"]
    valid = (
        value["schema_version"] == 1
        and not isinstance(value["schema_version"], bool)
        and value["url"] == spec.url
        and value["expected_bytes"] == spec.expected_bytes
        and not isinstance(value["expected_bytes"], bool)
        and isinstance(downloaded, int)
        and not isinstance(downloaded, bool)
        and 0 <= downloaded <= spec.expected_bytes
        and isinstance(etag, str)
        and _strong_etag(etag) == spec.etag
    )
    try:
        descriptor, opened = _open_regular_nofollow(partial, "partial")
    except FileNotFoundError as error:
        raise LandCoverError("land cover archive partial changed during use") from error
    try:
        actual = opened.st_size
        _verify_path_identity(partial, opened, "partial")
    finally:
        os.close(descriptor)
    if not valid or actual > spec.expected_bytes:
        _unlink_and_fsync(partial)
        _unlink_and_fsync(path)
        return None
    return ArchiveCheckpoint(actual, spec.etag)


class _RetryableArchiveResponse(Exception):
    pass


def _download_archive_attempt(
    client: httpx.Client,
    spec: ArchiveTransferSpec,
    partial: Path,
    checkpoint_path: Path,
    checkpoint: ArchiveCheckpoint | None,
) -> ArchiveCheckpoint:
    offset = _regular_size_if_exists(partial, "partial")
    if offset and checkpoint is None:
        partial.unlink()
        offset = 0
    headers = (
        {"Range": f"bytes={offset}-", "If-Range": checkpoint.etag}
        if offset and checkpoint is not None
        else {}
    )
    with client.stream("GET", spec.url, headers=headers) as response:
        if str(response.url) != spec.url or 300 <= response.status_code <= 399:
            raise LandCoverError(
                f"land cover archive redirected outside its pinned URL: {response.url}"
            )
        if response.status_code in {408, 429} or 500 <= response.status_code <= 599:
            raise _RetryableArchiveResponse(f"HTTP {response.status_code}")
        response.raise_for_status()
        response_etag = _strong_etag(response.headers.get("etag"))
        content_length = response.headers.get("content-length", "")
        if response_etag != spec.etag:
            _unlink_and_fsync(partial)
            _unlink_and_fsync(checkpoint_path)
            raise LandCoverError("land cover archive response ETag does not match the pinned ETag")
        if offset:
            content_range = response.headers.get("content-range", "")
            match = CONTENT_RANGE.fullmatch(content_range)
            expected_response_bytes = spec.expected_bytes - offset
            if (
                response.status_code != 206
                or match is None
                or int(match.group("start")) != offset
                or int(match.group("end")) != spec.expected_bytes - 1
                or int(match.group("total")) != spec.expected_bytes
                or content_length != str(expected_response_bytes)
            ):
                _unlink_and_fsync(partial)
                _unlink_and_fsync(checkpoint_path)
                raise LandCoverError("land cover archive resumed response is invalid")
        elif response.status_code != 200 or content_length != str(spec.expected_bytes):
            raise LandCoverError("land cover archive response size or status is invalid")
        state = ArchiveCheckpoint(offset, spec.etag)
        _write_checkpoint(checkpoint_path, state, spec)
        flags = os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW
        flags |= os.O_APPEND if offset else os.O_TRUNC
        descriptor = os.open(partial, flags, 0o600)
        if not stat.S_ISREG(os.fstat(descriptor).st_mode) or os.fstat(descriptor).st_size != offset:
            os.close(descriptor)
            raise LandCoverError("land cover archive partial changed during download")
        with os.fdopen(descriptor, "ab" if offset else "wb") as output:
            for raw_chunk in response.iter_raw():
                for start in range(0, len(raw_chunk), ARCHIVE_DOWNLOAD_CHUNK_BYTES):
                    output.write(raw_chunk[start : start + ARCHIVE_DOWNLOAD_CHUNK_BYTES])
                    output.flush()
                    os.fsync(output.fileno())
                    state = ArchiveCheckpoint(output.tell(), spec.etag)
                    _write_checkpoint(checkpoint_path, state, spec)
        return state


def _fetch_archive(
    destination: Path,
    spec: ArchiveTransferSpec,
    *,
    client: httpx.Client,
    sleep: Callable[[float], None] = time.sleep,
    max_attempts: int = ARCHIVE_DOWNLOAD_MAX_ATTEMPTS,
    audited_sha256: str | None = None,
) -> str:
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_suffix(destination.suffix + ".part")
    checkpoint_path = destination.with_suffix(destination.suffix + ".download.json")
    lock = destination.with_suffix(destination.suffix + ".download.lock")
    for path, label in (
        (destination, "destination"),
        (partial, "partial"),
        (checkpoint_path, "checkpoint"),
        (lock, "lock"),
    ):
        _require_regular_or_missing(path, label)
    try:
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(
                {"schema_version": 1, "pid": os.getpid(), "host": socket.gethostname()},
                output,
                sort_keys=True,
                separators=(",", ":"),
            )
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        _fsync_directory(lock.parent)
    except FileExistsError as error:
        _require_regular_or_missing(lock, "lock")
        raise LandCoverError(
            "land cover archive download lock exists; verify its recorded PID and host have no "
            f"running fetch, then remove only {lock}"
        ) from error
    try:
        if (
            _strong_etag(spec.etag) != spec.etag
            or spec.expected_bytes <= 0
            or max_attempts <= 0
            or (audited_sha256 is not None and not _is_sha256(audited_sha256))
        ):
            raise LandCoverError("land cover archive transfer specification is invalid")
        try:
            destination.lstat()
            destination_present = True
        except FileNotFoundError:
            destination_present = False
        if destination_present:
            digest, opened_destination = _sha256_regular_nofollow(destination, "destination")
            if opened_destination.st_size != spec.expected_bytes:
                raise LandCoverError("existing land cover archive has the wrong size")
            if audited_sha256 is not None and digest != audited_sha256:
                raise LandCoverError(
                    "existing land cover archive does not match the audited SHA-256"
                )
            _verify_path_identity(destination, opened_destination, "destination")
            return digest
        checkpoint = _load_checkpoint(checkpoint_path, partial, spec)
        if checkpoint is None or checkpoint.downloaded_bytes < spec.expected_bytes:
            for attempt in range(1, max_attempts + 1):
                try:
                    checkpoint = _download_archive_attempt(
                        client, spec, partial, checkpoint_path, checkpoint
                    )
                    break
                except (httpx.TransportError, _RetryableArchiveResponse) as error:
                    checkpoint = _load_checkpoint(checkpoint_path, partial, spec)
                    downloaded = _regular_size_if_exists(partial, "partial")
                    if attempt == max_attempts:
                        raise TransientLandCoverTransferError(
                            f"land cover archive transfer exhausted {max_attempts} attempts at "
                            f"{downloaded:,}/{spec.expected_bytes:,} bytes: {error}"
                        ) from error
                    delay = min(
                        ARCHIVE_DOWNLOAD_BACKOFF_SECONDS * (2 ** (attempt - 1)),
                        ARCHIVE_DOWNLOAD_MAX_BACKOFF_SECONDS,
                    )
                    print(
                        f"transient land cover archive failure on attempt {attempt}/"
                        f"{max_attempts} at {downloaded:,}/{spec.expected_bytes:,} bytes: "
                        f"{error}; retrying in {delay:g}s",
                        flush=True,
                    )
                    sleep(delay)
        if checkpoint is None or checkpoint.downloaded_bytes != spec.expected_bytes:
            raise LandCoverError("land cover archive transfer ended before the exact expected size")
        digest, opened_partial = _sha256_regular_nofollow(partial, "partial")
        if opened_partial.st_size != spec.expected_bytes:
            raise LandCoverError("land cover archive partial size does not match its checkpoint")
        if audited_sha256 is not None and digest != audited_sha256:
            raise LandCoverError("downloaded land cover archive does not match the audited SHA-256")
        _verify_path_identity(partial, opened_partial, "partial")
        _replace_and_fsync(partial, destination)
        _unlink_and_fsync(checkpoint_path)
        return digest
    finally:
        _unlink_and_fsync(lock)


def fetch_archive(destination: Path = DEFAULT_ARCHIVE_PATH) -> str:
    spec = ArchiveTransferSpec(ARCHIVE_URL, ARCHIVE_BYTES, ARCHIVE_ETAG)
    with httpx.Client(timeout=ARCHIVE_TIMEOUT, follow_redirects=False) as client:
        return _fetch_archive(
            destination,
            spec,
            client=client,
            audited_sha256=AUDITED_SOURCE_ARCHIVE_SHA256,
        )


def _json_int(value: object, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise LandCoverError(f"{name} must be a JSON integer")
    return value


def _json_float(value: object, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise LandCoverError(f"{name} must be a JSON number")
    return float(value)


def _json_string(value: object, name: str) -> str:
    if not isinstance(value, str):
        raise LandCoverError(f"{name} must be a JSON string")
    return value


def _parse_grid(value: object) -> GridSpec:
    if not isinstance(value, dict) or set(value) != {
        "epsg",
        "height",
        "max_x",
        "max_y",
        "min_x",
        "min_y",
        "resampling",
        "row_order",
        "width",
    }:
        raise LandCoverError("grid JSON has an unexpected schema")
    return GridSpec(
        epsg=_json_int(value["epsg"], "grid epsg"),
        width=_json_int(value["width"], "grid width"),
        height=_json_int(value["height"], "grid height"),
        min_x=_json_float(value["min_x"], "grid min_x"),
        min_y=_json_float(value["min_y"], "grid min_y"),
        max_x=_json_float(value["max_x"], "grid max_x"),
        max_y=_json_float(value["max_y"], "grid max_y"),
        row_order=_json_string(value["row_order"], "grid row_order"),
        resampling=_json_string(value["resampling"], "grid resampling"),
    )


def load_grid(path: Path) -> GridSpec:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise LandCoverError(f"cannot read land cover grid JSON: {path}") from error
    return _parse_grid(value)


def _class_chunks(classes: NDArray[np.uint8]) -> Iterator[memoryview]:
    flat = memoryview(classes).cast("B")
    for offset in range(0, len(flat), CHUNK_BYTES):
        yield flat[offset : offset + CHUNK_BYTES]


def _validate_classes(classes: NDArray[np.uint8], grid: GridSpec) -> str:
    if classes.dtype != np.uint8 or classes.ndim != 2:
        raise LandCoverError("class grid must be a two-dimensional uint8 NumPy array")
    if classes.shape != (grid.height, grid.width):
        raise LandCoverError(
            f"class grid shape {classes.shape} does not match {(grid.height, grid.width)}"
        )
    if not classes.flags.c_contiguous:
        raise LandCoverError("class grid must use contiguous row-major storage")
    digest = hashlib.sha256()
    for chunk in _class_chunks(classes):
        values = np.frombuffer(chunk, dtype=np.uint8)
        if values.size and int(values.max()) > 7:
            invalid = np.unique(values[values > 7]).tolist()
            raise LandCoverError(f"class grid contains values outside 0 through 7: {invalid}")
        digest.update(chunk)
    return digest.hexdigest()


def _class_counts(
    classes: NDArray[np.uint8], grid: GridSpec
) -> tuple[int, int, int, int, int, int, int, int]:
    _validate_classes(classes, grid)
    counts = np.zeros(8, dtype=np.uint64)
    flat = classes.reshape(-1)
    for start in range(0, flat.size, CHUNK_BYTES):
        chunk = np.asarray(flat[start : start + CHUNK_BYTES])
        counts += np.bincount(chunk, minlength=8).astype(np.uint64)
    return (
        int(counts[0]),
        int(counts[1]),
        int(counts[2]),
        int(counts[3]),
        int(counts[4]),
        int(counts[5]),
        int(counts[6]),
        int(counts[7]),
    )


def _header_json(header: MaskHeader) -> bytes:
    value = asdict(header)
    value.update(
        {
            "archive_url": ARCHIVE_URL,
            "classes": {str(code): name for code, name in CLASS_NAMES.items()},
            "dataset_id": DATASET_ID,
            "layer_id": LAYER_ID,
            "nodata": 0,
            "resampling_contract": "nearest",
            "rights_notice": RIGHTS_NOTICE,
        }
    )
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def write_mask(
    path: Path,
    classes: NDArray[np.uint8],
    grid: GridSpec,
    *,
    source_archive_sha256: str,
    source_archive_bytes: int,
    audited_source_sha256: str | None = AUDITED_SOURCE_ARCHIVE_SHA256,
) -> MaskHeader:
    path.parent.mkdir(parents=True, exist_ok=True)
    lock = path.with_name(f".{path.name}.lock")
    try:
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(descriptor)
    except FileExistsError as error:
        raise LandCoverError("another land cover mask writer is already running") from error
    try:
        return _write_mask_locked(
            path,
            classes,
            grid,
            source_archive_sha256=source_archive_sha256,
            source_archive_bytes=source_archive_bytes,
            audited_source_sha256=audited_source_sha256,
        )
    finally:
        lock.unlink(missing_ok=True)


def _write_mask_locked(
    path: Path,
    classes: NDArray[np.uint8],
    grid: GridSpec,
    *,
    source_archive_sha256: str,
    source_archive_bytes: int,
    audited_source_sha256: str | None,
) -> MaskHeader:
    verify_source_pin()
    if not _is_sha256(source_archive_sha256):
        raise LandCoverError("source archive SHA-256 must be 64 lowercase hexadecimal characters")
    if source_archive_bytes != ARCHIVE_BYTES:
        raise LandCoverError(
            f"source archive is {source_archive_bytes:,} bytes; expected {ARCHIVE_BYTES:,}"
        )
    if audited_source_sha256 is None:
        raise LandCoverError(
            "the source archive SHA-256 is not audited; run the source-candidate command, "
            "review the archive, and update AUDITED_SOURCE_ARCHIVE_SHA256"
        )
    if source_archive_sha256 != audited_source_sha256:
        raise LandCoverError("source archive SHA-256 does not match the audited source")
    payload_sha256 = _validate_classes(classes, grid)
    header = MaskHeader(
        schema_version=SCHEMA_VERSION,
        source_identity_sha256=SOURCE_IDENTITY_SHA256,
        source_archive_sha256=source_archive_sha256,
        source_archive_bytes=source_archive_bytes,
        payload_sha256=payload_sha256,
        grid=grid,
    )
    header_json = _header_json(header)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".part", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with temporary.open("wb") as output:
            output.write(HEADER_PREFIX.pack(MAGIC, SCHEMA_VERSION, len(header_json)))
            output.write(header_json)
            for chunk in _class_chunks(classes):
                output.write(chunk)
        audited = load_mask(
            temporary,
            audited_source_sha256=audited_source_sha256,
            expected_grid=grid,
        )
        if audited.header != header:
            raise LandCoverError("post-write land cover mask audit changed its header")
        del audited
        temporary.replace(path)
        return header
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def _parse_header(value: object) -> MaskHeader:
    if not isinstance(value, dict):
        raise LandCoverError("land cover header must be a JSON object")
    expected = {
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
    }
    if set(value) != expected:
        raise LandCoverError("land cover header has an unexpected schema")
    archive_url = _json_string(value["archive_url"], "archive URL")
    dataset_id = _json_int(value["dataset_id"], "dataset ID")
    layer_id = _json_int(value["layer_id"], "layer ID")
    nodata = _json_int(value["nodata"], "NoData class")
    classes = value["classes"]
    if not isinstance(classes, dict) or set(classes) != {str(code) for code in CLASS_NAMES}:
        raise LandCoverError("land cover artifact class table has an unexpected schema")
    parsed_classes = {
        _json_string(key, "class code"): _json_string(name, "class name")
        for key, name in classes.items()
    }
    if archive_url != ARCHIVE_URL or dataset_id != DATASET_ID:
        raise LandCoverError("land cover artifact names a different source")
    if layer_id != LAYER_ID or parsed_classes != {
        str(code): name for code, name in CLASS_NAMES.items()
    }:
        raise LandCoverError("land cover artifact names a different layer or class table")
    if nodata != 0:
        raise LandCoverError("land cover artifact uses an unsupported NoData class")
    if (
        _json_string(value["resampling_contract"], "resampling contract") != "nearest"
        or _json_string(value["rights_notice"], "rights notice") != RIGHTS_NOTICE
    ):
        raise LandCoverError("land cover artifact policy changed")
    header = MaskHeader(
        schema_version=_json_int(value["schema_version"], "schema version"),
        source_identity_sha256=_json_string(
            value["source_identity_sha256"], "source identity SHA-256"
        ),
        source_archive_sha256=_json_string(
            value["source_archive_sha256"], "source archive SHA-256"
        ),
        source_archive_bytes=_json_int(value["source_archive_bytes"], "source archive bytes"),
        payload_sha256=_json_string(value["payload_sha256"], "payload SHA-256"),
        grid=_parse_grid(value["grid"]),
    )
    if header.schema_version != SCHEMA_VERSION:
        raise LandCoverError(
            f"land cover schema {header.schema_version} is unsupported; regenerate the artifact"
        )
    if header.source_identity_sha256 != SOURCE_IDENTITY_SHA256:
        raise LandCoverError("land cover source identity pin does not match")
    if header.source_archive_bytes != ARCHIVE_BYTES:
        raise LandCoverError("land cover source archive size does not match the audited source")
    for digest in (header.source_archive_sha256, header.payload_sha256):
        if not _is_sha256(digest):
            raise LandCoverError("land cover header contains an invalid SHA-256")
    return header


def load_mask(
    path: Path,
    *,
    audited_source_sha256: str | None = AUDITED_SOURCE_ARCHIVE_SHA256,
    expected_grid: GridSpec | None = None,
) -> LandCoverMask:
    verify_source_pin()
    try:
        with path.open("rb") as source:
            prefix = source.read(HEADER_PREFIX.size)
            if len(prefix) != HEADER_PREFIX.size:
                raise LandCoverError("land cover artifact is truncated")
            magic, schema_version, header_length = HEADER_PREFIX.unpack(prefix)
            if magic != MAGIC or schema_version != SCHEMA_VERSION or header_length > 64 * 1024:
                raise LandCoverError("land cover artifact prefix is invalid")
            header_bytes = source.read(header_length)
            if len(header_bytes) != header_length:
                raise LandCoverError("land cover artifact header is truncated")
            try:
                value = json.loads(header_bytes)
            except json.JSONDecodeError as error:
                raise LandCoverError("land cover artifact header is invalid JSON") from error
            header = _parse_header(value)
            payload_offset = HEADER_PREFIX.size + header_length
    except OSError as error:
        raise LandCoverError(f"cannot read land cover artifact: {path}") from error
    if header.grid != (reviewed_grid() if expected_grid is None else expected_grid):
        raise LandCoverError("land cover artifact does not use the reviewed production grid")
    expected_bytes = header.grid.width * header.grid.height
    if path.stat().st_size != payload_offset + expected_bytes:
        raise LandCoverError("land cover artifact payload size does not match its grid")
    if audited_source_sha256 is None:
        raise LandCoverError(
            "the source archive SHA-256 is not audited; review the source before using this mask"
        )
    if header.source_archive_sha256 != audited_source_sha256:
        raise LandCoverError("land cover artifact does not use the audited source archive")
    classes = np.memmap(
        path,
        dtype=np.uint8,
        mode="r",
        offset=payload_offset,
        shape=(header.grid.height, header.grid.width),
        order="C",
    )
    if _validate_classes(classes, header.grid) != header.payload_sha256:
        raise LandCoverError("land cover artifact payload SHA-256 does not match")
    return LandCoverMask(header, classes)


def effective_class(
    land_cover: LandCoverClass | None, *, hydrology_contains_point: bool
) -> LandCoverClass | None:
    if hydrology_contains_point:
        return LandCoverClass.WATER
    return land_cover


def reviewed_grid() -> GridSpec:
    min_x, min_y, max_x, max_y, width, height = TARGET_GRID
    return GridSpec(
        epsg=32129,
        width=int(width),
        height=int(height),
        min_x=min_x,
        min_y=min_y,
        max_x=max_x,
        max_y=max_y,
    )


def _archive_members(path: Path) -> tuple[tuple[ArchiveMember, ...], str]:
    members: list[ArchiveMember] = []
    try:
        with zipfile.ZipFile(path) as archive:
            infos = sorted(archive.infolist(), key=lambda info: info.filename)
            names = [info.filename for info in infos]
            if len(names) != len(set(names)):
                raise LandCoverError("source archive has duplicate member names")
            for info in infos:
                member_path = Path(info.filename)
                if member_path.is_absolute() or ".." in member_path.parts:
                    raise LandCoverError(f"source archive has an unsafe member: {info.filename}")
                if info.is_dir():
                    continue
                digest = hashlib.sha256()
                with archive.open(info) as source:
                    while chunk := source.read(CHUNK_BYTES):
                        digest.update(chunk)
                members.append(ArchiveMember(info.filename, info.file_size, digest.hexdigest()))
    except (OSError, zipfile.BadZipFile) as error:
        raise LandCoverError(f"cannot inspect source archive: {path}") from error
    if not members or not any(member.name.endswith(".gdb/gdb") for member in members):
        raise LandCoverError("source archive does not contain a File Geodatabase")
    value = [asdict(member) for member in members]
    return tuple(members), canonical_sha256(value)


def _run(command: list[str]) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(command, check=True, capture_output=True, text=True)
    except FileNotFoundError as error:
        raise LandCoverError(f"required GDAL command is missing: {command[0]}") from error
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip() or error.stdout.strip() or f"exit {error.returncode}"
        raise LandCoverError(f"GDAL command failed: {detail}") from error


def _command_output(command: list[str]) -> str:
    result = _run(command)
    return (result.stdout + result.stderr).strip()


def _gdal_version(output: str, command: str) -> str:
    match = re.match(r"^GDAL (?P<version>[0-9]+\.[0-9]+\.[0-9]+)(?:\s|,|$)", output)
    if match is None:
        raise LandCoverError(f"cannot parse {command} version: {output}")
    version = match.group("version")
    if version != GDAL_VERSION:
        raise LandCoverError(f"{command} is GDAL {version}; reviewed version is {GDAL_VERSION}")
    return version


def _toolchain_evidence(gdalwarp: str, gdalinfo: str, proj: str) -> ToolchainEvidence:
    gdalinfo_version = _command_output([gdalinfo, "--version"])
    gdalwarp_version = _command_output([gdalwarp, "--version"])
    version = _gdal_version(gdalinfo_version, gdalinfo)
    if _gdal_version(gdalwarp_version, gdalwarp) != version:
        raise LandCoverError("gdalwarp and gdalinfo versions do not match")
    formats = _command_output([gdalinfo, "--formats"])
    matching_drivers = [
        line.strip() for line in formats.splitlines() if line.strip().startswith("OpenFileGDB ")
    ]
    if len(matching_drivers) != 1 or "raster" not in matching_drivers[0].lower():
        raise LandCoverError("GDAL OpenFileGDB raster driver is unavailable or ambiguous")
    gdalinfo_help = _command_output([gdalinfo, "--help-general"])
    gdalwarp_help = _command_output([gdalwarp, "--help-general"])
    gdalinfo_build = _command_output([gdalinfo, "--build"])
    gdalwarp_build = _command_output([gdalwarp, "--build"])
    proj_version_output = _command_output([proj, "--version"])
    words = proj_version_output.replace("Rel.", "").split()
    versions = [word.rstrip(",") for word in words if word[:1].isdigit()]
    if not versions:
        raise LandCoverError(f"cannot parse PROJ version: {proj_version_output}")
    return ToolchainEvidence(
        gdal_version=version,
        gdalinfo_version_sha256=canonical_sha256(gdalinfo_version),
        gdalinfo_build_sha256=canonical_sha256(gdalinfo_build),
        gdalinfo_formats_sha256=canonical_sha256(formats),
        gdalinfo_help_sha256=canonical_sha256(gdalinfo_help),
        gdalwarp_version_sha256=canonical_sha256(gdalwarp_version),
        gdalwarp_build_sha256=canonical_sha256(gdalwarp_build),
        gdalwarp_help_sha256=canonical_sha256(gdalwarp_help),
        proj_version=versions[0],
        proj_version_sha256=canonical_sha256(proj_version_output),
    )


def _gdal_info(command: str, source: str) -> dict[str, object]:
    result = _run([command, "-json", source])
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise LandCoverError(f"{command} returned invalid JSON") from error
    if not isinstance(value, dict):
        raise LandCoverError(f"{command} JSON must be an object")
    return value


def _raster_evidence(
    value: dict[str, object],
    *,
    require_target: bool,
    target_grid: GridSpec | None = None,
    archive_token: str | None = None,
) -> RasterEvidence:
    driver = _json_string(value.get("driverShortName"), "raster driver")
    description = _json_string(value.get("description"), "raster description")
    files_value = value.get("files")
    if driver not in ({"ENVI"} if require_target else {"OpenFileGDB"}):
        raise LandCoverError("GDAL opened the raster with an unexpected driver")
    if not isinstance(files_value, list) or not files_value:
        raise LandCoverError("GDAL raster metadata has no source files")
    files = tuple(_json_string(item, "raster file") for item in files_value)
    metadata = value.get("metadata")
    if isinstance(metadata, dict) and "SUBDATASETS" in metadata:
        raise LandCoverError("GDAL opened a FileGDB container instead of the exact raster")
    if archive_token is not None:
        description = description.replace(archive_token, "{archive}")
        files = tuple(item.replace(archive_token, "{archive}") for item in files)
    size = value.get("size")
    bands = value.get("bands")
    transform = value.get("geoTransform")
    coordinate_system = value.get("coordinateSystem")
    if not isinstance(size, list) or len(size) != 2:
        raise LandCoverError("GDAL raster metadata has an invalid size")
    if not isinstance(bands, list) or len(bands) != 1 or not isinstance(bands[0], dict):
        raise LandCoverError("land cover source must contain exactly one raster band")
    if not isinstance(transform, list) or len(transform) != 6:
        raise LandCoverError("GDAL raster metadata has no six-value geotransform")
    if not isinstance(coordinate_system, dict):
        raise LandCoverError("GDAL raster metadata has no coordinate system")
    width = _json_int(size[0], "raster width")
    height = _json_int(size[1], "raster height")
    if width <= 0 or height <= 0:
        raise LandCoverError("raster dimensions must be positive")
    geotransform = (
        _json_float(transform[0], "geotransform 0"),
        _json_float(transform[1], "geotransform 1"),
        _json_float(transform[2], "geotransform 2"),
        _json_float(transform[3], "geotransform 3"),
        _json_float(transform[4], "geotransform 4"),
        _json_float(transform[5], "geotransform 5"),
    )
    if not all(math.isfinite(item) for item in geotransform):
        raise LandCoverError("raster geotransform must be finite")
    if geotransform[1] <= 0 or geotransform[5] >= 0 or geotransform[2] != 0 or geotransform[4] != 0:
        raise LandCoverError("raster must be north up with no rotation")
    band = bands[0]
    data_type = _json_string(band.get("type"), "raster data type")
    if data_type != "Byte":
        raise LandCoverError("land cover raster must use Byte cells")
    raw_nodata = band.get("noDataValue")
    nodata = None if raw_nodata is None else _json_int(raw_nodata, "raster NoData")
    if nodata not in (None, 0):
        raise LandCoverError("land cover raster NoData must be absent or zero")
    wkt = _json_string(coordinate_system.get("wkt"), "raster CRS WKT")
    if require_target:
        grid = target_grid or reviewed_grid()
        expected = (grid.min_x, grid.pixel_width, 0.0, grid.max_y, 0.0, -grid.pixel_height)
        if (width, height) != (grid.width, grid.height) or any(
            not math.isclose(actual, wanted, rel_tol=0.0, abs_tol=1e-8)
            for actual, wanted in zip(geotransform, expected, strict=True)
        ):
            raise LandCoverError("converted raster does not match the reviewed target grid")
        if "32129" not in wkt:
            raise LandCoverError("converted raster CRS is not EPSG:32129")
    else:
        if "NAD83" not in wkt and "North_American_1983" not in wkt:
            raise LandCoverError("source raster CRS is not NAD83")
        if "Foot_US" not in wkt and "US survey foot" not in wkt:
            raise LandCoverError("source raster CRS does not use US survey feet")
        extent = (
            geotransform[0],
            geotransform[3] + height * geotransform[5],
            geotransform[0] + width * geotransform[1],
            geotransform[3],
        )
        tolerance = max(abs(geotransform[1]), abs(geotransform[5])) * 2
        if any(
            not math.isclose(actual, expected, rel_tol=0.0, abs_tol=tolerance)
            for actual, expected in zip(extent, SOURCE_EXTENT_US_SURVEY_FEET, strict=True)
        ):
            raise LandCoverError("source raster extent does not match PASDA layer 2 metadata")
    return RasterEvidence(
        driver=driver,
        description=description,
        files=files,
        width=width,
        height=height,
        data_type=data_type,
        nodata=nodata,
        geotransform=geotransform,
        crs_wkt_sha256=hashlib.sha256(wkt.encode("utf-8")).hexdigest(),
    )


def _warp_command(
    gdalwarp: str, source: str, output: Path, *, target_grid: GridSpec | None = None
) -> list[str]:
    grid = target_grid or reviewed_grid()
    return [
        gdalwarp,
        "-overwrite",
        "-of",
        "ENVI",
        "-ot",
        "Byte",
        "-r",
        "near",
        "-t_srs",
        "EPSG:32129",
        "-te_srs",
        "EPSG:32129",
        "-te",
        str(grid.min_x),
        str(grid.min_y),
        str(grid.max_x),
        str(grid.max_y),
        "-tr",
        str(grid.pixel_width),
        str(grid.pixel_height),
        "-tap",
        "-srcnodata",
        "0",
        "-dstnodata",
        "0",
        "-wm",
        "64",
        "-wo",
        "NUM_THREADS=1",
        "-co",
        "INTERLEAVE=BSQ",
        source,
        str(output),
    ]


def _parse_envi_header(path: Path, grid: GridSpec) -> EnviHeader:
    try:
        lines = path.read_text(encoding="ascii").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise LandCoverError("converted ENVI header is missing or not ASCII") from error
    if not lines or lines[0].strip() != "ENVI":
        raise LandCoverError("converted ENVI header has no ENVI signature")
    values: dict[str, str] = {}
    for line in lines[1:]:
        if "=" not in line:
            continue
        key, value = (part.strip() for part in line.split("=", maxsplit=1))
        key = key.lower()
        if key in values:
            raise LandCoverError(f"converted ENVI header repeats {key}")
        values[key] = value
    required = {
        "samples",
        "lines",
        "bands",
        "header offset",
        "data type",
        "interleave",
        "byte order",
    }
    if not required.issubset(values):
        raise LandCoverError("converted ENVI header is missing required fields")

    def integer(key: str) -> int:
        value = values[key]
        if not value.isascii() or not value.isdecimal():
            raise LandCoverError(f"converted ENVI {key} must be a nonnegative integer")
        return int(value)

    header = EnviHeader(
        samples=integer("samples"),
        lines=integer("lines"),
        bands=integer("bands"),
        header_offset=integer("header offset"),
        data_type=integer("data type"),
        interleave=values["interleave"].lower(),
        byte_order=integer("byte order"),
    )
    expected = EnviHeader(grid.width, grid.height, 1, 0, 1, "bsq", 0)
    if header != expected:
        raise LandCoverError("converted ENVI storage does not match the reviewed byte grid")
    return header


def _raw_to_npy(
    raw_path: Path, output_path: Path, grid: GridSpec
) -> tuple[str, tuple[int, int, int, int, int, int, int, int]]:
    expected_bytes = grid.width * grid.height
    if raw_path.stat().st_size != expected_bytes:
        raise LandCoverError("converted raw raster size does not match the reviewed grid")
    classes = np.memmap(raw_path, dtype=np.uint8, mode="r", shape=(grid.height, grid.width))
    output = np.lib.format.open_memmap(
        output_path, mode="w+", dtype=np.uint8, shape=(grid.height, grid.width)
    )
    counts = np.zeros(8, dtype=np.uint64)
    rows_per_chunk = max(1, CHUNK_BYTES // grid.width)
    try:
        for start in range(0, grid.height, rows_per_chunk):
            stop = min(grid.height, start + rows_per_chunk)
            chunk = np.asarray(classes[start:stop])
            if chunk.size and int(chunk.max()) > 7:
                invalid = np.unique(chunk[chunk > 7]).tolist()
                raise LandCoverError(f"converted raster has invalid classes: {invalid}")
            counts += np.bincount(chunk.reshape(-1), minlength=8).astype(np.uint64)
            output[start:stop] = chunk
        output.flush()
    finally:
        del output
        del classes
    return sha256_file(output_path), (
        int(counts[0]),
        int(counts[1]),
        int(counts[2]),
        int(counts[3]),
        int(counts[4]),
        int(counts[5]),
        int(counts[6]),
        int(counts[7]),
    )


def _write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def _source_dataset_uri(archive_path: Path, gdb_root: str, raster_name: str) -> str:
    simple_name = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*", flags=re.ASCII)
    for value, label, suffix in (
        (gdb_root, "FileGDB root", ".gdb"),
        (raster_name, "raster name", ""),
    ):
        if (
            not value
            or value != Path(value).name
            or simple_name.fullmatch(value) is None
            or (suffix and not value.endswith(suffix))
        ):
            raise LandCoverError(f"audited {label} is invalid")
    vsi_path = f"/vsizip/{{{archive_path.resolve()}}}/{gdb_root}"
    return f'OpenFileGDB:"{vsi_path}":{raster_name}'


def source_candidate(
    archive_path: Path,
    gdb_root: str,
    raster_name: str,
    *,
    gdalwarp: str = "gdalwarp",
    gdalinfo: str = "gdalinfo",
    proj: str = "proj",
    expected_archive_bytes: int = ARCHIVE_BYTES,
) -> dict[str, object]:
    verify_source_pin()
    source_dataset = _source_dataset_uri(archive_path, gdb_root, raster_name)
    archive_bytes = archive_path.stat().st_size
    if archive_bytes != expected_archive_bytes:
        raise LandCoverError(
            f"source archive is {archive_bytes:,} bytes; expected {expected_archive_bytes:,}"
        )
    archive_sha256 = sha256_file(archive_path)
    members, members_sha256 = _archive_members(archive_path)
    toolchain = _toolchain_evidence(gdalwarp, gdalinfo, proj)
    raster = _raster_evidence(
        _gdal_info(gdalinfo, source_dataset),
        require_target=False,
        archive_token=str(archive_path.resolve()),
    )
    return {
        "source_identity_sha256": SOURCE_IDENTITY_SHA256,
        "source_archive_bytes": archive_bytes,
        "audited_source_archive_sha256": archive_sha256,
        "audited_gdb_root": gdb_root,
        "audited_raster_name": raster_name,
        "audited_raster_evidence": asdict(raster),
        "audited_toolchain": asdict(toolchain),
        "archive_members_sha256": members_sha256,
        "archive_members": [asdict(member) for member in members],
    }


def convert_filegdb(
    archive_path: Path,
    raster_name: str,
    output_root: Path,
    *,
    gdalwarp: str = "gdalwarp",
    gdalinfo: str = "gdalinfo",
    proj: str = "proj",
    audited_source_sha256: str | None = AUDITED_SOURCE_ARCHIVE_SHA256,
    audited_gdb_root: str | None = AUDITED_GDB_ROOT,
    audited_raster_name: str | None = AUDITED_RASTER_NAME,
    audited_source_evidence: RasterEvidence | None = AUDITED_RASTER_EVIDENCE,
    audited_toolchain: ToolchainEvidence | None = AUDITED_TOOLCHAIN,
    target_grid: GridSpec | None = None,
    expected_archive_bytes: int = ARCHIVE_BYTES,
) -> Path:
    output_root.mkdir(parents=True, exist_ok=True)
    lock = output_root / ".convert.lock"
    try:
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(descriptor)
    except FileExistsError as error:
        raise LandCoverError("another land cover conversion is already running") from error
    staging: Path | None = None
    pointer: Path | None = None
    try:
        if (
            audited_source_sha256 is None
            or audited_gdb_root is None
            or audited_raster_name is None
            or audited_source_evidence is None
            or audited_toolchain is None
        ):
            raise LandCoverError(
                "archive, FileGDB raster, source evidence, and toolchain pins must be audited"
            )
        if raster_name != audited_raster_name:
            raise LandCoverError("requested raster name does not match the audited raster")
        grid = target_grid or reviewed_grid()
        if archive_path.stat().st_size != expected_archive_bytes:
            raise LandCoverError("source archive size does not match the audited PASDA archive")
        source_sha256 = sha256_file(archive_path)
        if source_sha256 != audited_source_sha256:
            raise LandCoverError("source archive SHA-256 does not match the audited source")
        toolchain = _toolchain_evidence(gdalwarp, gdalinfo, proj)
        if toolchain != audited_toolchain:
            raise LandCoverError("GDAL, OpenFileGDB, or PROJ evidence does not match the audit")
        members, members_sha256 = _archive_members(archive_path)
        if not any(member.name.startswith(f"{audited_gdb_root}/") for member in members):
            raise LandCoverError("audited FileGDB root is absent from the source archive")
        source_dataset = _source_dataset_uri(archive_path, audited_gdb_root, audited_raster_name)
        source_evidence = _raster_evidence(
            _gdal_info(gdalinfo, source_dataset),
            require_target=False,
            archive_token=str(archive_path.resolve()),
        )
        if source_evidence != audited_source_evidence:
            raise LandCoverError("FileGDB raster evidence does not match the audited raster")
        staging = Path(tempfile.mkdtemp(prefix=".staging-", dir=output_root))
        raw_path = staging / "classes.dat"
        _run(_warp_command(gdalwarp, source_dataset, raw_path, target_grid=grid))
        _raster_evidence(_gdal_info(gdalinfo, str(raw_path)), require_target=True, target_grid=grid)
        envi = _parse_envi_header(raw_path.with_suffix(".hdr"), grid)
        classes_path = staging / "classes.npy"
        classes_sha256, counts = _raw_to_npy(raw_path, classes_path, grid)
        raw_path.unlink()
        raw_path.with_suffix(".hdr").unlink(missing_ok=True)
        manifest = ConversionManifest(
            schema_version=CONVERSION_SCHEMA_VERSION,
            source_identity_sha256=SOURCE_IDENTITY_SHA256,
            source_archive_sha256=source_sha256,
            archive_members_sha256=members_sha256,
            archive_members=members,
            source_dataset=source_dataset.replace(str(archive_path.resolve()), "{archive}"),
            toolchain=toolchain,
            resampling="nearest",
            source=source_evidence,
            envi=envi,
            grid=grid,
            classes_npy_sha256=classes_sha256,
            class_counts=counts,
        )
        _write_json(staging / "grid.json", asdict(grid))
        _write_json(staging / "conversion.json", asdict(manifest))
        _audit_conversion(
            staging,
            audited_source_sha256=audited_source_sha256,
            audited_source_evidence=audited_source_evidence,
            audited_toolchain=audited_toolchain,
            audited_gdb_root=audited_gdb_root,
            audited_raster_name=audited_raster_name,
            expected_grid=grid,
        )
        generation_name = canonical_sha256(asdict(manifest))
        generation_root = output_root / "generations"
        generation_root.mkdir(exist_ok=True)
        generation = generation_root / generation_name
        if generation.exists():
            shutil.rmtree(staging)
        else:
            staging.replace(generation)
        _audit_conversion(
            generation,
            audited_source_sha256=audited_source_sha256,
            audited_source_evidence=audited_source_evidence,
            audited_toolchain=audited_toolchain,
            audited_gdb_root=audited_gdb_root,
            audited_raster_name=audited_raster_name,
            expected_grid=grid,
        )
        descriptor, pointer_name = tempfile.mkstemp(
            prefix=".current-", suffix=".json", dir=output_root
        )
        os.close(descriptor)
        pointer = Path(pointer_name)
        _write_json(pointer, {"schema_version": 1, "generation": generation_name})
        pointer.replace(output_root / "current.json")
        return generation
    except Exception:
        if staging is not None:
            shutil.rmtree(staging, ignore_errors=True)
        if pointer is not None:
            pointer.unlink(missing_ok=True)
        raise
    finally:
        lock.unlink(missing_ok=True)


def _audit_conversion(
    directory: Path,
    *,
    audited_source_sha256: str | None = AUDITED_SOURCE_ARCHIVE_SHA256,
    audited_source_evidence: RasterEvidence | None = AUDITED_RASTER_EVIDENCE,
    audited_toolchain: ToolchainEvidence | None = AUDITED_TOOLCHAIN,
    audited_gdb_root: str | None = AUDITED_GDB_ROOT,
    audited_raster_name: str | None = AUDITED_RASTER_NAME,
    expected_grid: GridSpec | None = None,
) -> ConversionManifest:
    if (
        audited_source_sha256 is None
        or audited_source_evidence is None
        or audited_toolchain is None
        or audited_gdb_root is None
        or audited_raster_name is None
    ):
        raise LandCoverError("source and toolchain pins must be audited before conversion use")
    try:
        raw = json.loads((directory / "conversion.json").read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise LandCoverError("conversion manifest is missing or invalid") from error
    if not isinstance(raw, dict):
        raise LandCoverError("conversion manifest must be a JSON object")
    expected_keys = {
        "archive_members",
        "archive_members_sha256",
        "class_counts",
        "classes_npy_sha256",
        "envi",
        "grid",
        "resampling",
        "schema_version",
        "source",
        "source_archive_sha256",
        "source_dataset",
        "source_identity_sha256",
        "toolchain",
    }
    if set(raw) != expected_keys:
        raise LandCoverError("conversion manifest has an unexpected schema")
    try:
        members_raw = raw["archive_members"]
        source_raw = raw["source"]
        envi_raw = raw["envi"]
        toolchain_raw = raw["toolchain"]
        counts_raw = raw["class_counts"]
        if (
            not isinstance(members_raw, list)
            or not isinstance(source_raw, dict)
            or not isinstance(envi_raw, dict)
            or not isinstance(toolchain_raw, dict)
        ):
            raise LandCoverError("conversion manifest evidence is invalid")
        if set(source_raw) != {
            "crs_wkt_sha256",
            "data_type",
            "description",
            "driver",
            "files",
            "geotransform",
            "height",
            "nodata",
            "width",
        }:
            raise LandCoverError("conversion source evidence has an unexpected schema")
        if set(envi_raw) != {
            "bands",
            "byte_order",
            "data_type",
            "header_offset",
            "interleave",
            "lines",
            "samples",
        }:
            raise LandCoverError("conversion ENVI evidence has an unexpected schema")
        toolchain_keys = {field.name for field in fields(ToolchainEvidence)}
        if set(toolchain_raw) != toolchain_keys:
            raise LandCoverError("conversion toolchain evidence has an unexpected schema")
        if any(not isinstance(item, dict) for item in members_raw):
            raise LandCoverError("conversion archive member evidence is invalid")
        if any(set(item) != {"bytes", "name", "sha256"} for item in members_raw):
            raise LandCoverError("conversion archive member schema is invalid")
        if not isinstance(counts_raw, list) or len(counts_raw) != 8:
            raise LandCoverError("conversion class counts are invalid")
        members = tuple(
            ArchiveMember(
                _json_string(item["name"], "archive member name"),
                _json_int(item["bytes"], "archive member bytes"),
                _json_string(item["sha256"], "archive member SHA-256"),
            )
            for item in members_raw
        )
        geotransform_raw = source_raw["geotransform"]
        if not isinstance(geotransform_raw, list) or len(geotransform_raw) != 6:
            raise LandCoverError("conversion source geotransform is invalid")
        source_nodata_raw = source_raw["nodata"]
        source_files_raw = source_raw["files"]
        if not isinstance(source_files_raw, list) or not source_files_raw:
            raise LandCoverError("conversion source files are invalid")
        source_nodata = (
            None if source_nodata_raw is None else _json_int(source_nodata_raw, "source NoData")
        )
        source = RasterEvidence(
            driver=_json_string(source_raw["driver"], "source driver"),
            description=_json_string(source_raw["description"], "source description"),
            files=tuple(_json_string(value, "source file") for value in source_files_raw),
            width=_json_int(source_raw["width"], "source width"),
            height=_json_int(source_raw["height"], "source height"),
            data_type=_json_string(source_raw["data_type"], "source data type"),
            nodata=source_nodata,
            geotransform=(
                _json_float(geotransform_raw[0], "source geotransform 0"),
                _json_float(geotransform_raw[1], "source geotransform 1"),
                _json_float(geotransform_raw[2], "source geotransform 2"),
                _json_float(geotransform_raw[3], "source geotransform 3"),
                _json_float(geotransform_raw[4], "source geotransform 4"),
                _json_float(geotransform_raw[5], "source geotransform 5"),
            ),
            crs_wkt_sha256=_json_string(source_raw["crs_wkt_sha256"], "source CRS digest"),
        )
        envi = EnviHeader(
            samples=_json_int(envi_raw["samples"], "ENVI samples"),
            lines=_json_int(envi_raw["lines"], "ENVI lines"),
            bands=_json_int(envi_raw["bands"], "ENVI bands"),
            header_offset=_json_int(envi_raw["header_offset"], "ENVI header offset"),
            data_type=_json_int(envi_raw["data_type"], "ENVI data type"),
            interleave=_json_string(envi_raw["interleave"], "ENVI interleave"),
            byte_order=_json_int(envi_raw["byte_order"], "ENVI byte order"),
        )
        toolchain = ToolchainEvidence(
            gdal_version=_json_string(toolchain_raw["gdal_version"], "GDAL version"),
            gdalinfo_version_sha256=_json_string(
                toolchain_raw["gdalinfo_version_sha256"], "gdalinfo version digest"
            ),
            gdalinfo_build_sha256=_json_string(
                toolchain_raw["gdalinfo_build_sha256"], "gdalinfo build digest"
            ),
            gdalinfo_formats_sha256=_json_string(
                toolchain_raw["gdalinfo_formats_sha256"], "GDAL formats digest"
            ),
            gdalinfo_help_sha256=_json_string(
                toolchain_raw["gdalinfo_help_sha256"], "gdalinfo help digest"
            ),
            gdalwarp_version_sha256=_json_string(
                toolchain_raw["gdalwarp_version_sha256"], "gdalwarp version digest"
            ),
            gdalwarp_build_sha256=_json_string(
                toolchain_raw["gdalwarp_build_sha256"], "gdalwarp build digest"
            ),
            gdalwarp_help_sha256=_json_string(
                toolchain_raw["gdalwarp_help_sha256"], "gdalwarp help digest"
            ),
            proj_version=_json_string(toolchain_raw["proj_version"], "PROJ version"),
            proj_version_sha256=_json_string(
                toolchain_raw["proj_version_sha256"], "PROJ version digest"
            ),
        )
        manifest = ConversionManifest(
            schema_version=_json_int(raw["schema_version"], "conversion schema"),
            source_identity_sha256=_json_string(
                raw["source_identity_sha256"], "source identity SHA-256"
            ),
            source_archive_sha256=_json_string(
                raw["source_archive_sha256"], "source archive SHA-256"
            ),
            archive_members_sha256=_json_string(
                raw["archive_members_sha256"], "archive members SHA-256"
            ),
            archive_members=members,
            source_dataset=_json_string(raw["source_dataset"], "source dataset"),
            toolchain=toolchain,
            resampling=_json_string(raw["resampling"], "resampling"),
            source=source,
            envi=envi,
            grid=_parse_grid(raw["grid"]),
            classes_npy_sha256=_json_string(raw["classes_npy_sha256"], "classes NPY SHA-256"),
            class_counts=(
                _json_int(counts_raw[0], "class count 0"),
                _json_int(counts_raw[1], "class count 1"),
                _json_int(counts_raw[2], "class count 2"),
                _json_int(counts_raw[3], "class count 3"),
                _json_int(counts_raw[4], "class count 4"),
                _json_int(counts_raw[5], "class count 5"),
                _json_int(counts_raw[6], "class count 6"),
                _json_int(counts_raw[7], "class count 7"),
            ),
        )
    except (KeyError, TypeError) as error:
        raise LandCoverError("conversion manifest has an unexpected schema") from error
    if manifest.schema_version != CONVERSION_SCHEMA_VERSION:
        raise LandCoverError("conversion manifest schema is unsupported")
    if manifest.source_identity_sha256 != SOURCE_IDENTITY_SHA256:
        raise LandCoverError("conversion source identity does not match")
    if manifest.source_archive_sha256 != audited_source_sha256:
        raise LandCoverError("conversion archive digest does not match the audited source")
    if manifest.source != audited_source_evidence or manifest.toolchain != audited_toolchain:
        raise LandCoverError("conversion source or toolchain evidence does not match the audit")
    expected_dataset = (
        f'OpenFileGDB:"/vsizip/{{{{archive}}}}/{audited_gdb_root}":{audited_raster_name}'
    )
    if manifest.source_dataset != expected_dataset:
        raise LandCoverError("conversion source dataset does not match the audited raster")
    if manifest.archive_members_sha256 != canonical_sha256(
        [asdict(member) for member in manifest.archive_members]
    ):
        raise LandCoverError("conversion archive member evidence does not match")
    if not manifest.archive_members:
        raise LandCoverError("conversion archive member evidence is empty")
    if tuple(sorted(member.name for member in manifest.archive_members)) != tuple(
        member.name for member in manifest.archive_members
    ) or len({member.name for member in manifest.archive_members}) != len(manifest.archive_members):
        raise LandCoverError("conversion archive members are not unique and sorted")
    if any(
        member.bytes < 0 or not _is_sha256(member.sha256) for member in manifest.archive_members
    ):
        raise LandCoverError("conversion archive member evidence is invalid")
    if manifest.toolchain.gdal_version != GDAL_VERSION or manifest.resampling != "nearest":
        raise LandCoverError("conversion tool or resampling policy does not match")
    if manifest.grid != (expected_grid or reviewed_grid()):
        raise LandCoverError("conversion target grid does not match the reviewed grid")
    if load_grid(directory / "grid.json") != manifest.grid:
        raise LandCoverError("conversion grid JSON does not match its manifest")
    if manifest.envi != EnviHeader(manifest.grid.width, manifest.grid.height, 1, 0, 1, "bsq", 0):
        raise LandCoverError("conversion ENVI storage does not match the target grid")
    if (
        manifest.source.width <= 0
        or manifest.source.height <= 0
        or manifest.source.data_type != "Byte"
        or manifest.source.nodata not in (None, 0)
        or not _is_sha256(manifest.source.crs_wkt_sha256)
        or not all(math.isfinite(value) for value in manifest.source.geotransform)
        or manifest.source.geotransform[1] <= 0
        or manifest.source.geotransform[5] >= 0
        or manifest.source.geotransform[2] != 0
        or manifest.source.geotransform[4] != 0
        or not manifest.source.description.startswith('OpenFileGDB:"/vsizip/{{archive}}/')
        or not manifest.source.files
        or any("{archive}" not in value for value in manifest.source.files)
    ):
        raise LandCoverError("conversion source raster evidence is invalid")
    tool_digests = (
        manifest.toolchain.gdalinfo_version_sha256,
        manifest.toolchain.gdalinfo_build_sha256,
        manifest.toolchain.gdalinfo_formats_sha256,
        manifest.toolchain.gdalinfo_help_sha256,
        manifest.toolchain.gdalwarp_version_sha256,
        manifest.toolchain.gdalwarp_build_sha256,
        manifest.toolchain.gdalwarp_help_sha256,
        manifest.toolchain.proj_version_sha256,
    )
    if any(not _is_sha256(value) for value in tool_digests):
        raise LandCoverError("conversion toolchain evidence contains an invalid SHA-256")
    if any(count < 0 for count in manifest.class_counts) or sum(manifest.class_counts) != (
        manifest.grid.width * manifest.grid.height
    ):
        raise LandCoverError("conversion class counts do not cover the target grid")
    if not _is_sha256(manifest.classes_npy_sha256):
        raise LandCoverError("converted NPY digest is invalid")
    classes_path = directory / "classes.npy"
    if sha256_file(classes_path) != manifest.classes_npy_sha256:
        raise LandCoverError("converted NPY digest does not match")
    try:
        classes = np.load(classes_path, mmap_mode="r", allow_pickle=False)
    except (OSError, ValueError) as error:
        raise LandCoverError("converted class grid is not a valid NumPy array") from error
    if _class_counts(classes, manifest.grid) != manifest.class_counts:
        raise LandCoverError("converted class counts do not match")
    return manifest


def build_from_conversion(
    conversion_directory: Path, source_archive_path: Path, output_path: Path
) -> MaskHeader:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    lock = output_path.with_name(f".{output_path.name}.lock")
    try:
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(descriptor)
    except FileExistsError as error:
        raise LandCoverError("another land cover mask writer is already running") from error
    try:
        if AUDITED_SOURCE_ARCHIVE_SHA256 is None:
            raise LandCoverError(
                "the source archive SHA-256 is not audited; run the source-candidate command, "
                "review the archive, and update AUDITED_SOURCE_ARCHIVE_SHA256"
            )
        source_archive_bytes = source_archive_path.stat().st_size
        if source_archive_bytes != ARCHIVE_BYTES:
            raise LandCoverError(
                f"source archive is {source_archive_bytes:,} bytes; expected {ARCHIVE_BYTES:,}"
            )
        source_sha256 = sha256_file(source_archive_path)
        if source_sha256 != AUDITED_SOURCE_ARCHIVE_SHA256:
            raise LandCoverError("source archive SHA-256 does not match the audited source")
        manifest = _audit_conversion(
            conversion_directory, audited_source_sha256=AUDITED_SOURCE_ARCHIVE_SHA256
        )
        members, members_sha256 = _archive_members(source_archive_path)
        if members != manifest.archive_members or members_sha256 != manifest.archive_members_sha256:
            raise LandCoverError("conversion does not match the audited archive members")
        try:
            classes = np.load(
                conversion_directory / "classes.npy", mmap_mode="r", allow_pickle=False
            )
        except (OSError, ValueError) as error:
            raise LandCoverError("converted class grid is not a valid NumPy array") from error
        return _write_mask_locked(
            output_path,
            classes,
            manifest.grid,
            source_archive_sha256=source_sha256,
            source_archive_bytes=source_archive_bytes,
            audited_source_sha256=AUDITED_SOURCE_ARCHIVE_SHA256,
        )
    finally:
        lock.unlink(missing_ok=True)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Build and audit the local 2018 land cover mask")
    subparsers = parser.add_subparsers(dest="command", required=True)
    fetch = subparsers.add_parser("fetch")
    fetch.add_argument("--output", type=Path, default=DEFAULT_ARCHIVE_PATH)
    convert = subparsers.add_parser("convert")
    convert.add_argument("--source-archive", type=Path, required=True)
    convert.add_argument("--raster-name", required=True, help="exact audited FileGDB raster name")
    convert.add_argument("--output-root", type=Path, default=DEFAULT_CONVERSION_ROOT)
    convert.add_argument("--gdalwarp", default="gdalwarp")
    convert.add_argument("--gdalinfo", default="gdalinfo")
    convert.add_argument("--proj", default="proj")
    build = subparsers.add_parser("build")
    build.add_argument("--conversion", type=Path, required=True)
    build.add_argument("--source-archive", type=Path, required=True)
    build.add_argument("--output", type=Path, default=DEFAULT_MASK_PATH)
    audit = subparsers.add_parser("audit")
    audit.add_argument("--input", type=Path, default=DEFAULT_MASK_PATH)
    candidate = subparsers.add_parser("source-candidate")
    candidate.add_argument("--source-archive", type=Path, required=True)
    candidate.add_argument("--gdb-root", required=True, help="simple exact FileGDB root name")
    candidate.add_argument("--raster-name", required=True, help="simple exact raster name")
    candidate.add_argument("--gdalwarp", default="gdalwarp")
    candidate.add_argument("--gdalinfo", default="gdalinfo")
    candidate.add_argument("--proj", default="proj")
    return parser


def main() -> None:
    arguments = _parser().parse_args()
    if arguments.command == "fetch":
        digest = fetch_archive(arguments.output)
        print(f"downloaded {arguments.output}")
        print(f"candidate SHA-256: {digest}")
        print("AUDITED_SOURCE_ARCHIVE_SHA256 remains unchanged until review")
    elif arguments.command == "convert":
        generation = convert_filegdb(
            arguments.source_archive,
            arguments.raster_name,
            arguments.output_root,
            gdalwarp=arguments.gdalwarp,
            gdalinfo=arguments.gdalinfo,
            proj=arguments.proj,
        )
        manifest = _audit_conversion(generation)
        print(f"wrote reviewed conversion {generation}")
        print(
            ", ".join(
                f"{CLASS_NAMES[index]}={count:,}"
                for index, count in enumerate(manifest.class_counts)
            )
        )
        print(RIGHTS_NOTICE)
    elif arguments.command == "build":
        header = build_from_conversion(
            arguments.conversion, arguments.source_archive, arguments.output
        )
        print(
            f"wrote {arguments.output} with {header.grid.width} by {header.grid.height} "
            f"nearest-neighbor classes"
        )
        print(RIGHTS_NOTICE)
    elif arguments.command == "audit":
        mask = load_mask(arguments.input)
        print(
            f"verified {arguments.input}: {mask.header.grid.width} by "
            f"{mask.header.grid.height}, source {mask.header.source_archive_sha256}"
        )
    else:
        evidence = source_candidate(
            arguments.source_archive,
            arguments.gdb_root,
            arguments.raster_name,
            gdalwarp=arguments.gdalwarp,
            gdalinfo=arguments.gdalinfo,
            proj=arguments.proj,
        )
        print(json.dumps(evidence, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
