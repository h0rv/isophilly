from __future__ import annotations

import argparse
import json
import math
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal
from urllib.parse import quote

import httpx
from pydantic import Field, SecretStr
from pydantic_settings import BaseSettings, SettingsConfigDict

TOKEN_URL = "https://apicenter.eagleview.com/oauth2/v1/token"
PRODUCTION_API_URL = "https://apis.eagleview.com"
SANDBOX_API_URL = "https://sandbox.apis.eagleview.com"
ORTHOMOSAIC_SEARCH_PATH = "/imagery/v3/discovery/orthomosaics/search"
RANK_LOCATION_PATH = "/imagery/v3/discovery/rank/location"
IMAGE_LOCATION_PATH = "/imagery/v3/images/{image_urn}/location"
DOCUMENTATION_URL = "https://developer.eagleview.com/docs/imagery/api-documentation.md"
EPSG_NAME = "EPSG:32129"
GRID_CELL_METERS = 500.0
IMAGE_SIZE_PIXELS = 4096

type Environment = Literal["sandbox", "production"]
type View = Literal["ortho", "north", "east", "south", "west"]
type JsonObject = dict[str, object]


@dataclass(frozen=True, slots=True)
class Bounds:
    min_x: float
    min_y: float
    max_x: float
    max_y: float

    def __post_init__(self) -> None:
        values = (self.min_x, self.min_y, self.max_x, self.max_y)
        if not all(math.isfinite(value) for value in values):
            raise ValueError("EagleView bounds must be finite")
        if self.min_x >= self.max_x or self.min_y >= self.max_y:
            raise ValueError("EagleView bounds must have positive area")

    @property
    def width(self) -> float:
        return self.max_x - self.min_x

    @property
    def height(self) -> float:
        return self.max_y - self.min_y

    def wkt(self) -> str:
        return (
            f"POLYGON(({self.min_x:.3f} {self.min_y:.3f},"
            f"{self.max_x:.3f} {self.min_y:.3f},"
            f"{self.max_x:.3f} {self.max_y:.3f},"
            f"{self.min_x:.3f} {self.max_y:.3f},"
            f"{self.min_x:.3f} {self.min_y:.3f}))"
        )


# Bounds of the official City Limits source in the project's State Plane CRS.
PHILADELPHIA_AOI = Bounds(810_944.701, 62_428.278, 838_235.877, 92_947.533)
CITY_HALL_AOI = Bounds(820_744.25, 71_744.46, 821_244.25, 72_244.46)


@dataclass(frozen=True, slots=True)
class GridCell:
    id: str
    row: int
    column: int
    bounds: Bounds


@dataclass(frozen=True, slots=True)
class ImportPlan:
    aoi: Bounds
    cell_size_m: float
    rows: int
    columns: int
    cells: tuple[GridCell, ...]

    def manifest(self) -> JsonObject:
        return {
            "schema_version": 1,
            "provider": "EagleView Imagery API",
            "documentation": DOCUMENTATION_URL,
            "crs": EPSG_NAME,
            "aoi_bounds_m": bounds_values(self.aoi),
            "grid": {
                "cell_size_m": self.cell_size_m,
                "rows": self.rows,
                "columns": self.columns,
                "cells": [
                    {
                        "id": cell.id,
                        "row": cell.row,
                        "column": cell.column,
                        "bounds_m": bounds_values(cell.bounds),
                        "wkt": cell.bounds.wkt(),
                    }
                    for cell in self.cells
                ],
            },
            "requests": {
                "token": {"method": "POST", "url": TOKEN_URL},
                "orthomosaic_search": {
                    "method": "POST",
                    "path": ORTHOMOSAIC_SEARCH_PATH,
                    "body": orthomosaic_request(self.aoi),
                },
                "rank_location": {
                    "method": "POST",
                    "path": RANK_LOCATION_PATH,
                    "scope": "one explicitly selected grid cell",
                    "example_cell": self.cells[0].id,
                    "body": rank_request(self.cells[0]),
                },
                "image_location": {
                    "method": "GET",
                    "path": IMAGE_LOCATION_PATH,
                    "example_cell": self.cells[0].id,
                    "query": image_parameters(self.cells[0]),
                },
            },
        }


