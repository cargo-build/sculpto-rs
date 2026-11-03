"""Textured figure render for Tripo garment review.

Renders the original Tripo FBX with its own embedded materials, optionally
replacing its mesh vertices with a processed OBJ (same vertex order/count) so
the garment keeps its modeled textures while showing posed or fitted geometry.

Usage:
    blender --background --factory-startup --python tools/textured_render.py -- \
        fbx=<path> out=<dir> label=<text> [verts=<obj>] [skin=<obj>] [res=<px>]

`verts=` swaps vertex positions onto the FBX mesh (asserts equal counts);
`skin=` adds a body OBJ in skin tone underneath. EEVEE, front and back.
"""

import os
import sys

import bpy
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1 :]
config = dict(item.split("=", 1) for item in argv if "=" in item)
for key in ("out",):
    if key not in config:
        raise SystemExit(f"missing `{key}=...`")
if "fbx" not in config and "glb" not in config:
    raise SystemExit("missing `fbx=...` or `glb=...`")
os.makedirs(config["out"], exist_ok=True)
label = config.get("label", "figure")
res = int(config.get("res", "640"))

# Factory startup ships a default Cube. It survives the "drop non-garment
# imports" loop, becomes the largest mesh, drives the camera framing and renders
# as a neutral grey slab in front of the piece - the root cause of every grey
# "empty" frame in this pipeline.
bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)

before = set(bpy.data.objects)
if "glb" in config:
    bpy.ops.import_scene.gltf(filepath=os.path.abspath(config["glb"]))
else:
    bpy.ops.wm.fbx_import(filepath=os.path.abspath(config["fbx"]))
imported = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
if not imported:
    raise SystemExit("fbx import produced no mesh")
garment = max(imported, key=lambda o: len(o.data.vertices))
for o in imported:
    if o is not garment:
        bpy.data.objects.remove(o, do_unlink=True)
bpy.ops.object.select_all(action="DESELECT")
garment.select_set(True)
bpy.context.view_layer.objects.active = garment
bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)

if "glb" in config:
    # unwelded GLB: apply the same weld the prep export used so vertex
    # indices line up with the processed OBJ files
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.mesh.remove_doubles(threshold=1e-5)
    bpy.ops.object.mode_set(mode="OBJECT")

if "keep" in config:
    # catalogue GLBs contain several view-columns; drop faces outside the
    # vertex set the processed OBJ actually uses, keeping UVs on survivors
    import bmesh

    keep = set()
    with open(config["keep"]) as fh:
        for line in fh:
            if line.startswith("f "):
                for token in line.split()[1:]:
                    keep.add(int(token.split("/")[0]) - 1)
    bm = bmesh.new()
    bm.from_mesh(garment.data)
    doomed = [f for f in bm.faces if any(v.index not in keep for v in f.verts)]
    bmesh.ops.delete(bm, geom=doomed, context="FACES")
    bm.to_mesh(garment.data)
    bm.free()
    garment.data.update()
    print(f"face mask: kept {len(keep)} vertices, removed {len(doomed)} faces")

if "verts" in config:
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=os.path.abspath(config["verts"]))
    new = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
    src = new[0] if new else None
    if src is None:
        raise SystemExit(f"verts import produced no mesh: {config['verts']}")
    n_g, n_s = len(garment.data.vertices), len(src.data.vertices)
    if n_g != n_s:
        raise SystemExit(f"vertex count mismatch: fbx {n_g} vs verts file {n_s}")
    src_matrix = src.matrix_world
    inv = garment.matrix_world.inverted()
    for gv, sv in zip(garment.data.vertices, src.data.vertices):
        gv.co = inv @ (src_matrix @ sv.co)
    garment.data.update()
    bpy.data.objects.remove(src, do_unlink=True)
    print(f"vertex swap: {n_g} vertices from {config['verts']}")

