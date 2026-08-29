from __future__ import annotations

import struct
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from geo_philly_ingest.mesh import (
    MeshParseError,
    merge_mesh_sources,
    parse_geometry,
    texture_digest,
)
from geo_philly_ingest.models import BuildingMesh, MeshFace


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


def mesh(identifier: int, left: float, bottom: float, right: float, top: float) -> BuildingMesh:
    face = MeshFace(
        ((left, bottom, 0.0), (right, bottom, 0.0), (left, top, 1.0)),
        ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0)),
    )
    return BuildingMesh(
        identifier,
        1.0,
        ((left, bottom), (right, bottom), (right, top), (left, top)),
        (face,),
    )


class I3SMeshTests(unittest.TestCase):
    def test_parses_textured_triangle_in_local_metres(self) -> None:
        mesh = parse_geometry(geometry(), node())

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

    def test_higher_priority_mesh_suppresses_overlapping_lower_source(self) -> None:
        high = mesh(1, 0.0, 0.0, 10.0, 10.0)
        overlapped = mesh(1_000_001, 8.0, 2.0, 12.0, 8.0)
        separate = mesh(1_000_002, 20.0, 0.0, 24.0, 4.0)

        self.assertEqual(
            merge_mesh_sources((high,), (overlapped, separate)),
            [high, separate],
        )

    def test_touching_meshes_from_different_sources_are_both_kept(self) -> None:
        high = mesh(1, 0.0, 0.0, 10.0, 10.0)
        touching = mesh(1_000_001, 10.0, 0.0, 14.0, 4.0)

        self.assertEqual(merge_mesh_sources((high,), (touching,)), [high, touching])

    def test_texture_digest_is_independent_of_mesh_order(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            texture_dir = Path(temporary)
            first = mesh(2, 0.0, 0.0, 1.0, 1.0)
            second = mesh(1, 2.0, 0.0, 3.0, 1.0)
            (texture_dir / "1.jpg").write_bytes(b"first")
            (texture_dir / "2.jpg").write_bytes(b"second")

            with patch("geo_philly_ingest.mesh.MESH_TEXTURE_DIR", texture_dir):
                forward = texture_digest([first, second])
                reverse = texture_digest([second, first])

            self.assertEqual(forward, reverse)


if __name__ == "__main__":
    unittest.main()
