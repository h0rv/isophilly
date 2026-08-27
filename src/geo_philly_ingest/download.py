from __future__ import annotations

import asyncio
import hashlib
import shutil
import tempfile
from datetime import UTC, datetime
from pathlib import Path
from urllib.request import Request, urlopen

from .config import RAW_DIR, Source
from .models import Snapshot

USER_AGENT = "geo-philly/0.1 (public-data ingest)"


def _download(source: Source) -> Snapshot:
    RAW_DIR.mkdir(parents=True, exist_ok=True)
    request = Request(source.url, headers={"User-Agent": USER_AGENT})
    digest = hashlib.sha256()
    fetched_at = datetime.now(UTC).isoformat()
    with (
        urlopen(request, timeout=300) as response,
        tempfile.NamedTemporaryFile(
            dir=RAW_DIR, suffix=f".{source.extension}", delete=False
        ) as temporary,
    ):
        while chunk := response.read(1024 * 1024):
            digest.update(chunk)
            temporary.write(chunk)
        temporary_path = Path(temporary.name)
        etag = response.headers.get("ETag")
        last_modified = response.headers.get("Last-Modified")

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
        etag=etag,
        last_modified=last_modified,
    )


async def download_all(sources: tuple[Source, ...]) -> dict[str, Snapshot]:
    tasks: dict[str, asyncio.Task[Snapshot]] = {}
    async with asyncio.TaskGroup() as group:
        for source in sources:
            tasks[source.filename] = group.create_task(asyncio.to_thread(_download, source))
    return {name: task.result() for name, task in tasks.items()}
