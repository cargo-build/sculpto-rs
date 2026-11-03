"""Sculpto Garment pipeline: raw Tripo asset -> dressed, animated character.

Every step is a plain function over Blender datablocks so the whole flow runs
headless (`blender --background scene.blend --python ...`) and inside the UI
operator alike. The garment is reconstructed as a body-anchored offset
surface over the region the raw asset covers: coverage and the full material
come from the raw asset, while the body-derived geometry guarantees zero
penetration, real openings (collar, cuffs, hem, shaft) and armature-following
weights.
"""

import bmesh
import bpy
from math import radians
from mathutils import Vector, bvhtree

# Garment offset from the body surface, meters.
GARMENT_OFFSET = 0.008
# Extra offset at boundary rings so openings do not cut into the body when
# the mesh deforms with the armature.
BOUNDARY_RELIEF = 0.004


def posed_body(body, armature, clip):
    """A world-baked copy of the body as posed at the clip's first frame.

    The garment is constructed on the posed body, not the rest pose: a
    rest-pose build drapes sleeves along A-pose arms and the weight transfer
    then mixes chest and arm bindings, splaying the sleeves while the real
    arms hang (measured: sleeve centroid 0.25 m from the arm). On the posed
    mesh the arm is separated from the torso and nearest-vertex binding is
    unambiguous. Vertex order is preserved, so weight transfer stays valid.
    """
    action = bpy.data.actions.get(clip) if clip else None
    scene = bpy.context.scene
    if action is not None and armature is not None:
        armature.animation_data_create()
        armature.animation_data.action = action
        scene.frame_set(int(action.frame_range[0]))
        bpy.context.view_layer.update()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = body.evaluated_get(depsgraph).to_mesh()
    mesh = evaluated.copy()
    mesh.transform(body.matrix_world)
    body.to_mesh_clear()
    posed = bpy.data.objects.new("SculptoBodyPosed", mesh)
    bpy.context.collection.objects.link(posed)
    posed.hide_set(True)
    posed.hide_render = True
    return posed


def import_raw(path):
    """Import a raw garment asset, bake import transforms, orient Y-up->Z-up.

    Returns the joined raw mesh object and its material. After this call the
    raw mesh sits in scene orientation at its own scale: x = width, y =
    depth, z = height, i.e. a front relief faces +y.
    """
    before = set(bpy.data.objects)
    if path.lower().endswith((".glb", ".gltf")):
        bpy.ops.import_scene.gltf(filepath=path)
    elif path.lower().endswith(".fbx"):
        bpy.ops.wm.fbx_import(filepath=path)
    else:
        bpy.ops.wm.obj_import(filepath=path)
    imported = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
    if not imported:
        raise RuntimeError(f"no mesh imported from {path}")
    for obj in imported:
        obj.hide_set(False)
        obj.hide_viewport = False
    if len(imported) > 1:
        bpy.context.view_layer.objects.active = imported[0]
        for obj in imported:
            obj.select_set(True)
        bpy.ops.object.join()
    raw = bpy.context.view_layer.objects.active
    raw.name = "SculptoRaw"
    # Bake the importer's rotation/scale into the mesh, then orient the
    # glTF frame (Y-up) into the scene frame (Z-up) on the object.
    bpy.context.view_layer.objects.active = raw
    raw.select_set(True)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)
    raw.rotation_euler = (radians(90.0), 0.0, 0.0)
    bpy.context.view_layer.update()
    material = raw.data.materials[0] if raw.data.materials else None
    return raw, material


