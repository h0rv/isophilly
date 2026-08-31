from __future__ import annotations

import hashlib
import json
import unittest
from contextlib import redirect_stderr
from dataclasses import asdict
from io import BytesIO, StringIO
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import httpx

from isophilly_ingest.coastal_obliques import (
    AUDITED_FRAME_BYTES,
    AUDITED_FRAME_MANIFEST_SHA256,
    AUDITED_LISTING_SHA256,
    COLLECTION_URLS,
    CoastalObliqueError,
    Frame,
    Inventory,
    _parser,
    _validate_jpeg_file,
    contact_sheet,
    create_inventory,
    download_frame,
    frame_manifest_sha256,
    inventory_dict,
    load_inventory,
    parse_inventory,
    parse_listing,
    read_jpeg_dimensions,
    sfm_handoff,
)


def jpeg_bytes(width: int = 1200, height: int = 800) -> bytes:
    return (
        b"\xff\xd8"
        b"\xff\xe0\x00\x04xx"
        b"\xff\xc0\x00\x11\x08"
        + height.to_bytes(2, "big")
        + width.to_bytes(2, "big")
        + b"\x03\x01\x11\x00\x02\x11\x00\x03\x11\x00\xff\xd9"
    )


def inventory_for(frame: Frame) -> Inventory:
    return Inventory(
        1,
        "schuylkill-2014",
        "https://example.test/JPEG/",
        "a" * 64,
        "now",
        (frame,),
    )


class ListingTests(unittest.TestCase):
    def test_parses_encoded_pasda_jpeg_listing(self) -> None:
        listing = (
            b'20 <A HREF="/JPEG/Schuylkill%20%20%20002.jpg">Schuylkill   002.jpg</A>'
            b'10 <A HREF="/JPEG/Schuylkill%20%20%20001.jpg">Schuylkill   001.jpg</A>'
        )

        frames = parse_listing(listing, "https://example.test/JPEG/")

        self.assertEqual(
            [frame.name for frame in frames], ["Schuylkill   001.jpg", "Schuylkill   002.jpg"]
        )
        self.assertEqual(frames[0].bytes, 10)
        self.assertEqual(frames[0].url, "https://example.test/JPEG/Schuylkill%20%20%20001.jpg")

    def test_ignores_non_jpeg_deliveries(self) -> None:
        listing = b'100 <A HREF="frame.tif">frame.tif</A> 100 <A HREF="frame.DNG">frame.DNG</A>'

        with self.assertRaises(CoastalObliqueError):
            parse_listing(listing, "https://example.test/")

    def test_inventory_rejects_url_outside_pinned_jpeg_directory(self) -> None:
        frames = [
            {
                "name": f"Schuylkill   {index:03}.jpg",
                "url": f"https://evil.test/{index}.jpg",
                "bytes": 10,
            }
            for index in range(1, 192)
        ]
        value = {
            "schema_version": 1,
            "collection": "schuylkill-2014",
            "source_url": (
                "https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/"
                "DelEstCZ/2014/DECZ/Obliques/DEP%20-%20Schuylkill/JPEG/"
            ),
            "listing_sha256": "a" * 64,
            "fetched_at": "now",
            "frames": frames,
        }

        with self.assertRaises(CoastalObliqueError):
            parse_inventory(value)

    def test_inventory_rejects_duplicate_or_noncanonical_frame_order(self) -> None:
        collection = "schuylkill-2014"
        source_url = COLLECTION_URLS[collection]
        first: dict[str, object] = {
            "name": "Schuylkill   001.jpg",
            "url": source_url + "Schuylkill%20%20%20001.jpg",
            "bytes": 10,
        }
        second: dict[str, object] = {
            "name": "Schuylkill   002.jpg",
            "url": source_url + "Schuylkill%20%20%20002.jpg",
            "bytes": 20,
        }

        def value(frames: list[dict[str, object]]) -> dict[str, object]:
            return {
                "schema_version": 1,
                "collection": collection,
                "source_url": source_url,
                "listing_sha256": "a" * 64,
                "fetched_at": "now",
                "frames": frames,
            }

        with patch("isophilly_ingest.coastal_obliques.EXPECTED_COUNTS", {collection: 2}):
            with self.assertRaisesRegex(CoastalObliqueError, "duplicate"):
                parse_inventory(value([first, first]))
            with self.assertRaisesRegex(CoastalObliqueError, "canonical order"):
                parse_inventory(value([second, first]))