@dataclass(frozen=True, slots=True)
class EagleViewAccess:
    environment: Environment
    client_id: str
    client_secret: str = field(repr=False)

    @property
    def api_url(self) -> str:
        if self.environment == "sandbox":
            return SANDBOX_API_URL
        return PRODUCTION_API_URL


class EagleViewSettings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
        frozen=True,
    )

    client_id: SecretStr = Field(validation_alias="EAGLE_VIEW_CLIENT_ID")
    client_secret: SecretStr = Field(validation_alias="EAGLE_VIEW_CLIENT_SECRET")
    environment: Environment = Field(
        default="sandbox",
        validation_alias="GEO_PHILLY_EAGLEVIEW_ENVIRONMENT",
    )

    def access(self) -> EagleViewAccess:
        return EagleViewAccess(
            self.environment,
            self.client_id.get_secret_value(),
            self.client_secret.get_secret_value(),
        )


@dataclass(frozen=True, slots=True)
class Orthomosaic:
    urn: str
    category: str | None
    level: str | None


@dataclass(frozen=True, slots=True)
class Image:
    urn: str
    view: View
    capture_start: str | None
    capture_end: str | None


@dataclass(frozen=True, slots=True)
class Token:
    value: str
    expires_at: float


@dataclass(frozen=True, slots=True)
class SmokeResult:
    view: View
    capture_start: str | None
    capture_end: str | None
    bytes: int
    output: Path


def build_plan(aoi: Bounds = PHILADELPHIA_AOI, cell_size_m: float = GRID_CELL_METERS) -> ImportPlan:
    if not math.isfinite(cell_size_m) or cell_size_m <= 0:
        raise ValueError("EagleView grid cell size must be positive")
    columns = math.ceil(aoi.width / cell_size_m)
    rows = math.ceil(aoi.height / cell_size_m)
    cells = tuple(
        GridCell(
            id=f"r{row:03d}-c{column:03d}",
            row=row,
            column=column,
            bounds=Bounds(
                aoi.min_x + column * cell_size_m,
                aoi.min_y + row * cell_size_m,
                min(aoi.max_x, aoi.min_x + (column + 1) * cell_size_m),
                min(aoi.max_y, aoi.min_y + (row + 1) * cell_size_m),
            ),
        )
        for row in range(rows)
        for column in range(columns)
    )
    return ImportPlan(aoi, cell_size_m, rows, columns, cells)


def bounds_values(bounds: Bounds) -> list[float]:
    return [bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y]


def geometry(bounds: Bounds) -> JsonObject:
    return {"wkt": {"value": bounds.wkt(), "epsg": EPSG_NAME}}


def orthomosaic_request(bounds: Bounds) -> JsonObject:
    return {
        "location": {
            "area": {
                "polygon": geometry(bounds),
                "operation": "SPATIAL_OPERATION_INTERSECTS",
            }
        },
        "filters": {
            "categories": {"items": ["ORTHOMOSAIC_CATEGORY_VISUAL"]},
            "aggregation_preference": "IMAGERY_AGGREGATION_PREFERENCE_ALL",
        },
        "response": {
            "properties": {
                "category": True,
                "level": True,
                "capture_window": True,
                "calculated_gsd": True,
                "zoom_range": True,
                "ground_footprint_bbox": True,
            },
            "geometry_format": "GEOMETRY_FORMAT_WKT",
        },
        "page": {"size": 100},
    }


