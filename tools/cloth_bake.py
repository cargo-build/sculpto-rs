"""Offline cloth bake: garment + actor + clip -> simulated frames + report.

Usage (headless, meters-scale actor scene):
  blender --background <character.blend> --python cloth_bake.py -- \
      garment=<fitted.obj> out=<dir> clip=Combat_Idle \
      [body=BaseShape] [settle=24] [frame_end=140] [pin_height=0.85] [sample=10]
      [pin_mode=auto|above] [pin_contact=0.02] [bendiness=0] [gravity=1]
      [collide=1] [pin_all=0]

Uses Blender 5.2's Geometry Nodes XPBD physics (the experimental
`Cloth Dynamics (Experimental)` and `Collider` node-group assets). The legacy
cloth modifier is not used: in 5.2.2 it integrates gravity and springs but
ignores pin groups and colliders in every configuration probed.

Pinning: boundary loops above the shoulder line (`pin_height` x body height)
are written to a
`pin` vertex group and bound to the asset's Pin Group input as an attribute.
Collision: a `Collider` modifier (Deforming) on the body; the body goes into
an effector collection bound to the Cloth Dynamics' Effectors Collection
socket as a datablock (binding the collection by name silently does nothing).

Outputs (per shell <shell>):
  out/bake.blend            scene with the XPBD setup
  out/rest_<shell>.obj      pre-simulation rest mesh
  out/frame_<n>_<shell>.obj garment at sampled frames (world space); rest and
                            frames carry the garment's `vt` block when the
                            fitted mesh has UVs (topology is constant)
  out/report.json           settle/stretch/contact metrics
"""

import json
import os
import sys
import time
from math import radians

import bpy
from mathutils.bvhtree import BVHTree

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)

garment_path = cfg["garment"]
out_dir = cfg["out"]
clip = cfg.get("clip", "Combat_Idle")
body_name = cfg.get("body", "BaseShape")
settle = int(cfg.get("settle", 24))
frame_end_cap = int(cfg.get("frame_end", 140))
pin_height = float(cfg.get("pin_height", 0.85))
substeps = int(cfg.get("substeps", 5))
constraint_steps = int(cfg.get("constraint_steps", 15))
stretchiness = float(cfg.get("stretchiness", 0.0))
bendiness = float(cfg.get("bendiness", 0.0))
linear_damping = float(cfg.get("linear_damping", 1.0))
gravity = int(cfg.get("gravity", 1))
collide = int(cfg.get("collide", 1))
pin_band = float(cfg.get("pin_band", 0.12))
pin_contact = float(cfg.get("pin_contact", 0.02))
pin_all = int(cfg.get("pin_all", 0))
pin_mode = cfg.get("pin_mode", "auto")
collider_friction = float(cfg.get("collider_friction", 0.2))
collider_margin = float(cfg.get("collider_margin", 0.0))
edge_contacts = int(cfg.get("edge_contacts", 0))
collision_radius = float(cfg.get("collision_radius", 0.004))
sample = int(cfg.get("sample", 10))

# `system_resource` resolves directories; file paths must be joined on.
ASSETS = os.path.join(
    bpy.utils.system_resource("DATAFILES", path="assets/nodes"),
    "geometry_nodes_dynamics_assets.blend",
)
if not os.path.exists(ASSETS):
    raise SystemExit(f"dynamics asset file not found: {ASSETS}")
for group in ("Cloth Dynamics (Experimental)", "Collider"):
    if group not in bpy.data.node_groups:
        bpy.ops.wm.append(filename=group, directory=ASSETS + "/NodeTree/", link=False)
cloth_group = bpy.data.node_groups["Cloth Dynamics (Experimental)"]
collider_group = bpy.data.node_groups["Collider"]


def socket_ids(group):
    return {
        item.name: item.identifier
        for item in group.interface.items_tree
        if item.item_type == "SOCKET" and item.in_out == "INPUT"
    }


os.makedirs(out_dir, exist_ok=True)
scene = bpy.context.scene
report = {"clip": clip, "settle_frames": settle, "garment": garment_path, "solver": "xpbd"}

body = bpy.data.objects.get(body_name)
if body is None:
    raise SystemExit(f"body mesh `{body_name}` not found")
armature = next(o for o in bpy.data.objects if o.type == "ARMATURE")
action = bpy.data.actions.get(clip)
if action is None:
    raise SystemExit(f"action `{clip}` not found")

# Hidden objects are excluded from the simulation depsgraph (the caster
# fixture ships BaseShape hidden).
body.hide_set(False)
body.hide_viewport = False
body.hide_render = False