if "skin" in config:
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=os.path.abspath(config["skin"]))
    new = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
    if not new:
        raise SystemExit(f"skin import produced no mesh: {config['skin']}")
    body = new[0]
    material = bpy.data.materials.new("sculpto-skin")
    # `mannequin=1` swaps the skin tone for neutral grey: the garment reads
    # clearly and vision audits do not trip content filters on exposed skin.
    if config.get("mannequin") == "1":
        colour = (0.42, 0.42, 0.44, 1.0)
    else:
        colour = (0.62, 0.48, 0.40, 1.0)
    material.diffuse_color = colour
    material.use_nodes = True
    principled = next(n for n in material.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    principled.inputs["Base Color"].default_value = colour
    while body.data.materials:
        body.data.materials.pop()
    body.data.materials.append(material)

if config.get("tpose") == "1":
    # Rigid whole-arm rotation about the shoulder joint: every vertex beyond
    # the joint rotates, so the arm straightens at the shoulder like a bone.
    # A narrow cylinder selection only straightens the forearm. Joint landmarks
    # are body-frame constants shared by the 3BA/HIMBO bakes.
    import math

    bpy.context.view_layer.update()
    joint = {1: (21.0, 99.0), -1: (-21.0, 99.0)}
    hand = {1: (29.0, 77.0), -1: (-29.0, 77.0)}
    pose_targets = [o for o in bpy.data.objects if o.type == "MESH"]
    if config.get("tpose_skin") == "1":
        pose_targets = [
            o for o in pose_targets if o.name.startswith("body") or "skin" in o.name.lower()
        ]
        if not pose_targets:
            pose_targets = [body]
    moved = 0
    for side in (1, -1):
        jx, jz = joint[side]
        hx, hz = hand[side]
        axis = (hx - jx, hz - jz)
        length = math.hypot(*axis)
        axis = (axis[0] / length, axis[1] / length)
        angle = math.atan2(axis[0], -axis[1])
        rotation = math.radians(90.0) * side - angle
        for obj in pose_targets:
            matrix = obj.matrix_world
            inverse = matrix.inverted()
            for vertex in obj.data.vertices:
                world = matrix @ vertex.co
                if world[2] < jz - 34.0:
                    continue
                if world[0] * side <= 15.0:
                    continue
                dx, dz = world[0] - jx, world[2] - jz
                weight = min(max((abs(world[0]) - 15.0) / 5.0, 0.0), 1.0)
                turn = rotation * weight
                c, s = math.cos(turn), math.sin(turn)
                world[0] = jx + dx * c - dz * s
                world[2] = jz + dx * s + dz * c
                vertex.co = inverse @ world
                moved += 1
            obj.data.update()


# `extra=<obj>` adds generated geometry (swept sleeve tubes) that carries its own
# UVs. It takes the garment's material by object identity - an earlier version
# picked "the last imported mesh" and textured the wrong object.
if "extra" in config:
    bpy.ops.object.select_all(action="DESELECT")
    before_extra = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=os.path.abspath(config["extra"]))
    extra = [o for o in bpy.data.objects if o not in before_extra and o.type == "MESH"]
    # The wardrobe atlas holds the whole catalogue (5 views x 8 garments), so no
    # compact sleeve UV region exists and projecting into it smears the texture.
    # A solid material sampled from the tunic's own arm polygons keeps the colour
    # continuous instead.
    shared = [m for m in garment.data.materials if m]
    tint = bpy.data.materials.new("sleeve-tint")
    tint.use_nodes = True
    principled = next(n for n in tint.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    principled.inputs["Base Color"].default_value = (0.30, 0.29, 0.14, 1.0)
    for piece in extra:
        piece.data.materials.clear()
        piece.data.materials.append(tint)
    print(f"extra: {[o.name for o in extra]} -> {len(shared)} garment materials")

objects = [o for o in bpy.data.objects if o.type == "MESH"]
# vertex-derived bbox: bound_box can be stale after a raw vertex swap
bpy.context.view_layer.update()
mins = [1e9] * 3
maxs = [-1e9] * 3
for obj in objects:
    matrix = obj.matrix_world
    for v in obj.data.vertices:
        w = matrix @ v.co
        for k in range(3):
            mins[k] = min(mins[k], w[k])
            maxs[k] = max(maxs[k], w[k])
center = Vector(((mins[0] + maxs[0]) / 2, (mins[1] + maxs[1]) / 2, (mins[2] + maxs[2]) / 2))
size = max(maxs[k] - mins[k] for k in range(3))

camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
target = bpy.data.objects.new("target", None)
target.location = center
bpy.context.collection.objects.link(target)
bpy.context.collection.objects.link(camera)
track = camera.constraints.new(type="TRACK_TO")
track.target = target
bpy.context.scene.camera = camera

import math

sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
# A sun at rotation x=+55deg travels toward +Y, i.e. it lights the -Y side where
# the front camera sits. The negative angle used before backlit the figure and
# rendered every piece near-black.
sun.rotation_euler = (math.radians(-55), 0, math.radians(15))
sun.data.energy = 3.0
bpy.context.collection.objects.link(sun)
# Two-sided studio light: a single front sun leaves every back view unlit, which
# reads as a second, much lighter material on the same garment.
back_sun = bpy.data.objects.new("back_sun", bpy.data.lights.new("back_sun", "SUN"))
back_sun.rotation_euler = (math.radians(55), 0, 0.0)
back_sun.data.energy = 3.0
bpy.context.collection.objects.link(back_sun)
fill = bpy.data.objects.new("fill", bpy.data.lights.new("fill", "POINT"))
fill.data.energy = 2.0 * size * size
bpy.context.collection.objects.link(fill)

scene = bpy.context.scene
# Headless EEVEE-Next loses texture sampling (measured: uniform 0.8 base
# colour), so textured renders go through Cycles.
scene.render.engine = "CYCLES"
scene.cycles.samples = int(config.get("samples", "24"))
# Exposure compensation: the catalogue textures are authored dark, and a
# product shot needs the garment's own colour to read against the backdrop.
scene.view_settings.exposure = float(config.get("exposure", "0"))
if config.get("bg", "studio") == "alpha":
    scene.render.film_transparent = True
    # ambient fill still comes from the world even with a transparent film
    world = bpy.data.worlds.new("ambient") if scene.world is None else scene.world
    world.use_nodes = True
    background = next(
        node for node in world.node_tree.nodes if node.type == "BACKGROUND"
    )
    background.inputs["Color"].default_value = (0.15, 0.15, 0.16, 1.0)
    background.inputs["Strength"].default_value = 1.0
    scene.world = world
else:
    # Studio background like the concept sheets: a lit world, no alpha.
    scene.render.film_transparent = False
    world = bpy.data.worlds.new("studio") if scene.world is None else scene.world
    world.use_nodes = True
    background = next(
        node for node in world.node_tree.nodes if node.type == "BACKGROUND"
    )
    background.inputs["Color"].default_value = (0.16, 0.16, 0.17, 1.0)
    background.inputs["Strength"].default_value = 1.0
    scene.world = world
scene.render.resolution_x = res
scene.render.resolution_y = res * 3 // 2
# Frame from the actual bounding box and the camera FOV instead of a fixed
# multiple of the size: a fixed distance left the figure at half frame height.
half_w = (maxs[0] - mins[0]) / 2.0
half_h = (maxs[2] - mins[2]) / 2.0
half_d = (maxs[1] - mins[1]) / 2.0
tan_v = (camera.data.sensor_width / 2.0) / camera.data.lens
if scene.render.resolution_x > scene.render.resolution_y:
    tan_h = tan_v
    tan_v = tan_h * scene.render.resolution_y / scene.render.resolution_x
else:
    tan_h = tan_v * scene.render.resolution_x / scene.render.resolution_y
fit = 0.92
distance = max(half_h / (fit * tan_v), half_w / (fit * tan_h)) + half_d

# The imported figures face +Y: the -Y camera renders the back (verified by
# look.py on the archer tunic), so "front" sits on +Y.
for name, direction in (("front", 1.0), ("back", -1.0)):
    camera.location = center + Vector((0.0, direction * distance, 0.0))
    fill.location = camera.location
    scene.render.filepath = os.path.abspath(os.path.join(config["out"], f"{name}.png"))
    bpy.ops.render.render(write_still=True)
    print(f"render: {scene.render.filepath}")
    # Self-check: a frame that is all background (or a uniform slab) has no
    # chroma and no covered area. Cheap guard against the default-Cube and
    # backlight failures that silently produced grey frames.
    stats = bpy.data.images.load(scene.render.filepath)
    pixels = list(stats.pixels)
    covered = [
        (pixels[i], pixels[i + 1], pixels[i + 2])
        for i in range(0, len(pixels), 4)
        if pixels[i + 3] > 0.5
    ]
    bpy.data.images.remove(stats)
    if covered:
        share = 100.0 * len(covered) / (len(pixels) / 4)
        means = tuple(sum(c[k] for c in covered) / len(covered) for k in range(3))
        chroma = max(means) - min(means)
        print(
            f"stats: {share:.1f}% covered, mean RGB "
            f"({means[0]:.3f},{means[1]:.3f},{means[2]:.3f}), chroma {chroma:.3f}"
        )
        if share < 1.0 or chroma < 0.002:
            print("WARNING: frame is near-empty or colourless - check the render setup")
    else:
        print("WARNING: no covered pixels - frame is empty")
print(f"figure: {label}")
