"""Sculpto panel for Blender: body selector, presets, sliders and garment fit.

Run from the managed wrapper root:

    just slider-panel

Bodies and garments come from explicit `bodies=...` and `garments=...` JSON
catalogs supplied by the caller. The 3D Viewport sidebar shows a **Body**
switch, a **Preset** dropdown for that body, a filter, Bake, Reset, one 0-100
slider per row, and a **Garment** picker with a **Fit garment** button. A slider
change rebakes through the CLI about 0.4 s later and the fitted garment follows.

This file owns Blender state only: it calls `sculpto garment fit`, which binds
and applies the garment in `sculpto-mesh`, and imports the OBJ that command
writes. No mesh arithmetic lives here.

`--once` bakes one body and saves `sliders.blend`; `--verify` switches body and
prints both mesh digests; `--fit-check` fits a garment, moves a slider and lets
the CLI report how far body and garment travelled.
"""

import hashlib
import json
import os
import subprocess
import sys

import bpy
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
once = "--once" in argv
verify = "--verify" in argv
fit_check = "--fit-check" in argv
config = dict(item.split("=", 1) for item in argv if "=" in item)

for key in ("sculpto", "out"):
    if key not in config:
        raise SystemExit(f"missing `{key}=...` argument")

SCULPTO = os.path.abspath(config["sculpto"])
OUT = os.path.abspath(config["out"])
OUT_DIR = os.path.dirname(OUT)
os.makedirs(OUT_DIR, exist_ok=True)
GARMENT_OUT = os.path.join(OUT_DIR, "garment.obj")
GARMENT_AFTER = os.path.join(OUT_DIR, "garment-after.obj")
BODY_AFTER = os.path.join(OUT_DIR, "body-after.obj")

def load_bodies(path):
    with open(path, encoding="utf-8") as handle:
        raw = json.load(handle)["bodies"]
    resolved = []
    for body in raw:
        home = os.path.expanduser(body["home"])
        entry = {"name": body["name"], "home": home}
        for key in ("project", "presets"):
            entry[key] = os.path.expanduser(body[key].replace("{home}", home))
        entry["set"] = body["set"]
        entry["preset"] = body["preset"]
        entry["endpoint"] = body["endpoint"]
        resolved.append(entry)
    return resolved


def load_garments(path):
    with open(path, encoding="utf-8") as handle:
        raw = json.load(handle)["garments"]
    return [
        {
            "name": garment["name"],
            "body": garment["body"],
            "path": os.path.expanduser(garment["path"]),
            "shape": garment["shape"],
            "body_shape": garment["body_shape"],
        }
        for garment in raw
    ]


if "bodies" not in config or "garments" not in config:
    raise SystemExit("slider_panel requires explicit bodies=... and garments=... catalog paths")
BODIES = load_bodies(config["bodies"])
GARMENTS = load_garments(config["garments"])
STATE = {"index": 0, "presets": [], "garment": None}


def current_body():
    return BODIES[STATE["index"]]


def garments_for(body):
    return [garment for garment in GARMENTS if garment["body"] == body["name"]]


def run_presets(body):
    result = subprocess.run(
        [SCULPTO, "presets", "--presets", body["presets"]],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"presets failed: {result.stderr.strip()}")
    names = [line.split("\t", 1)[0] for line in result.stdout.splitlines() if line]
    if not names:
        raise SystemExit("the presets file is empty")
    return names


def run_sliders(body):
    result = subprocess.run(
        [
            SCULPTO,
            "sliders",
            "--project",
            body["project"],
            "--presets",
            body["presets"],
            "--set",
            body["set"],
            "--preset",
            body["preset"],
            "--endpoint",
            body["endpoint"],
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"sliders failed: {result.stderr.strip()}")
    rows = []
    for line in result.stdout.splitlines():
        if "\t" in line:
            name, value = line.split("\t", 1)
            rows.append((name, float(value)))
    return rows


def bake(body, values, out_path=None):
    path = out_path or OUT
    command = [
        SCULPTO,
        "bake",
        "--project",
        body["project"],
        "--presets",
        body["presets"],
        "--set",
        body["set"],
        "--preset",
        body["preset"],
        "--endpoint",
        body["endpoint"],
        "--home",
        body["home"],
        "--out",
        path,
    ]
    for name, value in values:
        command += ["--slider", f"{name}={value}"]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"sculpto bake failed: {result.stderr.strip()}")
        return None
    print(result.stdout.strip())
    return path


