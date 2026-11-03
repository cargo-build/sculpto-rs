"""Generate sleeve tubes along a measured arm axis.

The wardrobe set authors sleeves as a flat relief, so no fitting step can give
them a round cross-section (verified twice: press clamp and authored-band
restore both leave a plane). This sweeps a tapered tube along the shoulder ->
wrist axis of the posed body and writes it as OBJ with UVs sampled from the
tunic's own sleeve patch, so the fabric texture carries over.

Usage:
    blender --background --factory-startup --python tools/sleeve_tube.py -- \
        body=<posed.obj> tunic=<tunic.obj> out=<tubes.obj>
"""

import math
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("body", "tunic", "out"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")

RINGS = 14
SIDES = 16
RADIUS_START = 7.0
RADIUS_END = 4.2

bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)
bpy.ops.wm.obj_import(filepath=cfg["body"])
body = [o for o in bpy.data.objects if o.type == "MESH"][0]
bpy.ops.wm.obj_import(filepath=cfg["tunic"])
tunic = [o for o in bpy.data.objects if o.type == "MESH" and o is not body][0]

# The sleeve unwrap is scattered across the atlas (measured spread 0.003..1.0),
# so there is no single patch to copy. Transfer UVs from the nearest vertex of
# the source garment instead, which follows its local fabric detail.
from mathutils import kdtree

source = None
if "atlas_src" in cfg:
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=cfg["atlas_src"])
    donors = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
    if donors:
        source = max(donors, key=lambda o: len(o.data.vertices))

uv_source = None
if source is not None and source.data.uv_layers.active is not None:
    layer = source.data.uv_layers.active
    size = len(source.data.vertices)
    tree = kdtree.KDTree(size)
    per_vertex = {}
    for polygon in source.data.polygons:
        for li in polygon.loop_indices:
            per_vertex.setdefault(source.data.loops[li].vertex_index, layer.data[li].uv[:])
    for index, vertex in enumerate(source.data.vertices):
        tree.insert(source.matrix_world @ vertex.co, index)
    tree.balance()
    # UV frame of the garment's own arm band: the sleeve unwrap has no single
    # compact patch, but its arm polygons still occupy a region we can project
    # into, which keeps the fabric continuous instead of nearest-vertex noise.
    src_points = [source.matrix_world @ v.co for v in source.data.vertices]
    sx = [q.x for q in src_points]
    sspan = max(sx) - min(sx)
    smin_x, smax_x = min(sx), max(sx)
    sz = [q.z for q in src_points]
    szmin, szmax = min(sz), max(sz)
    band_us, band_vs = [], []
    for polygon in source.data.polygons:
        centre = sum((src_points[i] for i in polygon.vertices), src_points[0] * 0) / len(polygon.vertices)
        # beyond the shoulder joint, not "a fraction from either edge": the old
        # test also caught torso panels and the UV frame collapsed to 0..1
        from_centre = abs(centre.x - (smin_x + smax_x) / 2)
        in_arm = from_centre > 0.17 * sspan
        if not in_arm or centre.z < szmin + 0.55 * (szmax - szmin):
            continue
        for loop in polygon.loop_indices:
            uvco = layer.data[loop].uv
            band_us.append(uvco[0])
            band_vs.append(uvco[1])
    if band_us:
        uv_frame = (min(band_us), max(band_us), min(band_vs), max(band_vs))
        print(f"arm UV frame u[{uv_frame[0]:.3f},{uv_frame[1]:.3f}] v[{uv_frame[2]:.3f},{uv_frame[3]:.3f}]")
    else:
        uv_frame = None
    uv_source = (tree, per_vertex, uv_frame)
    print(f"uv transfer source: {source.name} with {len(per_vertex)} mapped vertices")

points = [body.matrix_world @ v.co for v in body.data.vertices]
x_mid = (min(p.x for p in points) + max(p.x for p in points)) / 2
span = max(p.x for p in points) - min(p.x for p in points)

# sleeve texture patch: UV centroid of the tunic, reused for the tube surface
uv_layer = tunic.data.uv_layers.active
if uv_layer is not None and len(uv_layer.data):
    us = [uv_layer.data[i].uv[0] for i in range(0, len(uv_layer.data), 37)]
    vs = [uv_layer.data[i].uv[1] for i in range(0, len(uv_layer.data), 37)]
    u0, v0 = sum(us) / len(us), sum(vs) / len(vs)