# --- collider: XPBD Collider asset on the deformed body -------------------
collider = body.modifiers.new("XpbdCollider", "NODES")
collider.node_group = collider_group
collider_mod_inputs = collider.properties.inputs
collider_ids = socket_ids(collider_group)
collider_mod_inputs[collider_ids["Deforming"]]["value"] = True
collider_mod_inputs[collider_ids["Margin"]]["value"] = collider_margin
collider_mod_inputs[collider_ids["Edge Contacts"]]["value"] = bool(edge_contacts)
collider_mod_inputs[collider_ids["Friction"]]["value"] = collider_friction
report["contact"] = {"margin": collider_margin, "edge_contacts": bool(edge_contacts),
                     "collision_radius": collision_radius}
report["cloth"] = {"substeps": substeps, "constraint_steps": constraint_steps,
                   "stretchiness": stretchiness, "bendiness": bendiness,
                   "linear_damping": linear_damping, "gravity": bool(gravity), "collide": bool(collide),
                   "pin_contact": pin_contact}

effectors = bpy.data.collections.new("Effectors")
scene.collection.children.link(effectors)
if collide:
    effectors.objects.link(body)

# --- garment --------------------------------------------------------------
bpy.ops.wm.obj_import(filepath=os.path.abspath(garment_path))
garment = bpy.context.selected_objects[0] if bpy.context.selected_objects else bpy.context.active_object
garment.name = "SculptoGarment"
# Display frame (Y-up) -> Blender Z-up; obj_import applies no conversion.
garment.rotation_euler = (radians(90.0), 0.0, 0.0)
bpy.context.view_layer.objects.active = garment
bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
report["garment_verts"] = len(garment.data.vertices)

# Drop the loose vertices the repair left behind, then split the shells:
# only the largest (outer robe) is cloth-simulated; the inner layers follow
# it with a Surface Deform (5.2 XPBD has no cloth self-collision, so
# simulating both layers makes them tangle through each other).
bpy.context.view_layer.objects.active = garment
garment.select_set(True)
bpy.ops.object.mode_set(mode="EDIT")
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.delete_loose()
bpy.ops.object.mode_set(mode="OBJECT")
bpy.ops.mesh.separate(type="LOOSE")
shells = [o for o in bpy.context.selected_objects if o.type == "MESH"]
garment.name = "SculptoGarment0"
shells = [o for o in shells if len(o.data.vertices) >= 32]
for index, shell in enumerate(shells):
    shell.name = f"SculptoGarment{index}"

# Pins anchor at the shoulder line measured on the BODY, not the garment:
# a short bodice shell's own 82% line sits above its collar ring.
body_z = [(body.matrix_world @ v.co).z for v in body.data.vertices]
shoulder_z = min(body_z) + pin_height * (max(body_z) - min(body_z))

dyn_ids = socket_ids(cloth_group)


def boundary_verts(obj):
    edge_uses = {}
    for polygon in obj.data.polygons:
        verts = polygon.vertices
        for a, b in zip(verts, verts[1:] + verts[:1]):
            key = (min(a, b), max(a, b))
            edge_uses[key] = edge_uses.get(key, 0) + 1
    return sorted({v for edge, uses in edge_uses.items() if uses == 1 for v in edge})


def body_tree(obj):
    """World-space BVH of the posed body for contact pinning."""
    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = obj.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    verts = [evaluated.matrix_world @ vertex.co for vertex in mesh.vertices]
    polygons = [tuple(polygon.vertices) for polygon in mesh.polygons]
    tree = BVHTree.FromPolygons(verts, polygons)
    evaluated.to_mesh_clear()
    return tree


def pin_shell(obj, threshold_z, ring_fraction, tree, contact):
    """Pin boundary verts above `threshold_z`, or the shell's own top ring
    when the shell never reaches the body's shoulder line (a skirt pins at
    its waist ring). A closed lined shell has no boundary, so it pins the
    vertices that rest on the body above the chest line: a cloak is carried by
    its shoulders and clasp, not by a horizontal slice through free cloth."""
    z = [v.co.z for v in obj.data.vertices]
    top = max(z)
    bottom = min(z)
    pinned = []
    level = 0.0
    rule = "boundary"
    candidates = boundary_verts(obj)
    if candidates:
        if top >= threshold_z:
            level = threshold_z
        else:
            level = top - ring_fraction * (top - bottom)
        pinned = [v for v in candidates if obj.data.vertices[v].co.z >= level]
    if contact > 0.0:
        # A draped cloak also rests on the shoulders, which have no boundary:
        # without that support the whole weight hangs from the neck ring and
        # the surface pleats into strips.
        seen = set(pinned)
        resting = []
        for vertex in obj.data.vertices:
            world = obj.matrix_world @ vertex.co
            if world.z < threshold_z:
                continue
            hit = tree.find_nearest(world)
            if hit[0] is not None and hit[3] <= contact and vertex.index not in seen:
                seen.add(vertex.index)
                resting.append(vertex.index)
        if resting:
            pinned += resting
            rule = "boundary+contact" if candidates else "contact"
    if not pinned:
        rule = "band"
        band_bottom = max(bottom, threshold_z - 0.02)
        band_top = min(top, threshold_z + 0.06)
        pinned = [v.index for v in obj.data.vertices if band_bottom <= v.co.z <= band_top]
        level = round(band_bottom, 4)
    if pinned:
        obj.vertex_groups.new(name="pin").add(pinned, 1.0, "REPLACE")
    return len(pinned), level, rule


