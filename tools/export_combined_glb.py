"""Combine the accepted actor GLB and a cloth bake into one animated GLB.

Usage (headless):
  blender --background --factory-startup --python export_combined_glb.py -- \
      actor=<character.glb> bake=<bake dir> out=<combined.glb> [fps=24] \
      [material_blend=<garment.blend> material_object=<material name>]

Imports the actor (armature, skinned meshes, its clips) and rebuilds the cloth
shells from the bake's OBJ sequence as morph-target meshes, then exports one
GLB. Engine playback therefore gets body and cloth from a single asset with
one shared clock. With `material_blend`/`material_object` the garment material
and the images it references are appended to the cloth shells, so a textured
garment keeps its basecolor instead of exporting as grey cloth.
"""

import glob
import json
import os
import re
import sys
import struct
from math import radians
from pathlib import Path

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parent))
from blender_export import shared_animation_root

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
actor_path = cfg["actor"]
bake_dir = cfg["bake"]
out_path = cfg["out"]
fps = int(cfg.get("fps", 24))
material_blend = cfg.get("material_blend")
material_object = cfg.get("material_object")


def load_material():
    """Append the garment material and the images it references.

    `bpy.data.libraries.load` appends the material's image dependencies with
    it; a texture node left without an image means the append silently lost a
    dependency, and an untextured garment would reach the engine, so that is
    an error rather than a fallback.
    """
    if not material_object:
        raise SystemExit("material_blend=<blend> needs material_object=<material name>")
    blend_path = os.path.abspath(material_blend)
    if not os.path.exists(blend_path):
        raise SystemExit(f"material blend not found: {blend_path}")
    with bpy.data.libraries.load(blend_path) as (source, target):
        if material_object not in source.materials:
            raise SystemExit(
                f"material `{material_object}` not found in {blend_path}; "
                f"available: {list(source.materials)}"
            )
        target.materials = [material_object]
    material = bpy.data.materials[material_object]
    textures = [node for node in material.node_tree.nodes if node.type == "TEX_IMAGE"]
    missing = sorted(node.name for node in textures if node.image is None)
    if missing:
        raise SystemExit(f"material `{material_object}` lost image dependencies: {missing}")
    for node in textures:
        image = node.image
        if image.packed_file is None and not os.path.exists(bpy.path.abspath(image.filepath)):
            raise SystemExit(f"material `{material_object}` image missing on disk: {image.filepath}")
    print(f"material: {material.name}, {len(textures)} texture nodes")
    return material, len(textures)

for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)

scene = bpy.context.scene
scene.render.fps = fps
bpy.ops.import_scene.gltf(filepath=os.path.abspath(actor_path))
print(f"actor objects: {len(bpy.data.objects)}")

report = json.load(open(os.path.join(bake_dir, "report.json")))
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
    obj.rotation_euler = (0.0, 0.0, 0.0)
    rest_world = [vertex.co.copy() for vertex in obj.data.vertices]
    obj.shape_key_add(name="Basis", from_mix=False)
    for frame in sorted(frames):
        bpy.ops.wm.obj_import(filepath=os.path.abspath(frames[frame]))
        other = bpy.context.selected_objects[0]
        other.rotation_euler = (0.0, 0.0, 0.0)
        key = obj.shape_key_add(name=f"frame_{frame}", from_mix=False)
        if len(other.data.vertices) != len(obj.data.vertices):
            raise SystemExit(f"vertex mismatch in {frames[frame]}")
        for index, vertex in enumerate(other.data.vertices):
            key.data[index].co = vertex.co.copy()
        key.value = 0.0
        bpy.data.objects.remove(other, do_unlink=True)
    weights_curve[shell] = sorted(frames)

for shell, frames in weights_curve.items():
    obj = bpy.data.objects[shell]
    for frame in frames:
        for key in obj.data.shape_keys.key_blocks[1:]:
            key.value = 1.0 if key.name == f"frame_{frame}" else 0.0
            key.keyframe_insert("value", frame=frame)
    for key in obj.data.shape_keys.key_blocks:
        key.value = 0.0

material_name = None
texture_count = 0
if material_blend:
    material, texture_count = load_material()
    material_name = material.name
    for shell in weights_curve:
        bpy.data.objects[shell].data.materials.append(material)
else:
    # Baked shells carry no material of their own; without one the glTF gets
    # a default doubleSided=false material, so interior cloth faces vanish
    # into black holes (the chainmail chest case). A plain double-sided cloth
    # material keeps thin shells readable from both sides.
    material = bpy.data.materials.get("sculpto-cloth") or bpy.data.materials.new("sculpto-cloth")
    material.use_nodes = True
    material.use_backface_culling = False
    bsdf = next(n for n in material.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = (0.82, 0.80, 0.76, 1.0)
    for shell in weights_curve:
        bpy.data.objects[shell].data.materials.append(material)

shared_animation_root(scene)
bpy.ops.export_scene.gltf(
    filepath=os.path.abspath(out_path),
    export_format="GLB",
    export_animations=True,
    export_morph=True,
    export_morph_animation=True,
    export_apply=False,
    export_yup=True,
)

raw = bytearray(open(out_path, "rb").read())
magic, version, _total = struct.unpack("<III", raw[:12])
json_len, json_type = struct.unpack("<II", raw[12:20])
doc = json.loads(raw[20 : 20 + json_len])
changed = False
for mesh in doc.get("meshes", []):
    if mesh.get("weights"):
        mesh["weights"] = [0.0] * len(mesh["weights"])
        changed = True
if changed:
    payload = json.dumps(doc, separators=(",", ":")).encode()
    payload += b" " * ((-len(payload)) % 4)
    tail = bytes(raw[20 + json_len :])
    with open(out_path, "wb") as sink:
        sink.write(struct.pack("<III", magic, version, 12 + 8 + len(payload) + len(tail)))
        sink.write(struct.pack("<II", len(payload), json_type))
        sink.write(payload)
        sink.write(tail)

# Blender 5.2 texCoord -1 clamp lives in glb_sanitize (shared with
# skin_underlayers); it rewrites the GLB in place.
from glb_sanitize import sanitize_texcoord

sanitize_texcoord(out_path)

print(
    "COMBINED "
    + json.dumps(
        {
            "out": out_path,
            "shells": {shell: len(frames) for shell, frames in weights_curve.items()},
            "material": material_name,
            "textures": texture_count,
            "bytes": os.path.getsize(out_path),
        }
    )
)
