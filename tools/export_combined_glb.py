"""Combine the accepted actor GLB and a cloth bake into one animated GLB.

Usage (headless):
  blender --background --factory-startup --python export_combined_glb.py -- \
      actor=<character.glb> bake=<bake dir> out=<combined.glb> [fps=24] \
      [skin=1 clip=<action>] [frame_offset=<frames>] \
      [material_blend=<garment.blend> material_object=<material name>]

Imports the actor (armature, skinned meshes, its clips) and rebuilds the cloth
shells from the bake's OBJ sequence as morph-target meshes, then exports one
GLB. Engine playback therefore gets body and cloth from a single asset with
one shared clock. With `material_blend`/`material_object` the garment material
and the images it references are appended to the cloth shells, so a textured
garment keeps its basecolor instead of exporting as grey cloth.

With `skin=1 clip=<action>` the shells are skinned to the actor armature
(nearest-body weights, at most four influences) and every baked frame is
unskinned into a rest-space shape key. Blender then applies both the armature
deform and the morph, so the cloak inherits root/shoulder motion instead of
staying behind, and the exporter verifies the played geometry against every
bake frame before writing.
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
import numpy as np
from mathutils import kdtree

sys.path.insert(0, str(Path(__file__).resolve().parent))
from blender_export import shared_animation_root

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
actor_path = cfg["actor"]
bake_dir = cfg["bake"]
out_path = cfg["out"]
report = json.load(open(os.path.join(bake_dir, "report.json")))
fps = int(cfg.get("fps", report.get("fps", 24)))
# The bake scene and the actor GLB import can disagree on the action's first
# frame (Blender authoring starts at 1, glTF import at 0); the bake frame is
# shifted by the action's start before it is used as a scene time.
frame_offset = int(cfg.get("frame_offset", -int(round(report.get("action_start", 1)))))
# Bevy 0.17 rejects a mesh with more than 64 morph targets; stride thins a
# dense bake without changing the clip's time range.
stride = int(cfg.get("stride", "1"))
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

skin = cfg.get("skin") == "1"
clip_name = cfg.get("clip")
armature = next((o for o in bpy.data.objects if o.type == "ARMATURE"), None)
if skin and (armature is None or not clip_name):
    raise SystemExit("skin=1 needs an actor armature and clip=<action name>")
if skin and not armature.matrix_world.is_identity:
    raise SystemExit(f"armature {armature.name} is not at the world origin")

WEIGHT_NEIGHBOURS = 4
MAX_SOURCE_VERTICES = 200_000


def skin_sources(armature):
    """Rest-pose weight donors from the actor's own skinned meshes."""
    sources = []
    for part in bpy.data.objects:
        if part.type != "MESH" or not part.vertex_groups:
            continue
        if not any(mod.type == "ARMATURE" and mod.object is armature for mod in part.modifiers):
            continue
        if len(part.data.vertices) > MAX_SOURCE_VERTICES:
            print(f"skin source skipped (over {MAX_SOURCE_VERTICES} vertices): {part.name}")
            continue
        sources.append(part)
    world, rows = [], []
    for part in sources:
        names = [group.name for group in part.vertex_groups]
        matrix = part.matrix_world
        for vertex in part.data.vertices:
            weights = [(names[item.group], item.weight) for item in vertex.groups if item.weight > 0.0]
            total = sum(weight for _, weight in weights)
            if total <= 0.0:
                continue
            world.append(matrix @ vertex.co)
            rows.append({name: weight / total for name, weight in weights})
    print(f"skin sources: {[part.name for part in sources]}, {len(world)} weighted vertices")
    return world, rows


def transfer_weights(obj, tree, rows, armature):
    """Nearest-body weights, then attach the shell to the actor armature."""
    groups = {}
    for index, vertex in enumerate(obj.data.vertices):
        near = tree.find_n(obj.matrix_world @ vertex.co, WEIGHT_NEIGHBOURS)
        if not near:
            raise SystemExit(f"{obj.name}: vertex {index} has no body weight donor")
        mixed = {}
        for _co, source, _distance in near:
            for name, weight in rows[source].items():
                mixed[name] = mixed.get(name, 0.0) + weight / len(near)
        # glTF and the actor's own skin consumers carry four influences; keep
        # the same four here so Blender's modifier and the encoded skin agree.
        mixed = dict(sorted(mixed.items(), key=lambda item: -item[1])[:4])
        total = sum(mixed.values())
        if total <= 0.0:
            raise SystemExit(f"{obj.name}: vertex {index} lost every donor weight")
        for name, weight in mixed.items():
            group = groups.get(name)
            if group is None:
                group = obj.vertex_groups.get(name) or obj.vertex_groups.new(name=name)
                groups[name] = group
            group.add([index], weight / total, "REPLACE")
    obj.parent = armature
    obj.matrix_parent_inverse = armature.matrix_world.inverted()
    obj.modifiers.new(name="Armature", type="ARMATURE").object = armature