def current_method():
    return getattr(bpy.context.scene, "sculpto_fit_method", "BINDINGS")


def current_clearance():
    return round(float(getattr(bpy.context.scene, "sculpto_aura_clearance", 0.6)), 3)


def run_garment_fit(garment, body_path, out_path):
    if current_method() == "AURA":
        source = os.path.join(OUT_DIR, "garment-source.obj")
        export = subprocess.run(
            [
                SCULPTO,
                "export-obj",
                "--nif",
                garment["path"],
                "--shape",
                garment["shape"],
                "--out",
                source,
            ],
            capture_output=True,
            text=True,
        )
        if export.returncode != 0:
            print(f"garment export failed: {export.stderr.strip()}")
            return None
        print(export.stdout.strip())
        return run_aura_fit(source, body_path, out_path)
    return run_binding_fit(garment, body_path, out_path)


def run_aura_fit(garment_path, body_path, out_path):
    command = [
        SCULPTO,
        "garment",
        "aura",
        "--body",
        body_path,
        "--garment",
        garment_path,
        "--out",
        out_path,
        "--clearance",
        str(current_clearance()),
        "--mode",
        "press",
    ]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"garment aura failed: {result.stderr.strip()}")
        return None
    print(result.stdout.strip())
    return out_path


def run_binding_fit(garment, body_path, out_path):
    command = [
        SCULPTO,
        "garment",
        "fit",
        "--nif",
        garment["path"],
        "--shape",
        garment["shape"],
        "--body-shape",
        garment["body_shape"],
        "--body",
        body_path,
        "--out",
        out_path,
    ]
    if garment.get("body_after"):
        command += [
            "--body-after",
            garment["body_after"],
            "--out-after",
            garment["out_after"],
        ]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"garment fit failed: {result.stderr.strip()}")
        return None
    print(result.stdout.strip())
    return out_path


def import_mesh(path):
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=path)
    new = [obj for obj in bpy.data.objects if obj not in before]
    return new[0] if new else None


def garment_material():
    material = bpy.data.materials.get("sculpto-garment") or bpy.data.materials.new(
        "sculpto-garment"
    )
    material.diffuse_color = (0.18, 0.32, 0.75, 1.0)
    material.use_nodes = True
    principled = next(
        node for node in material.node_tree.nodes if node.type == "BSDF_PRINCIPLED"
    )
    principled.inputs["Base Color"].default_value = (0.18, 0.32, 0.75, 1.0)
    return material


def replace_garment_mesh(path):
    garment = bpy.data.objects.get("garment")
    imported = import_mesh(path)
    if imported is None:
        print(f"garment import produced no object: {path}")
        return None
    if garment is None:
        garment = imported
        garment.name = "garment"
    else:
        garment.data = imported.data
        bpy.data.objects.remove(imported, do_unlink=True)
    while garment.data.materials:
        garment.data.materials.pop()
    garment.data.materials.append(garment_material())
    return garment


def fit_garment(garment):
    if run_garment_fit(garment, OUT, GARMENT_OUT) is None:
        return None
    if replace_garment_mesh(GARMENT_OUT) is None:
        return None
    STATE["garment"] = garment
    print(f"garment fitted: {garment['name']} on {current_body()['name']}")
    return STATE["garment"]


def apply_garment():
    garment = STATE["garment"]
    if garment is None:
        return None
    if run_garment_fit(garment, OUT, GARMENT_OUT) is None:
        return None
    return replace_garment_mesh(GARMENT_OUT)


