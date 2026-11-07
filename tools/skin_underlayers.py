"""Skin tight garment OBJs onto the actor GLB's armature.

Usage (headless):
  blender --background --factory-startup --python skin_underlayers.py -- \
      actor=<character.glb> out=<actor-with-layers.glb> align=<body.obj> \
      under=<garment.obj> [under=<garment2.obj> ...] [color=<rrggbb>]

Imports the actor (armature, skinned body, clips), then skins each garment
OBJ by transferring weights from the skinned actor parts by k-nearest
averaging and parents the layer to the armature. The output is a regular
actor GLB that `export_combined_glb.py` accepts as its `actor=` input, so
one unit asset can carry skinned under-layers (boots, trousers, shirt)
under morph-baked cloth.

Frames: garment OBJs are parsed from text (never through the OBJ importer,
whose axis conversion is not reproducible across sessions) and moved from
the OBJ family's raw Y-up frame to the actor GLB's world frame by the
explicit conversion (x, y, z) -> (x, -z, y) plus the constant offset
measured by `align=`: the OBJ body export pairs with the GLB body by
vertex index, and a non-uniform delta is an error rather than a guess.

Tight layers only: this is not the cloth route. The skinnable envelope is a
median skin distance of at most 3 cm and p95 of at most 8 cm - enough for a
shirt's drape gap, far below the 5-16 cm a frame mismatch produces. Neutral
PBR material per layer (`color=`, default leather tone); textured layers
come from the material pipeline, not from here.
"""

import json
import sys
from pathlib import Path

import bpy
from mathutils import Vector, kdtree
from mathutils.bvhtree import BVHTree

sys.path.insert(0, str(Path(__file__).resolve().parent))

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
actor_path = cfg["actor"]
out_path = cfg["out"]
unders = [v for k, v in (a.split("=", 1) for a in argv if a.startswith("under="))]
align_path = cfg.get("align")
# lift=x,y,z replaces align= when no same-topology body OBJ exists (e.g.
# skinned garments onto a replacement-body actor): the OBJ-family -> GLB
# scene offset, applied without measurement.
lift_override = cfg.get("lift")
color_hex = cfg.get("color", "7a6a58")
# tex=<png> + tex_layer=<layer name>: attach a baked basecolor atlas to that
# layer's material (the layer must carry UVs; OBJ files that pass through
# uv-preserving writers keep their f v/vt pairs).
texture_path = cfg.get("tex")
texture_layer = cfg.get("tex_layer")
# replace_body=1 drops every existing skinned mesh after their weights are
# collected: the under= meshes then ARE the actor's body (male actor build —
# a body replaces the skin, so the tight-layer guard does not apply).
replace_body = cfg.get("replace_body") == "1"
# loose=1 skips the tight-layer guard for deliberately baggy garments
# (a baggy shell binds acceptably by nearest weights; the guard's job is
# catching wrong frames, which still shows as p50 > 20 cm).
loose = cfg.get("loose") == "1"
NEIGHBOURS = 4


def read_obj(path):
    vertices, faces = [], []
    for line in Path(path).read_text().splitlines():
        parts = line.split()
        if not parts:
            continue
        if parts[0] == "v":
            vertices.append(tuple(float(x) for x in parts[1:4]))
        elif parts[0] == "f":
            index = [int(token.split("/")[0]) - 1 for token in parts[1:]]
            for corner in range(1, len(index) - 1):
                faces.append((index[0], index[corner], index[corner + 1]))
    if not vertices or not faces:
        raise SystemExit(f"{path}: no vertices/faces parsed")
    return vertices, faces


def to_world(raw):
    """OBJ family raw Y-up meters -> the actor's Z-up world orientation."""
    return [(x, -z, y) for x, y, z in raw]


bpy.ops.import_scene.gltf(filepath=actor_path)
armature = next(o for o in bpy.data.objects if o.type == "ARMATURE")

