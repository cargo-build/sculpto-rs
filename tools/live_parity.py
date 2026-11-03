"""Live parity view: re-import the OBJs whenever they change on disk.

Usage (GUI):

    blender --factory-startup --python tools/live_parity.py -- \
        /tmp/sculpto-visual/native.obj /tmp/sculpto-visual/sculpto.obj

Keep the window open and rebuild with `just visual …`; the scene updates within
about a second, without reopening the file. The native mesh is a red wireframe,
the Sculpto mesh is green when it matches and red otherwise, and a camera label
shows the maximum deviation.

`--once` performs a single refresh and exits (used by the headless check).
"""

import os
import sys

import bpy
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
once = "--once" in argv
argv = [argument for argument in argv if argument != "--once"]
if len(argv) < 2:
    raise SystemExit(
        "usage: blender --python tools/live_parity.py -- native.obj sculpto.obj [--once]"
    )

NATIVE_PATH, SCULPTO_PATH = argv[0], argv[1]

for default in ("Cube", "Camera", "Light"):
    obj = bpy.data.objects.get(default)
    if obj is not None and obj.name == default:
        bpy.data.objects.remove(obj, do_unlink=True)

state: dict = {"mtimes": None, "objects": [], "camera": None}


def material(name: str, color: tuple[float, float, float, float]):
    mat = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    mat.diffuse_color = color
    mat.use_nodes = True
    principled = next(
        node for node in mat.node_tree.nodes if node.type == "BSDF_PRINCIPLED"
    )
    principled.inputs["Base Color"].default_value = color
    return mat


def load(path: str, name: str):
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=path)
    new = [obj for obj in bpy.data.objects if obj not in before]
    if not new:
        raise RuntimeError(f"import produced no object: {path}")
    obj = new[0]
    obj.name = name
    return obj


def drop_previous() -> None:
    for obj in state["objects"]:
        if obj.name in bpy.data.objects:
            bpy.data.objects.remove(obj, do_unlink=True)
    state["objects"] = []


def build_label(camera, size: float, maximum: float) -> None:
    font = bpy.data.curves.get("parity-label") or bpy.data.curves.new(
        "parity-label", type="FONT"
    )
    font.body = "PARITY " + ("MATCH" if maximum <= 1e-6 else "DIFFERENCE")
    font.align_x = "CENTER"
    font.size = max(size * 0.12, 0.5)
    label = bpy.data.objects.get("parity-label")
    if label is None:
        label = bpy.data.objects.new("parity-label", font)
        bpy.context.collection.objects.link(label)
    label.parent = camera
    label.location = (0.0, -0.15 * size, -1.6 * size)
    while font.materials:
        font.materials.pop()
    font.materials.append(
        material(
            "parity-label",
            (0.1, 1.0, 0.1, 1.0) if maximum <= 1e-6 else (1.0, 0.1, 0.1, 1.0),
        )
    )


def refresh() -> float | None:
    try:
        mtimes = (
            os.path.getmtime(NATIVE_PATH),
            os.path.getmtime(SCULPTO_PATH),
        )
    except OSError:
        return 1.0
    if mtimes == state["mtimes"]:
        return 1.0
    state["mtimes"] = mtimes

    drop_previous()
    native = load(NATIVE_PATH, "native")
    sculpto = load(SCULPTO_PATH, "sculpto")
    state["objects"] = [native, sculpto]

    deviation = [
        (native.data.vertices[i].co - sculpto.data.vertices[i].co).length
        for i in range(min(len(native.data.vertices), len(sculpto.data.vertices)))
    ]
    maximum = max(deviation, default=0.0)

    status = (0.1, 1.0, 0.1, 1.0) if maximum <= 1e-6 else (1.0, 0.1, 0.1, 1.0)
    mesh = sculpto.data
    while mesh.color_attributes:
        mesh.color_attributes.remove(mesh.color_attributes[0])
    attribute = mesh.color_attributes.new(
        name="deviation", type="FLOAT_COLOR", domain="POINT"
    )
    for index, value in enumerate(deviation):
        red = min(value / 0.1, 1.0)
        attribute.data[index].color = (red, 1.0 - red, 0.0, 1.0)
    while mesh.materials:
        mesh.materials.pop()
    deviation_material = material("sculpto-deviation", status)
    colors = deviation_material.node_tree.nodes.new("ShaderNodeVertexColor")
    colors.layer_name = attribute.name
    deviation_material.node_tree.links.new(
        colors.outputs["Color"],
        deviation_material.node_tree.nodes["Principled BSDF"].inputs["Base Color"],
    )
    mesh.materials.append(deviation_material)

    while native.data.materials:
        native.data.materials.pop()
    native.data.materials.append(material("native-wire", (1.0, 0.1, 0.1, 1.0)))
    native.display_type = "WIRE"

    corners = [sculpto.matrix_world @ Vector(corner) for corner in sculpto.bound_box]
    center = sum(corners, Vector()) / len(corners)
    size = max((corner - center).length for corner in corners)

    camera = state["camera"]
    if camera is None or camera.name not in bpy.data.objects:
        bpy.ops.object.camera_add(
            location=center + Vector((0.0, -2.2 * size, 0.35 * size))
        )
        camera = bpy.context.object
        camera.name = "parity-camera"
        target = bpy.data.objects.get("parity-target")
        if target is None:
            target = bpy.data.objects.new("parity-target", None)
            bpy.context.collection.objects.link(target)
        target.location = center
        track = camera.constraints.new(type="TRACK_TO")
        track.target = target
        if bpy.data.objects.get("Sun") is None:
            bpy.ops.object.light_add(
                type="SUN", location=center + Vector((1.0, -1.0, 2.0))
            )
            bpy.context.object.data.energy = 3.0
        state["camera"] = camera
    bpy.context.scene.camera = camera
    build_label(camera, size, maximum)

    print(
        f"refresh: {len(sculpto.data.vertices)} vertices, "
        f"max {maximum:.9f} u, "
        + ("MATCH" if maximum <= 1e-6 else "DIFFERENCE")
    )
    return 1.0


refresh()
if once:
    raise SystemExit(0)
bpy.app.timers.register(refresh, first_interval=1.0, persistent=True)