def ensure_scene():
    for default in ("Cube", "Camera", "Light"):
        obj = bpy.data.objects.get(default)
        if obj is not None and obj.name == default:
            bpy.data.objects.remove(obj, do_unlink=True)
    sculpto = bpy.data.objects.get("sculpto")
    if sculpto is None:
        sculpto = import_mesh(OUT)
        if sculpto is None:
            raise SystemExit(f"import produced no object: {OUT}")
        sculpto.name = "sculpto"
    material = bpy.data.materials.get("sculpto-skin") or bpy.data.materials.new(
        "sculpto-skin"
    )
    material.diffuse_color = (0.62, 0.48, 0.40, 1.0)
    material.use_nodes = True
    principled = next(
        node for node in material.node_tree.nodes if node.type == "BSDF_PRINCIPLED"
    )
    principled.inputs["Base Color"].default_value = (0.62, 0.48, 0.40, 1.0)
    while sculpto.data.materials:
        sculpto.data.materials.pop()
    sculpto.data.materials.append(material)

    if bpy.context.scene.camera is None:
        corners = [sculpto.matrix_world @ Vector(c) for c in sculpto.bound_box]
        center = sum(corners, Vector()) / len(corners)
        size = max((corner - center).length for corner in corners)
        bpy.ops.object.camera_add(
            location=center + Vector((0.0, -3.4 * size, 0.2 * size))
        )
        camera = bpy.context.object
        camera.name = "sculpto-camera"
        target = bpy.data.objects.get("sculpto-target")
        if target is None:
            target = bpy.data.objects.new("sculpto-target", None)
            bpy.context.collection.objects.link(target)
        target.location = center
        track = camera.constraints.new(type="TRACK_TO")
        track.target = target
        bpy.context.scene.camera = camera
        if bpy.data.objects.get("Sun") is None:
            bpy.ops.object.light_add(
                type="SUN", location=center + Vector((1.0, -1.0, 2.0))
            )
            bpy.context.object.data.energy = 3.0


def reload_mesh(path):
    imported = import_mesh(path)
    if imported is None:
        return
    sculpto = bpy.data.objects.get("sculpto")
    if sculpto is None:
        imported.name = "sculpto"
    else:
        sculpto.data = imported.data
        bpy.data.objects.remove(imported, do_unlink=True)


def values_from_scene():
    return [(item.name, item.value) for item in bpy.context.scene.sculpto_sliders]


def schedule_bake():
    bpy.context.scene.sculpto_dirty = True


def body_items(_self, _context):
    return [(body["name"], body["name"], "") for body in BODIES]


def preset_items(_self, _context):
    return [(name, name, "") for name in STATE["presets"]]


def garment_items(_self, _context):
    return [(garment["name"], garment["name"], "") for garment in GARMENTS]


def body_changed(self, context):
    name = self.sculpto_body
    STATE["index"] = next(
        index for index, body in enumerate(BODIES) if body["name"] == name
    )
    body = current_body()
    STATE["presets"] = run_presets(body)
    context.scene.sculpto_preset = body["preset"]
    choices = garments_for(body)
    context.scene.sculpto_garment = choices[0]["name"] if choices else ""
    context.scene.sculpto_dirty = True


def preset_changed(self, context):
    current_body()["preset"] = self.sculpto_preset
    populate()
    context.scene.sculpto_dirty = True


def garment_changed(self, context):
    name = self.sculpto_garment
    garment = next((entry for entry in GARMENTS if entry["name"] == name), None)
    if garment is not None:
        context.scene.sculpto_garment_path = garment["path"]
        context.scene.sculpto_garment_shape = garment["shape"]
        context.scene.sculpto_garment_body_shape = garment["body_shape"]


class SculptoSlider(bpy.types.PropertyGroup):
    name: bpy.props.StringProperty()
    value: bpy.props.IntProperty(
        name="",
        subtype="PERCENTAGE",
        min=0,
        max=100,
        update=lambda self, context: schedule_bake(),
    )


