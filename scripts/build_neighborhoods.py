"""Build the small, browser-ready neighborhood overlay from the PCPC layer."""

from __future__ import annotations

import argparse
import hashlib
import json
from collections.abc import Mapping
from pathlib import Path
from tempfile import NamedTemporaryFile
from typing import Final, Literal, NotRequired, TypedDict

import httpx
from pyproj import Transformer
from shapely.geometry import LinearRing, LineString, MultiPolygon, Point, Polygon, box, shape
from shapely.geometry.base import BaseGeometry
from shapely.ops import transform, unary_union

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
EXPECTED_PLANNING_COUNT: Final = 148
EXPECTED_PLANNING_NAMES_SHA256: Final = (
    "665f38fb70bd8bfe9d21b42d33cf189aa1cbbfd439715b21b12b02fd71b90bf7"
)
EXPECTED_PLANNING_PAYLOAD_SHA256: Final = (
    "b1be4984748a73c289fb80986db857cf179a541f7a075f117c3fd86126f98c8d"
)
MIN_VISIBLE_AREAS_PER_REGION: Final = 2

PLANNING_PARENT_ALIASES: Final[dict[str, tuple[str, ...]]] = {
    "Cecil B. Moore": ("Cecil B Moore",),
    "East Mount Airy": ("East Mt. Airy",),
    "East Passyunk Crossing": ("South Philadelphia",),
    "Elmwood": ("Southwest",),
    "Lower Bustleton": ("Bustleton",),
    "New Kensington": ("East Kensington",),
    "North Philadelphia": ("North Phila.",),
    "Passyunk Square": ("South Philadelphia",),
    "Penn-Knox": ("Germantown",),
    "Roxborough": ("Central Roxborough",),
    "Stadium Complex": ("South Philadelphia",),
    "West Mount Airy": ("West Mt. Airy",),
}


class PairPolicy(TypedDict):
    winner: str
    rationale: str


