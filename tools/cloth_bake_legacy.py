"""Offline cloth bake on Blender's legacy mass-spring solver.

Usage (headless, meters-scale actor scene):
  blender --background <character.blend> --python cloth_bake_legacy.py -- \
      garment=<fitted.obj> out=<dir> clip=Idle \
      [frame_end=60] [sample=1] [pin_z=1.406] [body=BaseShape] \
      [bending=20] [structural=200] [mass=0.006] [self_min=0.0015] \
      [body_min=0.002] [self_collision=1] [quality=10] [ground=0]

Why this exists next to `cloth_bake.py`: the XPBD asset has no cloth
self-collision and no bending stiffness that survives open flaps, so a cape's
free panels fold into strips. The legacy modifier has both. It honours the pin
vertex group and the body Collision modifier, and it writes the same frame
sequence the GLB packagers read.

Pinning is a garment vertex group named `pin`: every vertex at or above
`pin_z` in world space (a shoulder yoke). Collision is the body's Collision
modifier; `self_min`/`body_min` must stay well below the triangle size or a
dense mesh inflates itself. `mass` is per vertex, so a dense garment needs
`mass` scaled down with vertex count (22,676 vertices -> 0.006).

Outputs: out/rest_<shell>.obj, out/frame_<n>_<shell>.obj, out/report.json.
"""

import json
import math
import os
import sys
import time