def rank_request(cell: GridCell) -> JsonObject:
    return {
        "polygon": geometry(cell.bounds),
        "view": {
            "orthos": {},
            "obliques": {"cardinals": {"north": True, "east": True, "south": True, "west": True}},
            "max_images_per_view": 1,
        },
        "capture": {"from": {"any": {"result_count": 1}}},
        "response_props": {
            "shot_time": True,
            "calculated_gsd": True,
            "zoom_range": True,
            "ground_footprint": True,
            "look_at": True,
        },
    }


def image_parameters(cell: GridCell) -> dict[str, str | int]:
    return {
        "area": cell.bounds.wkt(),
        "epsg": EPSG_NAME,
        "size.width": IMAGE_SIZE_PIXELS,
        "size.height": IMAGE_SIZE_PIXELS,
        "format": "IMAGE_FORMAT_JPEG",
        "quality": 90,
        "scale": "IMAGE_SCALE_NONE",
    }


class EagleViewClient:
    def __init__(
        self,
        access: EagleViewAccess,
        *,
        transport: httpx.BaseTransport | None = None,
        timeout_seconds: float = 30.0,
    ) -> None:
        self._access = access
        self._http = httpx.Client(transport=transport, timeout=timeout_seconds)
        self._token: Token | None = None

    def __enter__(self) -> EagleViewClient:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def close(self) -> None:
        self._http.close()

    def search_orthomosaics(self, bounds: Bounds) -> tuple[Orthomosaic, ...]:
        response = self._post(ORTHOMOSAIC_SEARCH_PATH, orthomosaic_request(bounds))
        return parse_orthomosaics(response)

    def rank(self, cell: GridCell) -> tuple[Image, ...]:
        response = self._post(RANK_LOCATION_PATH, rank_request(cell))
        return parse_images(response)

    def image(self, image_urn: str, cell: GridCell) -> bytes:
        if not image_urn:
            raise ValueError("EagleView image URN cannot be empty")
        path = IMAGE_LOCATION_PATH.format(image_urn=quote(image_urn, safe=":"))
        response = self._http.get(
            f"{self._access.api_url}{path}",
            params=image_parameters(cell),
            headers=self._authorization(),
        )
        response.raise_for_status()
        return response.content

    def _post(self, path: str, body: JsonObject) -> object:
        response = self._http.post(
            f"{self._access.api_url}{path}",
            json=body,
            headers=self._authorization(),
        )
        response.raise_for_status()
        return response.json()

    def _authorization(self) -> dict[str, str]:
        token = self._access_token()
        return {"Authorization": f"Bearer {token}"}

    def _access_token(self) -> str:
        now = time.monotonic()
        if self._token is not None and self._token.expires_at > now:
            return self._token.value
        response = self._http.post(
            TOKEN_URL,
            data={"grant_type": "client_credentials"},
            auth=httpx.BasicAuth(self._access.client_id, self._access.client_secret),
        )
        response.raise_for_status()
        self._token = parse_token(response.json(), now)
        return self._token.value


def parse_token(raw: object, now: float) -> Token:
    value = object_mapping(raw, "token response")
    access_token = nonempty_string(value.get("access_token"), "access_token")
    token_type = nonempty_string(value.get("token_type"), "token_type")
    expires_in = value.get("expires_in")
    if token_type.lower() != "bearer" or not isinstance(expires_in, int) or expires_in <= 60:
        raise ValueError("EagleView token response is invalid")
    return Token(access_token, now + expires_in - 60)


def parse_orthomosaics(raw: object) -> tuple[Orthomosaic, ...]:
    value = object_mapping(raw, "orthomosaic response")
    items = object_list(value.get("orthomosaics"), "orthomosaics")
    return tuple(
        Orthomosaic(
            urn=nonempty_string(item.get("urn"), "orthomosaic urn"),
            category=optional_string(item.get("category"), "orthomosaic category"),
            level=optional_string(item.get("level"), "orthomosaic level"),
        )
        for item in items
    )


