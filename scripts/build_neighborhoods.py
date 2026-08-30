"""Build the small, browser-ready neighborhood overlay from the PCPC layer."""

from __future__ import annotations

import argparse
import json
from collections.abc import Mapping
from pathlib import Path
from typing import Final, Literal, NotRequired, TypedDict

import httpx
from pyproj import Transformer
from shapely.geometry import LinearRing, LineString, MultiPolygon, Polygon, box, shape
from shapely.geometry.base import BaseGeometry
from shapely.ops import transform

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
    "official boundaries. Locally named areas are approximate and separately identified."
)
LOCAL_CRS: Final = "EPSG:26918"
TO_LOCAL: Final = Transformer.from_crs("EPSG:4326", LOCAL_CRS, always_xy=True)
TO_WGS84: Final = Transformer.from_crs(LOCAL_CRS, "EPSG:4326", always_xy=True)

RETAIL_CORRIDORS: Final = (
    "https://www.visitphilly.com/media-center/press-releases/"
    "a-guide-to-philadelphias-retail-corridors-where-to-go-and-who-to-contact/"
)
FOOD_CORRIDORS: Final = (
    "https://www.visitphilly.com/media-center/press-releases/"
    "philadelphias-food-corridors-offer-neighborhood-dining-at-its-best-2/"
)
CITY_CORRIDORS: Final = (
    "https://www.phila.gov/programs/instore-forgivable-loan-program/eligible-commercial-corridors/"
)


class LocalAreaSpec(TypedDict):
    name: str
    parent: str
    source: str
    label: tuple[float, float]
    bounds: NotRequired[tuple[float, float, float, float]]
    corridor: NotRequired[tuple[tuple[float, float], ...]]
    width_m: NotRequired[float]
    priority: NotRequired[int]


def _bounds(
    name: str,
    parent: str,
    source: str,
    label: tuple[float, float],
    bounds: tuple[float, float, float, float],
    priority: int = 10,
) -> LocalAreaSpec:
    return {
        "name": name,
        "parent": parent,
        "source": source,
        "label": label,
        "bounds": bounds,
        "priority": priority,
    }


def _corridor(
    name: str,
    parent: str,
    source: str,
    label: tuple[float, float],
    points: tuple[tuple[float, float], ...],
    width_m: float = 115,
    priority: int = 10,
) -> LocalAreaSpec:
    return {
        "name": name,
        "parent": parent,
        "source": source,
        "label": label,
        "corridor": points,
        "width_m": width_m,
        "priority": priority,
    }


