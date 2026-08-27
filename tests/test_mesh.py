from __future__ import annotations

import struct
import unittest

from geo_philly_ingest.mesh import MeshParseError, parse_multipatch, validate_source_space
from geo_philly_ingest.models import MeshFace


def geometry(geometry_type: int, payload: bytes) -> bytes:
    return struct.pack("<BI", 1, geometry_type) + payload


def triangle(points: tuple[tuple[float, float, float], ...]) -> bytes:
    coordinates = b"".join(struct.pack("<ddd", *point) for point in points)
    return geometry(1017, struct.pack("<II", 1, len(points)) + coordinates)


class MultipatchTests(unittest.TestCase):
    def test_parses_nested_three_dimensional_faces(self) -> None:
        face = triangle(
            (
                (820_000.0, 72_000.0, 10.0),
                (820_010.0, 72_000.0, 10.0),
                (820_000.0, 72_010.0, 15.0),
                (820_000.0, 72_000.0, 10.0),
            )
        )
        tin = geometry(1016, struct.pack("<I", 1) + face)
        collection = geometry(1007, struct.pack("<I", 1) + tin)

        parsed = parse_multipatch(collection, z_min=10.0)

        self.assertEqual(
            parsed[0].points,
            (
                (820_000.0, 72_000.0, 0.0),
                (820_010.0, 72_000.0, 0.0),
                (820_000.0, 72_010.0, 5.0),
            ),
        )

    def test_rejects_two_dimensional_wkb(self) -> None:
        polygon = geometry(3, struct.pack("<II", 1, 0))

        with self.assertRaisesRegex(MeshParseError, "not three-dimensional"):
            parse_multipatch(polygon, z_min=0.0)

    def test_rejects_trailing_bytes(self) -> None:
        face = triangle(
            (
                (0.0, 0.0, 0.0),
                (1.0, 0.0, 0.0),
                (0.0, 1.0, 0.0),
                (0.0, 0.0, 0.0),
            )
        )

        with self.assertRaisesRegex(MeshParseError, "trailing bytes"):
            parse_multipatch(face + b"bad", z_min=0.0)

    def test_rejects_source_coordinates_in_feet(self) -> None:
        face = MeshFace(
            (
                (2_700_000.0, 240_000.0, 0.0),
                (2_700_001.0, 240_000.0, 1.0),
                (2_700_000.0, 240_001.0, 1.0),
            )
        )

        with self.assertRaisesRegex(MeshParseError, "X coordinate is not in metres"):
            validate_source_space(1, (face,))


if __name__ == "__main__":
    unittest.main()