def place_raw(raw, body, kind, width_reference=None):
    """Scale and move the raw asset in front of the body it will dress.

    Returns world-space coverage: (hem_z, collar_z, width). The placement
    follows the class rules: upper-body hangs the collar at the neck base,
    footwear rests the sole on the ground, trousers anchor at the waist.
    `width_reference` (the rest-pose body) supplies the width the garment
    class is scaled against: a posed body with arms tucked measures
    narrower and shrinks the garment into a crop top.
    """
    zs = [(body.matrix_world @ v.co).z for v in body.data.vertices]
    zmin, zmax = min(zs), max(zs)
    height = zmax - zmin
    width_source = width_reference or body
    xs = [(width_source.matrix_world @ v.co).x for v in width_source.data.vertices]
    body_cx = (min(xs) + max(xs)) / 2
    ys = [(body.matrix_world @ v.co).y for v in body.data.vertices]
    front_y = min(ys)

    world = [
        raw.matrix_world @ v.co for v in raw.data.vertices
    ]
    rw = max(p.x for p in world) - min(p.x for p in world)
    rh = max(p.z for p in world) - min(p.z for p in world)
    if rw <= 1e-6 or rh <= 1e-6:
        raise RuntimeError("raw asset is degenerate")

    if kind == "upper-body":
        # The relief spans the shoulders incl. sleeves in its width; scale so
        # the torso part (~40% of the width) matches the body torso.
        target_width = 1.15 * (max(xs) - min(xs))
        scale = target_width / rw
        collar_z = zmin + 0.87 * height
        hem_z = None  # assigned after the chain from the raw proportions
    elif kind == "footwear":
        target_width = 2.4 * 0.10 * height  # one foot sole length, generous
        scale = target_width / rw
        # Sole on the floor, shaft rises by the item's own proportions:
        # anchoring the collar at ankle height buries a knee-high shaft
        # below the ground.
        hem_z = zmin + 0.01
        collar_z = hem_z + rh * scale
        raw.scale = (scale, scale, scale)
        raw.location = (
            body_cx - (min(p.x for p in world) + rw / 2) * scale,
            front_y - 0.05 - height * 0.02,
            hem_z - min(p.z for p in world) * scale,
        )
        bpy.context.view_layer.update()
        bpy.context.view_layer.objects.active = raw
        for obj in bpy.data.objects:
            obj.select_set(False)
        raw.select_set(True)
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
        return hem_z, collar_z, rw * scale
    elif kind == "trousers":
        target_width = 1.1 * 0.42 * height  # hip width approx
        scale = target_width / rw
        collar_z = zmin + 0.62 * height
        # A waist-anchored hem cannot go through the floor.
        hem_z = max(collar_z - rh * scale, zmin + 0.02)
    else:
        raise RuntimeError(f"no placement rule for kind `{kind}`")

    if hem_z is None:
        hem_z = collar_z - rh * scale
    raw.scale = (scale, scale, scale)
    raw.location = (
        body_cx - (min(p.x for p in world) + rw / 2) * scale,
        front_y - 0.05 - height * 0.02,
        hem_z - min(p.z for p in world) * scale,
    )
    bpy.context.view_layer.update()
    # Bake the placement into the mesh: every later query (ray casting,
    # bounds) then works in world coordinates with an identity transform.
    bpy.context.view_layer.objects.active = raw
    for obj in bpy.data.objects:
        obj.select_set(False)
    raw.select_set(True)
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    return hem_z, collar_z, rw * scale


def select_region(body, hem_z, collar_z, kind="upper-body"):
    """Body faces whose vertices all sit between the hem and collar lines.

    Non-upper classes exclude the arms: the band of a trousers or footwear
    garment overlaps hanging arms, which would sew arm tubes onto legs.
    Returns face indices; BMFace references die with their bmesh.
    """
    bm = bmesh.new()
    bm.from_mesh(body.data)
    bm.faces.ensure_lookup_table()
    world = body.matrix_world
    # Faces with a majority of vertices inside the band: a strict
    # all-vertices rule zigzags the hem and collar into torn-looking edges.
    faces = [
        face.index
        for face in bm.faces
        if sum(1 for v in face.verts if hem_z <= (world @ v.co).z <= collar_z) >= 2
        and (
            kind == "upper-body"
            or abs(sum((world @ v.co).x for v in face.verts) / len(face.verts)) <= 0.21
        )
    ]
    bm.free()
    return faces


