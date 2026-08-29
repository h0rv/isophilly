from __future__ import annotations

import asyncio
import hashlib
import shutil
import sys
import tempfile
import time
from datetime import UTC, datetime
from pathlib import Path

import httpx

from .config import RAW_DIR, Source
from .models import Snapshot

USER_AGENT = "geo-philly/0.1 (public-data ingest)"
RETRY_DELAYS_SECONDS = (0.5, 1.0, 2.0)
RETRYABLE_HTTP_STATUS = frozenset({408, 425, 429, 500, 502, 503, 504})


class DownloadError(RuntimeError):
    pass


def _digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _cached(source: Source) -> Snapshot | None:
    prefix = f"{source.filename}-"
    candidates = sorted(
        RAW_DIR.glob(f"{prefix}*.{source.extension}"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    for path in candidates:
        if not source.accepts_size(path.stat().st_size):
            continue
        sha256 = _digest(path)
        if path.stem.removeprefix(prefix) != sha256[:12]:
            continue
        modified = datetime.fromtimestamp(path.stat().st_mtime, UTC).isoformat()
        return Snapshot(
            name=source.name,
            url=source.url,
            path=path,
            sha256=sha256,
            size=path.stat().st_size,
            fetched_at=modified,
            etag=None,
            last_modified=None,
        )
    return None


def local_snapshot(source: Source, path: Path) -> Snapshot:
    try:
        size = path.stat().st_size
    except FileNotFoundError as error:
        raise DownloadError(
            f"{source.name} archive is missing: {path}; restore the existing local archive"
        ) from error
    if not source.accepts_size(size):
        raise DownloadError(
            f"{source.name} archive has only {size:,} bytes; "
            f"expected at least {source.minimum_bytes:,}"
        )
    modified = datetime.fromtimestamp(path.stat().st_mtime, UTC).isoformat()
    return Snapshot(
        name=source.name,
        url=source.url,
        path=path,
        sha256=_digest(path),
        size=size,
        fetched_at=modified,
        etag=None,
        last_modified=None,
    )


def _retryable(error: httpx.HTTPError) -> bool:
    if isinstance(error, httpx.HTTPStatusError):
        return error.response.status_code in RETRYABLE_HTTP_STATUS
    return isinstance(error, httpx.TransportError)


def _save_response(source: Source, response: httpx.Response) -> Snapshot:
    digest = hashlib.sha256()
    fetched_at = datetime.now(UTC).isoformat()
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=RAW_DIR, suffix=f".{source.extension}", delete=False
        ) as temporary:
            temporary_path = Path(temporary.name)
            for chunk in response.iter_bytes(chunk_size=1024 * 1024):
                digest.update(chunk)
                temporary.write(chunk)
    except Exception:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
        raise

    if temporary_path is None:
        raise AssertionError("temporary download path was not created")

    sha256 = digest.hexdigest()
    snapshot_path = RAW_DIR / f"{source.filename}-{sha256[:12]}.{source.extension}"
    if snapshot_path.exists():
        temporary_path.unlink()
    else:
        shutil.move(temporary_path, snapshot_path)
    return Snapshot(
        name=source.name,
        url=source.url,
        path=snapshot_path,
        sha256=sha256,
        size=snapshot_path.stat().st_size,
        fetched_at=fetched_at,
        etag=response.headers.get("etag"),
        last_modified=response.headers.get("last-modified"),
    )


def _download_with_client(source: Source, client: httpx.Client) -> Snapshot:
    for attempt in range(len(RETRY_DELAYS_SECONDS) + 1):
        try:
            with client.stream("GET", source.url) as response:
                response.raise_for_status()
                snapshot = _save_response(source, response)
                if source.accepts_size(snapshot.size):
                    return snapshot
                cached = _cached(source)
                if cached is None:
                    raise DownloadError(
                        f"{source.name} returned only {snapshot.size:,} bytes; "
                        f"expected at least {source.minimum_bytes:,}"
                    )
                print(
                    f"warning: {source.name} refresh was incomplete "
                    f"({snapshot.size:,} bytes); using cached {cached.path.name}",
                    file=sys.stderr,
                )
                return cached
        except httpx.HTTPError as error:
            if not _retryable(error):
                raise DownloadError(f"failed to download {source.name}: {error}") from error
            if attempt < len(RETRY_DELAYS_SECONDS):
                time.sleep(RETRY_DELAYS_SECONDS[attempt])
                continue
            cached = _cached(source)
            if cached is None:
                raise DownloadError(f"failed to download {source.name}: {error}") from error
            print(
                f"warning: {source.name} refresh failed ({error}); using cached {cached.path.name}",
                file=sys.stderr,
            )
            return cached
    raise AssertionError("download retry loop is exhaustive")


def _download(source: Source) -> Snapshot:
    RAW_DIR.mkdir(parents=True, exist_ok=True)
    with httpx.Client(
        headers={"User-Agent": USER_AGENT},
        follow_redirects=True,
        timeout=300,
    ) as client:
        return _download_with_client(source, client)


async def download_all(sources: tuple[Source, ...]) -> dict[str, Snapshot]:
    tasks: dict[str, asyncio.Task[Snapshot]] = {}
    async with asyncio.TaskGroup() as group:
        for source in sources:
            tasks[source.filename] = group.create_task(asyncio.to_thread(_download, source))
    return {name: task.result() for name, task in tasks.items()}
