"""The legacy bake's OBJ writer keeps loop UVs on rest and sampled frames."""
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from obj_uv_sequence import write_obj


class Point:
    def __init__(self, x, y, z=0.0):
        self.x, self.y, self.z = x, y, z


class UvDatum:
    def __init__(self, u, v):
        self.uv = (u, v)


class Polygon:
    def __init__(self, vertices, loop_start):
        self.vertices = vertices
        self.loop_start = loop_start
        self.loop_total = len(vertices)


class UvLayer:
    def __init__(self, uvs):
        self.data = [UvDatum(u, v) for u, v in uvs]


SQUARE = [Point(0, 0), Point(1, 0), Point(1, 1), Point(0, 1)]


class ObjUvSequenceTest(unittest.TestCase):
    def write(self, coords, polygons, uv_layer):
        temporary = tempfile.TemporaryDirectory(prefix="sculpto-obj-uv-")
        self.addCleanup(temporary.cleanup)
        path = pathlib.Path(temporary.name) / "frame.obj"
        write_obj(path, coords, polygons, uv_layer)
        return path.read_text().splitlines()

    def test_rest_and_frame_repeat_the_same_loop_uvs(self):
        uv_layer = UvLayer([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
        polygons = [Polygon([0, 1, 2, 3], 0)]
        rest = self.write(SQUARE, polygons, uv_layer)
        moved = self.write([Point(0, 0, 0.1), *SQUARE[1:]], polygons, uv_layer)
        expected = [
            "vt 0.000000 0.000000",
            "vt 1.000000 0.000000",
            "vt 1.000000 1.000000",
            "vt 0.000000 1.000000",
        ]
        self.assertEqual([line for line in rest if line.startswith("vt ")], expected)
        self.assertEqual([line for line in moved if line.startswith("vt ")], expected)
        self.assertEqual([line for line in rest if line.startswith("f ")], ["f 1/1 2/2 3/3 4/4"])

    def test_shared_corners_resolve_to_one_uv_row(self):
        uv_layer = UvLayer([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
        polygons = [Polygon([0, 1, 2], 0), Polygon([0, 2, 3], 3)]
        lines = self.write(SQUARE, polygons, uv_layer)
        self.assertEqual(len([line for line in lines if line.startswith("vt ")]), 4)
        self.assertIn("f 1/1 2/2 3/3", lines)
        self.assertIn("f 1/1 3/3 4/4", lines)

    def test_missing_uv_layer_writes_plain_faces(self):
        lines = self.write(SQUARE[:3], [Polygon([0, 1, 2], 0)], None)
        self.assertFalse([line for line in lines if line.startswith("vt ")])
        self.assertEqual([line for line in lines if line.startswith("f ")], ["f 1 2 3"])


if __name__ == "__main__":
    unittest.main()
