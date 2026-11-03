"""Blender-side source preservation and invalid batch selection regression."""

from pathlib import Path
import sys

import bpy
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from extract_garment_batch import partition, split, values

bpy.ops.wm.read_factory_settings(use_empty=True)
mesh = bpy.data.meshes.new("Source")
mesh.from_pydata([(-2, 0, 0), (-1, 0, 0), (-1, 0, 1), (1, 0, 0), (2, 0, 0), (1, 0, 1)], [], [(0, 1, 2), (3, 4, 5)])
mesh.uv_layers.new(name="UVMap").data.foreach_set("uv", [0, 0, .4, 0, .4, 1, .6, 0, 1, 0, .6, 1])
mesh.normals_split_custom_set([(0, -1, 0)] * 6)
mesh.materials.append(bpy.data.materials.new("A"))
mesh.materials.append(bpy.data.materials.new("B"))
mesh.polygons[1].material_index = 1
obj = bpy.data.objects.new("Source", mesh)
bpy.context.collection.objects.link(obj)
obj.location.z = 3
bpy.context.view_layer.update()
parts = [dict(id="left", bounds=[-3, 0, 2, 5]), dict(id="right", bounds=[0, 3, 2, 5])]
children = split(obj, parts)
assert len(children) == 2
for index, child in enumerate(children):
    assert np.array_equal(np.array(child.matrix_world), np.array(obj.matrix_world))
    ids = values(child.data.attributes["source_vertex"].data, "value", dtype=np.int32)
    assert np.array_equal(values(child.data.vertices, "co", 3), values(mesh.vertices, "co", 3)[ids])
    assert child.data.attributes["source_face"].data[0].value == index
    assert child.data.polygons[0].material_index == index
    assert child.data.materials[index] == mesh.materials[index]
    assert np.array_equal(values(child.data.uv_layers[0].data, "uv", 2), values(mesh.uv_layers[0].data, "uv", 2)[3*index:3*index+3])
    assert np.max(np.abs(values(child.data.corner_normals, "vector", 3) - values(mesh.corner_normals, "vector", 3)[3*index:3*index+3])) < 1e-5
    assert np.array_equal(values(child.data.attributes["source_corner_normal"].data, "vector", 3), values(mesh.corner_normals, "vector", 3)[3*index:3*index+3])
for invalid, expected in [
    ([parts[0]], "uncovered"),
    ([parts[0], dict(id="overlap", bounds=[-3, 3, 2, 5])], "overlapping"),
    ([dict(id="a", bounds=[-3, -1.5, 2, 5]), dict(id="b", bounds=[-1.5, 3, 2, 5])], "cuts source faces"),
]:
    try:
        partition(obj, invalid)
    except ValueError as error:
        assert expected in str(error), str(error)
    else:
        raise AssertionError(f"accepted invalid recipe: {expected}")
assert len(mesh.vertices) == 6 and len(mesh.polygons) == 2
print("PASS: batch partition preserves coordinates, UVs, normals, materials and source IDs; rejects cutting/incomplete/overlapping recipes")
