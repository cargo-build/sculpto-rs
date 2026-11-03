"""Import a native and a Sculpto body, colour the deviation, and save a .blend.

Usage:

    blender --background --factory-startup --python tools/visual_parity.py -- \
        native.obj sculpto.obj parity.blend

Opening the saved file gives a framed camera, a sun, the Sculpto body coloured
green (match) to red (>= 0.1 source units of deviation per vertex) and the
native body as a red wireframe on top. Press Numpad 0 for the camera view and
Alt+H to toggle the native wire.

The numeric maximum is printed and also stored as a text block in the file.
"""

import sys

import bpy
from mathutils import Vector

# A factory-startup scene ships a Cube, Camera and Light; they only add noise to
# the comparison and are removed here (a user scene is not touched).
for default in ("Cube", "Camera", "Light"):
    obj = bpy.data.objects.get(default)
    if obj is not None and obj.name == default:
        bpy.data.objects.remove(obj, do_unlink=True)

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
if len(argv) < 2:
    raise SystemExit(
        "usage: blender --background --python tools/visual_parity.py -- "
        "native.obj sculpto.obj [out.blend]"
    )


def load(path: str, name: str):
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=path)
    new = [obj for obj in bpy.data.objects if obj not in before]
    if not new:
        raise SystemExit(f"import produced no object: {path}")
    obj = new[0]
    obj.name = name
    obj.hide_set(False)
    return obj


def material(name: str, color: tuple[float, float, float, float]):
    mat = bpy.data.materials.new(name)
    # Solid/viewport shading uses diffuse_color; the node color alone leaves the
    # mesh gray in the default view.
    mat.diffuse_color = color
    mat.use_nodes = True
    principled = next(
        node for node in mat.node_tree.nodes if node.type == "BSDF_PRINCIPLED"
    )
    principled.inputs["Base Color"].default_value = color
    principled.inputs["Roughness"].default_value = 0.6
    return mat


native = load(argv[0], "native")
ours = load(argv[1], "sculpto")

native_vertices = native.data.vertices
sculpto_vertices = ours.data.vertices
count = min(len(native_vertices), len(sculpto_vertices))
deviation = [
    (native_vertices[i].co - sculpto_vertices[i].co).length for i in range(count)
]
maximum = max(deviation, default=0.0)

mesh = ours.data
attribute = mesh.color_attributes.new(
    name="deviation", type="FLOAT_COLOR", domain="POINT"
)
for index, value in enumerate(deviation):
    red = min(value / 0.1, 1.0)
    attribute.data[index].color = (red, 1.0 - red, 0.0, 1.0)

# Solid view shows diffuse_color, not vertex colours: green for a match, red
# otherwise. Material Preview shows the per-vertex gradient through the nodes.
status_color = (0.1, 1.0, 0.1, 1.0) if maximum <= 1e-6 else (1.0, 0.1, 0.1, 1.0)
deviation_material = material("sculpto-deviation", status_color)
colors = deviation_material.node_tree.nodes.new("ShaderNodeVertexColor")
colors.layer_name = attribute.name
deviation_material.node_tree.links.new(
    colors.outputs["Color"], deviation_material.node_tree.nodes["Principled BSDF"].inputs["Base Color"]
)
mesh.materials.append(deviation_material)

native.data.materials.append(material("native-wire", (1.0, 0.1, 0.1, 1.0)))
native.display_type = "WIRE"

# Frame the body: camera above and in front, aimed at the bounding-box centre.
corners = [ours.matrix_world @ Vector(corner) for corner in ours.bound_box]
center = sum(corners, Vector()) / len(corners)
size = max((corner - center).length for corner in corners)

bpy.ops.object.camera_add(
    location=center + Vector((0.0, -2.2 * size, 0.35 * size))
)
camera = bpy.context.object
camera.name = "parity-camera"
bpy.context.scene.camera = camera

target = bpy.data.objects.new("parity-target", None)
bpy.context.collection.objects.link(target)
target.location = center
track = camera.constraints.new(type="TRACK_TO")
track.target = target

bpy.ops.object.light_add(type="SUN", location=center + Vector((1.0, -1.0, 2.0)))
bpy.context.object.data.energy = 3.0

# A label in the scene so the view is self-describing.
font = bpy.data.curves.new("parity-label", type="FONT")
font.body = "PARITY " + ("MATCH" if maximum <= 1e-6 else "DIFFERENCE")
font.align_x = "CENTER"
font.size = max(size * 0.12, 0.5)
label = bpy.data.objects.new("parity-label", font)
bpy.context.collection.objects.link(label)
label.parent = camera
label.location = (0.0, -0.15 * size, -1.6 * size)
label_material = material(
    "parity-label", (0.1, 1.0, 0.1, 1.0) if maximum <= 1e-6 else (1.0, 0.1, 0.1, 1.0)
)
font.materials.append(label_material)

report = (
    f"vertices: native {len(native_vertices)}, sculpto {len(sculpto_vertices)}\n"
    f"max deviation: {maximum:.9f} source units\n"
    + ("MATCH" if maximum <= 1e-6 else "DIFFERENCE")
)
text = bpy.data.texts.new("parity-report")
text.write(report)
print(report.replace("\n", " | "))

if len(argv) >= 3:
    bpy.ops.wm.save_as_mainfile(filepath=argv[2])
    print(f"saved {argv[2]}")