from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
import zipfile
from collections.abc import Iterator
from pathlib import Path
from typing import TypedDict
from unittest.mock import patch

import httpx
import numpy as np

import isophilly_ingest.land_cover as land_cover
from isophilly_ingest.land_cover import (
    ARCHIVE_BYTES,
    AUDITED_RASTER_EVIDENCE,
    AUDITED_TOOLCHAIN,
    SOURCE_IDENTITY_SHA256,
    ArchiveTransferSpec,
    EnviHeader,
    GridSpec,
    LandCoverClass,
    LandCoverError,
    RasterEvidence,
    ToolchainEvidence,
    TransientLandCoverTransferError,
    _fetch_archive,
    _header_json,
    _parse_envi_header,
    _parse_header,
    _replace_and_fsync,
    _source_dataset_uri,
    _unlink_and_fsync,
    build_from_conversion,
    canonical_sha256,
    convert_filegdb,
    effective_class,
    load_grid,
    load_mask,
    source_candidate,
    source_identity,
    write_mask,
)

SOURCE_SHA256 = "12" * 32


class InterruptingStream(httpx.SyncByteStream):
    def __init__(self, request: httpx.Request, prefix: bytes) -> None:
        self.request = request
        self.prefix = prefix

    def __iter__(self) -> Iterator[bytes]:
        yield self.prefix
        raise httpx.ReadTimeout("synthetic timeout", request=self.request)


class ConversionPins(TypedDict):
    audited_gdb_root: str
    audited_raster_name: str
    audited_source_evidence: RasterEvidence
    audited_toolchain: ToolchainEvidence


