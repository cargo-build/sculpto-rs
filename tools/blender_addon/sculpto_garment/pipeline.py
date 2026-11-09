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
import tempfile
from math import radians
from pathlib import Path
from mathutils import Vector

from . import cli_bridge

# Every measure/normalise/decide rule of the dress flow is a command now
# (DECL0002): `outfit.place` (this file's place_raw), `outfit.band`
# (select_region), `outfit.offsets` (build_garment), `outfit.projection`
# (project_texture) and `outfit.weights` (wire_animation). The numbers live in
# `crates/sculpto-mesh/src/garment.rs`; what stays here is Blender data
# construction — object transforms, bmesh edits, modifier wiring.


def mesh_data(obj, world):
    """Vertex positions and polygons of a mesh, in world or object space."""
    matrix = obj.matrix_world if world else None
    positions = [
        list(matrix @ vertex.co) if matrix is not None else list(vertex.co)
        for vertex in obj.data.vertices
    ]
    polygons = [list(polygon.vertices) for polygon in obj.data.polygons]
    return positions, polygons


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


def import_nif(path, operator, cli=None):
    """Import a NIF through the sculpto command and return its mesh.

    The command reads the NIF, writes the OBJ artifact and reports the shape's
    counts in the envelope; Blender then builds data from that artifact and
    the vertex count is asserted against the envelope (the model is the
    contract, not the importer's goodwill). The artifact lands in a temporary
    directory and dies with it: Blender data is the only thing that survives.
    A refused command is reported through `operator` (`kind: message`, F06)
    and None is returned; the caller cancels.
    """
    cli = cli or cli_bridge.SculptoCli()
    with tempfile.TemporaryDirectory(prefix="sculpto-import-") as work:
        artifact = Path(work) / (Path(path).stem + ".obj")
        result = cli_bridge.run_cli(
            operator, "export.obj", cli_bridge.export_obj_payload(path, artifact), cli=cli
        )
        if result is None:
            return None
        before = set(bpy.data.objects)
        bpy.ops.wm.obj_import(filepath=str(artifact))
        imported = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
        if not imported:
            raise RuntimeError(f"the artifact has no mesh: {result}")
        for obj in imported:
            obj.hide_set(False)
            obj.hide_viewport = False
        raw = imported[0] if len(imported) == 1 else join_imported(imported)
        raw.name = "SculptoRaw"
        # The artifact is already in the Y-up display frame the CLI writes
        # (crates/sculpto-engine/src/obj.rs), and the OBJ importer's default
        # Y-up preset maps it into scene Z-up — no glTF rotation to bake.
        vertices, triangles = len(raw.data.vertices), len(raw.data.polygons)
        if vertices != result["vertices"] or triangles != result["triangles"]:
            raise RuntimeError(
                f"imported {vertices} vertices / {triangles} faces but the envelope "
                f"reported {result['vertices']} / {result['triangles']}"
            )
    material = raw.data.materials[0] if raw.data.materials else None
    return raw, material


def join_imported(imported):
    """Join multiple imported mesh objects into the last-selected one."""
    bpy.context.view_layer.objects.active = imported[0]
    for obj in imported:
        obj.select_set(True)
    bpy.ops.object.join()
    return bpy.context.view_layer.objects.active


def import_raw(path, operator):
    """Import a raw garment asset, bake import transforms, orient Y-up->Z-up.

    `.nif` files never reach a Blender importer: they go through the sculpto
    command, which owns NIF reading (DECL0002), and come back as an OBJ
    artifact already in scene orientation. Every other format is Blender's
    own importer; after this call the raw mesh sits in scene orientation at
    its own scale: x = width, y = depth, z = height, i.e. a front relief
    faces +y. Returns (raw, material), or None after `operator` reported a
    refused NIF command.
    """
    if path.lower().endswith(".nif"):
        return import_nif(path, operator)
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
        raw = join_imported(imported)
    else:
        raw = imported[0]
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


