from __future__ import annotations

import hashlib
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import httpx

from geo_philly_ingest.config import Source
from geo_philly_ingest.download import _download_with_client


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
            patch("geo_philly_ingest.download.RAW_DIR", Path(directory)),
            patch("geo_philly_ingest.download.time.sleep") as sleep,
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
            patch("geo_philly_ingest.download.RAW_DIR", root),
            patch("geo_philly_ingest.download.time.sleep"),
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
            patch("geo_philly_ingest.download.RAW_DIR", root),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            snapshot = _download_with_client(source, client)

        self.assertEqual(snapshot.path, cached)
        self.assertEqual(snapshot.sha256, sha256)


if __name__ == "__main__":
    unittest.main()