else:
    u0, v0 = 0.5, 0.5

mesh = bpy.data.meshes.new("sleeves")
obj = bpy.data.objects.new("sleeves", mesh)
bpy.context.collection.objects.link(obj)
verts = []
faces = []
uvs = []
for side in (1, -1):
    shoulder = None
    candidates = [p for p in points if p.x * side > 0.16 * span / 2]
    wrist = max(candidates, key=lambda p: p.x * side)
    shoulder = [p for p in candidates if abs(p.x * side - 0.17 * span / 2) < 2.0]
    head = (sum(p.x for p in shoulder) / len(shoulder), sum(p.z for p in shoulder) / len(shoulder))
    axis = (wrist.x - head[0], wrist.z - head[1])
    length = math.hypot(*axis)
    direction = (axis[0] / length, axis[1] / length)
    normal = (-direction[1], direction[0])
    base = len(verts)
    # Arm-only point set: the previous attempt used every vertex of that body
    # half, so the maximum radial distance was a hip and the tube swallowed the
    # torso. Keep only points inside a narrow radial band around the arm axis.
    arm_band = []
    for point in candidates:
        rel = (point.x - head[0]) * direction[0] + (point.z - head[1]) * direction[1]
        if rel < 0 or rel > length * 1.05:
            continue
        along_x = head[0] + direction[0] * rel
        along_z = head[1] + direction[1] * rel
        radial = math.hypot(point.x - along_x, point.z - along_z)
        if radial < 0.06 * span:
            arm_band.append((rel, radial))
    for ring in range(RINGS + 1):
        t = ring / RINGS
        target = length * t
        near = [r for rel, r in arm_band if abs(rel - target) < max(length * 0.1, 0.5)]
        if near:
            radius = max(near) + 1.2
        else:
            radius = RADIUS_START + (RADIUS_END - RADIUS_START) * t
        # taper the last fifth into the wrist so the sleeve does not stop in a
        # flat disc that reads as a bent elbow
        if t > 0.8:
            radius *= max(0.35, (1.0 - t) / 0.2)
        centre_x = head[0] + axis[0] * t
        centre_z = head[1] + axis[1] * t
        for index in range(SIDES):
            angle = 2 * math.pi * index / SIDES
            ox = math.cos(angle) * radius * normal[0]
            oz = math.cos(angle) * radius * normal[1]
            oy = math.sin(angle) * radius
            position = (centre_x + ox, oy, centre_z + oz)
            verts.append(position)
            if uv_source is not None and uv_source[2] is not None:
                # parametric mapping: u follows the angle around the arm, v the
                # distance along it, projected into the garment's arm UV frame
                frame = uv_source[2]
                u = frame[0] + (index / SIDES) * (frame[1] - frame[0])
                v = frame[2] + t * (frame[3] - frame[2])
                uvs.append((u, v))
            else:
                uvs.append((u0, v0))
    for ring in range(RINGS):
        for index in range(SIDES):
            a = base + ring * SIDES + index
            b = base + ring * SIDES + (index + 1) % SIDES
            c = a + SIDES
            d = b + SIDES
            faces.append((a, b, d))
            faces.append((a, d, c))

mesh.from_pydata(verts, [], faces)
mesh.update()
layer = mesh.uv_layers.new(name="UVMap")
for polygon in mesh.polygons:
    for loop_index in polygon.loop_indices:
        vertex = mesh.loops[loop_index].vertex_index
        layer.data[loop_index].uv = uvs[vertex]

# Write by hand: bpy.ops.wm.obj_export was exporting the entire scene, so the
# "sleeve" file actually contained the body and tunic as well.
with open(cfg["out"], "w", encoding="utf-8") as sink:
    for x, y, z in verts:
        # OBJ is Y-up while these coordinates are Blender's Z-up, so convert on
        # the way out; writing them raw made the tubes land at the floor.
        sink.write(f"v {x:.6f} {z:.6f} {-y:.6f}\n")
    for u, v in uvs:
        sink.write(f"vt {u:.6f} {v:.6f}\n")
    for a, b, c in faces:
        sink.write(f"f {a + 1}/{a + 1} {b + 1}/{b + 1} {c + 1}/{c + 1}\n")
print(f"wrote {cfg['out']}: {len(verts)} verts, {len(faces)} faces, {SIDES} sides x {RINGS} rings")