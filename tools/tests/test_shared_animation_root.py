"""Blender regression for one player driving both skeleton and cloth channels."""
import json
from pathlib import Path
import struct
import sys
import tempfile

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from blender_export import shared_animation_root

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.frame_start = 1
scene.frame_end = 3
rig = bpy.data.objects.new("SkeletonRoot", None)
scene.collection.objects.link(rig)
for frame, x in [(1, 2.0), (3, 4.0)]:
    rig.location = (x, 0, 1)
    rig.keyframe_insert("location", frame=frame)
action = rig.animation_data.action
action.name = "Walk"
mesh = bpy.data.meshes.new("Cloth")
mesh.from_pydata([(0, 0, 0), (1, 0, 0), (0, 0, 1)], [], [(0, 1, 2)])
cloth = bpy.data.objects.new("Cloth", mesh)
scene.collection.objects.link(cloth)
cloth.location = (0, 3, 0)
cloth.shape_key_add(name="Basis")
key = cloth.shape_key_add(name="Moved")
key.data[2].co.x = 0.25
keys = cloth.data.shape_keys
keys.animation_data_create()
keys.animation_data.action = action
keys.animation_data.action_slot = action.slots.new(id_type="KEY", name=keys.name)
for frame, value in [(1, 0.0), (3, 1.0)]:
    key.value = value
    key.keyframe_insert("value", frame=frame)


def state():
    samples = []
    for frame in [1, 2, 3]:
        scene.frame_set(frame)
        bpy.context.view_layer.update()
        evaluated = cloth.evaluated_get(bpy.context.evaluated_depsgraph_get())
        samples.append((tuple(rig.matrix_world.translation),
                        tuple(evaluated.matrix_world @ evaluated.data.vertices[2].co)))
    return samples


def export(path):
    bpy.ops.export_scene.gltf(filepath=str(path), export_format="GLB",
                             export_animations=True, export_animation_mode="ACTIONS",
                             export_merge_animation="ACTION", export_optimize_animation_size=False)
    raw = path.read_bytes()
    size = struct.unpack_from("<I", raw, 12)[0]
    return json.loads(raw[20:20 + size])


before = state()
with tempfile.TemporaryDirectory(prefix="sculpto-animation-root-") as directory:
    old = export(Path(directory) / "before.glb")
    assert len(old["scenes"][0]["nodes"]) == 2
    root = shared_animation_root(scene)
    assert state() == before, "identity parenting changed evaluated animation"
    new = export(Path(directory) / "after.glb")
    roots = new["scenes"][0]["nodes"]
    assert len(roots) == 1 and new["nodes"][roots[0]]["name"] == root.name
    assert len(new["animations"]) == 1
    channels = new["animations"][0]["channels"]
    targets = {(new["nodes"][c["target"]["node"]]["name"], c["target"]["path"])
               for c in channels}
    assert ("SkeletonRoot", "translation") in targets and ("Cloth", "weights") in targets
    count = len(scene.objects)
    try:
        shared_animation_root(scene, [cloth])
    except ValueError:
        pass
    else:
        raise AssertionError("selection missing its parent was accepted")
    assert len(scene.objects) == count, "invalid selection mutated the scene"
print("PASS: shared animation root preserves motion and both exported channel families")