def build_garment(body, face_indices):
    """Offset surface over the selected body faces, as a new object."""
    bm = bmesh.new()
    bm.from_mesh(body.data)
    bm.faces.ensure_lookup_table()
    bm.verts.ensure_lookup_table()
    keep = set(face_indices)
    for face in list(bm.faces):
        if face.index not in keep:
            bm.faces.remove(face)
    for vert in list(bm.verts):
        if not vert.link_faces:
            bm.verts.remove(vert)
    boundary = {v.index for v in bm.verts if v.is_boundary}
    for vert in bm.verts:
        offset = GARMENT_OFFSET + (BOUNDARY_RELIEF if vert.index in boundary else 0.0)
        vert.co += vert.normal * offset
    mesh = bpy.data.meshes.new("SculptoGarment")
    bm.to_mesh(mesh)
    bm.free()
    garment = bpy.data.objects.new("SculptoGarment", mesh)
    bpy.context.collection.objects.link(garment)
    garment.matrix_world = body.matrix_world.copy()
    # The bmesh round-trip carries the body's vertex groups through its
    # deform layer; they are stale on the trimmed surface and would mix into
    # the transferred weights, so drop them here.
    while garment.vertex_groups:
        garment.vertex_groups.remove(garment.vertex_groups[0])
    print(f"build_garment: groups after clear: {len(garment.vertex_groups)}")
    return garment


def project_texture(garment, raw, material):
    """Garment UVs by ray-casting the placed raw mesh along the body axis."""
    if material is None or not material.use_nodes:
        return False
    image = next(
        (n.image for n in material.node_tree.nodes if n.type == "TEX_IMAGE" and n.image),
        None,
    )
    if image is None:
        return False
    raw_bm = bmesh.new()
    raw_bm.from_mesh(raw.data)
    raw_bm.faces.ensure_lookup_table()
    raw_bm.verts.ensure_lookup_table()
    raw_uv = raw_bm.loops.layers.uv.active
    if raw_uv is None:
        raw_bm.free()
        return False
    raw_mesh = bpy.data.meshes.new("SculptoRawBVH")
    raw_bm.to_mesh(raw_mesh)
    tree = bvhtree.BVHTree.FromBMesh(raw_bm)
    bm = bmesh.new()
    bm.from_mesh(garment.data)
    uv_layer = bm.loops.layers.uv.get("SculptoProjected")
    if uv_layer is None:
        uv_layer = bm.loops.layers.uv.new("SculptoProjected")
    axis = Vector((0.0, -1.0, 0.0))
    hits = 0
    for face in bm.faces:
        for loop in face.loops:
            origin = garment.matrix_world @ loop.vert.co
            location, _normal, index, hit = tree.ray_cast(origin, axis)
            if hit:
                # Barycentric interpolation of the hit face's corner UVs:
                # nearest-corner snapping fragments a shared catalog atlas
                # into mottle instead of coherent cloth.
                raw_face = raw_bm.faces[index]
                corners = raw_face.loops[:3]
                a, b, c = (l.vert.co for l in corners)
                ab, ac = b - a, c - a
                denom = ab.dot(ab) * ac.dot(ac) - ab.dot(ac) ** 2
                if abs(denom) < 1e-12:
                    loop[uv_layer].uv = corners[0][raw_uv].uv
                else:
                    ap = location - a
                    d20, d21 = ap.dot(ab), ap.dot(ac)
                    v = (ac.dot(ac) * d20 - ab.dot(ac) * d21) / denom
                    w = (ab.dot(ab) * d21 - ab.dot(ac) * d20) / denom
                    u = 1.0 - v - w
                    v = max(0.0, min(1.0, v))
                    w = max(0.0, min(1.0, w))
                    u = max(0.0, min(1.0 - w - v + 1e-6, u))
                    uvs = [l[raw_uv].uv.copy() for l in corners]
                    loop[uv_layer].uv = (
                        uvs[0] * u + uvs[1] * v + uvs[2] * w
                    )
                hits += 1
            else:
                loop[uv_layer].uv = (0.5, 0.5)
    bm.to_mesh(garment.data)
    bm.free()
    raw_bm.free()
    bpy.data.meshes.remove(raw_mesh)
    # The body's own UV layers ride along on the copied faces; drop them at
    # the mesh level (bmesh cannot delete UV layers) and make the projected
    # layer the active one the material samples.
    for layer in list(garment.data.uv_layers):
        if layer.name == "SculptoProjected":
            garment.data.uv_layers.active = layer
        else:
            garment.data.uv_layers.remove(layer)
    garment.data.materials.clear()
    mat = material.copy()
    mat.name = "SculptoGarmentMaterial"
    garment.data.materials.append(mat)
    return hits > 0


