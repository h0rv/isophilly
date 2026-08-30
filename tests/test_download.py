from __future__ import annotations

import hashlib
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import httpx

from isophilly_ingest.config import Source
from isophilly_ingest.download import (
    _cached_local_digest,
    _download,
    _download_with_client,
    _save_response,
)


class DownloadTests(unittest.TestCase):
    def setUp(self) -> None:
        self.source = Source("Test data", "test-data", "https://example.test/data", "bin")

    def test_retries_temporary_http_errors(self) -> None:
        calls = 0

        def handler(request: httpx.Request) -> httpx.Response:
            nonlocal calls
            calls += 1
            status = 504 if calls == 1 else 200
            return httpx.Response(status, content=b"current", request=request)

        with (
            TemporaryDirectory() as directory,
            patch("isophilly_ingest.download.RAW_DIR", Path(directory)),
            patch("isophilly_ingest.download.time.sleep") as sleep,
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            snapshot = _download_with_client(self.source, client)

        self.assertEqual(snapshot.sha256, hashlib.sha256(b"current").hexdigest())
        self.assertEqual(calls, 2)
        sleep.assert_called_once_with(0.5)

    def test_uses_verified_cache_after_retries_fail(self) -> None:
        payload = b"cached"
        sha256 = hashlib.sha256(payload).hexdigest()

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(504, request=request)

        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        cached = root / f"test-data-{sha256[:12]}.bin"
        cached.write_bytes(payload)
        with (
            patch("isophilly_ingest.download.RAW_DIR", root),
            patch("isophilly_ingest.download.time.sleep"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            snapshot = _download_with_client(self.source, client)

        self.assertEqual(snapshot.path, cached)
        self.assertEqual(snapshot.sha256, sha256)

    def test_uses_complete_cache_after_short_successful_response(self) -> None:
        payload = b"complete"
        sha256 = hashlib.sha256(payload).hexdigest()
        source = Source(
            "Test data",
            "test-data",
            "https://example.test/data",
            "bin",
            minimum_bytes=len(payload),
        )

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, content=b"short", request=request)

        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        cached = root / f"test-data-{sha256[:12]}.bin"
        cached.write_bytes(payload)
        with (
            patch("isophilly_ingest.download.RAW_DIR", root),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            snapshot = _download_with_client(source, client)

        self.assertEqual(snapshot.path, cached)
        self.assertEqual(snapshot.sha256, sha256)

    def test_replaces_corrupt_content_addressed_destination(self) -> None:
        payload = b"fresh complete response"
        sha256 = hashlib.sha256(payload).hexdigest()
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        destination = root / f"test-data-{sha256[:12]}.bin"
        destination.write_bytes(b"corrupt")
        request = httpx.Request("GET", self.source.url)
        response = httpx.Response(200, content=payload, request=request)

        with patch("isophilly_ingest.download.RAW_DIR", root):
            snapshot = _save_response(self.source, response)

        self.assertEqual(snapshot.path, destination)
        self.assertEqual(destination.read_bytes(), payload)
        self.assertEqual(snapshot.sha256, sha256)

    def test_immutable_source_uses_verified_cache_without_network(self) -> None:
        payload = b"immutable archive"
        sha256 = hashlib.sha256(payload).hexdigest()
        source = Source(
            "Test archive",
            "test-archive",
            "https://example.test/archive",
            "zip",
            immutable=True,
        )
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        cached = root / f"test-archive-{sha256[:12]}.zip"
        cached.write_bytes(payload)

        with (
            patch("isophilly_ingest.download.RAW_DIR", root),
            patch("isophilly_ingest.download.httpx.Client") as client,
        ):
            snapshot = _download(source)

        self.assertEqual(snapshot.path, cached)
        client.assert_not_called()

    def test_mutable_source_uses_verified_cache_unless_refresh_is_requested(self) -> None:
        payload = b"cached source"
        sha256 = hashlib.sha256(payload).hexdigest()
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        cached = root / f"test-data-{sha256[:12]}.bin"
        cached.write_bytes(payload)

        with (
            patch("isophilly_ingest.download.RAW_DIR", root),
            patch("isophilly_ingest.download.httpx.Client") as client,
        ):
            snapshot = _download(self.source)

        self.assertEqual(snapshot.path, cached)
        client.assert_not_called()

    def test_local_digest_is_reused_while_file_is_unchanged(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        archive = Path(directory.name) / "archive.zip"
        archive.write_bytes(b"verified archive")

        first = _cached_local_digest(archive)
        with patch("isophilly_ingest.download._digest") as digest:
            second = _cached_local_digest(archive)

        self.assertEqual(first, second)
        digest.assert_not_called()

    def test_local_digest_is_recomputed_after_file_changes(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        archive = Path(directory.name) / "archive.zip"
        archive.write_bytes(b"first")
        first = _cached_local_digest(archive)
        archive.write_bytes(b"second payload")

        second = _cached_local_digest(archive)

        self.assertNotEqual(first, second)

    def test_local_digest_rejects_malformed_cached_hash(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        archive = Path(directory.name) / "archive.zip"
        archive.write_bytes(b"verified archive")
        expected = _cached_local_digest(archive)
        sidecar = archive.with_suffix(".zip.sha256.json")
        cached = sidecar.read_text().replace(expected, "G" * 64)
        sidecar.write_text(cached)

        self.assertEqual(_cached_local_digest(archive), expected)


if __name__ == "__main__":
    unittest.main()