def vertex_skin(obj, armature):
    """Top-four bone indices and normalized weights per vertex."""
    names = [bone.name for bone in armature.pose.bones]
    index_of = {name: index for index, name in enumerate(names)}
    joints = np.zeros((len(obj.data.vertices), 4), dtype=np.int32)
    weights = np.zeros((len(obj.data.vertices), 4), dtype=np.float64)
    for vertex in obj.data.vertices:
        rows = [
            (index_of[obj.vertex_groups[item.group].name], item.weight)
            for item in vertex.groups
            if item.weight > 0.0
        ]
        rows.sort(key=lambda row: -row[1])
        rows = rows[:4]
        total = sum(weight for _, weight in rows)
        if total <= 0.0:
            raise SystemExit(f"{obj.name}: vertex {vertex.index} lost every weight")
        for slot, (joint, weight) in enumerate(rows):
            joints[vertex.index, slot] = joint
            weights[vertex.index, slot] = weight / total
    return joints, weights


def deform_matrices(armature):
    """World-space bone deform matrices for the current pose."""
    world = armature.matrix_world
    inverse = world.inverted()
    return np.array(
        [
            np.array(world @ bone.matrix @ bone.bone.matrix_local.inverted() @ inverse)
            for bone in armature.pose.bones
        ]
    )


def rest_space_targets(baked, joints, weights, matrices):
    """Unskin a baked world frame into the mesh's rest space."""
    skinning = np.einsum("nk,nkij->nij", weights, matrices[joints])
    inverse = np.linalg.inv(skinning)
    points = np.concatenate([baked, np.ones((len(baked), 1))], axis=1)
    return np.einsum("nij,nj->ni", inverse, points)[:, :3]


weight_donors = None
weight_tree = None
if skin:
    armature.data.pose_position = "REST"
    bpy.context.view_layer.update()
    donors, donor_rows = skin_sources(armature)
    weight_tree = kdtree.KDTree(len(donors))
    for index, coordinate in enumerate(donors):
        weight_tree.insert(coordinate, index)
    weight_tree.balance()
    weight_donors = donor_rows
    armature.data.pose_position = "POSE"
    action = bpy.data.actions.get(clip_name)
    if action is None:
        raise SystemExit(f"actor action `{clip_name}` not found")
    armature.animation_data_create()
    armature.animation_data.action = action

shell_names = sorted(report["shells"])
frame_index = {}
for path in glob.glob(os.path.join(bake_dir, "frame_*_*.obj")):
    match = re.match(r"frame_(\d+)_(.+)\.obj$", os.path.basename(path))
    if match:
        frame_index.setdefault(match.group(2), {})[int(match.group(1))] = path

