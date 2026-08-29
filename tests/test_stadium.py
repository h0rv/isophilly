from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

from geo_philly_ingest.stadium import StadiumParseError, _load_models

JPEG = b"\xff\xd8fixture\xff\xd9"


def _top_kml(*, west: float = -75.1700) -> str:
    return f"""<?xml version="1.0"?>
<kml xmlns="http://www.opengis.net/kml/2.2"><Document><Region><LatLonAltBox>
<north>39.9100</north><south>39.9000</south><east>-75.1500</east><west>{west}</west>
</LatLonAltBox></Region></Document></kml>"""


PLACEMENT_KML = """<?xml version="1.0"?>
<kml xmlns="http://www.opengis.net/kml/2.2"><Placemark>
<name>ph_stadium0001</name><Model><altitudeMode>clampToGround</altitudeMode>
<Location><longitude>-75.1600</longitude><latitude>39.9050</latitude><altitude>0</altitude></Location>
<Orientation><heading>0</heading><tilt>0</tilt><roll>0</roll></Orientation>
<Scale><x>1</x><y>1</y><z>1</z></Scale>
<Link><href>ph_stadium0001.dae</href></Link></Model></Placemark></kml>"""


def _dae(*, texcoord: bool = True) -> str:
    texture_input = '<input semantic="TEXCOORD" source="#uv" offset="1"/>' if texcoord else ""
    indices = "0 0 1 1 2 2" if texcoord else "0 1 2"
    return f"""<?xml version="1.0"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema">
<asset><unit meter="1"/><up_axis>Z_UP</up_axis></asset>
<library_images><image id="image"><init_from>tmaps/p1.jpg</init_from></image></library_images>
<library_effects><effect id="photo-effect"><profile_COMMON><technique sid="common"><lambert>
<diffuse><texture texture="image" texcoord="UVSET0"/></diffuse>
</lambert></technique></profile_COMMON></effect></library_effects>
<library_materials><material id="photo">
<instance_effect url="#photo-effect"/></material></library_materials>
<library_geometries><geometry id="geometry"><mesh>
<source id="position"><float_array count="9">0 0 0 10 0 0 0 10 10</float_array>
<technique_common><accessor count="3" stride="3"/></technique_common></source>
<source id="uv"><float_array count="6">0 0 1 0 0 .75</float_array>
<technique_common><accessor count="3" stride="2"/></technique_common></source>
<vertices id="vertices"><input semantic="POSITION" source="#position"/></vertices>
<triangles material="photo-symbol" count="1">
<input semantic="VERTEX" source="#vertices" offset="0"/>
{texture_input}<p>{indices}</p></triangles>
</mesh></geometry></library_geometries>
<library_visual_scenes><visual_scene id="scene"><node>
<instance_geometry url="#geometry"><bind_material>
<technique_common><instance_material symbol="photo-symbol" target="#photo"/></technique_common>
</bind_material></instance_geometry></node></visual_scene></library_visual_scenes>
</COLLADA>"""


def _archive(
    path: Path,
    *,
    texcoord: bool = True,
    west: float = -75.1700,
    model_name: str = "ph_stadium0001",
) -> None:
    inner_path = path.with_suffix(".inner.zip")
    with ZipFile(inner_path, "w", ZIP_DEFLATED) as inner:
        inner.writestr(f"kml/{model_name}.kml", _top_kml(west=west))
        inner.writestr(
            f"kml/r0/{model_name}.kml", PLACEMENT_KML.replace("ph_stadium0001", model_name)
        )
        inner.writestr(f"kml/r0/{model_name}.dae", _dae(texcoord=texcoord))
        inner.writestr("kml/r0/tmaps/p1.jpg", JPEG)
    with ZipFile(path, "w", ZIP_DEFLATED) as outer:
        outer.write(inner_path, "Stadium Area Processed w LiDAR-KML/ph_stadium_kml.zip")
    inner_path.unlink()


class StadiumTest(unittest.TestCase):
    def test_loads_textured_model_and_flips_collada_v(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "stadium.zip"
            textures = root / "textures"
            _archive(archive)

            (mesh,) = _load_models(archive, textures, 1)

            self.assertEqual(mesh.source_id, 1_000_001)
            self.assertEqual(mesh.texture_id, 1_000_001)
            self.assertAlmostEqual(mesh.height, 10.0)
            self.assertEqual(len(mesh.faces), 1)
            self.assertEqual(mesh.faces[0].uvs, ((0.0, 1.0), (1.0, 1.0), (0.0, 0.25)))
            self.assertGreater(mesh.faces[0].points[1][0], mesh.faces[0].points[0][0])
            self.assertGreater(mesh.faces[0].points[2][1], mesh.faces[0].points[0][1])
            self.assertEqual((textures / "1000001.jpg").read_bytes(), JPEG)

    def test_rejects_textured_triangle_without_uvs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "stadium.zip"
            _archive(archive, texcoord=False)
            with self.assertRaisesRegex(StadiumParseError, "TEXCOORD"):
                _load_models(archive, root / "textures", 1)

    def test_rejects_geometry_outside_published_region(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "stadium.zip"
            _archive(archive, west=-75.1501)
            with self.assertRaisesRegex(StadiumParseError, "outside its KML region"):
                _load_models(archive, root / "textures", 1)

    def test_excludes_demolished_spectrum_components(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "stadium.zip"
            _archive(archive, model_name="ph_stadium0778")
            self.assertEqual(_load_models(archive, root / "textures", 1), ())


if __name__ == "__main__":
    unittest.main()