report["pin"] = {"height_fraction": pin_height, "world_z": round(shoulder_z, 4), "shells": {}}
report["shells"] = {}
bvh = body_tree(body)
for shell in shells:
    if len(shell.data.vertices) < 32:
        continue
    if pin_mode == "above":
        pinned = [v.index for v in shell.data.vertices if (shell.matrix_world @ v.co).z >= shoulder_z]
        level, rule = shoulder_z, "above"
        shell.vertex_groups.new(name="pin").add(pinned, 1.0, "REPLACE")
    elif pin_all:
        pinned = [v.index for v in shell.data.vertices]
        level, rule = shoulder_z, "all"
        shell.vertex_groups.new(name="pin").add(pinned, 1.0, "REPLACE")
    else:
        pinned, level, rule = pin_shell(shell, shoulder_z, pin_band, bvh, pin_contact)
    if pinned == 0:
        raise SystemExit(f"shell `{shell.name}` has no pinnable boundary ring; it would fall")
    dynamics = shell.modifiers.new("ClothDynamics", "NODES")
    dynamics.node_group = cloth_group
    inputs = dynamics.properties.inputs
    inputs[dyn_ids["Pin Group"]]["attribute_name"] = "pin"
    inputs[dyn_ids["Collision Radius"]]["value"] = collision_radius
    inputs[dyn_ids["Substeps"]]["value"] = substeps
    inputs[dyn_ids["Constraint Steps"]]["value"] = constraint_steps
    inputs[dyn_ids["Stretchiness"]]["value"] = stretchiness
    inputs[dyn_ids["Bendiness"]]["value"] = bendiness
    inputs[dyn_ids["Linear Damping"]]["value"] = linear_damping
    inputs[dyn_ids["Gravity"]]["value"] = bool(gravity)
    inputs[dyn_ids["Friction"]]["value"] = collider_friction
    # The collection must be bound as a datablock; a name string is stored
    # but never resolved (probe: cloth falls straight through the body).
    inputs[dyn_ids["Effectors Collection"]]["value"] = effectors
    report["shells"][shell.name] = {
        "verts": len(shell.data.vertices),
        "pinned": len(pinned) if isinstance(pinned, list) else pinned,
        "pin_level": round(level, 4),
        "pin_rule": rule,
    }
    report["pin"]["shells"][shell.name] = round(level, 4)

# --- clip + settle pre-roll ------------------------------------------------
armature.animation_data.action = action
frame_end = min(int(action.frame_range[1]), frame_end_cap)
scene.frame_start = 0
scene.frame_end = frame_end
report["frame_range"] = [scene.frame_start, frame_end]

# --- simulate (XPBD simulation zones need sequential stepping) ------------
start = time.time()
depsgraph = bpy.context.evaluated_depsgraph_get()
rest = {shell.name: [v.co.copy() for v in shell.data.vertices] for shell in shells}


def write_obj(path, coords, polygons, uv_layer=None):
    """Write positions, faces and — when the mesh carries one — loop UVs.

    The OBJ importer keeps one vertex per `v` line and stores UVs per loop, so
    deduplicating the rounded loop UVs keeps the `vt` block small while the
    per-corner indices survive the combined export's re-import.
    """
    uv_rows = []
    uv_index = {}
    corners = []
    if uv_layer is not None:
        loop_uvs = [datum.uv for datum in uv_layer.data]
        for polygon in polygons:
            row = []
            for loop in range(polygon.loop_start, polygon.loop_start + polygon.loop_total):
                key = (round(loop_uvs[loop][0], 6), round(loop_uvs[loop][1], 6))
                index = uv_index.get(key)
                if index is None:
                    index = len(uv_rows)
                    uv_index[key] = index
                    uv_rows.append(key)
                row.append(index)
            corners.append(row)
    with open(path, "w", encoding="utf-8") as sink:
        for co in coords:
            sink.write(f"v {co.x:.6f} {co.y:.6f} {co.z:.6f}\n")
        for u, v in uv_rows:
            sink.write(f"vt {u:.6f} {v:.6f}\n")
        for index, polygon in enumerate(polygons):
            if uv_layer is None:
                sink.write("f " + " ".join(str(i + 1) for i in polygon.vertices) + "\n")
            else:
                sink.write(
                    "f "
                    + " ".join(
                        f"{vertex + 1}/{corner + 1}"
                        for vertex, corner in zip(polygon.vertices, corners[index])
                    )
                    + "\n"
                )