def place_raw(raw, body, kind, width_reference=None, cli=None):
    """Scale and move the raw asset in front of the body it will dress.

    Returns world-space coverage: (hem_z, collar_z, width). The class fractions
    and the front clearance are `outfit.place`'s; the add-on measures the body
    and the raw, then applies the returned object transform. `width_reference`
    (the rest-pose body) supplies the width the garment class is scaled against:
    a posed body with arms tucked measures narrower and shrinks the garment into
    a crop top.
    """
    zs = [(body.matrix_world @ v.co).z for v in body.data.vertices]
    width_source = width_reference or body
    xs = [(width_source.matrix_world @ v.co).x for v in width_source.data.vertices]
    ys = [(body.matrix_world @ v.co).y for v in body.data.vertices]
    world = [raw.matrix_world @ v.co for v in raw.data.vertices]
    placement = (cli or cli_bridge.SculptoCli()).run(
        "outfit.place",
        {
            "kind": kind,
            "body": {
                "min_z": min(zs),
                "max_z": max(zs),
                "front_y": min(ys),
                "width_min_x": min(xs),
                "width_max_x": max(xs),
            },
            "raw": {
                "min_x": min(p.x for p in world),
                "max_x": max(p.x for p in world),
                "min_z": min(p.z for p in world),
                "max_z": max(p.z for p in world),
            },
        },
    )
    raw.scale = (placement["scale"],) * 3
    raw.location = tuple(placement["location"])
    bpy.context.view_layer.update()
    # Bake the placement into the mesh: every later query (ray casting,
    # bounds) then works in world coordinates with an identity transform.
    bpy.context.view_layer.objects.active = raw
    for obj in bpy.data.objects:
        obj.select_set(False)
    raw.select_set(True)
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    return placement["hem_z"], placement["collar_z"], placement["width"]


def select_region(body, hem_z, collar_z, kind="upper-body", cli=None):
    """Body faces whose vertices all sit between the hem and collar lines.

    The majority rule and the arm limit are `outfit.band`'s (DECL0002); the
    add-on passes the body's world-space polygons and keeps the returned face
    indices. BMFace references die with their bmesh, so indices are the contract.
    """
    positions, polygons = mesh_data(body, world=True)
    band = (cli or cli_bridge.SculptoCli()).run(
        "outfit.band",
        {
            "kind": kind,
            "positions": positions,
            "polygons": polygons,
            "hem_z": hem_z,
            "collar_z": collar_z,
        },
    )
    return band["faces"]


def build_garment(body, face_indices, cli=None):
    """Offset surface over the selected body faces, as a new object.

    `outfit.offsets` trims the body to the selected faces and answers the
    per-vertex offset, including the boundary relief; Blender owns the vertex
    normals, so the displacement is applied here along them.
    """
    positions, polygons = mesh_data(body, world=False)
    surface = (cli or cli_bridge.SculptoCli()).run(
        "outfit.offsets",
        {"positions": positions, "polygons": polygons, "keep": list(face_indices)},
    )
    mesh = bpy.data.meshes.new("SculptoGarment")
    mesh.from_pydata(surface["positions"], [], surface["polygons"])
    mesh.update()
    if len(mesh.vertices) != len(surface["offsets"]):
        raise RuntimeError(
            f"the offset command answered {len(surface['offsets'])} offsets for "
            f"{len(mesh.vertices)} vertices"
        )
    # Snapshot the normals first: they are derived from the mesh, so displacing
    # as we read would fold each move into the next vertex's normal.
    normals = [vertex.normal.copy() for vertex in mesh.vertices]
    for vertex, normal, offset in zip(mesh.vertices, normals, surface["offsets"]):
        vertex.co += normal * offset
    garment = bpy.data.objects.new("SculptoGarment", mesh)
    bpy.context.collection.objects.link(garment)
    garment.matrix_world = body.matrix_world.copy()
    return garment


def project_texture(garment, raw, material, cli=None):
    """Garment UVs by ray-casting the placed raw mesh.

    `outfit.projection` casts one ray per garment loop and answers the atlas
    coordinate: barycentric interpolation over the hit triangle's corner UVs, or
    the miss snap. The raw copy is triangulated so the cast surface and the
    corner UVs the command interpolates describe the same triangles.
    """
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
    bmesh.ops.triangulate(raw_bm, faces=raw_bm.faces[:])
    raw_bm.faces.ensure_lookup_table()
    raw_bm.verts.ensure_lookup_table()
    raw_uv = raw_bm.loops.layers.uv.active
    if raw_uv is None:
        raw_bm.free()
        return False
    # Both sides stay in the raw asset's own frame: `place_raw` bakes the
    # placement into the mesh, and the garment shares the body's frame.
    raw_triangles = [
        {
            "positions": [list(corner.vert.co) for corner in face.loops],
            "uvs": [list(corner[raw_uv].uv) for corner in face.loops],
        }
        for face in raw_bm.faces
    ]
    raw_bm.free()
    bm = bmesh.new()
    bm.from_mesh(garment.data)
    uv_layer = bm.loops.layers.uv.get("SculptoProjected")
    if uv_layer is None:
        uv_layer = bm.loops.layers.uv.new("SculptoProjected")
    bm.faces.ensure_lookup_table()
    origins = [
        list(garment.matrix_world @ loop.vert.co)
        for face in bm.faces
        for loop in face.loops
    ]
    projection = (cli or cli_bridge.SculptoCli()).run(
        "outfit.projection",
        {"origins": origins, "raw": raw_triangles},
    )
    uvs = projection["uvs"]
    if len(uvs) != len(origins):
        bm.free()
        raise RuntimeError(
            f"the projection command answered {len(uvs)} UVs for {len(origins)} loops"
        )
    index = 0
    for face in bm.faces:
        for loop in face.loops:
            loop[uv_layer].uv = uvs[index]
            index += 1
    bm.to_mesh(garment.data)
    bm.free()
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
    return projection["hits"] > 0


