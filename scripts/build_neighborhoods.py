"""Build the small, browser-ready neighborhood overlay from the PCPC layer."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Final

import httpx
from shapely.geometry import MultiPolygon, Polygon, shape

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


def _round_ring(ring: Any) -> list[list[float]]:
    return [[round(float(x), 6), round(float(y), 6)] for x, y, *_ in ring.coords]


def _polygon_rings(geometry: Polygon | MultiPolygon) -> list[list[list[float]]]:
    polygons = [geometry] if isinstance(geometry, Polygon) else list(geometry.geoms)
    return [_round_ring(polygon.exterior) for polygon in polygons if not polygon.is_empty]


def build(source: dict[str, Any]) -> dict[str, Any]:
    features = source.get("features")
    if not isinstance(features, list) or len(features) < 140:
        raise ValueError("PCPC response is missing neighborhood features")

    neighborhoods: list[dict[str, Any]] = []
    names: set[str] = set()
    for feature in features:
        properties = feature.get("properties", {})
        raw_name = properties.get("NAME")
        if not isinstance(raw_name, str):
            continue
        geometry = shape(feature["geometry"]).simplify(0.00005, preserve_topology=True)
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


def download() -> dict[str, Any]:
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
    data: Any = response.json()
    if not isinstance(data, dict):
        raise TypeError("PCPC response is not a JSON object")
    return data


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, help="Use a saved ArcGIS GeoJSON response")
    parser.add_argument("--output", type=Path, default=Path("static/neighborhoods.json"))
    arguments = parser.parse_args()
    source = json.loads(arguments.input.read_text()) if arguments.input else download()
    output = build(source)
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(json.dumps(output, separators=(",", ":")) + "\n")
    print(f"wrote {arguments.output} ({len(output['features'])} areas)")


if __name__ == "__main__":
    main()