def pct(sorted_seq, q):
    return round(sorted_seq[min(len(sorted_seq) - 1, int(q * len(sorted_seq)))], 4) if sorted_seq else None


for shell in shells:
    write_obj(
        os.path.join(out_dir, f"rest_{shell.name}.obj"),
        rest[shell.name],
        shell.data.polygons,
        shell.data.uv_layers.active,
    )

# Edge rest lengths give the stretch measure: ratio of the simulated edge to
# its rest length, reported per shell over the whole clip.
rest_edges = {}
for shell in shells:
    verts = rest[shell.name]
    rest_edges[shell.name] = [
        (edge.vertices[0], edge.vertices[1], (verts[edge.vertices[0]] - verts[edge.vertices[1]]).length)
        for edge in shell.data.edges
    ]

samples = set(range(settle, frame_end + 1, sample))
samples.add(frame_end)
report["frames"] = {}
report["qualify"] = {shell.name: {"worst_min_distance": 9.9, "inside_any_frame": 0, "stretch_p99": 0.0, "stretch_max": 0.0} for shell in shells}
qualify = report["qualify"]
for frame in range(scene.frame_start, frame_end + 1):
    scene.frame_set(frame)
    if frame not in samples:
        # Qualification runs on every frame even when no OBJ is written.
        for shell in shells:
            mesh_eval = shell.evaluated_get(depsgraph).to_mesh()
            sim = [shell.matrix_world @ v.co for v in mesh_eval.vertices]
            shell.to_mesh_clear()
            worst = min(
                (body.closest_point_on_mesh(co, depsgraph=depsgraph)[1] - co).length for co in sim[::16]
            )
            inside = 0
            entry = qualify[shell.name]
            entry["worst_min_distance"] = round(min(entry["worst_min_distance"], worst), 4)
            if worst <= 0.0:
                inside += 1
                entry["inside_any_frame"] += 1
            ratios = sorted(
                (sim[a] - sim[b]).length / length
                for a, b, length in rest_edges[shell.name]
                if length > 0.002  # ignore degenerate rest edges
            )
            if ratios:
                entry["stretch_p99"] = round(max(entry["stretch_p99"], ratios[int(0.99 * (len(ratios) - 1))]), 3)
                entry["stretch_max"] = round(max(entry["stretch_max"], ratios[-1]), 3)
        continue
    frame_report = {}
    for shell in shells:
        mesh_eval = shell.evaluated_get(depsgraph).to_mesh()
        sim = [shell.matrix_world @ v.co for v in mesh_eval.vertices]
        shell.to_mesh_clear()
        rest_pts = rest[shell.name]
        displacements = sorted((sim[i] - rest_pts[i]).length for i in range(len(rest_pts)))
        distances = sorted(
            (body.closest_point_on_mesh(co, depsgraph=depsgraph)[1] - co).length for co in sim[::4]
        )
        frame_report[shell.name] = {
            "displacement": {
                "p50": pct(displacements, 0.5),
                "p95": pct(displacements, 0.95),
                "max": round(displacements[-1], 4),
            },
            "distance_to_body": {"min": round(distances[0], 4), "p50": pct(distances, 0.5)},
            "verts_at_or_inside_body": sum(1 for d in distances if d <= 0.0),
        }
        write_obj(
            os.path.join(out_dir, f"frame_{frame}_{shell.name}.obj"),
            sim,
            shell.data.polygons,
            shell.data.uv_layers.active,
        )
    report["frames"][str(frame)] = frame_report
report["sim_seconds"] = round(time.time() - start, 1)

# --- save ----------------------------------------------------------------------
bpy.ops.file.pack_all()
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out_dir, "bake.blend"))
with open(os.path.join(out_dir, "report.json"), "w", encoding="utf-8") as sink:
    json.dump(report, sink, indent=1)
print("REPORT " + json.dumps(report))