def _unpose_garment(garment, armature):
    """Undo the clip's first-frame deformation on the garment's own vertices.

    The garment is constructed on the posed body, but the Armature modifier
    deforms from the rest pose — without this compensation it would apply the
    frame-1 pose twice and overshoot (measured: sleeve verts pushed 0.15 m
    past the arm). For weights {(bone, w)} and frame-1 skinning matrices M,
    built v1 = (sum w*M) @ v0, so v0 = (sum w*M)^-1 @ v1 restores the rest
    geometry the modifier expects.
    """
    from mathutils import Matrix, Vector

    world = armature.matrix_world
    skinning = {}
    for bone in armature.pose.bones:
        bind = world @ bone.bone.matrix_local
        skinning[bone.name] = (world @ bone.matrix) @ bind.inverted()

    import bmesh

    bm = bmesh.new()
    bm.from_mesh(garment.data)
    layer = bm.verts.layers.deform.verify()
    index_to_name = {g.index: g.name for g in garment.vertex_groups}
    for vert in bm.verts:
        blend = Matrix.Identity(4) * 0.0
        total = 0.0
        for group_index, weight in vert[layer].items():
            name = index_to_name.get(group_index)
            matrix = skinning.get(name)
            if matrix is None or weight <= 0.0:
                continue
            blend += matrix * weight
            total += weight
        if total <= 0.0:
            continue
        try:
            rest = blend.inverted() @ vert.co
        except ValueError:
            continue
        vert.co = rest.copy() if isinstance(rest, Vector) else Vector(rest)
    bm.to_mesh(garment.data)
    bm.free()


def wire_animation(garment, armature, body, clip):
    """Bind the garment to the armature by transferring the body's weights.

    The body already carries the reviewed rigging, so each garment vertex
    takes the normalized weights of the nearest body vertex — no bone-space
    reconstruction, and sleeves track arms exactly as the body does. Weights
    are collected first and written once (`ADD` accumulation during
    collection produced cross-side phantom weights on shared indices), and
    the garment rides an Armature modifier because `parent_set` silently
    no-ops in background mode.
    """
    from mathutils import kdtree

    body_groups = {g.index: g.name for g in body.vertex_groups}
    if not body_groups:
        raise RuntimeError(f"body {body.name} has no vertex groups to transfer")
    size = len(body.data.vertices)
    tree = kdtree.KDTree(size)
    body_world = body.matrix_world
    for index, vert in enumerate(body.data.vertices):
        tree.insert(body_world @ vert.co, index)
    tree.balance()
    garment_world = garment.matrix_world

    plan = {}
    for vert in garment.data.vertices:
        _position, nearest_index, _distance = tree.find(garment_world @ vert.co)
        weights = {
            body_groups[entry.group]: entry.weight
            for entry in body.data.vertices[nearest_index].groups
        }
        total = sum(weights.values()) or 1.0
        plan[vert.index] = {name: weight / total for name, weight in weights.items()}

    while garment.vertex_groups:
        garment.vertex_groups.remove(garment.vertex_groups[0])
    # One group per bone name: calling new() per (vertex, group) pair spawned
    # thousands of suffixed duplicates that split the weights.
    names = sorted({name for weights in plan.values() for name in weights})
    group_index = {name: garment.vertex_groups.new(name=name).index for name in names}
    # Write through the bmesh deform layer: `VertexGroup.add` was observed to
    # alias group objects (one call landing in two groups with identical
    # weights), which duplicated memberships across sides. The deform layer
    # replaces each vertex's weights exactly.
    import bmesh

    bm = bmesh.new()
    bm.from_mesh(garment.data)
    layer = bm.verts.layers.deform.verify()
    for vert in bm.verts:
        weights = plan[vert.index]
        dvert = vert[layer]
        dvert.clear()
        for name, weight in weights.items():
            dvert[group_index[name]] = weight
    bm.to_mesh(garment.data)
    bm.free()
    modifier = garment.modifiers.new("SculptoArmature", "ARMATURE")
    modifier.object = armature
    if clip and clip in bpy.data.actions:
        armature.animation_data_create()
        armature.animation_data.action = bpy.data.actions[clip]
    _unpose_garment(garment, armature)