class SCULPTO_OT_set_body(bpy.types.Operator):
    bl_idname = "sculpto.set_body"
    bl_label = "Set body"
    bl_options = {"INTERNAL"}
    name: bpy.props.StringProperty()

    def execute(self, context):
        context.scene.sculpto_body = self.name
        return {"FINISHED"}


class SCULPTO_OT_bake(bpy.types.Operator):
    bl_idname = "sculpto.bake"
    bl_label = "Bake"
    bl_description = "Rebuild the body from the current slider values"

    def execute(self, context):
        path = bake(current_body(), values_from_scene())
        if path:
            reload_mesh(path)
            apply_garment()
        return {"FINISHED"}


class SCULPTO_OT_reset(bpy.types.Operator):
    bl_idname = "sculpto.reset"
    bl_label = "Reset to preset"

    def execute(self, context):
        rows = run_sliders(current_body())
        items = context.scene.sculpto_sliders
        for index, (name, value) in enumerate(rows):
            if index < len(items):
                items[index].value = int(round(value))
        schedule_bake()
        return {"FINISHED"}


class SCULPTO_OT_fit(bpy.types.Operator):
    bl_idname = "sculpto.fit"
    bl_label = "Fit garment"
    bl_description = "Conform the garment NIF to the current body"

    def execute(self, context):
        scene = context.scene
        garment = None
        for entry in GARMENTS:
            if entry["path"] == scene.sculpto_garment_path:
                garment = entry
                break
        if garment is None:
            garment = {
                "name": os.path.basename(scene.sculpto_garment_path),
                "body": scene.sculpto_body,
                "path": scene.sculpto_garment_path,
                "shape": scene.sculpto_garment_shape,
                "body_shape": scene.sculpto_garment_body_shape,
            }
        fit_garment(garment)
        return {"FINISHED"}


class SCULPTO_OT_remove_garment(bpy.types.Operator):
    bl_idname = "sculpto.remove_garment"
    bl_label = "Remove garment"

    def execute(self, context):
        garment = bpy.data.objects.get("garment")
        if garment is not None:
            bpy.data.objects.remove(garment, do_unlink=True)
        STATE["garment"] = None
        return {"FINISHED"}


class SCULPTO_PT_panel(bpy.types.Panel):
    bl_label = "Sculpto"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Item"

    def draw(self, context):
        layout = self.layout
        scene = context.scene
        layout.label(text="Body")
        active = current_body()["name"]
        for entry in BODIES:
            is_active = entry["name"] == active
            operator = layout.operator(
                "sculpto.set_body",
                text=entry["name"],
                icon="RADIOBUT_ON" if is_active else "RADIOBUT_OFF",
                depress=is_active,
            )
            operator.name = entry["name"]
        layout.prop(scene, "sculpto_preset", text="Preset")
        body = current_body()
        layout.label(text=f"{body['set']} · {body['endpoint']} endpoint")
        layout.prop(scene, "sculpto_filter", text="", icon="VIEWZOOM")
        row = layout.row(align=True)
        row.operator("sculpto.bake", icon="MESH_DATA")
        row.operator("sculpto.reset", icon="LOOP_BACK")

        layout.separator()
        layout.label(text="Garment")
        layout.prop(scene, "sculpto_fit_method", text="Method")
        if scene.sculpto_fit_method == "AURA":
            layout.prop(scene, "sculpto_aura_clearance", text="Clearance")
        choices = garments_for(body)
        if choices:
            layout.prop(scene, "sculpto_garment", text="")
        layout.prop(scene, "sculpto_garment_path", text="NIF")
        layout.prop(scene, "sculpto_garment_shape", text="Shape")
        row = layout.row(align=True)
        row.scale_y = 1.3
        row.operator("sculpto.fit", icon="MOD_SHRINKWRAP")
        row.operator("sculpto.remove_garment", icon="X")
        fitted = STATE["garment"]
        if fitted is not None:
            layout.label(text=f"fitted: {fitted['name']}", icon="CHECKMARK")

        layout.separator()
        needle = scene.sculpto_filter.lower()
        shown = 0
        for item in scene.sculpto_sliders:
            if needle and needle not in item.name.lower():
                continue
            layout.prop(item, "value", text=item.name, slider=True)
            shown += 1
            if shown >= 80:
                layout.label(text="… filter to narrow the list")
                break