# Authoring leftovers: tiny unskinned primitives (a forgotten 2 m helper
# cube reads as a full-frame white sheet in engine captures and occludes
# the whole figure below chest height). Drop them instead of shipping them.
junk = [
    o.name
    for o in bpy.data.objects
    if o.type == "MESH" and not o.vertex_groups and len(o.data.vertices) < 100
]
for name in junk:
    bpy.data.objects.remove(bpy.data.objects[name], do_unlink=True)
if junk:
    print(f"dropped authoring junk: {junk}")


def z_span(mesh):
    low = high = None
    for vertex in mesh.data.vertices:
        z = (mesh.matrix_world @ vertex.co).z
        low = z if low is None else min(low, z)
        high = z if high is None else max(high, z)
    return high - low


# The body is the tallest dense mesh: every skinned part carries the full
# joint set as vertex groups, so weight count cannot discriminate, and the
# densest mesh is usually the hair.
# The body is the tallest dense SKINNED mesh: vertex-group counts cannot
# discriminate (every skinned part carries all joints), the densest mesh is
# usually the hair, and morph-driven cloth shells span the body height while
# carrying no weights at all.
candidates = {}
for o in bpy.data.objects:
    if o.type == "MESH" and len(o.data.vertices) >= 1000 and len(o.vertex_groups) >= 8:
        candidates[o.name] = z_span(o)
body = bpy.data.objects[max(candidates, key=candidates.get)] if candidates else None
if body is None or candidates[body.name] < 1.2:
    raise SystemExit(f"no body-height skinned mesh among: {candidates or '{}'}")

# Weight source is every skinned part: bodies like CBBE split feet, hands
# and head into their own meshes, and a boot's sole must weigh against the
# feet mesh, not the nearest ankle vertex.
parts = [o for o in bpy.data.objects if o.type == "MESH" and len(o.vertex_groups) >= 8]
body_name = body.name
part_names = [p.name for p in parts]
world = []
body_weights = []
for part in parts:
    for vertex in part.data.vertices:
        weights = {}
        for item in vertex.groups:
            weights[part.vertex_groups[item.group].name] = item.weight
        total = sum(weights.values())
        if total <= 0.0:
            continue
        world.append(part.matrix_world @ vertex.co)
        body_weights.append({name: w / total for name, w in weights.items()})
tree = kdtree.KDTree(len(world))
for i, co in enumerate(world):
    tree.insert(co, i)
tree.balance()

skin_polygons = []
part_offsets = {}
flat = []
for part in parts:
    part_offsets[part.name] = len(flat)
    flat += [part.matrix_world @ v.co for v in part.data.vertices]
    for polygon in part.data.polygons:
        skin_polygons.append([part_offsets[part.name] + int(i) for i in polygon.vertices])
skin = BVHTree.FromPolygons(flat, skin_polygons)

if not align_path and not lift_override:
    raise SystemExit("align=<body.obj> or lift=<x,y,z> is required (frame offset is measured or declared, never guessed)")
lift = None
if lift_override:
    lift = Vector(tuple(float(c) for c in lift_override.split(",")))
    print(f"align lift (declared): {[round(c, 4) for c in lift]}")
elif True:
    ref, _ = read_obj(align_path)
    ref_world = to_world(ref)
    if len(ref) != len(body.data.vertices):
        raise SystemExit(
            f"align body has {len(ref)} v, GLB body {len(body.data.vertices)} v"
            " - not the same topology"
        )
    body_world = [body.matrix_world @ v.co for v in body.data.vertices]
    deltas = [
        body_world[i] - Vector(ref_world[i]) for i in range(len(body_world))
    ]
    mags = [d.length for d in deltas]
    if max(mags) - min(mags) > 0.005:
        raise SystemExit(
            f"align delta not uniform: {min(mags) * 100:.2f}..{max(mags) * 100:.2f} cm"
        )
    lift = deltas[0]
    print(f"align lift: {[round(c, 4) for c in lift]}")