class JpegTests(unittest.TestCase):
    def test_reads_dimensions_without_imaging_dependency(self) -> None:
        self.assertEqual(read_jpeg_dimensions(BytesIO(jpeg_bytes(6000, 4000))), (6000, 4000))

    def test_rejects_non_jpeg(self) -> None:
        with self.assertRaises(CoastalObliqueError):
            read_jpeg_dimensions(BytesIO(b"not a jpeg"))

    def test_rejects_jpeg_without_final_eoi_before_decoder(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "truncated.jpg"
        path.write_bytes(jpeg_bytes()[:-2])

        with self.assertRaisesRegex(CoastalObliqueError, "EOI"):
            _validate_jpeg_file(path)


class DownloadTests(unittest.TestCase):
    def test_resumes_and_atomically_records_checksum(self) -> None:
        payload = jpeg_bytes()
        source_url = "https://example.test/JPEG/"
        frame = Frame("Schuylkill   001.jpg", source_url + "frame.jpg", len(payload))
        inventory = inventory_for(frame)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        jpeg = root / "schuylkill-2014" / "jpeg"
        jpeg.mkdir(parents=True)
        partial = jpeg / f"{frame.name}.part"
        partial.write_bytes(payload[:8])

        def handler(request: httpx.Request) -> httpx.Response:
            self.assertEqual(request.headers["range"], "bytes=8-")
            return httpx.Response(
                206,
                content=payload[8:],
                headers={"Content-Range": f"bytes 8-{len(payload) - 1}/{len(payload)}"},
                request=request,
            )

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch(
                "isophilly_ingest.coastal_obliques._validate_jpeg_file",
                return_value=(1200, 800),
            ),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            path, digest = download_frame("schuylkill-2014", frame, inventory, client)

        self.assertEqual(path.read_bytes(), payload)
        self.assertFalse(partial.exists())
        self.assertEqual(digest, hashlib.sha256(payload).hexdigest())
        progress = json.loads((root / "schuylkill-2014" / "progress.json").read_text())
        self.assertEqual(progress["frames"][frame.name]["status"], "downloaded")
        self.assertEqual(progress["frames"][frame.name]["width"], 1200)

    def test_recovers_exact_final_without_progress_or_network(self) -> None:
        payload = jpeg_bytes()
        frame = Frame("Schuylkill   001.jpg", "https://example.test/frame.jpg", len(payload))
        inventory = inventory_for(frame)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        final = root / "schuylkill-2014" / "jpeg" / frame.name
        final.parent.mkdir(parents=True)
        final.write_bytes(payload)

        def handler(request: httpx.Request) -> httpx.Response:
            self.fail(f"unexpected network request: {request.url}")

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch(
                "isophilly_ingest.coastal_obliques._validate_jpeg_file",
                return_value=(1200, 800),
            ),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            path, digest = download_frame("schuylkill-2014", frame, inventory, client)

        self.assertEqual(path, final)
        self.assertEqual(digest, hashlib.sha256(payload).hexdigest())
        progress = json.loads((root / "schuylkill-2014" / "progress.json").read_text())
        self.assertEqual(progress["schema_version"], 2)
        self.assertEqual(progress["frames"][frame.name]["sha256"], digest)

    def test_recovers_exact_partial_with_corrupt_progress(self) -> None:
        payload = jpeg_bytes()
        frame = Frame("Schuylkill   001.jpg", "https://example.test/frame.jpg", len(payload))
        inventory = inventory_for(frame)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        partial = root / "schuylkill-2014" / "jpeg" / f"{frame.name}.part"
        partial.parent.mkdir(parents=True)
        partial.write_bytes(payload)
        (root / "schuylkill-2014" / "progress.json").write_text("not json")

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch(
                "isophilly_ingest.coastal_obliques._validate_jpeg_file",
                return_value=(1200, 800),
            ),
            httpx.Client(transport=httpx.MockTransport(lambda request: self.fail())) as client,
        ):
            final, _ = download_frame("schuylkill-2014", frame, inventory, client)

        self.assertTrue(final.exists())
        self.assertFalse(partial.exists())

    def test_inventory_dict_records_listing_hash_urls_and_sizes(self) -> None:
        frame = Frame("Schuylkill   001.jpg", "https://example.test/JPEG/frame.jpg", 42)
        value = inventory_dict(inventory_for(frame))

        self.assertEqual(value["listing_sha256"], "a" * 64)
        self.assertEqual(value["frames"], [{"name": frame.name, "url": frame.url, "bytes": 42}])
        warning = value["rights_warning"]
        self.assertIsInstance(warning, str)
        assert isinstance(warning, str)
        self.assertIn("RIGHTS WARNING", warning)