class LandCoverArchiveFetchTests(unittest.TestCase):
    def setUp(self) -> None:
        self.payload = b"abcdef"
        self.spec = ArchiveTransferSpec(
            "https://example.test/PhiladelphiaLandCoverRaster2018.zip",
            len(self.payload),
            '"reviewed-etag"',
        )

    def _write_checkpoint(self, destination: Path, downloaded_bytes: int) -> Path:
        checkpoint = destination.with_suffix(".zip.download.json")
        checkpoint.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "url": self.spec.url,
                    "expected_bytes": self.spec.expected_bytes,
                    "downloaded_bytes": downloaded_bytes,
                    "etag": self.spec.etag,
                },
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n"
        )
        return checkpoint

    def test_timeout_resumes_with_strong_if_range_and_promotes_atomically(self) -> None:
        requests: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            requests.append(request)
            if len(requests) == 1:
                return httpx.Response(
                    200,
                    request=request,
                    headers={
                        "ETag": self.spec.etag,
                        "Content-Length": str(len(self.payload)),
                    },
                    stream=InterruptingStream(request, self.payload[:3]),
                )
            self.assertEqual(request.headers["Range"], "bytes=3-")
            self.assertEqual(request.headers["If-Range"], self.spec.etag)
            return httpx.Response(
                206,
                request=request,
                headers={
                    "ETag": self.spec.etag,
                    "Content-Length": "3",
                    "Content-Range": "bytes 3-5/6",
                },
                stream=httpx.ByteStream(self.payload[3:]),
            )

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            delays: list[float] = []
            with httpx.Client(transport=httpx.MockTransport(handler)) as client:
                digest = _fetch_archive(destination, self.spec, client=client, sleep=delays.append)
            self.assertEqual(digest, hashlib.sha256(self.payload).hexdigest())
            self.assertEqual(destination.read_bytes(), self.payload)
            self.assertEqual(delays, [2.0])
            self.assertFalse(destination.with_suffix(".zip.part").exists())
            self.assertFalse(destination.with_suffix(".zip.download.json").exists())

    def test_existing_cache_enforces_optional_audited_digest_without_mutation(self) -> None:
        expected_digest = hashlib.sha256(self.payload).hexdigest()

        def unexpected_request(request: httpx.Request) -> httpx.Response:
            self.fail(f"existing cache made an unexpected request: {request.url}")

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            destination.write_bytes(self.payload)
            with httpx.Client(transport=httpx.MockTransport(unexpected_request)) as client:
                self.assertEqual(
                    _fetch_archive(
                        destination,
                        self.spec,
                        client=client,
                        audited_sha256=expected_digest,
                    ),
                    expected_digest,
                )
                with self.assertRaisesRegex(LandCoverError, "audited SHA-256"):
                    _fetch_archive(
                        destination,
                        self.spec,
                        client=client,
                        audited_sha256="00" * 32,
                    )
            self.assertEqual(destination.read_bytes(), self.payload)

    def test_pinned_digest_mismatch_never_promotes_full_partial(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                200,
                request=request,
                headers={"ETag": self.spec.etag, "Content-Length": "6"},
                stream=httpx.ByteStream(self.payload),
            )

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            with (
                httpx.Client(transport=httpx.MockTransport(handler)) as client,
                self.assertRaisesRegex(LandCoverError, "audited SHA-256"),
            ):
                _fetch_archive(
                    destination,
                    self.spec,
                    client=client,
                    audited_sha256="00" * 32,
                )
            self.assertFalse(destination.exists())
            self.assertEqual(destination.with_suffix(".zip.part").read_bytes(), self.payload)

    def test_retry_exhaustion_keeps_transfer_pending(self) -> None:
        calls = 0

        def handler(request: httpx.Request) -> httpx.Response:
            nonlocal calls
            calls += 1
            raise httpx.ConnectTimeout("synthetic timeout", request=request)

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            delays: list[float] = []
            with (
                httpx.Client(transport=httpx.MockTransport(handler)) as client,
                self.assertRaisesRegex(TransientLandCoverTransferError, "exhausted 3 attempts"),
            ):
                _fetch_archive(
                    destination,
                    self.spec,
                    client=client,
                    sleep=delays.append,
                    max_attempts=3,
                )
            self.assertEqual(calls, 3)
            self.assertEqual(delays, [2.0, 4.0])
            self.assertFalse(destination.exists())

    def test_restart_promotes_a_fully_checkpointed_partial_without_network(self) -> None:
        def interrupt_after_full_body(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                200,
                request=request,
                headers={"ETag": self.spec.etag, "Content-Length": "6"},
                stream=InterruptingStream(request, self.payload),
            )

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            with (
                httpx.Client(transport=httpx.MockTransport(interrupt_after_full_body)) as client,
                self.assertRaises(TransientLandCoverTransferError),
            ):
                _fetch_archive(destination, self.spec, client=client, max_attempts=1)

            def unexpected_request(request: httpx.Request) -> httpx.Response:
                self.fail(f"completed checkpoint made an unexpected request: {request.url}")

            with httpx.Client(transport=httpx.MockTransport(unexpected_request)) as client:
                digest = _fetch_archive(destination, self.spec, client=client)
            self.assertEqual(destination.read_bytes(), self.payload)
            self.assertEqual(digest, hashlib.sha256(self.payload).hexdigest())

    def test_invalid_resumed_range_is_not_retried_or_appended(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"

            def interrupt(request: httpx.Request) -> httpx.Response:
                return httpx.Response(
                    200,
                    request=request,
                    headers={"ETag": self.spec.etag, "Content-Length": "6"},
                    stream=InterruptingStream(request, self.payload[:3]),
                )

            with (
                httpx.Client(transport=httpx.MockTransport(interrupt)) as client,
                self.assertRaises(TransientLandCoverTransferError),
            ):
                _fetch_archive(destination, self.spec, client=client, max_attempts=1)

            calls = 0

            def bad_range(request: httpx.Request) -> httpx.Response:
                nonlocal calls
                calls += 1
                return httpx.Response(
                    206,
                    request=request,
                    headers={
                        "ETag": self.spec.etag,
                        "Content-Length": "3",
                        "Content-Range": "bytes 2-4/6",
                    },
                    content=b"xyz",
                )

            with (
                httpx.Client(transport=httpx.MockTransport(bad_range)) as client,
                self.assertRaisesRegex(LandCoverError, "resumed response is invalid"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertEqual(calls, 1)
            self.assertFalse(destination.exists())
            self.assertFalse(destination.with_suffix(".zip.part").exists())

    def test_weak_or_changed_etag_never_publishes(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                200,
                request=request,
                headers={"ETag": 'W/"reviewed-etag"', "Content-Length": "6"},
                content=self.payload,
            )

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            with (
                httpx.Client(transport=httpx.MockTransport(handler)) as client,
                self.assertRaisesRegex(LandCoverError, "ETag"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertFalse(destination.exists())

    def test_redirect_status_fails_without_retry(self) -> None:
        calls = 0

        def handler(request: httpx.Request) -> httpx.Response:
            nonlocal calls
            calls += 1
            return httpx.Response(
                302,
                request=request,
                headers={"Location": "https://other.test/archive.zip"},
            )

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            with (
                httpx.Client(transport=httpx.MockTransport(handler)) as client,
                self.assertRaisesRegex(LandCoverError, "redirected"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertEqual(calls, 1)

    def test_symlink_transfer_artifact_is_rejected_without_touching_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.write_bytes(b"do not touch")
            destination = root / "archive.zip"
            destination.symlink_to(target)
            with (
                httpx.Client(transport=httpx.MockTransport(lambda request: self.fail())) as client,
                self.assertRaisesRegex(LandCoverError, "not a regular file"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertEqual(target.read_bytes(), b"do not touch")
            self.assertTrue(destination.is_symlink())

    def test_existing_cache_inode_swap_during_hash_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "archive.zip"
            destination.write_bytes(self.payload)
            held = root / "held-cache"
            victim = root / "victim"
            victim.write_bytes(b"unchanged")
            original_sha256_fd = land_cover._sha256_fd

            def swap_after_open(descriptor: int) -> str:
                destination.replace(held)
                destination.symlink_to(victim)
                return original_sha256_fd(descriptor)

            with (
                patch("isophilly_ingest.land_cover._sha256_fd", swap_after_open),
                httpx.Client(transport=httpx.MockTransport(lambda request: self.fail())) as client,
                self.assertRaisesRegex(LandCoverError, "destination changed during use"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertEqual(victim.read_bytes(), b"unchanged")
            self.assertEqual(held.read_bytes(), self.payload)

    def test_full_partial_inode_swap_before_promotion_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "archive.zip"
            partial = destination.with_suffix(".zip.part")
            partial.write_bytes(self.payload)
            checkpoint = self._write_checkpoint(destination, len(self.payload))
            held = root / "held-partial"
            victim = root / "victim"
            victim.write_bytes(b"unchanged")
            original_sha256_fd = land_cover._sha256_fd

            def swap_after_open(descriptor: int) -> str:
                partial.replace(held)
                partial.symlink_to(victim)
                return original_sha256_fd(descriptor)

            with (
                patch("isophilly_ingest.land_cover._sha256_fd", swap_after_open),
                httpx.Client(transport=httpx.MockTransport(lambda request: self.fail())) as client,
                self.assertRaisesRegex(LandCoverError, "partial changed during use"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertFalse(destination.exists())
            self.assertEqual(victim.read_bytes(), b"unchanged")
            self.assertEqual(held.read_bytes(), self.payload)
            self.assertTrue(checkpoint.exists())

    def test_checkpoint_inode_swap_during_read_preserves_partial(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "archive.zip"
            partial = destination.with_suffix(".zip.part")
            partial.write_bytes(self.payload[:3])
            checkpoint = self._write_checkpoint(destination, 3)
            held = root / "held-checkpoint"
            victim = root / "victim"
            victim.write_bytes(b"unchanged")
            original_read_fd = land_cover._read_fd

            def swap_after_open(descriptor: int, maximum_bytes: int) -> bytes:
                checkpoint.replace(held)
                checkpoint.symlink_to(victim)
                return original_read_fd(descriptor, maximum_bytes)

            with (
                patch("isophilly_ingest.land_cover._read_fd", swap_after_open),
                httpx.Client(transport=httpx.MockTransport(lambda request: self.fail())) as client,
                self.assertRaisesRegex(LandCoverError, "checkpoint changed during use"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertEqual(partial.read_bytes(), self.payload[:3])
            self.assertEqual(victim.read_bytes(), b"unchanged")
            self.assertTrue(held.exists())

    def test_stale_lock_requires_explicit_manual_verification_and_removal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "archive.zip"
            lock = destination.with_suffix(".zip.download.lock")
            lock.write_text('{"host":"old","pid":123,"schema_version":1}\n')
            with (
                httpx.Client(transport=httpx.MockTransport(lambda request: self.fail())) as client,
                self.assertRaisesRegex(LandCoverError, "verify its recorded PID and host"),
            ):
                _fetch_archive(destination, self.spec, client=client)
            self.assertTrue(lock.exists())

    def test_replace_and_unlink_fsync_the_parent_after_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            destination = root / "destination"
            source.write_bytes(b"payload")
            events: list[str] = []
            original_replace = Path.replace
            original_unlink = Path.unlink

            def replace(path: Path, target: Path) -> Path:
                events.append("replace")
                return original_replace(path, target)

            def unlink(path: Path) -> None:
                events.append("unlink")
                original_unlink(path)

            with (
                patch.object(Path, "replace", replace),
                patch.object(Path, "unlink", unlink),
                patch(
                    "isophilly_ingest.land_cover._fsync_directory",
                    side_effect=lambda parent: events.append(f"fsync:{parent.name}"),
                ),
            ):
                _replace_and_fsync(source, destination)
                _unlink_and_fsync(destination)
            self.assertEqual(
                events, ["replace", f"fsync:{root.name}", "unlink", f"fsync:{root.name}"]
            )


class LandCoverSourceTests(unittest.TestCase):
    def test_reviewed_typed_evidence_pins_are_complete(self) -> None:
        self.assertIsInstance(AUDITED_RASTER_EVIDENCE, RasterEvidence)
        self.assertIsInstance(AUDITED_TOOLCHAIN, ToolchainEvidence)
        assert AUDITED_RASTER_EVIDENCE is not None
        assert AUDITED_TOOLCHAIN is not None
        self.assertEqual(AUDITED_RASTER_EVIDENCE.driver, "OpenFileGDB")
        self.assertEqual(AUDITED_RASTER_EVIDENCE.data_type, "Byte")
        self.assertEqual(AUDITED_TOOLCHAIN.gdal_version, "3.12.4")

    def test_reviewed_source_identity_digest_is_stable(self) -> None:
        self.assertEqual(canonical_sha256(source_identity()), SOURCE_IDENTITY_SHA256)

    def test_grid_parser_requires_nearest_neighbor_classes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "grid.json"
            path.write_text(
                json.dumps(
                    {
                        "epsg": 32129,
                        "width": 2,
                        "height": 2,
                        "min_x": 0,
                        "min_y": 0,
                        "max_x": 2,
                        "max_y": 2,
                        "row_order": "north_to_south",
                        "resampling": "bilinear",
                    }
                )
            )

            with self.assertRaisesRegex(LandCoverError, "nearest-neighbor"):
                load_grid(path)

    def test_grid_parser_rejects_boolean_and_fractional_dimensions(self) -> None:
        values = {
            "epsg": 32129,
            "width": True,
            "height": 2.5,
            "min_x": 0,
            "min_y": 0,
            "max_x": 2,
            "max_y": 2,
            "row_order": "north_to_south",
            "resampling": "nearest",
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "grid.json"
            path.write_text(json.dumps(values))

            with self.assertRaisesRegex(LandCoverError, "JSON integer"):
                load_grid(path)

    def test_header_parser_rejects_coercible_types_and_nested_extra_keys(self) -> None:
        grid = GridSpec(32129, 1, 1, 0.0, 0.0, 1.0, 1.0)
        classes = np.array([[1]], dtype=np.uint8)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mask.isomask"
            header = write_mask(
                path,
                classes,
                grid,
                source_archive_sha256=SOURCE_SHA256,
                source_archive_bytes=ARCHIVE_BYTES,
                audited_source_sha256=SOURCE_SHA256,
            )
            value = json.loads(_header_json(header))
            value["dataset_id"] = True
            with self.assertRaisesRegex(LandCoverError, "JSON integer"):
                _parse_header(value)
            value = json.loads(_header_json(header))
            value["grid"]["extra"] = 1
            with self.assertRaisesRegex(LandCoverError, "grid JSON"):
                _parse_header(value)


class LandCoverMaskTests(unittest.TestCase):
    def setUp(self) -> None:
        self.grid = GridSpec(
            epsg=32129,
            width=3,
            height=2,
            min_x=10.0,
            min_y=20.0,
            max_x=13.0,
            max_y=22.0,
        )
        self.classes = np.array([[1, 2, 3], [4, 0, 7]], dtype=np.uint8)

    def test_round_trip_samples_nearest_class_in_north_to_south_rows(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mask.isomask"
            write_mask(
                path,
                self.classes,
                self.grid,
                source_archive_sha256=SOURCE_SHA256,
                source_archive_bytes=ARCHIVE_BYTES,
                audited_source_sha256=SOURCE_SHA256,
            )

            with self.assertRaisesRegex(LandCoverError, "reviewed production grid"):
                load_mask(path, audited_source_sha256=SOURCE_SHA256)
            mask = load_mask(
                path,
                audited_source_sha256=SOURCE_SHA256,
                expected_grid=self.grid,
            )

            self.assertEqual(mask.sample(10.1, 21.9), LandCoverClass.TREE_CANOPY)
            self.assertEqual(mask.sample(11.9, 21.1), LandCoverClass.GRASS_SHRUB)
            self.assertEqual(mask.sample(10.1, 20.1), LandCoverClass.WATER)
            self.assertIsNone(mask.sample(11.1, 20.1))
            self.assertEqual(mask.sample(13.0, 21.9), LandCoverClass.BARE_EARTH)
            self.assertEqual(mask.sample(10.0, 20.0), LandCoverClass.WATER)
            self.assertIsNone(mask.sample(13.01, 21.0))

    def test_rejects_unknown_class_without_publishing_partial_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mask.isomask"
            invalid = self.classes.copy()
            invalid[0, 0] = 8

            with self.assertRaisesRegex(LandCoverError, "outside 0 through 7"):
                write_mask(
                    path,
                    invalid,
                    self.grid,
                    source_archive_sha256=SOURCE_SHA256,
                    source_archive_bytes=ARCHIVE_BYTES,
                    audited_source_sha256=SOURCE_SHA256,
                )

            self.assertFalse(path.exists())
            self.assertFalse(any(path.parent.glob(f".{path.name}.*.part")))
            self.assertFalse(path.with_name(f".{path.name}.lock").exists())

    def test_existing_writer_lock_preserves_current_mask(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mask.isomask"
            path.write_bytes(b"old")
            path.with_name(f".{path.name}.lock").write_text("busy\n")

            with self.assertRaisesRegex(LandCoverError, "already running"):
                write_mask(
                    path,
                    self.classes,
                    self.grid,
                    source_archive_sha256=SOURCE_SHA256,
                    source_archive_bytes=ARCHIVE_BYTES,
                    audited_source_sha256=SOURCE_SHA256,
                )

            self.assertEqual(path.read_bytes(), b"old")

    def test_failed_post_write_audit_preserves_current_mask(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mask.isomask"
            path.write_bytes(b"old")
            with (
                patch(
                    "isophilly_ingest.land_cover.load_mask",
                    side_effect=LandCoverError("synthetic post-write failure"),
                ),
                self.assertRaisesRegex(LandCoverError, "synthetic post-write failure"),
            ):
                write_mask(
                    path,
                    self.classes,
                    self.grid,
                    source_archive_sha256=SOURCE_SHA256,
                    source_archive_bytes=ARCHIVE_BYTES,
                    audited_source_sha256=SOURCE_SHA256,
                )
            self.assertEqual(path.read_bytes(), b"old")
            self.assertFalse(any(path.parent.glob(f".{path.name}.*.part")))
            self.assertFalse(path.with_name(f".{path.name}.lock").exists())

    def test_payload_digest_detects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mask.isomask"
            write_mask(
                path,
                self.classes,
                self.grid,
                source_archive_sha256=SOURCE_SHA256,
                source_archive_bytes=ARCHIVE_BYTES,
                audited_source_sha256=SOURCE_SHA256,
            )
            with path.open("r+b") as artifact:
                artifact.seek(-1, 2)
                artifact.write(b"\x06")

            with self.assertRaisesRegex(LandCoverError, "payload SHA-256"):
                load_mask(
                    path,
                    audited_source_sha256=SOURCE_SHA256,
                    expected_grid=self.grid,
                )

    def test_normal_writer_fails_closed_until_archive_digest_is_audited(self) -> None:
        with (
            tempfile.TemporaryDirectory() as directory,
            self.assertRaisesRegex(LandCoverError, "not audited"),
        ):
            write_mask(
                Path(directory) / "mask.isomask",
                self.classes,
                self.grid,
                source_archive_sha256=SOURCE_SHA256,
                source_archive_bytes=ARCHIVE_BYTES,
                audited_source_sha256=None,
            )

    def test_hydrology_has_precedence_over_land_cover(self) -> None:
        self.assertEqual(
            effective_class(
                LandCoverClass.GRASS_SHRUB,
                hydrology_contains_point=True,
            ),
            LandCoverClass.WATER,
        )
        self.assertEqual(
            effective_class(
                LandCoverClass.TREE_CANOPY,
                hydrology_contains_point=False,
            ),
            LandCoverClass.TREE_CANOPY,
        )
        self.assertEqual(
            effective_class(None, hydrology_contains_point=True),
            LandCoverClass.WATER,
        )


class LandCoverConversionTests(unittest.TestCase):
    def test_gdal_release_codename_does_not_change_numeric_version(self) -> None:
        self.assertEqual(
            land_cover._gdal_version('GDAL 3.12.4 "Chicoutimi", released 2026/04/22', "gdalinfo"),
            "3.12.4",
        )

    def setUp(self) -> None:
        self.grid = GridSpec(
            epsg=32129,
            width=2,
            height=3,
            min_x=0.0,
            min_y=0.0,
            max_x=2.0,
            max_y=3.0,
        )

    def _archive(self, root: Path) -> tuple[Path, str]:
        path = root / "source.zip"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("PPR_LandCover_2018.gdb/gdb", b"catalog")
            archive.writestr("PPR_LandCover_2018.gdb/a00000001.gdbtable", b"cells")
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_source_dataset_is_constructed_from_simple_pinned_names(self) -> None:
        archive = Path("/tmp/source.zip")
        self.assertEqual(
            _source_dataset_uri(archive, "PPR_LandCover_2018.gdb", "landcover"),
            'OpenFileGDB:"/vsizip/{/tmp/source.zip}/PPR_LandCover_2018.gdb":landcover',
        )
        with self.assertRaisesRegex(LandCoverError, "raster name"):
            _source_dataset_uri(
                archive,
                "PPR_LandCover_2018.gdb",
                "/vsizip/source.zip/PPR_LandCover_2018.gdb:landcover",
            )
        for invalid in (
            "space name",
            "back\\slash",
            "control\nname",
            "punctuation!",
            "é",
            "..",
        ):
            with self.subTest(invalid=invalid), self.assertRaises(LandCoverError):
                _source_dataset_uri(archive, "PPR_LandCover_2018.gdb", invalid)
        for invalid_root in ("folder/PPR.gdb", "PPR GDB.gdb", "PPR\\GDB.gdb", "PPR"):
            with self.subTest(invalid_root=invalid_root), self.assertRaises(LandCoverError):
                _source_dataset_uri(archive, invalid_root, "landcover")

    def test_source_candidate_emits_every_future_pin_as_canonical_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive, digest = self._archive(Path(directory))
            with patch("isophilly_ingest.land_cover._run", self._runner()):
                evidence = source_candidate(
                    archive,
                    "PPR_LandCover_2018.gdb",
                    "landcover",
                    expected_archive_bytes=archive.stat().st_size,
                )
            self.assertEqual(evidence["audited_source_archive_sha256"], digest)
            self.assertEqual(evidence["audited_gdb_root"], "PPR_LandCover_2018.gdb")
            self.assertEqual(evidence["audited_raster_name"], "landcover")
            self.assertEqual(
                set(evidence),
                {
                    "archive_members",
                    "archive_members_sha256",
                    "audited_gdb_root",
                    "audited_raster_evidence",
                    "audited_raster_name",
                    "audited_source_archive_sha256",
                    "audited_toolchain",
                    "source_archive_bytes",
                    "source_identity_sha256",
                },
            )
            canonical = json.dumps(evidence, sort_keys=True, separators=(",", ":"))
            self.assertEqual(
                json.dumps(json.loads(canonical), sort_keys=True, separators=(",", ":")),
                canonical,
            )

    def test_build_acquires_output_lock_before_reading_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "mask.isomask"
            output.with_name(f".{output.name}.lock").write_text("busy\n")
            with (
                patch(
                    "isophilly_ingest.land_cover.AUDITED_SOURCE_ARCHIVE_SHA256",
                    SOURCE_SHA256,
                ),
                self.assertRaisesRegex(LandCoverError, "already running"),
            ):
                build_from_conversion(root / "missing-conversion", root / "missing.zip", output)

    def _toolchain(self) -> ToolchainEvidence:
        values = {
            "gdalinfo_version": "GDAL 3.12.4, released",
            "gdalinfo_build": "GDAL build details",
            "formats": "OpenFileGDB -raster,vector- (ro): ESRI FileGDB",
            "gdalinfo_help": "gdalinfo help",
            "gdalwarp_version": "GDAL 3.12.4, released",
            "gdalwarp_build": "GDAL build details",
            "gdalwarp_help": "gdalwarp help",
            "proj": "Rel. 9.6.2, March 1st, 2025",
        }
        return ToolchainEvidence(
            gdal_version="3.12.4",
            gdalinfo_version_sha256=canonical_sha256(values["gdalinfo_version"]),
            gdalinfo_build_sha256=canonical_sha256(values["gdalinfo_build"]),
            gdalinfo_formats_sha256=canonical_sha256(values["formats"]),
            gdalinfo_help_sha256=canonical_sha256(values["gdalinfo_help"]),
            gdalwarp_version_sha256=canonical_sha256(values["gdalwarp_version"]),
            gdalwarp_build_sha256=canonical_sha256(values["gdalwarp_build"]),
            gdalwarp_help_sha256=canonical_sha256(values["gdalwarp_help"]),
            proj_version="9.6.2",
            proj_version_sha256=canonical_sha256(values["proj"]),
        )

    def _source_evidence(self) -> RasterEvidence:
        source_min_x, source_min_y, source_max_x, source_max_y = (
            2_645_347.999999997,
            186_454.00000000006,
            2_753_588.0000000014,
            307_894.0000000009,
        )
        description = 'OpenFileGDB:"/vsizip/{{archive}}/PPR_LandCover_2018.gdb":landcover'
        return RasterEvidence(
            driver="OpenFileGDB",
            description=description,
            files=("/vsizip/{{archive}}/PPR_LandCover_2018.gdb/a00000001.gdbtable",),
            width=2,
            height=2,
            data_type="Byte",
            nodata=0,
            geotransform=(
                source_min_x,
                (source_max_x - source_min_x) / 2,
                0.0,
                source_max_y,
                0.0,
                -(source_max_y - source_min_y) / 2,
            ),
            crs_wkt_sha256=hashlib.sha256(b"NAD83 North_American_1983 Foot_US").hexdigest(),
        )

    def _runner(self, *, fail_warp: bool = False, pixels: bytes = bytes((1, 2, 3, 4, 5, 7))):
        source_min_x, source_min_y, source_max_x, source_max_y = (
            2_645_347.999999997,
            186_454.00000000006,
            2_753_588.0000000014,
            307_894.0000000009,
        )
        source_info: dict[str, object] = {
            "driverShortName": "OpenFileGDB",
            "size": [2, 2],
            "geoTransform": [
                source_min_x,
                (source_max_x - source_min_x) / 2,
                0,
                source_max_y,
                0,
                -(source_max_y - source_min_y) / 2,
            ],
            "coordinateSystem": {"wkt": "NAD83 North_American_1983 Foot_US"},
            "bands": [{"type": "Byte", "noDataValue": 0}],
        }
        target_info: dict[str, object] = {
            "driverShortName": "ENVI",
            "size": [2, 3],
            "geoTransform": [0, 1, 0, 3, 0, -1],
            "coordinateSystem": {"wkt": "EPSG:32129"},
            "bands": [{"type": "Byte", "noDataValue": 0}],
        }

        def run(command: list[str]) -> subprocess.CompletedProcess[str]:
            if command[0] in {"gdalinfo", "gdalwarp"} and command[1:] == ["--version"]:
                return subprocess.CompletedProcess(command, 0, "GDAL 3.12.4, released\n", "")
            if command == ["gdalinfo", "--formats"]:
                return subprocess.CompletedProcess(
                    command, 0, "OpenFileGDB -raster,vector- (ro): ESRI FileGDB\n", ""
                )
            if command == ["gdalinfo", "--help-general"]:
                return subprocess.CompletedProcess(command, 0, "gdalinfo help\n", "")
            if command == ["gdalwarp", "--help-general"]:
                return subprocess.CompletedProcess(command, 0, "gdalwarp help\n", "")
            if command in (["gdalinfo", "--build"], ["gdalwarp", "--build"]):
                return subprocess.CompletedProcess(command, 0, "GDAL build details\n", "")
            if command == ["proj", "--version"]:
                return subprocess.CompletedProcess(command, 0, "Rel. 9.6.2, March 1st, 2025\n", "")
            if command[0] == "gdalinfo":
                target = command[-1].endswith("classes.dat")
                info = dict(target_info if target else source_info)
                info["description"] = command[-1]
                if target:
                    info["files"] = [command[-1], str(Path(command[-1]).with_suffix(".hdr"))]
                else:
                    archive = command[-1].split("{/", maxsplit=1)[1].split("}", maxsplit=1)[0]
                    info["files"] = [
                        f"/vsizip/{{/{archive}}}/PPR_LandCover_2018.gdb/a00000001.gdbtable"
                    ]
                return subprocess.CompletedProcess(command, 0, json.dumps(info), "")
            if fail_warp:
                raise LandCoverError("synthetic warp failure")
            self.assertEqual(command[command.index("-r") + 1], "near")
            self.assertEqual(command[command.index("-wo") + 1], "NUM_THREADS=1")
            output = Path(command[-1])
            output.write_bytes(pixels)
            output.with_suffix(".hdr").write_text(
                "ENVI\nsamples = 2\nlines = 3\nbands = 1\nheader offset = 0\n"
                "data type = 1\ninterleave = bsq\nbyte order = 0\n"
            )
            return subprocess.CompletedProcess(command, 0, "", "")

        return run

    def _pins(self) -> ConversionPins:
        return {
            "audited_gdb_root": "PPR_LandCover_2018.gdb",
            "audited_raster_name": "landcover",
            "audited_source_evidence": self._source_evidence(),
            "audited_toolchain": self._toolchain(),
        }

    def test_envi_header_binds_orientation_and_storage(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            header = Path(directory) / "classes.hdr"
            header.write_text(
                "ENVI\nsamples = 2\nlines = 3\nbands = 1\nheader offset = 0\n"
                "data type = 1\ninterleave = bsq\nbyte order = 0\n"
            )
            self.assertEqual(
                _parse_envi_header(header, self.grid),
                EnviHeader(2, 3, 1, 0, 1, "bsq", 0),
            )
            header.write_text(
                "ENVI\nsamples = 2\nlines = 3\nbands = 1\nheader offset = 0\n"
                "data type = 1\ninterleave = bil\nbyte order = 1\n"
            )
            with self.assertRaisesRegex(LandCoverError, "storage"):
                _parse_envi_header(header, self.grid)

    def test_conversion_is_bounded_atomic_and_records_review_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, digest = self._archive(root)
            output = root / "converted"
            with patch("isophilly_ingest.land_cover._run", self._runner()):
                generation = convert_filegdb(
                    archive,
                    "landcover",
                    output,
                    audited_source_sha256=digest,
                    **self._pins(),
                    target_grid=self.grid,
                    expected_archive_bytes=archive.stat().st_size,
                )

            classes = np.load(generation / "classes.npy", allow_pickle=False)
            manifest = json.loads((generation / "conversion.json").read_text())
            self.assertEqual(classes.tolist(), [[1, 2], [3, 4], [5, 7]])
            self.assertEqual(manifest["class_counts"], [0, 1, 1, 1, 1, 1, 0, 1])
            self.assertEqual(manifest["source_archive_sha256"], digest)
            self.assertEqual(len(manifest["archive_members"]), 2)
            self.assertEqual(manifest["resampling"], "nearest")
            self.assertEqual(
                json.loads((output / "current.json").read_text())["generation"], generation.name
            )
            self.assertFalse(any(output.glob(".staging-*")))

    def test_failed_refresh_preserves_current_generation_and_cleans_lock(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, digest = self._archive(root)
            output = root / "converted"
            with patch("isophilly_ingest.land_cover._run", self._runner()):
                convert_filegdb(
                    archive,
                    "landcover",
                    output,
                    audited_source_sha256=digest,
                    **self._pins(),
                    target_grid=self.grid,
                    expected_archive_bytes=archive.stat().st_size,
                )
            before = (output / "current.json").read_bytes()

            with (
                patch("isophilly_ingest.land_cover._run", self._runner(fail_warp=True)),
                self.assertRaisesRegex(LandCoverError, "synthetic warp failure"),
            ):
                convert_filegdb(
                    archive,
                    "landcover",
                    output,
                    audited_source_sha256=digest,
                    **self._pins(),
                    target_grid=self.grid,
                    expected_archive_bytes=archive.stat().st_size,
                )

            self.assertEqual((output / "current.json").read_bytes(), before)
            self.assertFalse((output / ".convert.lock").exists())
            self.assertFalse(any(output.glob(".staging-*")))

    def test_existing_conversion_lock_fails_without_touching_current(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, digest = self._archive(root)
            output = root / "converted"
            output.mkdir()
            (output / "current.json").write_text('{"generation":"old","schema_version":1}\n')
            (output / ".convert.lock").write_text("busy\n")

            with (
                patch("isophilly_ingest.land_cover._run", self._runner()),
                self.assertRaisesRegex(LandCoverError, "already running"),
            ):
                convert_filegdb(
                    archive,
                    "landcover",
                    output,
                    audited_source_sha256=digest,
                    **self._pins(),
                    target_grid=self.grid,
                    expected_archive_bytes=archive.stat().st_size,
                )

            self.assertEqual(json.loads((output / "current.json").read_text())["generation"], "old")

    def test_invalid_converted_class_is_not_published(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, digest = self._archive(root)
            output = root / "converted"

            with (
                patch(
                    "isophilly_ingest.land_cover._run",
                    self._runner(pixels=bytes((1, 2, 8, 4, 5, 7))),
                ),
                self.assertRaisesRegex(LandCoverError, "invalid classes"),
            ):
                convert_filegdb(
                    archive,
                    "landcover",
                    output,
                    audited_source_sha256=digest,
                    **self._pins(),
                    target_grid=self.grid,
                    expected_archive_bytes=archive.stat().st_size,
                )

            self.assertFalse((output / "current.json").exists())
            self.assertFalse((output / ".convert.lock").exists())
            self.assertFalse(any(output.glob(".staging-*")))


if __name__ == "__main__":
    unittest.main()