weights_curve = {}
skinned = {}
for shell in shell_names:
    frames = frame_index.get(shell, {})
    if not frames:
        continue
    rest_path = os.path.join(bake_dir, f"rest_{shell}.obj")
    bpy.ops.wm.obj_import(
        filepath=os.path.abspath(rest_path), forward_axis="Y", up_axis="Z"
    )
    obj = bpy.context.selected_objects[0] if bpy.context.selected_objects else bpy.context.active_object
    obj.name = shell
    obj.rotation_euler = (0.0, 0.0, 0.0)
    bpy.context.view_layer.update()
    if skin:
        if not obj.matrix_world.is_identity:
            raise SystemExit(f"{shell}: rest shell is not at the world origin")
        transfer_weights(obj, weight_tree, weight_donors, armature)
        joints, vertex_weights = vertex_skin(obj, armature)
        skinned[shell] = (joints, vertex_weights)
    rest_world = [vertex.co.copy() for vertex in obj.data.vertices]
    obj.shape_key_add(name="Basis", from_mix=False)
    baked_frames = {}
    selected = sorted(frames)[::stride]
    if sorted(frames) and selected[-1] != sorted(frames)[-1]:
        selected.append(sorted(frames)[-1])
    for frame in selected:
        bpy.ops.wm.obj_import(
            filepath=os.path.abspath(frames[frame]), forward_axis="Y", up_axis="Z"
        )
        other = bpy.context.selected_objects[0]
        other.rotation_euler = (0.0, 0.0, 0.0)
        if len(other.data.vertices) != len(obj.data.vertices):
            raise SystemExit(f"vertex mismatch in {frames[frame]}")
        baked = np.array([tuple(other.matrix_world @ vertex.co) for vertex in other.data.vertices])
        bpy.data.objects.remove(other, do_unlink=True)
        if skin:
            scene.frame_set(max(0, frame + frame_offset))
            bpy.context.view_layer.update()
            target = rest_space_targets(
                baked, skinned[shell][0], skinned[shell][1], deform_matrices(armature)
            )
        else:
            target = baked
        key = obj.shape_key_add(name=f"frame_{frame}", from_mix=False)
        key.data.foreach_set("co", target.ravel())
        key.value = 0.0
        baked_frames[frame] = baked
    weights_curve[shell] = selected
    if skin:
        skinned[shell] = (skinned[shell][0], skinned[shell][1], baked_frames)

for shell, frames in weights_curve.items():
    obj = bpy.data.objects[shell]
    for frame in frames:
        for key in obj.data.shape_keys.key_blocks[1:]:
            key.value = 1.0 if key.name == f"frame_{frame}" else 0.0
            key.keyframe_insert("value", frame=max(0, frame + frame_offset))
    for key in obj.data.shape_keys.key_blocks:
        key.value = 0.0

if skin:
    # The bake scene starts its action at frame 1 while the imported clip
    # starts at 0, so the last baked pose lands one frame before the actor's
    # end. Repeat that pose at the actor's end: the cloth tail is constant and
    # both clips keep the same time range.
    actor_end = int(round(action.frame_range[1]))
    for shell, frames in weights_curve.items():
        obj = bpy.data.objects[shell]
        last = frames[-1] + frame_offset
        if last < actor_end:
            for key in obj.data.shape_keys.key_blocks[1:]:
                key.value = 1.0 if key.name == f"frame_{frames[-1]}" else 0.0
            for key in obj.data.shape_keys.key_blocks[1:]:
                key.keyframe_insert("value", frame=actor_end)
    for shell in weights_curve:
        for key in bpy.data.objects[shell].data.shape_keys.key_blocks:
            key.value = 0.0

if skin:
    for shell, (_, _, baked_frames) in skinned.items():
        obj = bpy.data.objects[shell]
        worst = 0.0
        # Bake frames 0 and 1 can share the actor's first pose (the bake scene
        # clamps at its action start); the later frame wins the key slot, so
        # compare the played frame against the bake frame that actually owns it.
        keyed = {max(0, frame + frame_offset): frame for frame in sorted(baked_frames)}
        for scene_frame, frame in sorted(keyed.items()):
            baked = baked_frames[frame]
            scene.frame_set(scene_frame)
            bpy.context.view_layer.update()
            depsgraph = bpy.context.evaluated_depsgraph_get()
            evaluated = obj.evaluated_get(depsgraph)
            mesh = evaluated.to_mesh()
            played = np.array([tuple(obj.matrix_world @ vertex.co) for vertex in mesh.vertices])
            evaluated.to_mesh_clear()
            frame_worst = float(np.linalg.norm(played - baked, axis=1).max())
            worst = max(worst, frame_worst)
            if scene_frame in (min(keyed), max(keyed)):
                keys = obj.data.shape_keys
                active = keys.animation_data.action.name if keys.animation_data and keys.animation_data.action else None
                print(f"RECONSTRUCTION frame {scene_frame} max_mm {frame_worst * 1000:.3f} action {active}")
        print(
            "RECONSTRUCTION "
            + json.dumps(
                {
                    "shell": shell,
                    "clip": clip_name,
                    "frames": len(baked_frames),
                    "max_mm": round(worst * 1000.0, 4),
                }
            )
        )
        if worst > 0.0005:
            raise SystemExit(f"{shell}: skinned reconstruction misses the bake by {worst * 1000:.3f} mm")

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