CLASSES = (
    SculptoSlider,
    SCULPTO_OT_set_body,
    SCULPTO_OT_bake,
    SCULPTO_OT_reset,
    SCULPTO_OT_fit,
    SCULPTO_OT_remove_garment,
    SCULPTO_PT_panel,
)


def register_properties():
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    bpy.types.Scene.sculpto_sliders = bpy.props.CollectionProperty(type=SculptoSlider)
    bpy.types.Scene.sculpto_body = bpy.props.EnumProperty(
        name="Body", items=body_items, update=body_changed
    )
    bpy.types.Scene.sculpto_preset = bpy.props.EnumProperty(
        name="Preset", items=preset_items, update=preset_changed
    )
    bpy.types.Scene.sculpto_filter = bpy.props.StringProperty(name="Filter")
    bpy.types.Scene.sculpto_garment = bpy.props.EnumProperty(
        name="Garment", items=garment_items, update=garment_changed
    )
    bpy.types.Scene.sculpto_fit_method = bpy.props.EnumProperty(
        name="Method",
        items=[
            (
                "BINDINGS",
                "Bindings",
                "bind each vertex to the body shape the garment was authored on",
            ),
            (
                "AURA",
                "Aura",
                "cross-section envelope of the body with clearance and slack",
            ),
        ],
        default="BINDINGS",
    )
    bpy.types.Scene.sculpto_aura_clearance = bpy.props.FloatProperty(
        name="Clearance", default=0.6, min=0.0, max=5.0
    )
    bpy.types.Scene.sculpto_garment_path = bpy.props.StringProperty(name="NIF")
    bpy.types.Scene.sculpto_garment_shape = bpy.props.StringProperty(name="Shape")
    bpy.types.Scene.sculpto_garment_body_shape = bpy.props.StringProperty(
        name="Body shape"
    )
    bpy.types.Scene.sculpto_dirty = bpy.props.BoolProperty(default=False)


def populate():
    rows = run_sliders(current_body())
    items = bpy.context.scene.sculpto_sliders
    items.clear()
    for name, value in rows:
        item = items.add()
        item.name = name
        item.value = int(round(value))
    print(f"sculpto: {current_body()['name']} · {len(rows)} sliders")


def tick():
    scene = bpy.context.scene
    if not getattr(scene, "sculpto_dirty", False):
        return 0.25
    scene.sculpto_dirty = False
    path = bake(current_body(), values_from_scene())
    if path:
        reload_mesh(path)
        apply_garment()
    return 0.25


def open_sidebar():
    screen = bpy.context.screen
    if screen is not None:
        for area in screen.areas:
            if area.type != "VIEW_3D":
                continue
            for space in area.spaces:
                if space.type == "VIEW_3D":
                    space.show_region_ui = True
                    space.shading.type = "SOLID"
                    space.shading.color_type = "MATERIAL"
                    space.region_3d.view_perspective = "CAMERA"
    return None