class InventoryRefreshTests(unittest.TestCase):
    def test_missing_inventory_refuses_orphan_artifacts_without_network(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        collection = "schuylkill-2014"
        jpeg = root / collection / "jpeg"
        jpeg.mkdir(parents=True)
        (jpeg / "Schuylkill   001.jpg.part").write_bytes(b"partial")

        def handler(request: httpx.Request) -> httpx.Response:
            self.fail(f"unexpected network request: {request.url}")

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
            self.assertRaisesRegex(CoastalObliqueError, "without their matching inventory"),
        ):
            create_inventory(collection, client=client)

    def test_clean_plan_rejects_listing_outside_audited_hash(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        collection = "schuylkill-2014"
        listing = b'20 <A HREF="Schuylkill%20%20%20001.jpg">Schuylkill   001.jpg</A>'
        transport = httpx.MockTransport(lambda request: httpx.Response(200, content=listing))

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch(
                "isophilly_ingest.coastal_obliques.EXPECTED_COUNTS",
                {collection: 1},
            ),
            httpx.Client(transport=transport) as client,
            self.assertRaisesRegex(CoastalObliqueError, "listing SHA-256 changed"),
        ):
            create_inventory(collection, client=client)

        self.assertFalse((root / collection / "inventory.json").exists())

    def test_audited_hash_constants_cover_every_collection(self) -> None:
        self.assertEqual(set(AUDITED_LISTING_SHA256), set(COLLECTION_URLS))
        self.assertEqual(set(AUDITED_FRAME_MANIFEST_SHA256), set(COLLECTION_URLS))
        self.assertEqual(set(AUDITED_FRAME_BYTES), set(COLLECTION_URLS))
        self.assertTrue(all(len(digest) == 64 for digest in AUDITED_LISTING_SHA256.values()))
        self.assertTrue(all(len(digest) == 64 for digest in AUDITED_FRAME_MANIFEST_SHA256.values()))

    def test_load_inventory_enforces_ordered_semantic_frame_pin(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        collection = "schuylkill-2014"
        source_url = COLLECTION_URLS[collection]
        frames = (
            Frame("Schuylkill   001.jpg", source_url + "Schuylkill%20%20%20001.jpg", 10),
            Frame("Schuylkill   002.jpg", source_url + "Schuylkill%20%20%20002.jpg", 20),
        )
        inventory = Inventory(1, collection, source_url, "a" * 64, "now", frames)
        path = root / collection / "inventory.json"
        path.parent.mkdir(parents=True)
        value = inventory_dict(inventory)
        value.pop("frame_manifest_sha256")
        path.write_text(json.dumps(value))
        semantic_pin = frame_manifest_sha256(frames)
        patches = (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.EXPECTED_COUNTS", {collection: 2}),
            patch(
                "isophilly_ingest.coastal_obliques.AUDITED_LISTING_SHA256", {collection: "a" * 64}
            ),
            patch(
                "isophilly_ingest.coastal_obliques.AUDITED_FRAME_MANIFEST_SHA256",
                {collection: semantic_pin},
            ),
            patch("isophilly_ingest.coastal_obliques.AUDITED_FRAME_BYTES", {collection: 30}),
        )
        with patches[0], patches[1], patches[2], patches[3], patches[4]:
            self.assertEqual(load_inventory(collection).frames, frames)
            changed_frames = [asdict(frame) for frame in frames]
            changed_frames[0]["bytes"] = 11
            changed_value = {**value, "frames": changed_frames}
            path.write_text(json.dumps(changed_value))
            with self.assertRaisesRegex(CoastalObliqueError, "byte total"):
                load_inventory(collection)

    def test_existing_inventory_outside_audited_hash_is_rejected(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        collection = "schuylkill-2014"
        frames = [
            {
                "name": f"Schuylkill   {index:03}.jpg",
                "url": COLLECTION_URLS[collection] + f"Schuylkill%20%20%20{index:03}.jpg",
                "bytes": 10,
            }
            for index in range(1, 192)
        ]
        path = root / collection / "inventory.json"
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "collection": collection,
                    "source_url": COLLECTION_URLS[collection],
                    "listing_sha256": "0" * 64,
                    "fetched_at": "now",
                    "frames": frames,
                }
            )
        )

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            self.assertRaisesRegex(CoastalObliqueError, "not the audited pin"),
        ):
            load_inventory(collection)

    def test_changed_listing_refuses_cached_file_even_without_progress(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        collection = "schuylkill-2014"
        collection_root = root / collection
        collection_root.mkdir(parents=True)
        old_frame = Frame("Schuylkill   001.jpg", COLLECTION_URLS[collection] + "old.jpg", 10)
        old = Inventory(1, collection, COLLECTION_URLS[collection], "a" * 64, "then", (old_frame,))
        (collection_root / "inventory.json").write_text(json.dumps(inventory_dict(old)))
        jpeg = collection_root / "jpeg"
        jpeg.mkdir()
        (jpeg / "orphan.jpg.part").write_bytes(b"partial")
        listing = b'20 <A HREF="Schuylkill%20%20%20001.jpg">Schuylkill   001.jpg</A>'
        transport = httpx.MockTransport(lambda request: httpx.Response(200, content=listing))

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch(
                "isophilly_ingest.coastal_obliques.EXPECTED_COUNTS",
                {collection: 1},
            ),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=old),
            httpx.Client(transport=transport) as client,
            self.assertRaisesRegex(CoastalObliqueError, "cached JPEG"),
        ):
            create_inventory(collection, refresh=True, client=client)