def _unpose_garment(garment, armature, weights, cli=None):
    """Undo the clip's first-frame deformation on the garment's own vertices.

    The garment is constructed on the posed body, but the Armature modifier
    deforms from the rest pose — without this compensation it would apply the
    frame-1 pose twice and overshoot (measured: sleeve verts pushed 0.15 m past
    the arm). `outfit.weights` inverts the blended skin matrix; the matrices
    are Blender's, the inversion and the blend are the command's.
    """
    world = armature.matrix_world
    skinning = {}
    for bone in armature.pose.bones:
        bind = world @ bone.bone.matrix_local
        skinning[bone.name] = (world @ bone.matrix) @ bind.inverted()
    unposed = (cli or cli_bridge.SculptoCli()).run(
        "outfit.weights",
        {
            "stage": "unpose",
            "positions": [list(vertex.co) for vertex in garment.data.vertices],
            "weights": [weights[vertex.index] for vertex in garment.data.vertices],
            "skinning": {
                name: [value for row in matrix for value in row]
                for name, matrix in skinning.items()
            },
        },
    )
    for vertex, position in zip(garment.data.vertices, unposed["positions"]):
        vertex.co = position


def wire_animation(garment, armature, body, clip, cli=None):
    """Bind the garment to the armature by transferring the body's weights.

    The body already carries the reviewed rigging, so each garment vertex takes
    the normalized weights of the nearest body vertex (`outfit.weights`) — no
    bone-space reconstruction, and sleeves track arms exactly as the body does.
    Weights are written once through the deform layer: `VertexGroup.add` and
    `ADD` accumulation were both observed to duplicate memberships across sides.
    The garment rides an Armature modifier because `parent_set` silently no-ops
    in background mode.
    """
    body_groups = {g.index: g.name for g in body.vertex_groups}
    if not body_groups:
        raise RuntimeError(f"body {body.name} has no vertex groups to transfer")
    body_world = body.matrix_world
    garment_world = garment.matrix_world
    client = cli or cli_bridge.SculptoCli()
    transferred = client.run(
        "outfit.weights",
        {
            "stage": "transfer",
            "garment_positions": [
                list(garment_world @ vertex.co) for vertex in garment.data.vertices
            ],
            "body_positions": [list(body_world @ vertex.co) for vertex in body.data.vertices],
            "body_weights": [
                {body_groups[entry.group]: entry.weight for entry in vertex.groups}
                for vertex in body.data.vertices
            ],
        },
    )["weights"]

    while garment.vertex_groups:
        garment.vertex_groups.remove(garment.vertex_groups[0])
    # One group per bone name: calling new() per (vertex, group) pair spawned
    # thousands of suffixed duplicates that split the weights.
    names = sorted({name for weights in transferred for name in weights})
    group_index = {name: garment.vertex_groups.new(name=name).index for name in names}
    bm = bmesh.new()
    bm.from_mesh(garment.data)
    layer = bm.verts.layers.deform.verify()
    for vert in bm.verts:
        dvert = vert[layer]
        dvert.clear()
        for name, weight in transferred[vert.index].items():
            dvert[group_index[name]] = weight
    bm.to_mesh(garment.data)
    bm.free()
    modifier = garment.modifiers.new("SculptoArmature", "ARMATURE")
    modifier.object = armature
    if clip and clip in bpy.data.actions:
        armature.animation_data_create()
        armature.animation_data.action = bpy.data.actions[clip]
    _unpose_garment(garment, armature, transferred, cli=client)

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