import bpy

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("garment", "out", "clip"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")

garment_path = os.path.abspath(cfg["garment"])
out_dir = cfg["out"]
clip = cfg["clip"]
frame_end_cap = int(cfg.get("frame_end", 60))
sample = int(cfg.get("sample", 1))
pin_z = float(cfg.get("pin_z", "1.406"))
# pin_back=1 also pins the back yoke/hood above pin_back_z (the hood is a
# second cloth layer over the back panel; a partial pin tears it across the
# boundary and the two layers fold through each other on broad backs).
pin_back = int(cfg.get("pin_back", "0"))
pin_back_z = float(cfg.get("pin_back_z", "1.0"))
body_name = cfg.get("body", "BaseShape")
bending = float(cfg.get("bending", "20"))
structural = float(cfg.get("structural", "200"))
mass = float(cfg.get("mass", "0.006"))
self_min = float(cfg.get("self_min", "0.0015"))
body_min = float(cfg.get("body_min", "0.002"))
self_collision = int(cfg.get("self_collision", 1))
quality = int(cfg.get("quality", 10))
ground = int(cfg.get("ground", 0))
os.makedirs(out_dir, exist_ok=True)

scene = bpy.context.scene
body = bpy.data.objects.get(body_name)
if body is None:
    raise SystemExit(f"body mesh `{body_name}` not found")
armature = next(o for o in bpy.data.objects if o.type == "ARMATURE")
action = bpy.data.actions.get(clip)
if action is None:
    raise SystemExit(f"action `{clip}` not found")
armature.animation_data.action = action
body.hide_set(False)
body.hide_viewport = False

# Display frame (Y-up) -> Blender Z-up, the same convention as cloth_bake.py.
bpy.ops.wm.obj_import(filepath=garment_path)
garment = bpy.context.selected_objects[0]
garment.rotation_euler = (math.radians(90.0), 0.0, 0.0)
bpy.context.view_layer.objects.active = garment
bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
garment.name = "SculptoGarment0"

pinned = [
    v.index for v in garment.data.vertices if (garment.matrix_world @ v.co).z >= pin_z
]
if pin_back:
    pinned += [
        v.index
        for v in garment.data.vertices
        if (garment.matrix_world @ v.co).z >= pin_back_z
        and (garment.matrix_world @ v.co).y >= 0.0
        and v.index not in set(pinned)
    ]
if not pinned:
    raise SystemExit(f"no vertices at or above pin_z={pin_z}; the garment would fall")
garment.vertex_groups.new(name="pin").add(pinned, 1.0, "REPLACE")

body.modifiers.new("Collision", "COLLISION")
if ground:
    # A floor-length garment pools on the ground; without a floor the hem
    # keeps falling below the feet through the whole clip.
    floor_z = min((body.matrix_world @ v.co).z for v in body.data.vertices)
    bpy.ops.mesh.primitive_plane_add(size=6, location=(0.0, 0.0, floor_z))
    floor = bpy.context.object
    floor.modifiers.new("Collision", "COLLISION")
    floor.collision.thickness_outer = 0.002
    floor.collision.cloth_friction = 5.0
    floor.hide_render = True
body.collision.thickness_outer = 0.005
body.collision.cloth_friction = 5.0

modifier = garment.modifiers.new("Cloth", "CLOTH")
settings = modifier.settings
settings.vertex_group_mass = "pin"
settings.pin_stiffness = 1.0
settings.quality = quality
settings.mass = mass
settings.air_damping = 1.0
settings.tension_stiffness = structural
settings.compression_stiffness = structural
settings.shear_stiffness = structural
settings.bending_stiffness = bending
settings.bending_stiffness_max = bending
collision = modifier.collision_settings
collision.use_self_collision = bool(self_collision)
collision.self_distance_min = self_min
collision.self_friction = 5.0
collision.distance_min = body_min
collision.collision_quality = 4
modifier.point_cache.frame_start = 0
modifier.point_cache.frame_end = frame_end_cap

frame_end = min(int(action.frame_range[1]), frame_end_cap)
scene.frame_start = 0
scene.frame_end = frame_end

rest = [v.co.copy() for v in garment.data.vertices]
report = {
    "solver": "legacy-cloth",
    "clip": clip,
    "garment_verts": len(garment.data.vertices),
    "shells": {"SculptoGarment0": {"verts": len(garment.data.vertices)}},
    "pin": {"z": pin_z, "pinned": len(pinned)},
    "cloth": {
        "bending": bending,
        "structural": structural,
        "mass": mass,
        "self_collision": bool(self_collision),
        "self_distance_min": self_min,
        "body_distance_min": body_min, "ground": bool(ground),
    },
    "frame_range": [0, frame_end],
    "frames": {},
}

with open(os.path.join(out_dir, "rest_SculptoGarment0.obj"), "w", encoding="utf-8") as sink:
    for co in rest:
        sink.write(f"v {co.x:.6f} {co.y:.6f} {co.z:.6f}\n")
    for polygon in garment.data.polygons:
        sink.write("f " + " ".join(str(i + 1) for i in polygon.vertices) + "\n")

samples = set(range(0, frame_end + 1, sample)) | {frame_end}
start = time.time()
for frame in range(0, frame_end + 1):
    scene.frame_set(frame)
    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = garment.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    sim = [garment.matrix_world @ vertex.co for vertex in mesh.vertices]
    evaluated.to_mesh_clear()
    if frame in samples:
        with open(
            os.path.join(out_dir, f"frame_{frame}_SculptoGarment0.obj"), "w", encoding="utf-8"
        ) as sink:
            for co in sim:
                sink.write(f"v {co.x:.6f} {co.y:.6f} {co.z:.6f}\n")
            for polygon in garment.data.polygons:
                sink.write("f " + " ".join(str(i + 1) for i in polygon.vertices) + "\n")
        displacements = sorted((sim[i] - rest[i]).length for i in range(len(rest)))
        report["frames"][str(frame)] = {
            "displacement": {
                "p50": round(displacements[len(displacements) // 2], 4),
                "max": round(displacements[-1], 4),
            },
            "zmin": round(min(co.z for co in sim), 4),
        }
report["sim_seconds"] = round(time.time() - start, 1)
with open(os.path.join(out_dir, "report.json"), "w", encoding="utf-8") as sink:
    json.dump(report, sink, indent=1)
print("REPORT " + json.dumps({k: v for k, v in report.items() if k != "frames"}))