REVIEWED_PAIR_POLICIES: Final[dict[frozenset[str], PairPolicy]] = {
    frozenset(("Africatown", "Woodland Avenue Africatown")): {
        "winner": "Africatown",
        "rationale": "The broader cultural identity wins over its named commercial corridor.",
    },
    frozenset(("East Passyunk", "Italian Market")): {
        "winner": "Italian Market",
        "rationale": "The historic market wins where the two South Philadelphia districts meet.",
    },
    frozenset(("East Passyunk", "Mexican Market")): {
        "winner": "Mexican Market",
        "rationale": (
            "The smaller cultural market wins where it lies inside the longer avenue corridor."
        ),
    },
    frozenset(("Gayborhood", "Market East Retail District")): {
        "winner": "Gayborhood",
        "rationale": "The cultural district wins over the broad retail district.",
    },
    frozenset(("Gayborhood", "Midtown Village")): {
        "winner": "Gayborhood",
        "rationale": "The cultural district wins where the two local identities overlap.",
    },
    frozenset(("Italian Market", "Little Saigon")): {
        "winner": "Italian Market",
        "rationale": (
            "The launch market label wins while Little Saigon remains at the next zoom tier."
        ),
    },
    frozenset(("Italian Market", "Mexican Market")): {
        "winner": "Italian Market",
        "rationale": (
            "The broader market wins while the Mexican market remains at the closest tier."
        ),
    },
    frozenset(("Little Saigon", "Mexican Market")): {
        "winner": "Mexican Market",
        "rationale": "The smaller market wins where the two cultural areas cross.",
    },
    frozenset(("Little Saigon", "Washington Avenue Food Corridor")): {
        "winner": "Little Saigon",
        "rationale": "The cultural district wins over the longer food corridor.",
    },
    frozenset(("Market East Retail District", "Midtown Village")): {
        "winner": "Midtown Village",
        "rationale": "The named local district wins over the broader retail district.",
    },
    frozenset(("52nd Street", "Africatown")): {
        "winner": "Africatown",
        "rationale": (
            "Africatown is the broader cultural identity. 52nd Street remains at closer views."
        ),
    },
    frozenset(("Africatown", "Baltimore Avenue")): {
        "winner": "Africatown",
        "rationale": (
            "Africatown is the broader cultural identity. Baltimore Avenue keeps its corridor "
            "shape."
        ),
    },
    frozenset(("Africatown", "Clark Park")): {
        "winner": "Africatown",
        "rationale": (
            "Africatown is the broader cultural identity. Clark Park remains a lower priority "
            "destination."
        ),
    },
    frozenset(("Antique Row", "Avenue of the Arts")): {
        "winner": "Avenue of the Arts",
        "rationale": "The longer arts corridor wins where its narrow edge crosses Antique Row.",
    },
    frozenset(("Antique Row", "Gayborhood")): {
        "winner": "Gayborhood",
        "rationale": "The cultural district wins over the smaller retail corridor.",
    },
    frozenset(("Avenue of the Arts", "Rittenhouse Row")): {
        "winner": "Rittenhouse Row",
        "rationale": "Rittenhouse Row wins at its western crossing with the arts corridor.",
    },
    frozenset(("Baltimore Avenue", "Clark Park")): {
        "winner": "Baltimore Avenue",
        "rationale": "The commercial corridor wins over the park destination at their edge.",
    },
    frozenset(("Castor Avenue", "Lower Bustleton & Castor Gardens")): {
        "winner": "Castor Avenue",
        "rationale": "The reviewed launch corridor wins over the broader local district.",
    },
    frozenset(("East Kensington Frankford Avenue", "Kensington & Allegheny")): {
        "winner": "Kensington & Allegheny",
        "rationale": "The major intersection wins where the two commercial corridors meet.",
    },
    frozenset(("Fabric Row", "South Street Headhouse")): {
        "winner": "South Street Headhouse",
        "rationale": "The main South Street corridor wins over the smaller Fabric Row segment.",
    },
    frozenset(("Independence Mall", "Jewelers' Row")): {
        "winner": "Independence Mall",
        "rationale": "The civic destination wins over the narrow retail row.",
    },
    frozenset(("Independence Mall", "Market East Retail District")): {
        "winner": "Independence Mall",
        "rationale": "The civic destination wins where the broad retail district overlaps it.",
    },
    frozenset(("Independence Mall", "Old City Arts District")): {
        "winner": "Old City Arts District",
        "rationale": "The named arts district wins over the contained civic destination.",
    },
    frozenset(("Jewelers' Row", "Market East Retail District")): {
        "winner": "Market East Retail District",
        "rationale": (
            "The broader retail district wins while Jewelers' Row remains at the closest tier."
        ),
    },
    frozenset(("Market East Retail District", "Reading Terminal & Convention Center")): {
        "winner": "Market East Retail District",
        "rationale": "The district wins while the destination label remains at the closest tier.",
    },
    frozenset(("North Broad", "Temple & Cecil B. Moore")): {
        "winner": "North Broad",
        "rationale": "The major corridor wins over the campus intersection area.",
    },
    frozenset(("Old City Arts District", "Penn's Landing")): {
        "winner": "Old City Arts District",
        "rationale": "The arts district wins where the waterfront corridor reaches Old City.",
    },
    frozenset(("Philadelphia Navy Yard", "Stadium District")): {
        "winner": "Stadium District",
        "rationale": "The public stadium destination wins over the adjacent employment campus.",
    },
}

type RelevanceClass = Literal[
    "cultural_district",
    "commercial_corridor",
    "arts_entertainment",
    "civic_destination",
    "mixed_local_area",
]


class LocalAreaSpec(TypedDict):
    name: str
    parent: str
    source: str
    label: tuple[float, float]
    bounds: NotRequired[tuple[float, float, float, float]]
    corridor: NotRequired[tuple[tuple[float, float], ...]]
    width_m: NotRequired[float]
    priority: NotRequired[int]
    display: bool
    display_label: str
    display_tier: Literal[1, 2, 3]
    draw_geometry: bool
    relevance: RelevanceClass
    rationale: str
    suppresses: tuple[str, ...]
    overlap_group: NotRequired[str]


