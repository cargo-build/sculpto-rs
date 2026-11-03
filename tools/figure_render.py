"""Render a dressed figure from OBJ parts or a concept .blend, front and back.

Usage:
    blender --background --factory-startup --python tools/figure_render.py -- \
        out=<dir> label=<text> [blend=<path>] [skin=<obj>] [cloth=<obj>] ...

`skin=` and `cloth=` repeat; cloth parts get the garment material. `blend=`
opens that .blend; `fbx=` imports an FBX instead of OBJs. Writes <out>/front.png and
<out>/back.png from the same framing. Review aid: gates are the CLI numbers.
"""

import os
import sys

import bpy
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1 :]
config = dict(item.split("=", 1) for item in argv if "=" in item)
if "out" not in config:
    raise SystemExit("missing `out=...`")
os.makedirs(config["out"], exist_ok=True)
label = config.get("label", "figure")

skins = [v for k, v in config.items() if k.startswith("skin")]
cloths = [v for k, v in config.items() if k.startswith("cloth")]


def paint(obj, name, color):
    material = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    material.diffuse_color = (*color, 1.0)
    material.use_nodes = True
    principled = next(
        node for node in material.node_tree.nodes if node.type == "BSDF_PRINCIPLED"
    )
    principled.inputs["Base Color"].default_value = (*color, 1.0)
    while obj.data.materials:
        obj.data.materials.pop()
    obj.data.materials.append(material)


if "blend" in config:
    bpy.ops.wm.open_mainfile(filepath=os.path.abspath(config["blend"]))
elif "fbx" in config:
    before = set(bpy.data.objects)
    bpy.ops.wm.fbx_import(filepath=os.path.abspath(config["fbx"]))
    imported = [obj for obj in bpy.data.objects if obj not in before]
    if not imported:
        raise SystemExit(f"fbx import produced no object: {config['fbx']}")
    # Mixamo exports sit on a huge ground platform; drop flat oversized meshes
    # so the figure fills the frame.
    for obj in imported:
        if obj.type == "MESH":
            size = obj.dimensions
            if max(size) > 6.0 * min(size.z, 0.01 + max(size.x, size.y)):
                bpy.data.objects.remove(obj, do_unlink=True)
                continue
            if max(size) > 400.0:
                bpy.data.objects.remove(obj, do_unlink=True)
else:
    for default in ("Cube", "Camera", "Light"):
        obj = bpy.data.objects.get(default)
        if obj is not None and obj.name == default:
            bpy.data.objects.remove(obj, do_unlink=True)
    for path in skins + cloths:
        before = set(bpy.data.objects)
        bpy.ops.wm.obj_import(filepath=os.path.abspath(path))
        new = [obj for obj in bpy.data.objects if obj not in before]
        if not new:
            raise SystemExit(f"import produced no object: {path}")
        if path in skins:
            paint(new[0], "sculpto-skin", (0.62, 0.48, 0.40))
        else:
            paint(new[0], "sculpto-garment", (0.18, 0.32, 0.75))

objects = [obj for obj in bpy.data.objects if obj.type == "MESH"]
if not objects:
    raise SystemExit("no meshes to render")
corners = []
for obj in objects:
    corners += [obj.matrix_world @ Vector(c) for c in obj.bound_box]
center = sum(corners, Vector()) / len(corners)
size = max((corner - center).length for corner in corners)

camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
target = bpy.data.objects.new("target", None)
target.location = center
bpy.context.collection.objects.link(target)
bpy.context.collection.objects.link(camera)
track = camera.constraints.new(type="TRACK_TO")
track.target = target
bpy.context.scene.camera = camera
sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
sun.location = center + Vector((1.0, -1.0, 2.0))
sun.data.energy = 3.0
bpy.context.collection.objects.link(sun)

scene = bpy.context.scene
engine = config.get("engine", "workbench")
if engine == "eevee":
    scene.render.engine = "BLENDER_EEVEE"
else:
    scene.render.engine = "BLENDER_WORKBENCH"
    scene.display.shading.light = "STUDIO"
    scene.display.shading.color_type = "MATERIAL"
scene.render.resolution_x = 640
scene.render.resolution_y = 960

up = Vector((0.0, 0.0, 1.0)) if max(abs(c[2] - center[2]) for c in corners) > 0.5 * size else Vector((0.0, 1.0, 0.0))
for name, direction in (("front", -1.0), ("back", 1.0)):
    camera.location = center + Vector((0.0, direction * 2.8 * size, 0.0))
    scene.render.filepath = os.path.abspath(os.path.join(config["out"], f"{name}.png"))
    bpy.ops.render.render(write_still=True)
    print(f"render: {scene.render.filepath}")
print(f"figure: {label}")