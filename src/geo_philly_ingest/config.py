from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent.parent
RAW_DIR = ROOT / "data" / "raw"
CLEAN_DIR = ROOT / "data" / "clean"
WORLD_BIN = CLEAN_DIR / "philly.bin"
STREETS_BIN = CLEAN_DIR / "streets.bin"
METADATA_JSON = CLEAN_DIR / "meta.json"
MESH_TEXTURE_DIR = CLEAN_DIR / "mesh-textures"

# NAD83 / Pennsylvania South: the City's local State Plane projection in metres.
# This is equivalent to EPSG:2272 with US-survey-foot coordinates converted to metres,
# while keeping the coordinate reference system honest.
EPSG = 32129

DEFAULT_HEIGHT_METERS = 8.0
MIN_HEIGHT_METERS = 2.4
MAX_HEIGHT_METERS = 400.0
BUILDING_SIMPLIFY_METERS = 0.35
GROUND_SIMPLIFY_METERS = 1.0
STREET_SIMPLIFY_METERS = 1.0
MIN_BUILDING_AREA_METERS = 10.0
MIN_BUILDING_COUNT = 500_000


@dataclass(frozen=True, slots=True)
class Source:
    name: str
    filename: str
    url: str
    extension: str = "geojson"
    minimum_bytes: int = 1

    def accepts_size(self, size: int) -> bool:
        return size >= self.minimum_bytes


@dataclass(frozen=True, slots=True)
class Sources:
    city: Source
    buildings: Source
    water: Source
    parks: Source
    streets: Source
    building_parts: Source
    downtown_meshes: Source

    def all(self) -> tuple[Source, ...]:
        return (
            self.city,
            self.buildings,
            self.water,
            self.parks,
            self.streets,
            self.building_parts,
            self.downtown_meshes,
        )


CENTER_CITY_BOUNDS = (39.94018, -75.19042, 39.96987, -75.13356)
_south, _west, _north, _east = CENTER_CITY_BOUNDS
_building_parts_query = (
    "[out:json][timeout:180];"
    f'way["building:part"]({_south},{_west},{_north},{_east});'
    "out tags geom;"
)


SOURCES = Sources(
    city=Source(
        "City Limits",
        "city-limits",
        "https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/"
        "City_Limits/FeatureServer/0/query?outFields=*&where=1%3D1&f=geojson",
    ),
    buildings=Source(
        "Building Footprints",
        "building-footprints",
        "https://hub.arcgis.com/api/v3/datasets/"
        "ab9e89e1273f445bb265846c90b38a96_0/downloads/data?"
        "format=geojson&spatialRefId=4326&where=1%3D1",
        minimum_bytes=300_000_000,
    ),
    water=Source(
        "Hydrology Polygons",
        "hydrology-polygons",
        "https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/"
        "Hydrographic_Features_Poly/FeatureServer/1/query?"
        "outFields=*&where=1%3D1&f=geojson",
    ),
    parks=Source(
        "PPR Properties",
        "ppr-properties",
        "https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/"
        "PPR_Properties/FeatureServer/0/query?outFields=*&where=1%3D1&f=geojson",
    ),
    streets=Source(
        "Street Centerline",
        "street-centerline",
        "https://hub.arcgis.com/api/v3/datasets/"
        "c36d828494cd44b5bd8b038be696c839_0/downloads/data?"
        "format=geojson&spatialRefId=4326&where=1%3D1",
    ),
    building_parts=Source(
        "OpenStreetMap Center City Building Parts",
        "center-city-building-parts",
        "https://overpass-api.de/api/interpreter?data=" + quote(_building_parts_query),
        "json",
    ),
    downtown_meshes=Source(
        "Philadelphia 2015 Center City Textured 3D Buildings",
        "center-city-3d-buildings",
        "https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/"
        "Philadelphia_Buildings/SceneServer?f=pjson",
        "json",
        minimum_bytes=5_000,
    ),
)
