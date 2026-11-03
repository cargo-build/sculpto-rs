"""Render a garment before and after an aura fit, for human review.

Usage (headless):

    blender --background --factory-startup --python tools/fit_review.py -- \
        body=<body.obj> before=<garment.obj> after=<fitted.obj> \
        out=<dir> [label=<text>]

Produces `<out>/before.png` and `<out>/after.png` from the same camera, with the
body in skin tone and the garment in blue. `frame=garment` frames body plus
garment instead of the body alone; `zoom=<f>` scales the camera distance. Review aid only: the fit gates are
the numbers `sculpto garment aura` prints.
"""

import os
import sys

import bpy
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1 :]
config = dict(item.split("=", 1) for item in argv if "=" in item)
for key in ("body", "before", "after", "out"):
    if key not in config:
        raise SystemExit(f"missing `{key}=...` argument")

label = config.get("label", "aura fit")
os.makedirs(config["out"], exist_ok=True)


def import_named(path, name):
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=os.path.abspath(path))
    new = [obj for obj in bpy.data.objects if obj not in before]
    if not new:
        raise SystemExit(f"import produced no object: {path}")
    new[0].name = name
    return new[0]


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


def reset():
    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj, do_unlink=True)


def frame(objects):
    corners = []
    for obj in objects:
        corners += [obj.matrix_world @ Vector(c) for c in obj.bound_box]
    center = sum(corners, Vector()) / len(corners)
    size = max((corner - center).length for corner in corners)
    camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
    zoom = float(config.get("zoom", "1.0"))
    camera.location = center + Vector((0.0, -2.6 * size * zoom, 0.15 * size))
    bpy.context.collection.objects.link(camera)
    track = camera.constraints.new(type="TRACK_TO")
    target = bpy.data.objects.new("target", None)
    target.location = center
    bpy.context.collection.objects.link(target)
    track.target = target
    bpy.context.scene.camera = camera
    sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
    sun.location = center + Vector((1.0, -1.0, 2.0))
    sun.data.energy = 3.0
    bpy.context.collection.objects.link(sun)


def render(path):
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_WORKBENCH"
    scene.display.shading.light = "STUDIO"
    scene.display.shading.color_type = "MATERIAL"
    scene.render.resolution_x = 720
    scene.render.resolution_y = 960
    scene.render.filepath = os.path.abspath(path)
    bpy.ops.render.render(write_still=True)
    print(f"render: {path}")


body = import_named(config["body"], "body")
paint(body, "sculpto-skin", (0.62, 0.48, 0.40))
before = import_named(config["before"], "garment")
paint(before, "sculpto-garment", (0.18, 0.32, 0.75))
frame([body, before] if config.get("frame") == "garment" else [body])
render(os.path.join(config["out"], "before.png"))
bpy.data.objects.remove(before, do_unlink=True)
after = import_named(config["after"], "garment")
paint(after, "sculpto-garment", (0.18, 0.32, 0.75))
render(os.path.join(config["out"], "after.png"))
print(f"review: {label}")