"""Diagnostic views: body, collision proxies, garment and the overlay.

Usage (headless):
  blender --background --factory-startup --python diagnose.py -- \
      body=<actor-body.obj> proxies=<proxies.obj> garment=<fitted.obj> \
      out=<dir> [views=body,proxies,garment,overlay]

The OBJs are sculpto display-frame files (the proxy OBJ carries `l` polyline
records). Each view renders the same camera with only the relevant parts
visible, so the fitting envelope, the collision proxies and the garment can be
inspected separately, per the adaptation contract.
"""

import math
import os
import sys

import bpy
from mathutils import Matrix

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
out_dir = cfg["out"]
views = cfg.get("views", "body,proxies,garment,overlay").split(",")
os.makedirs(out_dir, exist_ok=True)

for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj, do_unlink=True)

scene = bpy.context.scene
scene.render.engine = "BLENDER_WORKBENCH"
scene.display.shading.light = "STUDIO"
scene.display.shading.color_type = "OBJECT"
scene.render.resolution_x = 720
scene.render.resolution_y = 960
scene.render.film_transparent = True


def import_display(path, name, color):
    """Import a display-frame OBJ and turn it upright in Blender space."""
    bpy.ops.wm.obj_import(filepath=os.path.abspath(path))
    obj = bpy.context.selected_objects[0]
    obj.name = name
    obj.rotation_euler = (math.radians(90.0), 0.0, 0.0)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    obj.color = color
    return obj


body = import_display(cfg["body"], "Body", (0.82, 0.74, 0.66, 1.0))
garment = import_display(cfg["garment"], "Garment", (0.72, 0.25, 0.25, 1.0))

# Proxies: polyline records (v + l) rendered as bevelled curves.
proxy_verts, proxy_chains = [], []
for line in open(os.path.abspath(cfg["proxies"])):
    if line.startswith("v "):
        _, x, y, z = line.split()
        # Display frame (x, up, -depth) -> Blender world (x, depth, up).
        proxy_verts.append((float(x), -float(z), float(y)))
    elif line.startswith("l "):
        proxy_chains.append([int(i) - 1 for i in line.split()[1:]])

curve = bpy.data.curves.new("Proxies", "CURVE")
curve.dimensions = "3D"
curve.bevel_depth = 0.004
curve.bevel_resolution = 1
for chain in proxy_chains:
    spline = curve.splines.new("POLY")
    spline.points.add(len(chain) - 1)
    for point, index in zip(spline.points, chain):
        x, y, z = proxy_verts[index]
        point.co = (x, y, z, 1.0)
proxies = bpy.data.objects.new("Proxies", curve)
scene.collection.objects.link(proxies)
proxies.color = (0.2, 0.55, 0.95, 1.0)
curve.materials.append(None)

cam_data = bpy.data.cameras.new("Cam")
cam = bpy.data.objects.new("Cam", cam_data)
scene.collection.objects.link(cam)
scene.camera = cam
cam.location = (2.2, -2.2, 1.05)
cam.rotation_euler = (math.radians(82.0), 0.0, math.radians(45.0))

light_data = bpy.data.lights.new("Key", "SUN")
light = bpy.data.objects.new("Key", light_data)
scene.collection.objects.link(light)
light.rotation_euler = (math.radians(50.0), 0.0, math.radians(30.0))

visibility = {
    "body": {body: True, proxies: False, garment: False},
    "proxies": {body: False, proxies: True, garment: False},
    "garment": {body: False, proxies: False, garment: True},
    "overlay": {body: True, proxies: True, garment: True},
}
for view in views:
    for obj, visible in visibility[view].items():
        obj.hide_render = not visible
    scene.render.filepath = os.path.join(out_dir, f"{view}.png")
    bpy.ops.render.render(write_still=True)
    print(f"DIAGNOSE {view} -> {scene.render.filepath}")