if replace_body:
    for mesh in [o for o in bpy.data.objects if o.type == "MESH"]:
        bpy.data.objects.remove(mesh, do_unlink=True)
    print(f"replace_body: dropped the old skinned body set; armature and clips kept")

report = {
    "actor": actor_path,
    "out": out_path,
    "body": body_name,
    "dropped": junk,
    "lift": [round(c, 4) for c in lift] if lift else None,
    "skin_parts": part_names,
    "replace_body": replace_body,
    "layers": [],
}
for path in unders:
    raw, faces = read_obj(path)
    coords = [Vector(p) + lift for p in to_world(raw)]
    name = path.rsplit("/", 1)[-1].removesuffix(".obj")
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([tuple(c) for c in coords], [], faces)
    # Smooth shading: flat facets triple the exported vertex count (one
    # corner per triangle) and read as broken geometry in engine captures.
    for polygon in mesh.polygons:
        polygon.use_smooth = True
    mesh.validate()
    mesh.update()
    layer = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(layer)

    far = 0
    unweighted = 0
    surface = []
    for index, co in enumerate(coords):
        hit = skin.find_nearest(co)
        if hit[0] is None:
            raise SystemExit(f"layer {name}: vertex {index} outside skin BVH")
        surface.append(hit[3])
        if hit[3] > 0.04:
            far += 1
        near = tree.find_n(co, NEIGHBOURS)
        mixed = {}
        for _co, src, _dist in near:
            for bone, weight in body_weights[src].items():
                mixed[bone] = mixed.get(bone, 0.0) + weight / len(near)
        for bone, share in mixed.items():
            group = layer.vertex_groups.get(bone) or layer.vertex_groups.new(name=bone)
            group.add([index], share, "REPLACE")
        if not layer.data.vertices[index].groups:
            unweighted += 1
    if unweighted:
        raise SystemExit(f"layer {name}: {unweighted} vertices got no weights")
    surface.sort()
    p50 = surface[len(surface) // 2]
    p95 = surface[int(len(surface) * 0.95)]
    if not replace_body and not loose and (p50 > 0.03 or p95 > 0.08):
        raise SystemExit(
            f"layer {name}: skin distance p50 {p50 * 100:.1f} cm, "
            f"p95 {p95 * 100:.1f} cm - not a tight layer, refusing to skin"
            " (baggy garment: pass loose=1)"
        )

    layer.parent = armature
    layer.matrix_parent_inverse = armature.matrix_world.inverted()
    layer.modifiers.new(name="Armature", type="ARMATURE").object = armature

    material = bpy.data.materials.new(name)
    material.use_nodes = True
    rgb = [int(color_hex[i : i + 2], 16) / 255.0 for i in (0, 2, 4)]
    bsdf = next(n for n in material.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = (*rgb, 1.0)
    if texture_path and (texture_layer is None or texture_layer == name):
        image = bpy.data.images.load(texture_path)
        tex_node = material.node_tree.nodes.new("ShaderNodeTexImage")
        tex_node.image = image
        material.node_tree.links.new(tex_node.outputs["Color"], bsdf.inputs["Base Color"])
        print(f"texture: {texture_path} -> {name}")
    layer.data.materials.append(material)

    report["layers"].append(
        {
            "name": name,
            "vertices": len(coords),
            "bones": len(layer.vertex_groups),
            "skin_cm_p50": round(p50 * 100, 2),
            "skin_cm_p95": round(p95 * 100, 2),
            "far_gt_4cm": far,
        }
    )

bpy.ops.export_scene.gltf(
    filepath=out_path,
    export_format="GLB",
    export_animations=True,
    export_morph=True,
    export_morph_animation=True,
    export_yup=True,
)
from glb_sanitize import sanitize_texcoord

sanitize_texcoord(out_path)
report["bytes"] = Path(out_path).stat().st_size
print("UNDER " + json.dumps(report))
