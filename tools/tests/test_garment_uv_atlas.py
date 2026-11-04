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
padding = dict(pixels=18, resolution=2048, fraction=18 / 2048)


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


def many_charts():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    vertices = [(2 * i + x, y, 0) for i in range(64) for x, y in ((0, 0), (1, 0), (0, .7))]
    triangles = [(3 * i, 3 * i + 1, 3 * i + 2) for i in range(64)]
    mesh = bpy.data.meshes.new("Many triangle charts")
    mesh.from_pydata(vertices, [], triangles)
    uv = np.asarray([[(i % 8) * .1 + .02 + x, (i // 8) * .1 + .02 + y]
                     for i in range(64) for x, y in ((0, 0), (.05, 0), (0, .035))], dtype=np.float32)
    uv[3:6] = uv[:3]
    mesh.uv_layers.new().data.foreach_set("uv", uv.ravel())
    obj = bpy.data.objects.new("Many triangle charts", mesh)
    bpy.context.collection.objects.link(obj)
    return obj


def measured_square_envelopes(uv, resolution, radius):
    """This fixture has one triangle per chart; measure all segment pairs in L-infinity."""
    corners = uv.reshape(-1, 3, 2).astype(np.float64)
    segments = np.stack((corners, np.roll(corners, 1, axis=1)), axis=2).reshape(-1, 2, 2)
    a, b = segments[:, None, 0], segments[:, None, 1]
    c, d = segments[None, :, 0], segments[None, :, 1]
    def point_segment(point, first, second):
        delta, edge = point - first, second - first
        result = np.minimum(np.max(np.abs(delta), axis=-1), np.max(np.abs(delta - edge), axis=-1))
        for sign in (-1, 1):
            numerator = delta[..., 0] + sign * delta[..., 1]
            denominator = edge[..., 0] + sign * edge[..., 1]
            t = np.clip(np.divide(numerator, denominator, out=np.zeros_like(numerator), where=denominator != 0), 0, 1)
            result = np.minimum(result, np.max(np.abs(delta - t[..., None] * edge), axis=-1))
        return result
    distances = np.minimum.reduce([point_segment(a, c, d), point_segment(b, c, d),
                                   point_segment(c, a, b), point_segment(d, a, b)])
    owners = np.repeat(np.arange(len(corners)), 3)
    distances[owners[:, None] == owners[None, :]] = np.inf
    gap = float(distances.min()) * resolution
    border = float(min(uv.min(), 1 - uv.max())) * resolution
    assert gap > 2 * radius and border > radius, (gap, border, radius)
    return dict(square_gap_texels=gap, border_texels=border, radius_texels=radius)


with tempfile.TemporaryDirectory(prefix="sculpto-atlas-native-") as temporary:
    root = Path(temporary)
    target = fixture()
    positions = [tuple(v.co) for v in target.data.vertices]
    triangles = [tuple(f.vertices) for f in target.data.polygons]
    record = {}
    output = root / "repair"
    output.mkdir()
    atlas.qualify_atlas(target, binary, output, record, padding)
    assert record["padding"] is padding
    assert record["pack_settings"]["margin"] == padding["fraction"]
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
        atlas.qualify_atlas(target, binary, clean_output, clean_record, padding)
    assert np.array_equal(clean, atlas.coordinates(target.data))
    assert clean_record["before_sha256"] == clean_record["after_sha256"]
    target = fixture()
    failed = root / "failed"
    failed.mkdir()
    failure = {}
    with patch.object(atlas, "pack_once", return_value=None):
        try:
            atlas.qualify_atlas(target, binary, failed, failure, padding)
        except ValueError as error:
            assert "after one native repack" in str(error)
        else:
            raise AssertionError("accepted failed packing")
    assert (failed / "uv-before-report.json").is_file()
    assert (failed / "uv-after-report.json").is_file()
    assert not failure["after"]["verdict"]["passed"]
    target = fixture()
    distorted = root / "distorted"
    distorted.mkdir()
    distortion = {}
    native_pack = atlas.pack_once
    def distort_pack(obj, padding):
        native_pack(obj, padding)
        uv = atlas.coordinates(obj.data)
        uv[:, 0] *= 0.5
        obj.data.uv_layers.active.data.foreach_set("uv", uv.ravel())
    with patch.object(atlas, "pack_once", side_effect=distort_pack):
        try:
            atlas.qualify_atlas(target, binary, distorted, distortion, padding)
        except ValueError as error:
            assert "distorted unaffected chart" in str(error)
        else:
            raise AssertionError("accepted a nonuniform chart deformation")
    assert distortion["after"]["verdict"]["passed"]
    assert distortion["unaffected_adjacency_preserved"]
    assert distortion["unaffected_edge_ratio_max_error"] > distortion["unaffected_edge_ratio_error_limit"]
    bpy.ops.wm.read_factory_settings(use_empty=True)
    mesh = bpy.data.meshes.new("Collapsed UV")
    mesh.from_pydata([(0, 0, 0), (1, 0, 0), (0, 1, 0)], [], [(0, 1, 2)])
    mesh.uv_layers.new().data.foreach_set("uv", [0] * 6)
    target = bpy.data.objects.new("Collapsed UV", mesh)
    bpy.context.collection.objects.link(target)
    collapsed = root / "collapsed"
    collapsed.mkdir()
    record = {}
    atlas.qualify_atlas(target, binary, collapsed, record, padding)
    assert record["before"]["degenerate_triangles"] == [0]
    assert record["after"]["verdict"]["passed"]
    for resolution in (1024, 2048):
        target = many_charts()
        config = dict(pixels=18, resolution=resolution, fraction=18 / resolution)
        output = root / f"many-{resolution}"
        output.mkdir()
        record = {}
        with patch.object(atlas, "pack_once", wraps=atlas.pack_once) as packing:
            atlas.qualify_atlas(target, binary, output, record, config)
        packing.assert_called_once_with(target, config)
        assert record["before"]["overlap_pairs"] == [[0, 1]] and record["after"]["verdict"]["passed"]
        assert record["padding"] is config and record["pack_settings"]["margin"] == config["fraction"]
        packed = atlas.coordinates(target.data).copy()
        measured = measured_square_envelopes(packed, resolution, 8)
        clean_output = root / f"many-clean-{resolution}"
        clean_output.mkdir()
        with patch.object(atlas, "pack_once", side_effect=AssertionError("clean many-chart atlas was repacked")):
            atlas.qualify_atlas(target, binary, clean_output, {}, config)
        assert np.array_equal(packed, atlas.coordinates(target.data))
        assert measured_square_envelopes(atlas.coordinates(target.data), resolution, 8) == measured
        print("MEASURED_BASE_LEVEL_FIXTURE", resolution, measured)
    print("PASS: explicit padding propagation; many-chart repaired/retained base-level envelopes; complete UV/shape/topology and one-pack refusal")
