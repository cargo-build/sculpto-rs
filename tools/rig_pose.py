"""Pose a body and a garment with a real armature.

The vertex-space motor in `textured_render.py` rotated raw vertices about
hardcoded landmarks with a linear blend and no skin weights, so the deltoid did
not behave like a bone rotation and an unskinned garment could never follow the
arm. This builds an armature from landmarks measured on the body, binds body and
garment with automatic weights through the same bones, and rotates the upper-arm
bones to a horizontal T-pose.

Usage:
    blender --background --factory-startup --python tools/rig_pose.py -- \
        body=<obj> garment=<obj> out_body=<obj> out_garment=<obj> [angle=90]
"""

import math
import sys

import bpy
from mathutils import Matrix, Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("body", "garment", "out_body", "out_garment"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
TARGET = float(cfg.get("angle", "90"))

bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)

bpy.ops.wm.obj_import(filepath=cfg["body"])
body = [o for o in bpy.data.objects if o.type == "MESH"][0]
body.name = "body"

bpy.ops.wm.obj_import(filepath=cfg["garment"])
garment = [o for o in bpy.data.objects if o.type == "MESH" and o is not body][0]
garment.name = "garment"

points = [body.matrix_world @ v.co for v in body.data.vertices]
xs = [p.x for p in points]
ys = [p.y for p in points]
zs = [p.z for p in points]
x_mid = (min(xs) + max(xs)) / 2
height = max(zs) - min(zs)
print(f"body bounds x[{min(xs):.1f},{max(xs):.1f}] z[{min(zs):.1f},{max(zs):.1f}] height {height:.1f}")

# Arm axis per side: the shoulder end of the arm band and the farthest vertex.
bones = {}
for side, tag in ((1, "L"), (-1, "R")):
    outer = [p for p in points if (p.x - x_mid) * side > 0.16 * height]
    if not outer:
        raise SystemExit("no arm vertices found")
    wrist = max(outer, key=lambda p: (p.x - x_mid) * side)
    band = [p for p in outer if abs((p.x - x_mid) * side - 0.17 * height) < 0.03 * height]
    shoulder = max(band, key=lambda p: p.z) if band else Vector((x_mid + side * 0.17 * height, 0, wrist.z + 0.2 * height))
    elbow = shoulder + (wrist - shoulder) * 0.55
    bones[tag] = (shoulder, elbow, wrist)
    print(f"arm {tag}: shoulder {tuple(round(v,1) for v in shoulder)} wrist {tuple(round(v,1) for v in wrist)}")

arm_data = bpy.data.armatures.new("rig")
rig = bpy.data.objects.new("rig", arm_data)
bpy.context.collection.objects.link(rig)
bpy.context.view_layer.objects.active = rig
bpy.ops.object.mode_set(mode="EDIT")

spine_head = Vector((x_mid, 0.0, min(zs) + 0.45 * height))
spine_tail = Vector((x_mid, 0.0, min(zs) + 0.75 * height))
root = arm_data.edit_bones.new("spine")
root.head = spine_head
root.tail = spine_tail

chest = arm_data.edit_bones.new("chest")
chest.head = spine_tail
chest.tail = Vector((x_mid, 0.0, min(zs) + 0.86 * height))
chest.parent = root
chest.use_connect = True

for tag, (shoulder, elbow, wrist) in bones.items():
    clavicle = arm_data.edit_bones.new(f"clavicle.{tag}")
    clavicle.head = Vector((x_mid, 0.0, min(zs) + 0.84 * height))
    clavicle.tail = shoulder
    clavicle.parent = chest
    upper = arm_data.edit_bones.new(f"upper_arm.{tag}")
    upper.head = shoulder
    upper.tail = elbow
    upper.parent = clavicle
    fore = arm_data.edit_bones.new(f"forearm.{tag}")
    fore.head = elbow
    fore.tail = wrist
    fore.parent = upper
    fore.use_connect = True

# Leg bones: without them the heat solver gives the legs to whichever arm or
# spine bone happens to be closest, and posing the arms bends the knees.
for tag, side in (("L", 1), ("R", -1)):
    hip = Vector((x_mid + side * 0.09 * height, 0.0, min(zs) + 0.52 * height))
    knee = Vector((x_mid + side * 0.09 * height, 0.0, min(zs) + 0.28 * height))
    ankle = Vector((x_mid + side * 0.07 * height, 0.0, min(zs) + 0.04 * height))
    thigh = arm_data.edit_bones.new(f"thigh.{tag}")
    thigh.head = hip
    thigh.tail = knee
    thigh.parent = root
    shin = arm_data.edit_bones.new(f"shin.{tag}")
    shin.head = knee
    shin.tail = ankle
    shin.parent = thigh
    shin.use_connect = True