class ReviewArtifactTests(unittest.TestCase):
    def _prepared_collection(self, count: int) -> tuple[TemporaryDirectory[str], Path, Inventory]:
        directory = TemporaryDirectory()
        root = Path(directory.name)
        frames = tuple(
            Frame(
                f"Schuylkill   {index:03}.jpg",
                f"https://example.test/{index:03}.jpg",
                len(jpeg_bytes()),
            )
            for index in range(1, count + 1)
        )
        inventory = Inventory(
            1, "schuylkill-2014", "https://example.test/", "a" * 64, "now", frames
        )
        jpeg = root / "schuylkill-2014" / "jpeg"
        jpeg.mkdir(parents=True)
        entries: dict[str, object] = {}
        for frame in frames:
            path = jpeg / frame.name
            path.write_bytes(jpeg_bytes())
            entries[frame.name] = {
                "status": "downloaded",
                "bytes": frame.bytes,
                "sha256": hashlib.sha256(jpeg_bytes()).hexdigest(),
                "width": 1200,
                "height": 800,
            }
        (root / "schuylkill-2014" / "progress.json").write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "inventory_listing_sha256": "a" * 64,
                    "frames": entries,
                }
            )
        )
        return directory, root, inventory

    def test_contact_sheet_has_filename_labels_and_ordered_hash_sidecar(self) -> None:
        directory, root, inventory = self._prepared_collection(2)
        self.addCleanup(directory.cleanup)

        def run(command: list[str], **kwargs: object) -> object:
            if command[-1] == "-version":
                return type("Result", (), {"stdout": "ImageMagick 7.test\n"})()
            Path(command[-1]).write_bytes(b"sheet")
            return type("Result", (), {"stdout": ""})()

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=inventory),
            patch("isophilly_ingest.coastal_obliques.shutil.which", return_value="/usr/bin/magick"),
            patch(
                "isophilly_ingest.coastal_obliques._label_font",
                return_value=(root / "NotoSans-Regular.ttf", "f" * 64, ["fc-match"]),
            ),
            patch("isophilly_ingest.coastal_obliques.subprocess.run", side_effect=run) as run_mock,
        ):
            output = contact_sheet("schuylkill-2014")

        commands = [call.args[0] for call in run_mock.call_args_list]
        command = commands[-1]
        self.assertEqual(command[command.index("-label") + 1], "%t")
        self.assertEqual(command[command.index("-font") + 1], str(root / "NotoSans-Regular.ttf"))
        full_resolution = {
            str(root / "schuylkill-2014" / "jpeg" / frame.name) for frame in inventory.frames
        }
        for invoked in commands:
            self.assertLessEqual(sum(argument in full_resolution for argument in invoked), 1)
        thumbnail_commands = [
            invoked
            for invoked in commands
            if any(argument in full_resolution for argument in invoked)
        ]
        self.assertEqual(len(thumbnail_commands), 2)
        self.assertFalse(any(argument in full_resolution for argument in command))
        sidecar = json.loads(output.with_suffix(".json").read_text())
        self.assertEqual(
            [frame["name"] for frame in sidecar["ordered_frames"]],
            ["Schuylkill   001.jpg", "Schuylkill   002.jpg"],
        )
        self.assertEqual(sidecar["contact_sheet_sha256"], hashlib.sha256(b"sheet").hexdigest())
        self.assertEqual(sidecar["toolchain"]["version"], "ImageMagick 7.test")
        self.assertEqual(sidecar["toolchain"]["thumbnail_commands_run"], thumbnail_commands)
        self.assertEqual(sidecar["toolchain"]["montage_command"], command)
        self.assertEqual(sidecar["label_font"]["sha256"], "f" * 64)
        self.assertIn("exactly one full-resolution", sidecar["memory_model"])

    def test_contact_sheet_rejects_stale_cached_thumbnail(self) -> None:
        directory, root, inventory = self._prepared_collection(1)
        self.addCleanup(directory.cleanup)
        commands: list[list[str]] = []

        def run(command: list[str], **kwargs: object) -> object:
            commands.append(command)
            if command[-1] == "-version":
                return type("Result", (), {"stdout": "ImageMagick 7.test\n"})()
            Path(command[-1]).write_bytes(b"rendered")
            return type("Result", (), {"stdout": ""})()

        patches = (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=inventory),
            patch("isophilly_ingest.coastal_obliques.shutil.which", return_value="/usr/bin/magick"),
            patch(
                "isophilly_ingest.coastal_obliques._label_font",
                return_value=(root / "NotoSans-Regular.ttf", "f" * 64, ["fc-match"]),
            ),
            patch("isophilly_ingest.coastal_obliques.subprocess.run", side_effect=run),
        )
        with patches[0], patches[1], patches[2], patches[3], patches[4]:
            contact_sheet("schuylkill-2014")

        metadata_path = next(
            (root / "schuylkill-2014" / ".contact-sheet-cache").rglob("*.jpg.json")
        )
        metadata = json.loads(metadata_path.read_text())
        metadata["source_sha256"] = "0" * 64
        metadata_path.write_text(json.dumps(metadata))
        commands.clear()

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=inventory),
            patch("isophilly_ingest.coastal_obliques.shutil.which", return_value="/usr/bin/magick"),
            patch(
                "isophilly_ingest.coastal_obliques._label_font",
                return_value=(root / "NotoSans-Regular.ttf", "f" * 64, ["fc-match"]),
            ),
            patch("isophilly_ingest.coastal_obliques.subprocess.run", side_effect=run),
        ):
            contact_sheet("schuylkill-2014")

        source = str(root / "schuylkill-2014" / "jpeg" / inventory.frames[0].name)
        conversions = [command for command in commands if source in command]
        self.assertEqual(len(conversions), 1)
        repaired = json.loads(metadata_path.read_text())
        self.assertEqual(repaired["source_sha256"], hashlib.sha256(jpeg_bytes()).hexdigest())

    def test_sfm_handoff_rejects_short_sequence_without_override(self) -> None:
        directory, root, inventory = self._prepared_collection(2)
        self.addCleanup(directory.cleanup)
        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=inventory),
            patch("isophilly_ingest.coastal_obliques._exif_metadata", return_value={}),
            self.assertRaisesRegex(CoastalObliqueError, "at least 20 contiguous"),
        ):
            sfm_handoff("schuylkill-2014")

    def test_sfm_override_records_incompleteness(self) -> None:
        directory, root, inventory = self._prepared_collection(2)
        self.addCleanup(directory.cleanup)
        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=inventory),
            patch("isophilly_ingest.coastal_obliques._exif_metadata", return_value={}),
        ):
            output = sfm_handoff("schuylkill-2014", allow_incomplete=True)

        completeness = json.loads(output.read_text())["completeness"]
        self.assertTrue(completeness["contiguous"])
        self.assertTrue(completeness["incomplete_override"])
        self.assertFalse(completeness["sufficient_for_default_handoff"])

    def test_sfm_handoff_prohibits_shared_intrinsics_for_zoom_flight(self) -> None:
        directory, root, inventory = self._prepared_collection(20)
        self.addCleanup(directory.cleanup)
        focal = iter(["70/1"] * 10 + ["110/1"] * 10)

        def exif(path: Path) -> dict[str, str | None]:
            del path
            return {"FocalLength": next(focal), "Orientation": "1"}

        with (
            patch("isophilly_ingest.coastal_obliques.COASTAL_DIR", root),
            patch("isophilly_ingest.coastal_obliques.load_inventory", return_value=inventory),
            patch("isophilly_ingest.coastal_obliques._exif_metadata", side_effect=exif),
        ):
            output = sfm_handoff("schuylkill-2014")

        handoff = json.loads(output.read_text())
        self.assertEqual(handoff["schema_version"], 2)
        self.assertEqual(handoff["camera_intrinsics"]["distinct_focal_lengths"], 2)
        self.assertIn("shared intrinsic is prohibited", handoff["camera_intrinsics"]["policy"])
        self.assertIn("per-image EXIF-seeded", handoff["next_step"])


class CliTests(unittest.TestCase):
    def test_command_specific_flags_are_not_silently_ignored(self) -> None:
        with redirect_stderr(StringIO()), self.assertRaises(SystemExit):
            _parser().parse_args(["status", "--refresh"])


if __name__ == "__main__":
    unittest.main()
