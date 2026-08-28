from __future__ import annotations

import struct
import unittest

from geo_philly_ingest.mesh import MeshParseError, parse_geometry


def node(vertex_count: int = 3) -> dict[str, object]:
    return {
        "index": 1,
        "obb": {"center": [-75.1635, 39.9526, 20.0]},
        "mesh": {
            "geometry": {"vertexCount": vertex_count, "resource": 0},
            "material": {"resource": 0},
        },
    }


def geometry() -> bytes:
    positions = (0.0, 0.0, -5.0, 0.0001, 0.0, -5.0, 0.0, 0.0001, 5.0)
    normals = (0.0, 0.0, 1.0) * 3
    uvs = (0.0, 0.0, 1.0, 0.0, 0.0, 1.0)
    colors = bytes((255, 255, 255, 255) * 3)
    regions = (0, 0, 65_535, 65_535) * 3
    return b"".join(
        (
            struct.pack("<II", 3, 1),
            struct.pack("<9f", *positions),
            struct.pack("<9f", *normals),
            struct.pack("<6f", *uvs),
            colors,
            struct.pack("<12H", *regions),
            struct.pack("<QII", 7, 0, 1),
        )
    )


class I3SMeshTests(unittest.TestCase):
    def test_parses_textured_triangle_in_local_metres(self) -> None:
        mesh = parse_geometry(geometry(), node())

        self.assertEqual(mesh.source_id, 1)
        self.assertEqual(mesh.texture_id, 0)
        self.assertEqual(mesh.height, 10.0)
        self.assertEqual(mesh.faces[0].uvs, ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0)))
        self.assertGreater(mesh.faces[0].points[1][0], mesh.faces[0].points[0][0])
        self.assertGreater(mesh.faces[0].points[2][1], mesh.faces[0].points[0][1])

    def test_rejects_non_triangular_vertex_count(self) -> None:
        data = bytearray(geometry())
        struct.pack_into("<I", data, 0, 4)

        with self.assertRaisesRegex(MeshParseError, "triangulated"):
            parse_geometry(bytes(data), node(4))

    def test_rejects_truncated_geometry(self) -> None:
        with self.assertRaisesRegex(MeshParseError, "bytes"):
            parse_geometry(geometry()[:-1], node())


if __name__ == "__main__":
    unittest.main()
