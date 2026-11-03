"""Export explicitly selected actor solids in rest pose as a Wavefront OBJ.

Usage (headless):
  blender --background <character.blend> --python export_body_obj.py -- \
      mesh=BaseShape out=<path>
  Multiple solids: meshes=BaseShape,Hands,Head out=<path> [report=<receipt.json>]

The OBJ is written in the display convention sculpto expects (Y-up, world
X preserved); armature evaluation uses REST so actions, NLA and retained pose
transforms cannot contaminate the fitting reference.
"""

import hashlib
import json
from pathlib import Path
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
if "mesh" in cfg and "meshes" in cfg:
    raise SystemExit("choose `mesh=<name>` or `meshes=<comma-separated names>`, not both")
mesh_names = cfg["meshes"].split(",") if "meshes" in cfg else [cfg.get("mesh", "BaseShape")]
if any(not name for name in mesh_names) or len(set(mesh_names)) != len(mesh_names):
    raise SystemExit("mesh selection contains an empty or duplicate name")
out = cfg.get("out")
if not out:
    raise SystemExit("missing `out=<path>`")
report_path = cfg.get("report")
if report_path and Path(report_path).resolve() == Path(out).resolve():
    raise SystemExit("report and OBJ output must be different paths")

objects = []
for name in mesh_names:
    obj = bpy.data.objects.get(name)
    if obj is None or obj.type != "MESH":
        names = [o.name for o in bpy.data.objects if o.type == "MESH"]
        raise SystemExit(f"mesh `{name}` not found; available: {names[:10]}")
    objects.append(obj)

# Clearing an action leaves pose transforms behind; REST bypasses them and NLA.
for armature in (o for o in bpy.data.objects if o.type == "ARMATURE"):
    armature.data.pose_position = "REST"
bpy.context.view_layer.update()

depsgraph = bpy.context.evaluated_depsgraph_get()
vertex_count = 0
triangle_count = 0
parts = []
with open(out, "w", encoding="utf-8") as sink:
    for obj in objects:
        evaluated = obj.evaluated_get(depsgraph)
        mesh = evaluated.to_mesh()
        try:
            mesh.calc_loop_triangles()
            world = evaluated.matrix_world
            sink.write(f"o {obj.name}\n")
            for vertex in mesh.vertices:
                x, y, z = world @ vertex.co
                sink.write(f"v {x:.17g} {z:.17g} {-y:.17g}\n")
            for triangle in mesh.loop_triangles:
                sink.write("f " + " ".join(str(i + vertex_count + 1)
                                            for i in triangle.vertices) + "\n")
            parts.append({"name": obj.name, "vertex_start": vertex_count,
                          "vertex_count": len(mesh.vertices), "triangle_start": triangle_count,
                          "triangle_count": len(mesh.loop_triangles),
                          "world_matrix": [list(row) for row in world]})
            vertex_count += len(mesh.vertices)
            triangle_count += len(mesh.loop_triangles)
        finally:
            evaluated.to_mesh_clear()
receipt = {"source_blend": bpy.data.filepath or None, "pose": "REST",
           "coordinate_frame": "world Y-up (x,z,-y)", "parts": parts,
           "vertices": vertex_count, "triangles": triangle_count,
           "obj_sha256": hashlib.sha256(Path(out).read_bytes()).hexdigest()}
if report_path:
    Path(report_path).write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
print("BODY_EXPORT " + json.dumps(receipt))