# Local names intentionally use simple, reviewable geometry. Bounds follow the named blocks in
# the cited source; linear commercial districts are buffered around explicit street endpoints.
LOCAL_AREAS: Final[tuple[LocalAreaSpec, ...]] = (
    # Center City
    _bounds(
        "Gayborhood",
        "Washington Square West",
        RETAIL_CORRIDORS,
        (-75.1617, 39.9476),
        (-75.1645, 39.9448, -75.1588, 39.9502),
    ),
    _bounds(
        "Midtown Village",
        "Washington Square West / Market East",
        RETAIL_CORRIDORS,
        (-75.1616, 39.9499),
        (-75.1643, 39.9452, -75.1582, 39.9534),
    ),
    _corridor(
        "Jewelers' Row",
        "Washington Square West",
        RETAIL_CORRIDORS,
        (-75.1539, 39.9488),
        ((-75.1561, 39.9491), (-75.1518, 39.9485)),
        70,
    ),
    _corridor(
        "Antique Row",
        "Washington Square West",
        "https://www.visitphilly.com/articles/philadelphia/streets-alleys/",
        (-75.1620, 39.9450),
        ((-75.1670, 39.9456), (-75.1574, 39.9444)),
        80,
    ),
    _bounds(
        "Market East Retail District",
        "Market East",
        RETAIL_CORRIDORS,
        (-75.1570, 39.9521),
        (-75.1644, 39.9466, -75.1500, 39.9569),
    ),
    _bounds(
        "Reading Terminal & Convention Center",
        "Market East / Chinatown",
        RETAIL_CORRIDORS,
        (-75.1592, 39.9547),
        (-75.1640, 39.9526, -75.1531, 39.9582),
    ),
    _bounds(
        "Rittenhouse Row",
        "Rittenhouse Square",
        RETAIL_CORRIDORS,
        (-75.1715, 39.9506),
        (-75.1810, 39.9451, -75.1642, 39.9558),
    ),
    _corridor(
        "Avenue of the Arts",
        "Washington Square West / Rittenhouse Square",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/avenue-of-the-arts/",
        (-75.1649, 39.9467),
        ((-75.1635, 39.9536), (-75.1672, 39.9392)),
        105,
    ),
    _corridor(
        "Parkway Museum District",
        "Logan Square / Fairmount",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/parkway-museum-district/",
        (-75.1784, 39.9615),
        ((-75.1646, 39.9542), (-75.1809, 39.9655)),
        180,
    ),
    _bounds(
        "Old City Arts District",
        "Old City",
        RETAIL_CORRIDORS,
        (-75.1455, 39.9517),
        (-75.1514, 39.9467, -75.1403, 39.9572),
    ),
    _bounds(
        "Independence Mall",
        "Old City / Society Hill",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/old-city/",
        (-75.1498, 39.9507),
        (-75.1531, 39.9467, -75.1468, 39.9554),
    ),
    _corridor(
        "Penn's Landing",
        "Old City / Society Hill",
        "https://www.delawareriverwaterfront.com/",
        (-75.1399, 39.9467),
        ((-75.1400, 39.9550), (-75.1433, 39.9378)),
        160,
    ),
    _corridor(
        "South Street Headhouse",
        "Queen Village / Society Hill",
        RETAIL_CORRIDORS,
        (-75.1495, 39.9415),
        ((-75.1430, 39.9423), (-75.1565, 39.9407)),
        125,
    ),
    _corridor(
        "Fabric Row",
        "Queen Village / Bella Vista",
        "https://www.visitphilly.com/things-to-do/attractions/fabric-row/",
        (-75.1495, 39.9406),
        ((-75.1486, 39.9433), (-75.1501, 39.9378)),
        80,
    ),
    # South Philadelphia
    _bounds(
        "Italian Market",
        "Bella Vista / Passyunk Square",
        RETAIL_CORRIDORS,
        (-75.1584, 39.9368),
        (-75.1628, 39.9296, -75.1541, 39.9430),
        100,
    ),
    _corridor(
        "Mexican Market",
        "Passyunk Square",
        FOOD_CORRIDORS,
        (-75.1596, 39.9328),
        ((-75.1587, 39.9368), (-75.1603, 39.9296)),
        95,
    ),
    _corridor(
        "East Passyunk",
        "Passyunk Square / East Passyunk Crossing",
        RETAIL_CORRIDORS,
        (-75.1626, 39.9274),
        ((-75.1566, 39.9367), (-75.1695, 39.9154)),
        135,
    ),
    _corridor(
        "Washington Avenue Food Corridor",
        "South Philadelphia",
        FOOD_CORRIDORS,
        (-75.1700, 39.9373),
        ((-75.1438, 39.9348), (-75.1918, 39.9404)),
        115,
    ),
    _bounds(
        "Little Saigon",
        "Bella Vista / Hawthorne",
        FOOD_CORRIDORS,
        (-75.1580, 39.9361),
        (-75.1640, 39.9339, -75.1516, 39.9384),
    ),
    _corridor(
        "Point Breeze Avenue",
        "Point Breeze",
        "https://www.phila.gov/media/20240311130540/Phila2035-Implementation-Update-REVISED.pdf",
        (-75.1800, 39.9307),
        ((-75.1730, 39.9370), (-75.1860, 39.9232)),
        120,
    ),
    _corridor(
        "Two Street",
        "Pennsport / Whitman",
        "https://www.visitphilly.com/media-center/press-releases/neighborhood-guide-east-passyunk-avenue-pennsport/",
        (-75.1477, 39.9237),
        ((-75.1457, 39.9350), (-75.1504, 39.9115)),
        100,
    ),
    _bounds(
        "Stadium District",
        "Stadium Complex / Packer Park",
        "https://www.visitphilly.com/areas/south-philadelphia/",
        (-75.1667, 39.9048),
        (-75.1767, 39.8970, -75.1565, 39.9122),
    ),
    _bounds(
        "Philadelphia Navy Yard",
        "Navy Yard",
        "https://navyyard.org/",
        (-75.1704, 39.8918),
        (-75.1864, 39.8790, -75.1538, 39.9040),
    ),
    # River Wards and Lower Northeast
    _corridor(
        "Northern Liberties 2nd Street",
        "Northern Liberties",
        RETAIL_CORRIDORS,
        (-75.1424, 39.9651),
        ((-75.1409, 39.9606), (-75.1436, 39.9705)),
        120,
    ),
    _corridor(
        "Fishtown Frankford Avenue",
        "Fishtown",
        RETAIL_CORRIDORS,
        (-75.1332, 39.9745),
        ((-75.1400, 39.9690), (-75.1278, 39.9815)),
        120,
        100,
    ),
    _corridor(
        "East Kensington Frankford Avenue",
        "New Kensington / East Kensington",
        CITY_CORRIDORS,
        (-75.1238, 39.9867),
        ((-75.1280, 39.9814), (-75.1168, 39.9960)),
        120,
    ),
    _corridor(
        "Front Street Corridor",
        "West Kensington / Fairhill",
        "https://www.phila.gov/media/20190604092046/HACE-2025-Neighborhood-Plan_Final-compressed.pdf",
        (-75.1356, 39.9934),
        ((-75.1378, 39.9838), (-75.1334, 40.0024)),
        115,
    ),
    _corridor(
        "El Centro de Oro",
        "Fairhill / West Kensington",
        "https://www.phila.gov/media/20190604092046/HACE-2025-Neighborhood-Plan_Final-compressed.pdf",
        (-75.1386, 39.9930),
        ((-75.1412, 39.9863), (-75.1373, 40.0000)),
        145,
    ),
    _corridor(
        "Kensington & Allegheny",
        "Kensington / Harrowgate",
        CITY_CORRIDORS,
        (-75.1148, 39.9985),
        ((-75.1224, 39.9890), (-75.1070, 40.0084)),
        155,
    ),
    _corridor(
        "Frankford Avenue El Corridor",
        "Frankford / Juniata Park",
        CITY_CORRIDORS,
        (-75.0847, 40.0161),
        ((-75.0955, 40.0071), (-75.0719, 40.0283)),
        145,
    ),
    _corridor(
        "Tacony Torresdale Avenue",
        "Wissinoming / Tacony",
        CITY_CORRIDORS,
        (-75.0513, 40.0258),
        ((-75.0665, 40.0133), (-75.0358, 40.0392)),
        140,
    ),
    # North and Northwest Philadelphia
    _corridor(
        "Fairmount Avenue",
        "Spring Garden / Fairmount",
        RETAIL_CORRIDORS,
        (-75.1738, 39.9675),
        ((-75.1605, 39.9659), (-75.1902, 39.9696)),
        125,
    ),
    _corridor(
        "Brewerytown Girard Avenue",
        "Brewerytown",
        RETAIL_CORRIDORS,
        (-75.1844, 39.9752),
        ((-75.1756, 39.9741), (-75.1915, 39.9762)),
        125,
    ),
    _corridor(
        "North Broad",
        "North Philadelphia",
        RETAIL_CORRIDORS,
        (-75.1584, 39.9880),
        ((-75.1632, 39.9571), (-75.1522, 40.0208)),
        165,
    ),
    _bounds(
        "Temple & Cecil B. Moore",
        "Yorktown / Cecil B. Moore",
        CITY_CORRIDORS,
        (-75.1588, 39.9789),
        (-75.1646, 39.9758, -75.1529, 39.9820),
    ),
    _corridor(
        "Ridge & Cecil B. Moore",
        "Sharswood / Brewerytown",
        CITY_CORRIDORS,
        (-75.1749, 39.9801),
        ((-75.1680, 39.9758), (-75.1822, 39.9850)),
        130,
    ),
    _corridor(
        "22nd & Allegheny",
        "Allegheny West",
        CITY_CORRIDORS,
        (-75.1709, 40.0030),
        ((-75.1691, 39.9977), (-75.1726, 40.0090)),
        130,
    ),
    _bounds(
        "Broad Germantown & Erie",
        "Tioga / Nicetown",
        CITY_CORRIDORS,
        (-75.1557, 40.0106),
        (-75.1639, 40.0051, -75.1474, 40.0162),
    ),
    _bounds(
        "Logan Business District",
        "Logan",
        CITY_CORRIDORS,
        (-75.1477, 40.0284),
        (-75.1555, 40.0223, -75.1397, 40.0342),
    ),
    _bounds(
        "Historic Germantown",
        "Germantown / Penn-Knox",
        RETAIL_CORRIDORS,
        (-75.1734, 40.0362),
        (-75.1818, 40.0284, -75.1649, 40.0442),
    ),
    _corridor(
        "Mt. Airy Germantown Avenue",
        "East Mount Airy / West Mount Airy",
        RETAIL_CORRIDORS,
        (-75.1917, 40.0547),
        ((-75.1810, 40.0444), (-75.2041, 40.0657)),
        145,
    ),
    _corridor(
        "Chestnut Hill Germantown Avenue",
        "Chestnut Hill",
        RETAIL_CORRIDORS,
        (-75.2103, 40.0752),
        ((-75.2038, 40.0655), (-75.2185, 40.0869)),
        145,
    ),
    _corridor(
        "Manayunk Main Street",
        "Manayunk",
        RETAIL_CORRIDORS,
        (-75.2223, 40.0265),
        ((-75.2092, 40.0212), (-75.2353, 40.0340)),
        140,
        100,
    ),
    _corridor(
        "Roxborough Ridge Avenue",
        "Roxborough",
        RETAIL_CORRIDORS,
        (-75.2238, 40.0414),
        ((-75.2152, 40.0318), (-75.2327, 40.0519)),
        140,
    ),
    _bounds(
        "East Falls River & Ridge",
        "East Falls",
        RETAIL_CORRIDORS,
        (-75.1924, 40.0117),
        (-75.1994, 40.0058, -75.1851, 40.0178),
    ),
    _corridor(
        "Ogontz Avenue",
        "West Oak Lane / Ogontz",
        CITY_CORRIDORS,
        (-75.1514, 40.0658),
        ((-75.1606, 40.0553), (-75.1421, 40.0771)),
        140,
    ),
    # West and Southwest Philadelphia
    _corridor(
        "40th Street & University Square",
        "University City / Spruce Hill",
        "https://www.universitycity.org/sites/default/files/shopping_card.pdf",
        (-75.2027, 39.9532),
        ((-75.2015, 39.9589), (-75.2042, 39.9469)),
        125,
    ),
    _corridor(
        "Baltimore Avenue",
        "Spruce Hill / Cedar Park",
        RETAIL_CORRIDORS,
        (-75.2165, 39.9486),
        ((-75.2102, 39.9507), (-75.2254, 39.9458)),
        130,
    ),
    _bounds(
        "Clark Park",
        "Spruce Hill",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/cedar-park-spruce-hill/",
        (-75.2107, 39.9494),
        (-75.2143, 39.9464, -75.2072, 39.9523),
    ),
    _corridor(
        "52nd Street",
        "Haddington / Walnut Hill / Cobbs Creek",
        RETAIL_CORRIDORS,
        (-75.2265, 39.9586),
        ((-75.2242, 39.9747), (-75.2300, 39.9413)),
        150,
    ),
    _corridor(
        "Lancaster Avenue",
        "Powelton Village / Mantua / Belmont / Haddington",
        CITY_CORRIDORS,
        (-75.2147, 39.9704),
        ((-75.1922, 39.9621), (-75.2494, 39.9841)),
        145,
    ),
    _bounds(
        "Parkside Centennial District",
        "East Parkside / West Parkside",
        "https://www.visitphilly.com/areas/west-philadelphia/",
        (-75.2107, 39.9802),
        (-75.2239, 39.9727, -75.1975, 39.9875),
    ),
    _corridor(
        "West Market Street",
        "Walnut Hill / Haddington / Mill Creek",
        CITY_CORRIDORS,
        (-75.2337, 39.9618),
        ((-75.2142, 39.9589), (-75.2540, 39.9647)),
        155,
    ),
    _bounds(
        "Africatown",
        "Cedar Park / Kingsessing / Elmwood",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/africatown/",
        (-75.2301, 39.9302),
        (-75.2778, 39.9106, -75.2116, 39.9561),
        100,
    ),
    _corridor(
        "Woodland Avenue Africatown",
        "Kingsessing / Elmwood",
        CITY_CORRIDORS,
        (-75.2301, 39.9302),
        ((-75.2246, 39.9323), (-75.2405, 39.9248)),
        150,
    ),
    # Northeast Philadelphia
    _corridor(
        "Castor Avenue",
        "Oxford Circle / Castor Gardens / Rhawnhurst",
        CITY_CORRIDORS,
        (-75.0789, 40.0475),
        ((-75.0910, 40.0300), (-75.0664, 40.0675)),
        155,
        100,
    ),
    _corridor(
        "Rising Sun Avenue",
        "Lawncrest / Burholme / Fox Chase",
        CITY_CORRIDORS,
        (-75.0921, 40.0554),
        ((-75.1017, 40.0342), (-75.0816, 40.0778)),
        150,
    ),
    _corridor(
        "North 5th & Hunting Park",
        "Hunting Park / Feltonville",
        CITY_CORRIDORS,
        (-75.1352, 40.0187),
        ((-75.1370, 40.0123), (-75.1334, 40.0250)),
        135,
    ),
    _corridor(
        "North 5th & Roosevelt Boulevard",
        "Feltonville / Olney",
        CITY_CORRIDORS,
        (-75.1323, 40.0301),
        ((-75.1334, 40.0255), (-75.1308, 40.0354)),
        135,
    ),
    _corridor(
        "North 5th & Olney",
        "Olney",
        CITY_CORRIDORS,
        (-75.1294, 40.0398),
        ((-75.1309, 40.0352), (-75.1280, 40.0445)),
        135,
    ),
    _bounds(
        "Lower Bustleton & Castor Gardens",
        "Lower Bustleton / Castor Gardens",
        "https://www.visitphilly.com/articles/philadelphia/top-things-to-do-in-june-in-philadelphia/",
        (-75.0576, 40.0635),
        (-75.0752, 40.0520, -75.0397, 40.0750),
    ),
)


