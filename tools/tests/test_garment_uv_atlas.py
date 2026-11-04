"""Native complete-atlas qualification, isolated chart repair and one-pack refusal."""

from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

import bpy
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import garment_uv_atlas as atlas

arguments = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(arguments) != 1:
    raise SystemExit("expected explicit sculpto executable argument")
binary = Path(arguments[0]).resolve()


def fixture():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    mesh = bpy.data.meshes.new("Mixed charts")
    mesh.from_pydata([(0, 0, 0), (1, 0, 0), (0, 1, 0), (2, 0, 0), (3, 0, 0), (2, 1, 0),
                     (4, 0, 0), (5, 0, 0), (5, 1, 0), (4, 1, 0)], [],
                    [(0, 1, 2), (3, 4, 5), (6, 7, 8), (6, 8, 9)])
    mesh.uv_layers.new().data.foreach_set("uv", [0, 0, .4, 0, 0, .4] * 2 +
        [.6, .6, 1, .6, 1, 1, .6, .6, 1, 1, .6, 1])
    obj = bpy.data.objects.new("Mixed charts", mesh)
    bpy.context.collection.objects.link(obj)
    return obj


with tempfile.TemporaryDirectory(prefix="sculpto-atlas-native-") as temporary:
    root = Path(temporary)
    target = fixture()
    positions = [tuple(v.co) for v in target.data.vertices]
    triangles = [tuple(f.vertices) for f in target.data.polygons]
    record = {}
    output = root / "repair"
    output.mkdir()
    atlas.qualify_atlas(target, binary, output, record)
    assert record["before"]["overlap_pairs"] == [[0, 1]]
    assert record["affected_faces"] == [0, 1]
    assert record["after"]["verdict"]["passed"]
    assert record["unaffected_adjacency_preserved"]
    assert record["unaffected_corners_before_pack_preserved"]
    assert record["unaffected_edge_ratio_max_error"] < 1e-5
    assert positions == [tuple(v.co) for v in target.data.vertices]
    assert triangles == [tuple(f.vertices) for f in target.data.polygons]
    clean = atlas.coordinates(target.data).copy()
    clean_output = root / "clean"
    clean_output.mkdir()
    clean_record = {}
    with patch.object(atlas, "pack_once", side_effect=AssertionError("clean atlas was repacked")):
        atlas.qualify_atlas(target, binary, clean_output, clean_record)
    assert np.array_equal(clean, atlas.coordinates(target.data))
    assert clean_record["before_sha256"] == clean_record["after_sha256"]
    target = fixture()
    failed = root / "failed"
    failed.mkdir()
    failure = {}
    with patch.object(atlas, "pack_once", return_value=None):
        try:
            atlas.qualify_atlas(target, binary, failed, failure)
        except ValueError as error:
            assert "after one native repack" in str(error)
        else:
            raise AssertionError("accepted failed packing")
    assert (failed / "uv-before-report.json").is_file()
    assert (failed / "uv-after-report.json").is_file()
    assert not failure["after"]["verdict"]["passed"]
    bpy.ops.wm.read_factory_settings(use_empty=True)
    mesh = bpy.data.meshes.new("Collapsed UV")
    mesh.from_pydata([(0, 0, 0), (1, 0, 0), (0, 1, 0)], [], [(0, 1, 2)])
    mesh.uv_layers.new().data.foreach_set("uv", [0] * 6)
    target = bpy.data.objects.new("Collapsed UV", mesh)
    bpy.context.collection.objects.link(target)
    collapsed = root / "collapsed"
    collapsed.mkdir()
    record = {}
    atlas.qualify_atlas(target, binary, collapsed, record)
    assert record["before"]["degenerate_triangles"] == [0]
    assert record["after"]["verdict"]["passed"]
    print("PASS: diagnosed charts repair once; clean UVs stay exact; physical topology/unaffected chart adjacency remain; failed pack refuses")