def parse_images(raw: object) -> tuple[Image, ...]:
    value = object_mapping(raw, "rank response")
    captures = object_list(value.get("captures"), "captures")
    images: list[Image] = []
    for capture_group in captures:
        capture_raw = capture_group.get("capture")
        capture = {} if capture_raw is None else object_mapping(capture_raw, "capture")
        capture_start = optional_string(capture.get("start_date"), "capture start date")
        capture_end = optional_string(capture.get("end_date"), "capture end date")
        append_view(images, capture_group.get("orthos"), "ortho", capture_start, capture_end)
        obliques_raw = capture_group.get("obliques")
        obliques = {} if obliques_raw is None else object_mapping(obliques_raw, "obliques")
        for view in ("north", "east", "south", "west"):
            append_view(images, obliques.get(view), view, capture_start, capture_end)
    return tuple(images)


def append_view(
    destination: list[Image],
    raw: object,
    view: View,
    capture_start: str | None,
    capture_end: str | None,
) -> None:
    if raw is None:
        return
    group = object_mapping(raw, f"{view} image group")
    for image in object_list(group.get("images"), f"{view} images"):
        destination.append(
            Image(
                nonempty_string(image.get("urn"), f"{view} image urn"),
                view,
                capture_start,
                capture_end,
            )
        )


def object_mapping(raw: object, name: str) -> dict[str, object]:
    if not isinstance(raw, dict) or not all(isinstance(key, str) for key in raw):
        raise ValueError(f"EagleView {name} must be an object")
    return {str(key): value for key, value in raw.items()}


def object_list(raw: object, name: str) -> tuple[dict[str, object], ...]:
    if not isinstance(raw, list):
        raise ValueError(f"EagleView {name} must be an array")
    return tuple(object_mapping(item, name) for item in raw)


def nonempty_string(raw: object, name: str) -> str:
    if not isinstance(raw, str) or not raw:
        raise ValueError(f"EagleView {name} must be a non-empty string")
    return raw


def optional_string(raw: object, name: str) -> str | None:
    if raw is None:
        return None
    return nonempty_string(raw, name)


def write_plan(path: Path, plan: ImportPlan) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".json.part")
    temporary.write_text(json.dumps(plan.manifest(), indent=2) + "\n")
    temporary.replace(path)


def smoke_test(settings: EagleViewSettings, output: Path) -> SmokeResult:
    cell = GridCell("city-hall", 0, 0, CITY_HALL_AOI)
    with EagleViewClient(settings.access()) as client:
        images = client.rank(cell)
        try:
            selected = next(image for image in images if image.view != "ortho")
        except StopIteration:
            if not images:
                raise ValueError(
                    "EagleView returned no imagery for the City Hall smoke-test area"
                ) from None
            selected = images[0]
        content = client.image(selected.urn, cell)
    if len(content) < 1_000 or not content.startswith(b"\xff\xd8"):
        raise ValueError("EagleView smoke-test response is not a valid JPEG")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(f"{output.suffix}.part")
    temporary.write_bytes(content)
    temporary.replace(output)
    return SmokeResult(
        selected.view,
        selected.capture_start,
        selected.capture_end,
        len(content),
        output,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Plan or smoke-test EagleView imagery access")
    parser.add_argument("--output", type=Path, default=Path("data/raw/eagleview-plan.json"))
    parser.add_argument(
        "--smoke",
        action="store_true",
        help="make one authorized City Hall discovery and image request",
    )
    arguments = parser.parse_args()
    if arguments.smoke:
        output = Path("data/raw/eagleview-city-hall-smoke.jpg")
        result = smoke_test(EagleViewSettings(), output)
        print(
            f"downloaded {result.view} City Hall image "
            f"({result.bytes / 1_000_000:.1f} MB) to {result.output}"
        )
        if result.capture_start is not None:
            print(f"capture started {result.capture_start}")
        return
    write_plan(arguments.output, build_plan())
    print(f"wrote offline EagleView request plan to {arguments.output}")
    print("no credentials were read and no network requests were made")


if __name__ == "__main__":
    main()