def _bounds(
    name: str,
    parent: str,
    source: str,
    label: tuple[float, float],
    bounds: tuple[float, float, float, float],
    priority: int = 10,
    *,
    display_label: str | None = None,
    display_tier: Literal[1, 2, 3] = 2,
    relevance: RelevanceClass = "mixed_local_area",
    suppresses: tuple[str, ...] = (),
    overlap_group: str | None = None,
    draw_geometry: bool | None = None,
) -> LocalAreaSpec:
    spec: LocalAreaSpec = {
        "name": name,
        "parent": parent,
        "source": source,
        "label": label,
        "bounds": bounds,
        "priority": priority,
        "display": True,
        "display_label": display_label or name,
        "display_tier": display_tier,
        "draw_geometry": display_tier < 3 if draw_geometry is None else draw_geometry,
        "relevance": relevance,
        "rationale": (
            "Included because a public source names this cultural, commercial, or civic local area."
        ),
        "suppresses": suppresses,
    }
    if overlap_group is not None:
        spec["overlap_group"] = overlap_group
    return spec


def _corridor(
    name: str,
    parent: str,
    source: str,
    label: tuple[float, float],
    points: tuple[tuple[float, float], ...],
    width_m: float = 115,
    priority: int = 10,
    *,
    display_label: str | None = None,
    display_tier: Literal[1, 2, 3] = 2,
    relevance: RelevanceClass = "commercial_corridor",
    suppresses: tuple[str, ...] = (),
    overlap_group: str | None = None,
    draw_geometry: bool | None = None,
) -> LocalAreaSpec:
    spec: LocalAreaSpec = {
        "name": name,
        "parent": parent,
        "source": source,
        "label": label,
        "corridor": points,
        "width_m": width_m,
        "priority": priority,
        "display": True,
        "display_label": display_label or name,
        "display_tier": display_tier,
        "draw_geometry": display_tier < 3 if draw_geometry is None else draw_geometry,
        "relevance": relevance,
        "rationale": (
            "Included because a public source names this cultural, commercial, or civic local area."
        ),
        "suppresses": suppresses,
    }
    if overlap_group is not None:
        spec["overlap_group"] = overlap_group
    return spec


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
        90,
        relevance="cultural_district",
        suppresses=("Washington Square West",),
        overlap_group="washington-square-west",
    ),
    _bounds(
        "Midtown Village",
        "Washington Square West / Market East",
        RETAIL_CORRIDORS,
        (-75.1616, 39.9499),
        (-75.1643, 39.9452, -75.1582, 39.9534),
        60,
        suppresses=("Washington Square West", "Market East"),
        overlap_group="washington-square-west",
    ),
    _corridor(
        "Jewelers' Row",
        "Washington Square West",
        RETAIL_CORRIDORS,
        (-75.1539, 39.9488),
        ((-75.1561, 39.9491), (-75.1518, 39.9485)),
        70,
        25,
        display_tier=3,
    ),
    _corridor(
        "Antique Row",
        "Washington Square West",
        "https://www.visitphilly.com/articles/philadelphia/streets-alleys/",
        (-75.1620, 39.9450),
        ((-75.1670, 39.9456), (-75.1574, 39.9444)),
        80,
        30,
    ),
    _bounds(
        "Market East Retail District",
        "Market East",
        RETAIL_CORRIDORS,
        (-75.1570, 39.9521),
        (-75.1644, 39.9466, -75.1500, 39.9569),
        35,
        overlap_group="washington-square-west",
    ),
    _bounds(
        "Reading Terminal & Convention Center",
        "Market East / Chinatown",
        RETAIL_CORRIDORS,
        (-75.1592, 39.9547),
        (-75.1640, 39.9526, -75.1531, 39.9582),
        10,
        display_label="Reading Terminal Market / Convention Center",
        display_tier=3,
        relevance="civic_destination",
    ),
    _bounds(
        "Rittenhouse Row",
        "Rittenhouse Square",
        RETAIL_CORRIDORS,
        (-75.1715, 39.9506),
        (-75.1810, 39.9451, -75.1642, 39.9558),
        65,
    ),
    _corridor(
        "Avenue of the Arts",
        "Washington Square West / Rittenhouse Square",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/avenue-of-the-arts/",
        (-75.1649, 39.9467),
        ((-75.1635, 39.9536), (-75.1672, 39.9392)),
        105,
        55,
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
        70,
        suppresses=("Old City",),
    ),
    _bounds(
        "Independence Mall",
        "Old City / Society Hill",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/old-city/",
        (-75.1498, 39.9507),
        (-75.1531, 39.9467, -75.1468, 39.9554),
        50,
        relevance="civic_destination",
    ),
    _corridor(
        "Penn's Landing",
        "Old City / Society Hill",
        "https://www.delawareriverwaterfront.com/",
        (-75.1399, 39.9467),
        ((-75.1400, 39.9550), (-75.1433, 39.9378)),
        160,
        10,
    ),
    _corridor(
        "South Street Headhouse",
        "Queen Village / Society Hill",
        RETAIL_CORRIDORS,
        (-75.1495, 39.9415),
        ((-75.1430, 39.9423), (-75.1565, 39.9407)),
        125,
        50,
    ),
    _corridor(
        "Fabric Row",
        "Queen Village / Bella Vista",
        "https://www.visitphilly.com/things-to-do/attractions/fabric-row/",
        (-75.1495, 39.9406),
        ((-75.1486, 39.9433), (-75.1501, 39.9378)),
        80,
        20,
        display_tier=3,
    ),
    # South Philadelphia
    _bounds(
        "Italian Market",
        "Bella Vista / Passyunk Square",
        RETAIL_CORRIDORS,
        (-75.1584, 39.9368),
        (-75.1628, 39.9296, -75.1541, 39.9430),
        100,
        display_tier=1,
        relevance="cultural_district",
        suppresses=("Bella Vista", "South Philadelphia"),
        overlap_group="italian-market",
    ),
    _corridor(
        "Mexican Market",
        "Passyunk Square",
        FOOD_CORRIDORS,
        (-75.1596, 39.9328),
        ((-75.1587, 39.9368), (-75.1603, 39.9296)),
        95,
        95,
        display_label="Mexican 9th Street",
        display_tier=3,
        relevance="cultural_district",
        overlap_group="italian-market",
    ),
    _corridor(
        "East Passyunk",
        "Passyunk Square / East Passyunk Crossing",
        RETAIL_CORRIDORS,
        (-75.1626, 39.9274),
        ((-75.1566, 39.9367), (-75.1695, 39.9154)),
        135,
        80,
        overlap_group="italian-market",
    ),
    _corridor(
        "Washington Avenue Food Corridor",
        "South Philadelphia",
        FOOD_CORRIDORS,
        (-75.1700, 39.9373),
        ((-75.1438, 39.9348), (-75.1918, 39.9404)),
        115,
        45,
        overlap_group="italian-market",
    ),
    _bounds(
        "Little Saigon",
        "Bella Vista / Hawthorne",
        FOOD_CORRIDORS,
        (-75.1580, 39.9361),
        (-75.1640, 39.9339, -75.1516, 39.9384),
        90,
        relevance="cultural_district",
        overlap_group="italian-market",
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
        50,
    ),
    _bounds(
        "Philadelphia Navy Yard",
        "Navy Yard",
        "https://navyyard.org/",
        (-75.1704, 39.8918),
        (-75.1864, 39.8790, -75.1538, 39.9040),
        30,
    ),
    # River Wards and Lower Northeast
    _corridor(
        "Northern Liberties 2nd Street",
        "Northern Liberties",
        RETAIL_CORRIDORS,
        (-75.1424, 39.9651),
        ((-75.1409, 39.9606), (-75.1436, 39.9705)),
        120,
        display_label="2nd Street / Northern Liberties",
    ),
    _corridor(
        "Fishtown Frankford Avenue",
        "Fishtown",
        RETAIL_CORRIDORS,
        (-75.1332, 39.9745),
        ((-75.1400, 39.9690), (-75.1278, 39.9815)),
        120,
        100,
        display_label="Frankford Avenue Arts Corridor",
        display_tier=1,
        relevance="arts_entertainment",
        suppresses=("Fishtown", "East Kensington", "Kensington"),
    ),
    _corridor(
        "East Kensington Frankford Avenue",
        "New Kensington / East Kensington",
        CITY_CORRIDORS,
        (-75.1238, 39.9867),
        ((-75.1280, 39.9814), (-75.1168, 39.9960)),
        120,
        40,
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
        55,
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
        60,
    ),
    _bounds(
        "Temple & Cecil B. Moore",
        "Yorktown / Cecil B. Moore",
        CITY_CORRIDORS,
        (-75.1588, 39.9789),
        (-75.1646, 39.9758, -75.1529, 39.9820),
        20,
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
        relevance="cultural_district",
    ),
    _corridor(
        "Mt. Airy Germantown Avenue",
        "East Mount Airy / West Mount Airy",
        RETAIL_CORRIDORS,
        (-75.1917, 40.0547),
        ((-75.1810, 40.0444), (-75.2041, 40.0657)),
        145,
        display_label="Mt. Airy Village",
    ),
    _corridor(
        "Chestnut Hill Germantown Avenue",
        "Chestnut Hill",
        RETAIL_CORRIDORS,
        (-75.2103, 40.0752),
        ((-75.2038, 40.0655), (-75.2185, 40.0869)),
        145,
        display_label="Chestnut Hill Village",
    ),
    _corridor(
        "Manayunk Main Street",
        "Manayunk",
        RETAIL_CORRIDORS,
        (-75.2223, 40.0265),
        ((-75.2092, 40.0212), (-75.2353, 40.0340)),
        140,
        100,
        display_label="Main Street Manayunk",
        display_tier=1,
        suppresses=("Manayunk",),
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
        50,
    ),
    _bounds(
        "Clark Park",
        "Spruce Hill",
        "https://www.visitphilly.com/areas/philadelphia-neighborhoods/cedar-park-spruce-hill/",
        (-75.2107, 39.9494),
        (-75.2143, 39.9464, -75.2072, 39.9523),
        20,
    ),
    _corridor(
        "52nd Street",
        "Haddington / Walnut Hill / Cobbs Creek",
        RETAIL_CORRIDORS,
        (-75.2265, 39.9586),
        ((-75.2242, 39.9747), (-75.2300, 39.9413)),
        150,
        50,
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
        (-75.2745, 39.9106, -75.2116, 39.9561),
        100,
        display_tier=1,
        relevance="cultural_district",
        suppresses=("Cedar Park", "Kingsessing"),
        overlap_group="africatown",
        draw_geometry=False,
    ),
    _corridor(
        "Woodland Avenue Africatown",
        "Kingsessing / Elmwood",
        CITY_CORRIDORS,
        (-75.2301, 39.9302),
        ((-75.2246, 39.9323), (-75.2405, 39.9248)),
        150,
        70,
        display_label="Woodland Avenue",
        relevance="cultural_district",
        overlap_group="africatown",
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
        display_tier=1,
        relevance="cultural_district",
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
    display: NotRequired[bool]
    display_label: NotRequired[str]
    display_tier: NotRequired[Literal[1, 2, 3]]
    draw_geometry: NotRequired[bool]
    relevance: NotRequired[RelevanceClass]
    rationale: NotRequired[str]
    associations: NotRequired[list[str]]
    planning_parents: NotRequired[list[str]]
    suppresses: NotRequired[list[str]]
    overlap_group: NotRequired[str]


class NeighborhoodCollection(TypedDict):
    source: str
    disclaimer: str
    features: list[Neighborhood]


class AuditCounts(TypedDict):
    planning_neighborhoods: int
    local_areas: int
    displayed_local_areas: int


class PinnedSourceAudit(TypedDict):
    planning_count: int
    planning_names_sha256: str
    planning_payload_sha256: str


class OverlapFinding(TypedDict):
    areas: list[str]
    known_group: str | None
    reviewed_policy: PairPolicy | None
    ratio: float


class NearLabelFinding(TypedDict):
    areas: list[str]
    distance_m: float
    known_group: str | None
    reviewed_policy: PairPolicy | None


class NeighborhoodAudit(TypedDict):
    schema_version: int
    ok: bool
    failures: list[str]
    pinned_source: PinnedSourceAudit
    counts: AuditCounts
    city_bounds: list[float]
    visible_by_region: dict[str, int]
    overlaps_at_least_25_percent: list[OverlapFinding]
    label_anchors_within_150m: list[NearLabelFinding]


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


def _planning_parents(associations: list[str]) -> list[str]:
    parents: list[str] = []
    for association in associations:
        for parent in PLANNING_PARENT_ALIASES.get(association, (association,)):
            if parent not in parents:
                parents.append(parent)
    return parents


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
        associations = spec["parent"].split(" / ")
        areas.append(
            {
                "name": spec["name"],
                "kind": "local_area",
                "label": [round(value, 6) for value in spec["label"]],
                "rings": _polygon_rings(geometry),
                "source": spec["source"],
                "priority": spec.get("priority", 10),
                "display": spec["display"],
                "display_label": spec["display_label"],
                "display_tier": spec["display_tier"],
                "draw_geometry": spec["draw_geometry"],
                "relevance": spec["relevance"],
                "rationale": spec["rationale"],
                "associations": associations,
                "planning_parents": _planning_parents(associations),
                "suppresses": list(spec["suppresses"]),
                "note": (
                    f"Approximate, non-official local area; associated planning neighborhood(s): "
                    f"{spec['parent']}."
                ),
                **({"overlap_group": spec["overlap_group"]} if "overlap_group" in spec else {}),
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


def _write_json_atomic(path: Path, value: object, *, indent: int | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with NamedTemporaryFile("w", dir=path.parent, prefix=f".{path.name}.", delete=False) as file:
        temporary = Path(file.name)
        json.dump(
            value,
            file,
            indent=indent,
            sort_keys=True,
            separators=None if indent is not None else (",", ":"),
        )
        file.write("\n")
    temporary.replace(path)


def _feature_geometry(feature: Mapping[str, object]) -> Polygon | MultiPolygon:
    raw_rings = feature.get("rings")
    if not isinstance(raw_rings, list) or not raw_rings:
        raise ValueError(f"{feature.get('name', '<unnamed>')} has no rings")
    polygons: list[Polygon] = []
    for raw_ring in raw_rings:
        if not isinstance(raw_ring, list):
            raise ValueError(f"{feature.get('name', '<unnamed>')} has an invalid ring")
        polygon = Polygon(raw_ring)
        if polygon.is_empty or not polygon.is_valid:
            raise ValueError(f"{feature.get('name', '<unnamed>')} has invalid geometry")
        polygons.append(polygon)
    merged = unary_union(polygons)
    if not isinstance(merged, (Polygon, MultiPolygon)):
        raise ValueError(f"{feature.get('name', '<unnamed>')} has invalid merged geometry")
    return merged


def _numeric_label(value: object, name: str) -> tuple[float, float]:
    if (
        not isinstance(value, list)
        or len(value) != 2
        or not all(isinstance(item, (int, float)) and not isinstance(item, bool) for item in value)
    ):
        raise ValueError(f"{name} has an invalid label")
    return float(value[0]), float(value[1])


def _region(label: tuple[float, float]) -> str:
    longitude, latitude = label
    if latitude < 39.94 and longitude > -75.20:
        return "south"
    if latitude < 39.97 and longitude > -75.20:
        return "center_city"
    if longitude <= -75.195:
        return "west_southwest"
    if longitude > -75.13 and latitude < 40.03:
        return "river_wards_lower_northeast"
    if longitude > -75.13:
        return "northeast"
    return "north_northwest"


def _priority(feature: Mapping[str, object]) -> int:
    value = feature.get("priority")
    return value if isinstance(value, int) and not isinstance(value, bool) else 0


def audit(collection: Mapping[str, object]) -> NeighborhoodAudit:
    raw_features = collection.get("features")
    if not isinstance(raw_features, list):
        raise ValueError("neighborhood collection has no features")
    features = [_mapping(value, "neighborhood feature") for value in raw_features]
    planning = [feature for feature in features if feature.get("kind") == "planning_neighborhood"]
    local = [feature for feature in features if feature.get("kind") == "local_area"]
    failures: list[str] = []

    names = [str(feature.get("name", "")) for feature in features]
    folded = [name.casefold() for name in names]
    if len(folded) != len(set(folded)):
        failures.append("feature names are not unique")

    planning_names = sorted(str(feature.get("name", "")) for feature in planning)
    names_sha256 = hashlib.sha256(("\n".join(planning_names) + "\n").encode()).hexdigest()
    planning_payload = sorted(planning, key=lambda feature: str(feature.get("name", "")))
    payload_sha256 = hashlib.sha256(
        json.dumps(planning_payload, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    if len(planning) != EXPECTED_PLANNING_COUNT:
        failures.append(
            f"planning count {len(planning)} does not match pinned {EXPECTED_PLANNING_COUNT}"
        )
    if names_sha256 != EXPECTED_PLANNING_NAMES_SHA256:
        failures.append("planning name set does not match the pinned local source snapshot")
    if payload_sha256 != EXPECTED_PLANNING_PAYLOAD_SHA256:
        failures.append("planning geometry payload does not match the pinned local source snapshot")

    registry_names = {spec["name"] for spec in LOCAL_AREAS}
    local_names = {str(feature.get("name", "")) for feature in local}
    if local_names != registry_names or len(local) != len(LOCAL_AREAS):
        failures.append("generated local areas and Python registry are not a bijection")
    registry_features = {feature["name"]: feature for feature in _build_local_areas()}
    for feature in local:
        name = str(feature.get("name", ""))
        expected = registry_features.get(name)
        if expected is None:
            continue
        if feature != expected:
            failures.append(f"{name} full generated record differs from the Python registry")

    planning_geometries = [_feature_geometry(feature) for feature in planning]
    city_bounds = unary_union(planning_geometries).bounds
    region_counts: dict[str, int] = {}
    local_geometries: dict[str, Polygon | MultiPolygon] = {}
    label_points: dict[str, Point] = {}
    planning_name_set = set(planning_names)
    for feature in local:
        name = str(feature.get("name", ""))
        display = feature.get("display")
        label = feature.get("label")
        associations = feature.get("associations")
        planning_parents = feature.get("planning_parents")
        suppresses = feature.get("suppresses")
        if display is not True:
            failures.append(f"{name} has no explicit displayed decision")
        try:
            longitude, latitude = _numeric_label(label, name)
        except ValueError:
            failures.append(f"{name} has an invalid label")
            continue
        if (
            not isinstance(associations, list)
            or not associations
            or not all(isinstance(association, str) and association for association in associations)
        ):
            failures.append(f"{name} has invalid contextual associations")
        if (
            not isinstance(planning_parents, list)
            or not planning_parents
            or not all(
                isinstance(parent, str) and parent in planning_name_set
                for parent in planning_parents
            )
        ):
            failures.append(f"{name} has an unknown canonical planning parent")
        if not isinstance(suppresses, list) or not all(
            isinstance(parent, str) and parent in planning_name_set for parent in suppresses
        ):
            failures.append(f"{name} suppresses an unknown planning neighborhood")
        if not isinstance(feature.get("display_label"), str):
            failures.append(f"{name} has no display label")
        if feature.get("display_tier") not in {1, 2, 3}:
            failures.append(f"{name} has no valid display tier")
        if not isinstance(feature.get("draw_geometry"), bool):
            failures.append(f"{name} has no explicit geometry display decision")
        if not isinstance(feature.get("rationale"), str) or not feature.get("rationale"):
            failures.append(f"{name} has no inclusion rationale")
        if not (city_bounds[0] <= longitude <= city_bounds[2]) or not (
            city_bounds[1] <= latitude <= city_bounds[3]
        ):
            failures.append(f"{name} label is outside the pinned city bounds")
        region = _region((longitude, latitude))
        region_counts[region] = region_counts.get(region, 0) + 1
        local_geometry = _feature_geometry(feature)
        local_bounds = local_geometry.bounds
        if (
            local_bounds[0] < city_bounds[0]
            or local_bounds[1] < city_bounds[1]
            or local_bounds[2] > city_bounds[2]
            or local_bounds[3] > city_bounds[3]
        ):
            failures.append(f"{name} geometry exceeds the pinned city bounds")
        local_geometries[name] = transform(TO_LOCAL.transform, local_geometry)
        label_points[name] = transform(TO_LOCAL.transform, Point(longitude, latitude))

    for region in (
        "center_city",
        "south",
        "west_southwest",
        "north_northwest",
        "river_wards_lower_northeast",
        "northeast",
    ):
        if region_counts.get(region, 0) < MIN_VISIBLE_AREAS_PER_REGION:
            failures.append(f"{region} has fewer than {MIN_VISIBLE_AREAS_PER_REGION} visible areas")

    overlaps: list[OverlapFinding] = []
    near_labels: list[NearLabelFinding] = []
    sorted_names = sorted(local_geometries)
    by_name = {str(feature.get("name", "")): feature for feature in local}
    for pair, policy in REVIEWED_PAIR_POLICIES.items():
        if (
            len(pair) != 2
            or not pair <= local_names
            or policy["winner"] not in pair
            or not isinstance(policy.get("rationale"), str)
            or not policy["rationale"].strip()
        ):
            failures.append(f"invalid reviewed overlap policy for {sorted(pair)}")
            continue
        loser = next(name for name in pair if name != policy["winner"])
        winner_priority = _priority(by_name[policy["winner"]])
        loser_priority = _priority(by_name[loser])
        if winner_priority <= loser_priority:
            failures.append(
                f"reviewed winner {policy['winner']} does not outrank {loser} for label collision"
            )
    for index, left_name in enumerate(sorted_names):
        left = local_geometries[left_name]
        for right_name in sorted_names[index + 1 :]:
            right = local_geometries[right_name]
            intersection = left.intersection(right).area
            overlap_ratio = intersection / min(left.area, right.area)
            pair = frozenset((left_name, right_name))
            left_group = by_name[left_name].get("overlap_group")
            right_group = by_name[right_name].get("overlap_group")
            known_group = (
                left_group if isinstance(left_group, str) and left_group == right_group else None
            )
            reviewed_policy = REVIEWED_PAIR_POLICIES.get(pair)
            if overlap_ratio >= 0.25:
                overlaps.append(
                    {
                        "areas": [left_name, right_name],
                        "known_group": known_group,
                        "reviewed_policy": reviewed_policy,
                        "ratio": round(overlap_ratio, 4),
                    }
                )
                if reviewed_policy is None:
                    failures.append(
                        f"unreviewed >=25% overlap between {left_name} and {right_name}"
                    )
            distance = label_points[left_name].distance(label_points[right_name])
            if distance <= 150:
                near_labels.append(
                    {
                        "areas": [left_name, right_name],
                        "distance_m": round(distance, 1),
                        "known_group": known_group,
                        "reviewed_policy": reviewed_policy,
                    }
                )
                if reviewed_policy is None:
                    failures.append(
                        f"unreviewed <=150m label anchors for {left_name} and {right_name}"
                    )

    return {
        "schema_version": 2,
        "ok": not failures,
        "failures": failures,
        "pinned_source": {
            "planning_count": len(planning),
            "planning_names_sha256": names_sha256,
            "planning_payload_sha256": payload_sha256,
        },
        "counts": {
            "planning_neighborhoods": len(planning),
            "local_areas": len(local),
            "displayed_local_areas": sum(feature.get("display") is True for feature in local),
        },
        "city_bounds": [round(value, 6) for value in city_bounds],
        "visible_by_region": dict(sorted(region_counts.items())),
        "overlaps_at_least_25_percent": overlaps,
        "label_anchors_within_150m": near_labels,
    }


def publish_build(collection: NeighborhoodCollection, output: Path) -> None:
    report = audit(collection)
    if not report["ok"]:
        details = "\n".join(f"- {failure}" for failure in report["failures"])
        raise ValueError(f"refusing to replace {output}; audit failed:\n{details}")
    _write_json_atomic(output, collection)


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
    parser.add_argument(
        "--audit",
        action="store_true",
        help="Audit the checked-in overlay without network access or rebuilding it",
    )
    parser.add_argument(
        "--audit-output",
        type=Path,
        default=Path("artifacts/neighborhood-audit.json"),
    )
    arguments = parser.parse_args()
    if arguments.audit:
        raw_collection: object = json.loads(arguments.output.read_text())
        collection = _mapping(raw_collection, "neighborhood collection")
        report = audit(collection)
        _write_json_atomic(arguments.audit_output, report, indent=2)
        print(
            f"wrote {arguments.audit_output} "
            f"({report['counts']['planning_neighborhoods']} planning, "
            f"{report['counts']['displayed_local_areas']} displayed local areas)"
        )
        if not report["ok"]:
            raise SystemExit("neighborhood audit failed; inspect the report")
        return
    raw: object = json.loads(arguments.input.read_text()) if arguments.input else download()
    source = _mapping(raw, "PCPC response")
    output = build(source)
    publish_build(output, arguments.output)
    print(f"wrote {arguments.output} ({len(output['features'])} areas)")


if __name__ == "__main__":
    main()