# Native outfit families a dressed class replaces, so the audit frames show
# one tunic/trouser/boot, not the actor's own pieces layered under ours.
REPLACED_FAMILIES = {
    "upper-body": ("Tunic_", "WaistBelt", "Collar"),
    "trousers": ("Trousers_",),
    "footwear": ("Boot_", "femalefeet"),
}


def render_audit(body, clip, out_dir, frames, views, kind="upper-body"):
    """Render auditor screenshots: clip frames x camera views."""
    import os

    os.makedirs(out_dir, exist_ok=True)
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_EEVEE"
    scene.render.resolution_x = 720
    scene.render.resolution_y = 960
    world = bpy.data.worlds.new("SculptoAuditWorld")
    world.use_nodes = True
    background = world.node_tree.nodes["Background"]
    background.inputs[0].default_value = (0.25, 0.25, 0.28, 1.0)
    background.inputs[1].default_value = 1.0
    scene.world = world
    hidden = []
    for prefix in REPLACED_FAMILIES.get(kind, ()):
        for obj in scene.objects:
            if obj.type == "MESH" and obj.name.startswith(prefix) and not obj.hide_render:
                obj.hide_render = True
                hidden.append(obj)
    zs = [(body.matrix_world @ v.co).z for v in body.data.vertices]
    focus = Vector((0.0, 0.0, (min(zs) + max(zs)) / 2))
    span = max(zs) - min(zs)
    camera = bpy.data.objects.new("SculptoAuditCam", bpy.data.cameras.new("SculptoAuditCam"))
    camera.data.lens = 85
    bpy.context.collection.objects.link(camera)
    scene.camera = camera
    light = bpy.data.objects.new("SculptoAuditSun", bpy.data.lights.new("SculptoAuditSun", "SUN"))
    light.data.energy = 15.0
    bpy.context.collection.objects.link(light)
    action = bpy.data.actions.get(clip) if clip else None
    armature = next((o for o in bpy.data.objects if o.type == "ARMATURE"), None)
    if action is not None and armature is not None:
        armature.animation_data_create()
        armature.animation_data.action = action
    written = []
    directions = {
        "front": Vector((0.0, -1.0, 0.0)),
        "side": Vector((1.0, 0.0, 0.0)),
        "back": Vector((0.0, 1.0, 0.0)),
    }
    garment = bpy.data.objects.get("SculptoGarment")
    for frame in frames:
        scene.frame_set(frame)
        bpy.context.view_layer.update()
        for view in views:
            direction = directions.get(view, directions["front"])
            camera.location = focus + direction * (2.6 * span)
            camera.rotation_euler = (
                (focus - camera.location).normalized().to_track_quat("-Z", "Y").to_euler()
            )
            light.location = focus + Vector((2.0, -2.0, 2.0)) * span
            path = os.path.join(out_dir, f"f{frame:03d}_{view}.png")
            scene.render.filepath = path
            bpy.ops.render.render(write_still=True)
            written.append(path)
            # Garment-only frame at the same pose: unambiguous silhouette and
            # texture without the actor's own outfit under it.
            if garment is not None:
                isolated = []
                for obj in scene.objects:
                    if obj.type == "MESH" and obj.name not in ("SculptoGarment",):
                        if not obj.hide_render:
                            obj.hide_render = True
                            isolated.append(obj)
                path = os.path.join(out_dir, f"f{frame:03d}_{view}_garment.png")
                scene.render.filepath = path
                bpy.ops.render.render(write_still=True)
                written.append(path)
                for obj in isolated:
                    obj.hide_render = False
    for obj in hidden:
        obj.hide_render = False
    return written