def digest(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


if config.get("body"):
    STATE["index"] = next(
        index for index, body in enumerate(BODIES) if body["name"] == config["body"]
    )
    print(f"sculpto: starting body {current_body()['name']}")

print(f"baking {OUT}")
if bake(current_body(), run_sliders(current_body())) is None:
    raise SystemExit("initial bake failed")
ensure_scene()

if once:
    blend = os.path.join(OUT_DIR, "sliders.blend")
    bpy.ops.wm.save_as_mainfile(filepath=blend)
    print(f"saved {blend}")
    raise SystemExit(0)

bpy.context.preferences.view.ui_scale = float(config.get("ui_scale", "1.0"))
register_properties()
scene = bpy.context.scene
if config.get("method"):
    scene.sculpto_fit_method = config["method"].upper()
if config.get("clearance"):
    scene.sculpto_aura_clearance = float(config["clearance"])
scene.sculpto_body = current_body()["name"]
if not len(scene.sculpto_sliders):
    STATE["presets"] = run_presets(current_body())
    populate()

if fit_check:
    if config.get("garment_name"):
        scene.sculpto_garment = config["garment_name"]
    scene.sculpto_garment_path = os.path.expanduser(config["garment"])
    scene.sculpto_garment_shape = config.get("garment_shape", "")
    scene.sculpto_garment_body_shape = config.get("garment_body_shape", "")
    garment = {
        "name": os.path.basename(scene.sculpto_garment_path),
        "body": scene.sculpto_body,
        "path": scene.sculpto_garment_path,
        "shape": scene.sculpto_garment_shape,
        "body_shape": scene.sculpto_garment_body_shape,
    }
    if fit_garment(garment) is None:
        raise SystemExit("garment fit failed")
    if config.get("slider") and current_method() == "AURA":
        print("fit-check: the movement report needs Bindings; skipping the follow step")
    if config.get("slider") and current_method() != "AURA":
        name, value = config["slider"].split("=", 1)
        override = dict(values_from_scene())
        override[name] = float(value)
        if bake(current_body(), list(override.items()), BODY_AFTER) is None:
            raise SystemExit("second bake failed")
        movable = dict(garment)
        movable["body_after"] = BODY_AFTER
        movable["out_after"] = GARMENT_AFTER
        if run_garment_fit(movable, OUT, GARMENT_OUT) is None:
            raise SystemExit("follow check failed")
    blend = os.path.join(OUT_DIR, "garment.blend")
    bpy.ops.wm.save_as_mainfile(filepath=blend)
    print(f"fit-check: saved {blend}")
    raise SystemExit(0)

if verify:
    first = (current_body()["name"], current_body()["preset"], digest(OUT))
    scene.sculpto_body = BODIES[1]["name"]
    scene.sculpto_dirty = True
    tick()
    second = (current_body()["name"], current_body()["preset"], digest(OUT))
    print(
        f"verify: {first[0]}/{first[1]} {first[2][:12]} -> "
        f"{second[0]}/{second[1]} {second[2][:12]}, "
        f"{len(scene.sculpto_sliders)} sliders"
    )
    blend = os.path.join(OUT_DIR, "sliders.blend")
    bpy.ops.wm.save_as_mainfile(filepath=blend)
    print(f"verify: saved {blend}")
    raise SystemExit(0)

if config.get("garment"):
    fit_garment(
        {
            "name": os.path.basename(config["garment"]),
            "body": current_body()["name"],
            "path": os.path.expanduser(config["garment"]),
            "shape": config.get("garment_shape", ""),
            "body_shape": config.get("garment_body_shape", ""),
        }
    )

if config.get("render"):
    render_scene = bpy.context.scene
    render_scene.render.engine = "BLENDER_WORKBENCH"
    render_scene.display.shading.light = "STUDIO"
    render_scene.display.shading.color_type = "MATERIAL"
    render_scene.render.resolution_x = 900
    render_scene.render.resolution_y = 1200
    render_scene.render.film_transparent = False
    render_scene.render.filepath = os.path.abspath(config["render"])
    bpy.ops.render.render(write_still=True)
    print(f"render: {render_scene.render.filepath}")
    blend = os.path.join(OUT_DIR, "garment.blend")
    bpy.ops.wm.save_as_mainfile(filepath=blend)
    print(f"render: saved {blend}")
    raise SystemExit(0)

open_sidebar()
bpy.app.timers.register(open_sidebar, first_interval=0.5)
bpy.app.timers.register(tick, first_interval=0.25, persistent=True)
