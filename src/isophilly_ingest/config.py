from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parent.parent.parent
RAW_DIR = ROOT / "data" / "raw"
CLEAN_DIR = ROOT / "data" / "clean"
WORLD_BIN = CLEAN_DIR / "philly.bin"
METADATA_JSON = CLEAN_DIR / "meta.json"
TEXTURE_COVERAGE_JSON = CLEAN_DIR / "texture-coverage.json"
MESH_TEXTURE_DIR = CLEAN_DIR / "mesh-textures"
LEGACY_DOWNTOWN_ARCHIVE = RAW_DIR / "Philadelphia2008_downtown_kml.zip"
STADIUM_ARCHIVE = RAW_DIR / "Philadelphia2008_stadium_kml.zip"

# NAD83 / Pennsylvania South: the City's local State Plane projection in metres.
# This is equivalent to EPSG:2272 with US-survey-foot coordinates converted to metres,
# while keeping the coordinate reference system honest.
EPSG = 32129

DEFAULT_HEIGHT_METERS = 8.0
MIN_HEIGHT_METERS = 2.4
MAX_HEIGHT_METERS = 400.0
BUILDING_SIMPLIFY_METERS = 0.35
CITY_SIMPLIFY_METERS = 1.0
GROUND_SIMPLIFY_METERS = 1.0
MIN_BUILDING_AREA_METERS = 10.0
MIN_BUILDING_COUNT = 500_000
STREET_TREE_SOURCE_ITEM_ID = "dc6826e1319c4b35a7b662bc6be68104_0"
STREET_TREE_SOURCE_RECORD_COUNT = 151_726
STREET_TREE_SOURCE_SHA256 = "cdec5a2141ef4c754ef714c76ca4a0203356dffb2bd14cde6d362e9353bd5a05"
STREET_TREE_ACCEPTED_COUNT = 151_371
STREET_TREE_PAYLOAD_SHA256 = "846992de1b2289410a714fea86c3e81ce96fd643e4ffda7ba426da1c53333868"


@dataclass(frozen=True, slots=True)
class Source:
    name: str
    filename: str
    url: str
    extension: str = "geojson"
    minimum_bytes: int = 1
    attribution: str | None = None
    terms_url: str | None = None
    immutable: bool = False
    expected_sha256: str | None = None

    def accepts_size(self, size: int) -> bool:
        return size >= self.minimum_bytes

    def accepts_digest(self, sha256: str) -> bool:
        return self.expected_sha256 is None or sha256 == self.expected_sha256

    def provenance(self) -> dict[str, str]:
        result: dict[str, str] = {}
        if self.attribution is not None:
            result["attribution"] = self.attribution
        if self.terms_url is not None:
            result["terms_url"] = self.terms_url
        return result


@dataclass(frozen=True, slots=True)
class Sources:
    city: Source
    buildings: Source
    water: Source
    parks: Source
    streets: Source
    street_trees: Source
    building_parts: Source
    downtown_meshes: Source
    legacy_downtown_meshes: Source
    stadium_meshes: Source

    def all(self) -> tuple[Source, ...]:
        return (
            self.city,
            self.buildings,
            self.water,
            self.parks,
            self.streets,
            self.street_trees,
            self.building_parts,
            self.downtown_meshes,
            self.legacy_downtown_meshes,
            self.stadium_meshes,
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
        "Street Centerlines",
        "street-centerline",
        "https://www.opendataphilly.org/datasets/street-centerlines.geojson",
        minimum_bytes=10_000_000,
        attribution="City of Philadelphia",
        terms_url="https://www.opendataphilly.org/datasets/street-centerlines/",
        immutable=True,
        expected_sha256="b9a1466fce07dd0463198995d6f4fd705c463aa483e58ce594ab9a07de22fd5f",
    ),
    street_trees=Source(
        "2025 Philadelphia Tree Inventory",
        "philadelphia-tree-inventory-2025",
        "https://hub.arcgis.com/api/v3/datasets/"
        f"{STREET_TREE_SOURCE_ITEM_ID}/downloads/data?"
        "format=geojson&spatialRefId=4326&where=1%3D1",
        minimum_bytes=40_000_000,
        attribution="Philadelphia Parks & Recreation",
        terms_url="https://opendataphilly.org/datasets/philadelphia-tree-inventory/",
        immutable=True,
        expected_sha256=STREET_TREE_SOURCE_SHA256,
    ),
    building_parts=Source(
        "OpenStreetMap Center City Building Parts",
        "center-city-building-parts",
        "https://overpass-api.de/api/interpreter?data=" + quote(_building_parts_query),
        "json",
        minimum_bytes=100_000,
        attribution="OpenStreetMap contributors",
        terms_url="https://www.openstreetmap.org/copyright",
    ),
    downtown_meshes=Source(
        "Philadelphia 2015 Center City Textured 3D Buildings",
        "center-city-3d-buildings",
        "https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/"
        "Philadelphia_Buildings/SceneServer?f=pjson",
        "json",
        minimum_bytes=5_000,
        attribution="City of Philadelphia via PASDA",
        terms_url="https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?"
        "file=Philadelphia_Building_3DModels.xml",
    ),
    legacy_downtown_meshes=Source(
        "Philadelphia 2008/09 Legacy Downtown Textured 3D Models",
        "legacy-downtown-2008-09",
        "https://www.pasda.psu.edu/download/philacity/data/3D_Models/2010/kml00.zip",
        "zip",
        minimum_bytes=800_000_000,
        attribution="City of Philadelphia via PASDA",
        terms_url="https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?"
        "file=Philadelphia_Building_3DModels.xml",
        immutable=True,
    ),
    stadium_meshes=Source(
        "Philadelphia 2008 Stadium Area Textured 3D Models",
        "stadium-area-2008",
        "https://www.pasda.psu.edu/download/philacity/data/3D_Models/2008/"
        "Stadium%20Area%20Processed%20w%20LiDAR-KML.zip",
        "zip",
        minimum_bytes=600_000_000,
        attribution="City of Philadelphia via PASDA",
        terms_url="https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?"
        "file=Philadelphia_Building_3DModels.xml",
        immutable=True,
    ),
)