bpy.ops.object.mode_set(mode="OBJECT")

bpy.ops.object.select_all(action="DESELECT")
body.select_set(True)
rig.select_set(True)
bpy.context.view_layer.objects.active = rig
bpy.ops.object.parent_set(type="ARMATURE_AUTO")
print(f"bound {body.name} with automatic weights")

# The garment is never authored around these bones, so heat weights give it an
# arbitrary blend. Copy the weights of the nearest body vertex instead: the
# garment then follows the same skin as the body it covers.
from mathutils import kdtree

size = len(body.data.vertices)
tree = kdtree.KDTree(size)
for index, vertex in enumerate(body.data.vertices):
    tree.insert(body.matrix_world @ vertex.co, index)
tree.balance()

groups = {group.name: garment.vertex_groups.new(name=group.name) for group in body.vertex_groups}
body_groups = {group.index: group.name for group in body.vertex_groups}
for index, vertex in enumerate(garment.data.vertices):
    _, nearest, _ = tree.find(garment.matrix_world @ vertex.co)
    for membership in body.data.vertices[nearest].groups:
        name = body_groups.get(membership.group)
        if name is not None and membership.weight > 0.01:
            groups[name].add([index], membership.weight, "REPLACE")

garment.parent = rig
modifier = garment.modifiers.new(name="Armature", type="ARMATURE")
modifier.object = rig
print(f"bound {garment.name} by weight transfer from {body.name}")

bpy.context.view_layer.objects.active = rig
bpy.ops.object.mode_set(mode="POSE")
for tag in ("L", "R"):
    pose = rig.pose.bones[f"upper_arm.{tag}"]
    rest = pose.bone.matrix_local.copy()
    head = rest.translation.copy()
    current = (pose.bone.tail_local - pose.bone.head_local).normalized()
    # angle measured from the +X axis in the XZ plane; a horizontal arm points
    # to +X on the left side and to -X on the right side
    angle_now = math.atan2(current.z, current.x)
    target = 0.0 if tag == "L" else math.pi
    turn = target - angle_now
    pivot = Matrix.Translation(head)
    # a positive Y-rotation maps +X toward -Z, so raising the arm takes -turn
    rotation = Matrix.Rotation(-turn, 4, "Y")
    pose.matrix = pivot @ rotation @ pivot.inverted() @ rest

    # Straighten the elbow: the forearm keeps its authored bend when only the
    # upper arm is rotated, so the chain is rotated again about the elbow until
    # it is parallel to the ground in the same direction as the upper arm.
    forearm = rig.pose.bones[f"forearm.{tag}"]
    rest_fore = forearm.bone.matrix_local.copy()
    fore_dir = (forearm.bone.tail_local - forearm.bone.head_local).normalized()
    fore_angle = math.atan2(fore_dir.z, fore_dir.x) + turn
    extra = (0.0 if tag == "L" else math.pi) - fore_angle
    elbow = rest_fore.translation.copy()
    fore_pivot = Matrix.Translation(elbow)
    fore_rotation = Matrix.Rotation(-extra, 4, "Y")
    forearm.matrix = fore_pivot @ fore_rotation @ fore_pivot.inverted() @ rest_fore
    print(
        f"posed upper_arm.{tag} by {math.degrees(turn):.1f} deg, "
        f"forearm straightened by {math.degrees(extra):.1f} deg"
    )
bpy.ops.object.mode_set(mode="OBJECT")
bpy.context.view_layer.update()

# Write the deformed mesh by hand: the OBJ exporter kept including every
# object in the scene, and the evaluated mesh is exactly what the renderer needs.
depsgraph = bpy.context.evaluated_depsgraph_get()
for mesh, out in ((body, cfg["out_body"]), (garment, cfg["out_garment"])):
    evaluated = mesh.evaluated_get(depsgraph)
    data = evaluated.to_mesh()
    with open(out, "w", encoding="utf-8") as sink:
        for vertex in data.vertices:
            world = evaluated.matrix_world @ vertex.co
            # OBJ is Y-up: convert from Blender's Z-up so a default import lands
            # the figure exactly where it was posed
            sink.write(f"v {world.x:.6f} {world.z:.6f} {-world.y:.6f}\n")
        for polygon in data.polygons:
            sink.write("f " + " ".join(str(i + 1) for i in polygon.vertices) + "\n")
    print(f"wrote {out}: {len(data.vertices)} verts, {len(data.polygons)} faces")
    evaluated.to_mesh_clear()