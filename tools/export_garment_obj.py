"""Export one garment mesh from a blend or GLB as a Wavefront OBJ with UVs.

Usage (headless):
  blender --background <garment.blend> --python export_garment_obj.py -- \
      mesh=<object name> out=<path>
  blender --background --factory-startup --python export_garment_obj.py -- \
      source=<garment.glb> mesh=<node name> out=<path>

`source=` imports a glTF/GLB (raw Tripo exports) instead of opening a blend;
`mesh=` is then the glTF node name. The OBJ is written in the display
convention sculpto expects (Y-up, `(x, z, -y)`), with `export_uv=True` so the
merged pipeline can carry the garment's corner UVs through fitting, and
`export_materials=True` so the OBJ records the material the combined export
later reads back. Normals are omitted: every consumer here rebuilds topology
from positions and triangles.

Optional `bounds=xmin,xmax,height_min,height_max` selects a complete wardrobe
cell in Blender world X/Z coordinates; crossing faces are rejected. `topology=position`
connects exact-position render seams while retaining corner UVs. This is not
cloth reconstruction. `material_blend=<path>` retains packed source materials
for downstream appearance transfer; OBJ/MTL alone is not a PBR handoff.
Every export writes a source digest and selection receipt beside the OBJ.
"""

import os
import sys
import hashlib
import json
import time

import bpy
import bmesh

started = time.monotonic()

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
mesh_name = cfg.get("mesh")
out = cfg.get("out")
source = cfg.get("source")
material_blend = cfg.get("material_blend")
identity_path = source if source else bpy.data.filepath
topology = cfg.get("topology", "render")
if topology not in ("render", "position"):
    raise SystemExit("topology must be render or position")
if not mesh_name:
    raise SystemExit("missing `mesh=<object name>`")
if not out:
    raise SystemExit("missing `out=<path>`")
for destination in (out, out + ".json", material_blend):
    if destination and os.path.exists(destination):
        raise SystemExit(f"refusing to overwrite task artifact: {destination}")

if source:
    if os.path.splitext(source)[1].lower() not in (".glb", ".gltf"):
        raise SystemExit(f"source must be a .glb/.gltf, got `{source}`")
    if not os.path.exists(source):
        raise SystemExit(f"source not found: {source}")
    bpy.ops.import_scene.gltf(filepath=os.path.abspath(source))

obj = bpy.data.objects.get(mesh_name)
if obj is None or obj.type != "MESH":
    names = [o.name for o in bpy.data.objects if o.type == "MESH"]
    raise SystemExit(f"mesh `{mesh_name}` not found; available: {names}")

original_vertices = len(obj.data.vertices)
original_faces = len(obj.data.polygons)
bounds = None
if "bounds" in cfg:
    bounds = [float(value) for value in cfg["bounds"].split(",")]
    if len(bounds) != 4 or not (bounds[0] < bounds[1] and bounds[2] < bounds[3]):
        raise SystemExit("bounds expects xmin,xmax,height_min,height_max in the display frame")

if bounds is not None or topology == "position":
    # Operate on a private datablock; UVs remain face-corner attributes.
    obj.data = obj.data.copy()
    mesh = bmesh.new()
    mesh.from_mesh(obj.data)
    if bounds is not None:
        selected = set()
        for vertex in mesh.verts:
            point = obj.matrix_world @ vertex.co
            if bounds[0] <= point.x <= bounds[1] and bounds[2] <= point.z <= bounds[3]:
                selected.add(vertex)
        crossing = sum(0 < sum(v in selected for v in face.verts) < len(face.verts)
                       for face in mesh.faces)
        if crossing or not selected:
            mesh.free()
            raise SystemExit(f"selection is not a whole part: crossing_faces={crossing}, vertices={len(selected)}")
        bmesh.ops.delete(mesh, geom=[v for v in mesh.verts if v not in selected], context="VERTS")
    selected_vertices = len(mesh.verts)
    if topology == "position":
        representatives = {}
        targetmap = {}
        for vertex in mesh.verts:
            key = tuple(vertex.co)
            if key in representatives:
                targetmap[vertex] = representatives[key]
            else:
                representatives[key] = vertex
        bmesh.ops.weld_verts(mesh, targetmap=targetmap)
    mesh.to_mesh(obj.data)
    mesh.free()
    obj.data.update()
else:
    selected_vertices = original_vertices

# `export_selected_objects` only sees objects selectable in the view layer, and
# a garment left hidden by an earlier editing session exports as nothing.
# The saved selection is not the target: the file also carries the source
# sculpt and the default cube.
for other in bpy.data.objects:
    other.select_set(False)
obj.hide_set(False)
obj.hide_viewport = False
obj.hide_render = False
obj.select_set(True)
bpy.context.view_layer.objects.active = obj

uv_layers = [layer.name for layer in obj.data.uv_layers]
if material_blend:
    bpy.ops.file.pack_all()
    bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(material_blend))
bpy.ops.wm.obj_export(
    filepath=out,
    export_selected_objects=True,
    export_uv=bool(uv_layers),
    export_materials=True,
    export_normals=False,
    forward_axis="NEGATIVE_Z",
    up_axis="Y",
    apply_modifiers=False,
)
with open(out, encoding="utf-8") as sink:
    uv_count = sum(1 for line in sink if line.startswith("vt "))
print(
    f"wrote {out}: {len(obj.data.vertices)} verts, {len(obj.data.polygons)} faces, "
    f"{uv_count} uvs, uv layers {uv_layers}, "
    f"materials {[m.name for m in obj.data.materials if m]}"
)
with open(identity_path, "rb") as original:
    digest = hashlib.file_digest(original, "sha256").hexdigest()
report = {
    "source": os.path.abspath(identity_path), "source_sha256": digest,
    "mesh": mesh_name, "display_xy_bounds": bounds,
    "topology": topology, "source_vertices": original_vertices,
    "source_faces": original_faces, "selected_vertices": selected_vertices,
    "export_vertices": len(obj.data.vertices), "export_faces": len(obj.data.polygons),
    "uv_coordinates": uv_count, "seconds": time.monotonic() - started,
    "material_blend": os.path.abspath(material_blend) if material_blend else None,
    "material_object": obj.name,
    "status": "extracted; surface preparation and fit are not qualified",
}
with open(out + ".json", "x", encoding="utf-8") as receipt:
    json.dump(report, receipt, indent=2)