class Neighborhood(TypedDict):
    name: str
    kind: Literal["planning_neighborhood", "local_area"]
    label: list[float]
    rings: list[list[list[float]]]
    source: NotRequired[str]
    note: NotRequired[str]
    priority: NotRequired[int]


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


def _local_geometry(spec: LocalAreaSpec) -> Polygon | MultiPolygon:
    raw_bounds = spec.get("bounds")
    if raw_bounds is not None:
        return box(*raw_bounds)
    points = spec.get("corridor")
    width_m = spec.get("width_m")
    if points is None or width_m is None or len(points) < 2:
        raise ValueError(f"local area {spec['name']} needs bounds or a buffered corridor")
    projected = transform(TO_LOCAL.transform, LineString(points))
    buffered = projected.buffer(width_m, cap_style="flat", join_style="round")
    geometry = transform(TO_WGS84.transform, buffered)
    if not isinstance(geometry, (Polygon, MultiPolygon)):
        raise ValueError(f"local area {spec['name']} produced invalid geometry")
    return geometry


def _build_local_areas() -> list[Neighborhood]:
    areas: list[Neighborhood] = []
    names: set[str] = set()
    for spec in LOCAL_AREAS:
        if spec["name"].casefold() in names:
            raise ValueError(f"duplicate local area: {spec['name']}")
        names.add(spec["name"].casefold())
        geometry = _local_geometry(spec)
        if geometry.is_empty or not geometry.is_valid:
            raise ValueError(f"local area {spec['name']} has invalid geometry")
        areas.append(
            {
                "name": spec["name"],
                "kind": "local_area",
                "label": [round(value, 6) for value in spec["label"]],
                "rings": _polygon_rings(geometry),
                "source": spec["source"],
                "priority": spec.get("priority", 10),
                "note": (
                    f"Approximate, non-official local area; associated planning neighborhood(s): "
                    f"{spec['parent']}."
                ),
            }
        )
    return areas


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

    neighborhoods.extend(_build_local_areas())
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
        headers={"User-Agent": "isophilly neighborhood builder"},
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
