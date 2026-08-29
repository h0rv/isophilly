"""Build the small, browser-ready neighborhood overlay from the PCPC layer."""

from __future__ import annotations

import argparse
import json
from collections.abc import Mapping
from pathlib import Path
from typing import Final, Literal, NotRequired, TypedDict

import httpx
from shapely.geometry import LinearRing, MultiPolygon, Polygon, shape
from shapely.geometry.base import BaseGeometry

SOURCE_URL: Final = (
    "https://services1.arcgis.com/CtMjdUqInecbPao9/arcgis/rest/services/"
    "Philly_Planning_Neighborhoods/FeatureServer/11/query"
)
SOURCE_PAGE: Final = (
    "https://services1.arcgis.com/CtMjdUqInecbPao9/arcgis/rest/services/"
    "Philly_Planning_Neighborhoods/FeatureServer/11"
)
DISCLAIMER: Final = (
    "PCPC describes these as general historic and development boundaries; they are not "
    "official boundaries. Cultural areas are approximate and separately identified."
)


class Neighborhood(TypedDict):
    name: str
    kind: Literal["planning_neighborhood", "cultural_area"]
    label: list[float]
    rings: list[list[list[float]]]
    note: NotRequired[str]


class NeighborhoodCollection(TypedDict):
    source: str
    disclaimer: str
    features: list[Neighborhood]


def _mapping(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ValueError(f"{label} must be an object with string keys")
    return {str(key): item for key, item in value.items()}


def _geometry(value: object) -> BaseGeometry:
    parsed = shape(_mapping(value, "neighborhood geometry"))
    if not isinstance(parsed, BaseGeometry):
        raise ValueError("neighborhood geometry has the wrong shape")
    return parsed


def _round_ring(ring: LinearRing) -> list[list[float]]:
    return [[round(float(x), 6), round(float(y), 6)] for x, y, *_ in ring.coords]


def _polygon_rings(geometry: Polygon | MultiPolygon) -> list[list[list[float]]]:
    polygons = [geometry] if isinstance(geometry, Polygon) else list(geometry.geoms)
    return [_round_ring(polygon.exterior) for polygon in polygons if not polygon.is_empty]


def build(source: Mapping[str, object]) -> NeighborhoodCollection:
    features = source.get("features")
    if not isinstance(features, list) or len(features) < 140:
        raise ValueError("PCPC response is missing neighborhood features")

    neighborhoods: list[Neighborhood] = []
    names: set[str] = set()
    for raw_feature in features:
        feature = _mapping(raw_feature, "neighborhood feature")
        properties = _mapping(feature.get("properties"), "neighborhood properties")
        raw_name = properties.get("NAME")
        if not isinstance(raw_name, str):
            continue
        geometry = _geometry(feature.get("geometry")).simplify(0.00005, preserve_topology=True)
        if not isinstance(geometry, (Polygon, MultiPolygon)):
            continue
        point = geometry.representative_point()
        name = raw_name.title().replace(" Sq.", " Square")
        names.add(name.upper())
        neighborhoods.append(
            {
                "name": name,
                "kind": "planning_neighborhood",
                "label": [round(point.x, 6), round(point.y, 6)],
                "rings": _polygon_rings(geometry),
            }
        )

    required = {"BELLA VISTA", "WASHINGTON SQUARE WEST", "RITTENHOUSE SQUARE"}
    if not required <= names:
        raise ValueError(f"PCPC response is missing expected names: {sorted(required - names)}")

    neighborhoods.append(
        {
            "name": "Gayborhood",
            "kind": "cultural_area",
            "label": [-75.16165, 39.94755],
            "rings": [
                [
                    [-75.1645, 39.9448],
                    [-75.1594, 39.9453],
                    [-75.1588, 39.9502],
                    [-75.1639, 39.9497],
                    [-75.1645, 39.9448],
                ]
            ],
            "note": (
                "Approximate cultural area from Visit Philadelphia's 11th-to-Broad and "
                "Pine-to-Chestnut description; nested within Washington Square West."
            ),
        }
    )
    neighborhoods.sort(key=lambda item: (item["kind"], item["name"]))
    return {
        "source": SOURCE_PAGE,
        "disclaimer": DISCLAIMER,
        "features": neighborhoods,
    }


def download() -> dict[str, object]:
    response = httpx.get(
        SOURCE_URL,
        params={
            "where": "1=1",
            "outFields": "NAME",
            "outSR": "4326",
            "returnGeometry": "true",
            "f": "geojson",
        },
        headers={"User-Agent": "geo-philly neighborhood builder"},
        timeout=60,
    )
    response.raise_for_status()
    data: object = response.json()
    return _mapping(data, "PCPC response")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, help="Use a saved ArcGIS GeoJSON response")
    parser.add_argument("--output", type=Path, default=Path("static/neighborhoods.json"))
    arguments = parser.parse_args()
    raw: object = json.loads(arguments.input.read_text()) if arguments.input else download()
    source = _mapping(raw, "PCPC response")
    output = build(source)
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(json.dumps(output, separators=(",", ":")) + "\n")
    print(f"wrote {arguments.output} ({len(output['features'])} areas)")


if __name__ == "__main__":
    main()
