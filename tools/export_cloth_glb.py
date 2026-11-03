"""Package a cloth bake's OBJ frame sequence as an animated glTF/GLB.

Usage (headless):
  blender --background --factory-startup --python export_cloth_glb.py -- \
      bake=<dir> out=<path.glb> [fps=24]

Reads `rest_<shell>.obj` and `frame_<n>_<shell>.obj` from the bake
directory, builds one mesh per shell with a shape key per sampled frame,
keyframes the weights and exports a single GLB whose animation drives the
morph targets. Engines play the morph animation natively (Bevy: glTF morph
animation; Unity: glTF importer), no physics needed at runtime.
"""

import glob
import json
import os
import re
import sys
from math import radians
from pathlib import Path

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parent))
from blender_export import shared_animation_root

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
bake_dir = cfg["bake"]
out_path = cfg["out"]
fps = int(cfg.get("fps", 24))

# Factory startup ships a default cube; the export must contain only the
# garment shells.
for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)

report = json.load(open(os.path.join(bake_dir, "report.json")))
scene = bpy.context.scene
scene.render.fps = fps

shell_names = sorted(report["shells"])
frame_index = {}
for path in glob.glob(os.path.join(bake_dir, "frame_*_*.obj")):
    match = re.match(r"frame_(\d+)_(.+)\.obj$", os.path.basename(path))
    if match:
        frame_index.setdefault(match.group(2), {})[int(match.group(1))] = path

weights_curve = {}
for shell in shell_names:
    frames = frame_index.get(shell, {})
    if not frames:
        continue
    rest_path = os.path.join(bake_dir, f"rest_{shell}.obj")
    bpy.ops.wm.obj_import(filepath=os.path.abspath(rest_path))
    obj = bpy.context.selected_objects[0] if bpy.context.selected_objects else bpy.context.active_object
    obj.name = shell
    bpy.context.view_layer.objects.active = obj
    # The bake writes Blender world Z-up coordinates, so the import frame
    # rotation is cleared and the local data used directly.
    obj.rotation_euler = (0.0, 0.0, 0.0)
    rest_world = [vertex.co.copy() for vertex in obj.data.vertices]
    for index, co in enumerate(rest_world):
        obj.data.vertices[index].co = co
    obj.shape_key_add(name="Basis", from_mix=False)
    for frame in sorted(frames):
        bpy.ops.wm.obj_import(filepath=os.path.abspath(frames[frame]))
        other = bpy.context.selected_objects[0]
        other.name = f"{shell}_frame_{frame}"
        key = obj.shape_key_add(name=f"frame_{frame}", from_mix=False)
        if len(other.data.vertices) != len(obj.data.vertices):
            raise SystemExit(f"vertex mismatch in {frames[frame]}")
        other.rotation_euler = (0.0, 0.0, 0.0)
        world = other.matrix_world
        for index, vertex in enumerate(other.data.vertices):
            key.data[index].co = world @ vertex.co
        key.value = 0.0
        bpy.data.objects.remove(other, do_unlink=True)
    weights_curve[shell] = sorted(frames)

if not weights_curve:
    raise SystemExit("no frame sequence found in the bake directory")

# One shared animation: each sampled frame drives exactly one shape key.
keyframe_pointer = 0
for frame in sorted({f for frames in weights_curve.values() for f in frames}):
    for shell, frames in weights_curve.items():
        obj = bpy.data.objects[shell]
        for key in obj.data.shape_keys.key_blocks[1:]:
            target = 1.0 if key.name == f"frame_{frame}" and frame in frames else 0.0
            key.value = target
            key.keyframe_insert("value", frame=frame)

scene.frame_start = min(f for frames in weights_curve.values() for f in frames)
scene.frame_end = max(f for frames in weights_curve.values() for f in frames)

# glTF is Y-up. Rotate every vertex and shape key -90 deg about X (Blender's
# mesh.transform does not touch shape keys) and export without the exporter's
# own axis conversion, so the data is Y-up with identity node transforms.
from math import pi
from mathutils import Matrix

rotation = Matrix.Rotation(-pi / 2.0, 4, "X")
for shell in weights_curve:
    obj = bpy.data.objects[shell]
    obj.data.transform(rotation)
    keys = obj.data.shape_keys
    if keys:
        for key in keys.key_blocks:
            for index, point in enumerate(key.data):
                point.co = rotation @ point.co

shared_animation_root(scene)
bpy.ops.export_scene.gltf(
    filepath=os.path.abspath(out_path),
    export_format="GLB",
    export_animations=True,
    export_morph=os.environ.get("SCULPTO_GLB_MORPH", "1") == "1",
    export_morph_animation=True,
    export_apply=False,
    use_selection=False,
    export_yup=False,
)
# The exporter samples the animated shape-key values at the current frame for
# `mesh.weights`; a non-zero default applies that target at load time in every
# engine (Bevy renders the cloth as a crumpled sheet). Zero the defaults in the
# container, keeping the animation channels intact.
import struct as _struct

raw = bytearray(open(out_path, "rb").read())
_magic, _version, _total = _struct.unpack("<III", raw[:12])
_json_len, _json_type = _struct.unpack("<II", raw[12:20])
_doc = json.loads(raw[20 : 20 + _json_len])
_changed = False
for _mesh in _doc.get("meshes", []):
    if _mesh.get("weights"):
        _mesh["weights"] = [0.0] * len(_mesh["weights"])
        _changed = True
if _changed:
    _json = json.dumps(_doc, separators=(",", ":")).encode()
    _json += b" " * ((-len(_json)) % 4)
    _tail = bytes(raw[20 + _json_len :])
    with open(out_path, "wb") as _sink:
        _sink.write(_struct.pack("<III", _magic, _version, 12 + 8 + len(_json) + len(_tail)))
        _sink.write(_struct.pack("<II", len(_json), _json_type))
        _sink.write(_json)
        _sink.write(_tail)

print(
    "GLB " + json.dumps(
        {
            "out": out_path,
            "shells": {shell: len(frames) for shell, frames in weights_curve.items()},
            "bytes": os.path.getsize(out_path),
        }
    )
)
