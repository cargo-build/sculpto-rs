"""Bake original PBR appearance onto ordered control geometry (@req DECL0013).

Blender: --background --factory-startup --python-exit-code 1
--python tools/bake_garment_materials.py -- recipe.json new-output-directory
See README.md for the explicit source/frame recipe. No fitting is performed.
"""

import hashlib
import json
import os
from pathlib import Path
import sys
import time

import bpy
import numpy as np
from mathutils import Matrix
from mathutils.bvhtree import BVHTree

sys.path.insert(0, str(Path(__file__).resolve().parent))
from garment_uv_atlas import qualify_atlas


CHANNELS = {"basecolor": "Base Color", "roughness": "Roughness", "metallic": "Metallic"}
FRAMES = ("sculpto-source-local-y-up", "blender-source-local-z-up", "sculpto-world-y-up", "blender-world-z-up")


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def array_hash(values):
    return hashlib.sha256(np.ascontiguousarray(values).tobytes()).hexdigest()


def geometry(path):
    """Require ordered finite noncollapsed triangles without repairing input."""
    with np.load(path, allow_pickle=False) as data:
        vertices = np.asarray(data["vertices"], dtype=np.float32)
        triangles = np.asarray(data["triangles"])
    if vertices.ndim != 2 or vertices.shape[1] != 3 or not len(vertices) or not np.isfinite(vertices).all():
        raise ValueError("vertices must be a nonempty finite Nx3 array")
    if triangles.ndim != 2 or triangles.shape[1] != 3 or not len(triangles) or triangles.dtype.kind not in "iu":
        raise ValueError("triangles must be a nonempty integer Nx3 array")
    if triangles.min() < 0 or triangles.max() >= len(vertices):
        raise ValueError("triangle index outside vertex array")
    points = vertices[triangles].astype(np.float64)
    if np.any(np.linalg.norm(np.cross(points[:, 1] - points[:, 0], points[:, 2] - points[:, 0]), axis=1) == 0):
        raise ValueError("collapsed triangle geometry")
    return vertices, triangles.astype(np.int32)


def control_transform(frame, source):
    if frame not in FRAMES:
        raise ValueError(f"control_frame must be one of {FRAMES}")
    if frame == "blender-world-z-up":
        return Matrix.Identity(4)
    axes = Matrix(((1, 0, 0, 0), (0, 0, -1, 0), (0, 1, 0, 0), (0, 0, 0, 1)))
    if frame == "sculpto-world-y-up":
        return axes
    return source.matrix_world @ axes if frame == FRAMES[0] else source.matrix_world.copy()


def image_input(socket, images, normal=False):
    """Accept the explicit native image PBR subset; reject graph approximations."""
    if not socket.is_linked:
        return
    node = socket.links[0].from_node
    if normal:
        if node.type != "NORMAL_MAP" or node.space != "TANGENT" or node.inputs["Strength"].is_linked:
            raise ValueError("normal must use an unmodulated tangent Normal Map node")
        image_input(node.inputs["Color"], images)
        if not node.inputs["Color"].is_linked:
            raise ValueError("Normal Map requires an image")
        return
    if node.type == "SEPARATE_COLOR" and node.mode == "RGB":
        image_input(node.inputs["Color"], images)
        if not node.inputs["Color"].is_linked:
            raise ValueError("Separate Color requires an image")
        return
    if node.type != "TEX_IMAGE" or node.image is None:
        raise ValueError(f"unsupported source channel graph: {node.type}")
    if node.inputs["Vector"].is_linked or node.projection != "FLAT":
        raise ValueError("source images require native active UV coordinates and flat projection")
    image = node.image
    path = Path(bpy.path.abspath(image.filepath, library=image.library)) if image.filepath else None
    if image.source != "FILE" or (not image.packed_file and (path is None or not path.is_file())):
        raise ValueError(f"missing source image: {image.name}: {path}")
    # Library-appended images decode lazily; requesting pixels loads native data.
    if not len(image.pixels) or not image.has_data or min(image.size) <= 0:
        raise ValueError(f"source image has no readable pixels: {image.name}")
    if image.packed_file:
        digest = hashlib.sha256(image.packed_file.data).hexdigest()
    else:
        digest = sha256(path)
    images[image.name] = dict(path=str(path) if path is not None else None, sha256=digest, packed=bool(image.packed_file),
                              size=list(image.size), colorspace=image.colorspace_settings.name)


def source_materials(source):
    if source.modifiers or source.data.shape_keys or source.vertex_groups or source.constraints or source.animation_data or source.parent:
        raise ValueError("source must be a static unbound extracted mesh")
    if source.data.uv_layers.active is None:
        raise ValueError("source mesh has no active UV map")
    if not source.data.materials or any(material is None for material in source.data.materials):
        raise ValueError("source mesh has missing materials")
    materials = []
    images = {}
    for original in source.data.materials:
        if not original.use_nodes:
            raise ValueError("source material requires a native Principled graph")
        outputs = [node for node in original.node_tree.nodes if node.type == "OUTPUT_MATERIAL" and node.is_active_output]
        if len(outputs) != 1 or not outputs[0].inputs["Surface"].is_linked:
            raise ValueError("source material requires exactly one connected active output")
        output = outputs[0]
        shader = output.inputs["Surface"].links[0].from_node
        if shader.type != "BSDF_PRINCIPLED" or output.inputs["Volume"].is_linked or output.inputs["Displacement"].is_linked:
            raise ValueError("source requires direct Principled surface without volume or displacement")
        if sum(node.type == "BSDF_PRINCIPLED" for node in original.node_tree.nodes) != 1:
            raise ValueError("source material requires exactly one Principled shader")
        supported = set(CHANNELS.values()) | {"Normal"}
        for socket in shader.inputs:
            if socket.is_linked and socket.name not in supported:
                raise ValueError(f"unsupported linked Principled input: {socket.name}")
        for name in ("Coat Weight", "Transmission Weight", "Subsurface Weight", "Sheen Weight", "Anisotropic"):
            if shader.inputs[name].default_value != 0:
                raise ValueError(f"unsupported nonzero Principled input: {name}")
        if shader.inputs["Emission Strength"].default_value and any(shader.inputs["Emission Color"].default_value[:3]):
            raise ValueError("emissive source materials are unsupported")
        if shader.inputs["Alpha"].default_value != 1:
            raise ValueError("transparent source materials are unsupported")
        for name in CHANNELS.values():
            image_input(shader.inputs[name], images)
        image_input(shader.inputs["Normal"], images, normal=True)
        materials.append(original)
    return materials, images


def constant_settings(materials):
    """One target shader cannot preserve conflicting ancillary material values."""
    settings = None
    for material in materials:
        shader = next(node for node in material.node_tree.nodes if node.type == "BSDF_PRINCIPLED")
        current = {}
        for socket in shader.inputs:
            if not socket.is_unavailable and socket.name not in set(CHANNELS.values()) | {"Normal"}:
                value = socket.default_value
                current[socket.name] = tuple(value) if hasattr(value, "__len__") else value
        if settings is not None and settings != current:
            raise ValueError("source materials have conflicting unbaked Principled settings")
        settings = current
    return settings


def emit_channel(materials, channel):
    """Temporarily expose actual scalar/color sockets through native emission."""
    for material in materials:
        tree = material.node_tree
        output = next(n for n in tree.nodes if n.type == "OUTPUT_MATERIAL" and n.is_active_output)
        shader = next(n for n in tree.nodes if n.type == "BSDF_PRINCIPLED")
        emission = tree.nodes.get("SculptoBakeEmission")
        if emission is None:
            emission = tree.nodes.new("ShaderNodeEmission")
            emission.name = "SculptoBakeEmission"
        for link in tuple(emission.inputs["Color"].links):
            tree.links.remove(link)
        if channel == "coverage":
            emission.inputs["Color"].default_value = (1, 1, 1, 1)
        else:
            socket = shader.inputs[CHANNELS[channel]]
            if socket.is_linked:
                tree.links.new(socket.links[0].from_socket, emission.inputs["Color"])
            else:
                value = socket.default_value
                emission.inputs["Color"].default_value = tuple(value) if hasattr(value, "__len__") else (value, value, value, 1)
        tree.links.new(emission.outputs[0], output.inputs["Surface"])


def restore_surface(materials):
    for material in materials:
        tree = material.node_tree
        output = next(n for n in tree.nodes if n.type == "OUTPUT_MATERIAL" and n.is_active_output)
        shader = next(n for n in tree.nodes if n.type == "BSDF_PRINCIPLED")
        tree.links.new(shader.outputs[0], output.inputs["Surface"])
        emission = tree.nodes.get("SculptoBakeEmission")
        if emission:
            tree.nodes.remove(emission)


def select(target, source=None):
    bpy.ops.object.select_all(action="DESELECT")
    target.select_set(True)
    if source is not None:
        source.select_set(True)
    bpy.context.view_layer.objects.active = target


def pixels(image):
    data = np.empty(len(image.pixels), dtype=np.float32)
    image.pixels.foreach_get(data)
    return data.reshape(-1, 4)


def save_image(image, path):
    image.filepath_raw = str(path)
    image.file_format = "PNG"
    image.save()
    image.pack()
    return dict(path=str(path), sha256=sha256(path), colorspace=image.colorspace_settings.name)


def frame_evidence(source, target, tolerance, extrusion, distance):
    """Native BVH diagnostics are witnesses, never a second appearance transfer."""
    points = [source.matrix_world @ vertex.co for vertex in source.data.vertices]
    tree = BVHTree.FromPolygons(points, [list(face.vertices) for face in source.data.polygons])
    world = [target.matrix_world @ vertex.co for vertex in target.data.vertices]
    distances = np.asarray([tree.find_nearest(point)[3] for point in world])
    if float(distances.max()) > tolerance:
        raise ValueError(f"declared frame exceeds frame_max_distance: {distances.max()} > {tolerance}")
    normals = target.matrix_world.to_3x3().inverted().transposed()
    witnesses = []
    wrong_side = 0
    misses = 0
    for index, face in enumerate(target.data.polygons):
        point = target.matrix_world @ face.center
        normal = (normals @ face.normal).normalized()
        hit, hit_normal, source_face, ray_distance = tree.ray_cast(point + normal * extrusion, -normal, distance)
        dot = float(normal.dot(hit_normal)) if hit is not None else None
        wrong_side += int(dot is not None and dot <= 0)
        misses += int(hit is None)
        if index in (0, len(target.data.polygons)//4, len(target.data.polygons)//2, 3*len(target.data.polygons)//4) or (dot is not None and dot <= 0 and len(witnesses) < 12):
            witnesses.append(dict(target_face=index, target_world=list(point), normal=list(normal),
                                  source_face=source_face, hit_world=list(hit) if hit is not None else None,
                                  normal_dot=dot, ray_distance=ray_distance))
    return dict(source_matrix_world=np.asarray(source.matrix_world).tolist(),
                control_to_world=np.asarray(target.matrix_world).tolist(),
                source_world_bounds=[np.min(points, axis=0).tolist(), np.max(points, axis=0).tolist()],
                control_world_bounds=[np.min(world, axis=0).tolist(), np.max(world, axis=0).tolist()],
                vertex_nearest_distance=dict(max=float(distances.max()), p95=float(np.percentile(distances, 95)), mean=float(distances.mean())),
                centroid_ray_diagnostics=dict(misses=misses, opposite_normal_hits=wrong_side,
                    limitation="face-normal centroid witnesses approximate the interpolated native bake rays", witnesses=witnesses))


def main(recipe_path, output):
    started = time.monotonic()
    recipe_path, output = Path(recipe_path).resolve(), Path(output).resolve()
    recipe = json.loads(recipe_path.read_text())
    if output.exists():
        raise FileExistsError(f"refusing to overwrite {output}")
    if "sculpto" not in recipe or "sculpto_sha256" not in recipe:
        raise ValueError("recipe requires sculpto and sculpto_sha256")
    binary = Path(recipe["sculpto"]).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK) or sha256(binary) != recipe["sculpto_sha256"]:
        raise ValueError("sculpto binary digest differs from recipe or executable is missing")
    source_path = Path(recipe["source"]).resolve()
    control_path = Path(recipe["control"]).resolve()
    for path, key in ((source_path, "source_sha256"), (control_path, "control_sha256")):
        if sha256(path) != recipe[key]:
            raise ValueError(f"{key} differs from recipe")
    if source_path.suffix.lower() != ".blend":
        raise ValueError("source must be an original extracted .blend")
    if recipe["control_frame"] not in FRAMES:
        raise ValueError(f"control_frame must be one of {FRAMES}")
    vertices, triangles = geometry(control_path)
    fitted = None
    if "fitted" in recipe:
        if recipe.get("fitted_frame") not in FRAMES:
            raise ValueError(f"fitted_frame is required with fitted geometry and must be one of {FRAMES}")
        fitted_path = Path(recipe["fitted"]).resolve()
        if sha256(fitted_path) != recipe["fitted_sha256"]:
            raise ValueError("fitted_sha256 differs from recipe")
        fitted, fitted_triangles = geometry(fitted_path)
        if fitted.shape != vertices.shape or not np.array_equal(fitted_triangles, triangles):
            raise ValueError("fitted geometry must retain exactly the same ordered topology")
    size = recipe["resolution"]
    margin = recipe["margin"]
    extrusion, distance, tolerance = (recipe[k] for k in ("cage_extrusion", "max_ray_distance", "frame_max_distance"))
    if type(size) is not int or not 32 <= size <= 8192:
        raise ValueError("resolution must be an integer in 32..8192")
    if type(margin) is not int or not 0 <= margin <= size // 16:
        raise ValueError("margin must be a nonnegative integer <= resolution/16")
    if "atlas_padding_px" not in recipe:
        raise ValueError("recipe requires atlas_padding_px")
    padding_px = recipe["atlas_padding_px"]
    if type(padding_px) is not int or not margin <= padding_px <= size // 16:
        raise ValueError("atlas_padding_px must be an integer >= margin and <= resolution/16")
    padding = dict(pixels=padding_px, resolution=size, fraction=padding_px / size)
    if not all(np.isfinite(value) and value > 0 for value in (extrusion, distance, tolerance)) or distance <= extrusion:
        raise ValueError("finite positive native ray settings require max_ray_distance > cage_extrusion")
    bpy.ops.wm.read_factory_settings(use_empty=True)
    with bpy.data.libraries.load(str(source_path)) as (available, loaded):
        if recipe["object"] not in available.objects:
            raise ValueError(f"source mesh not found: {recipe['object']}")
        loaded.objects = [recipe["object"]]
    source = loaded.objects[0]
    if source.type != "MESH":
        raise ValueError("source object must be a mesh")
    bpy.context.collection.objects.link(source)
    bpy.context.view_layer.update()
    originals, source_images = source_materials(source)
    settings = constant_settings(originals)
    # Copy materials, retaining original datablocks and external source bytes.
    temporary = [material.copy() for material in originals]
    for index, material in enumerate(temporary):
        source.data.materials[index] = material
    mesh = bpy.data.meshes.new("BakedControl")
    mesh.from_pydata(vertices.tolist(), [], triangles.tolist())
    mesh.update()
    target = bpy.data.objects.new("BakedControl", mesh)
    bpy.context.collection.objects.link(target)
    target.matrix_world = control_transform(recipe["control_frame"], source)
    for polygon in mesh.polygons:
        polygon.use_smooth = True
    bpy.context.view_layer.update()
    frame = frame_evidence(source, target, tolerance, extrusion, distance)
    select(target)
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=1.151917306, island_margin=padding["fraction"], margin_method="FRACTION", correct_aspect=True)
    bpy.ops.object.mode_set(mode="OBJECT")
    uv = np.asarray([tuple(loop.uv) for loop in mesh.uv_layers.active.data], dtype=np.float32)
    material = bpy.data.materials.new("BakedControlPBR")
    material.use_nodes = True
    mesh.materials.append(material)
    tree = material.node_tree
    shader = next(node for node in tree.nodes if node.type == "BSDF_PRINCIPLED")
    for name, value in settings.items():
        shader.inputs[name].default_value = value
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 1
    bake = scene.render.bake
    bake.cage_extrusion = extrusion
    bake.max_ray_distance = distance
    bake.use_cage = False
    bake.use_clear = True
    bake.margin = 0
    bake.normal_space = "TANGENT"
    output.mkdir(parents=True)
    report = dict(recipe=recipe, recipe_sha256=sha256(recipe_path), source_images=source_images, atlas_padding=padding,
                  sculpto=str(binary), sculpto_sha256=recipe["sculpto_sha256"], atlas={},
                  adapter_sha256=sha256(__file__), unbaked_principled_settings=settings,
                  atlas_adapter_sha256=sha256(Path(__file__).with_name("garment_uv_atlas.py")),
                  blender_version=bpy.app.version_string, frame=frame,
                  vertices=len(vertices), triangles=len(triangles), ordered_triangles_sha256=array_hash(triangles),
                  source_positions_sha256=array_hash(vertices), uv_sha256=array_hash(uv), uv_layer=mesh.uv_layers.active.name,
                  native_settings=dict(engine="CYCLES", device="CPU", samples=1, selected_to_active=True,
                      cage_extrusion=extrusion, max_ray_distance=distance, use_cage=False, margin=margin, coverage_margin=0,
                      normal_space="TANGENT", uv="smart_project", angle_limit=1.151917306, island_margin=padding["fraction"]), maps={})
    images = {}
    try:
        qualify_atlas(target, binary, output, report["atlas"], padding)
        uv = np.asarray([tuple(loop.uv) for loop in mesh.uv_layers.active.data], dtype=np.float32)
        report["uv_sha256"] = array_hash(uv)
        for channel in ("footprint", "coverage", "basecolor", "roughness", "metallic", "normal"):
            tick = time.monotonic()
            image = bpy.data.images.new("BakedControl_" + channel, width=size, height=size, alpha=True)
            image.colorspace_settings.name = "sRGB" if channel == "basecolor" else "Non-Color"
            node = tree.nodes.new("ShaderNodeTexImage")
            node.image = image
            tree.nodes.active = node
            images[channel] = (image, node)
            bake.use_selected_to_active = channel != "footprint"
            bake.margin = 0 if channel in ("footprint", "coverage") else margin
            select(target, source if channel != "footprint" else None)
            if channel == "footprint":
                emit_channel([material], "coverage")
            elif channel == "normal":
                restore_surface(temporary)
            else:
                emit_channel(temporary, "coverage" if channel == "coverage" else channel)
            bpy.ops.object.bake(type="NORMAL" if channel == "normal" else "EMIT")
            if channel == "footprint":
                restore_surface([material])
            report["maps"][channel] = save_image(image, output / (channel + ".png"))
            report["maps"][channel]["seconds"] = time.monotonic() - tick
            print("BAKED", channel, flush=True)
        footprint = pixels(images["footprint"][0])[:, 0] > 0.5
        covered = pixels(images["coverage"][0])[:, 0] > 0.5
        total = int(np.count_nonzero(footprint))
        if total == 0:
            raise ValueError("native UV footprint is empty")
        uncovered = int(np.count_nonzero(footprint & ~covered))
        report["coverage"] = dict(footprint_texels=total, uncovered_texels=uncovered,
                                  uncovered_fraction=uncovered / total, threshold=0.5,
                                  method="native white self-emission UV footprint versus native selected-to-active white source emission; zero margin")
        for channel, input_name in CHANNELS.items():
            tree.links.new(images[channel][1].outputs["Color"], shader.inputs[input_name])
        normal = tree.nodes.new("ShaderNodeNormalMap")
        tree.links.new(images["normal"][1].outputs["Color"], normal.inputs["Color"])
        tree.links.new(normal.outputs["Normal"], shader.inputs["Normal"])
        report["status"] = "baked with measured misses; appearance unqualified" if uncovered else "baked with complete measured UV-footprint coverage; visual and fit qualification separate"
        if fitted is not None:
            mesh.vertices.foreach_set("co", fitted.ravel())
            mesh.update()
            target.matrix_world = control_transform(recipe["fitted_frame"], source)
            bpy.context.view_layer.update()
        actual_vertices = np.asarray([tuple(vertex.co) for vertex in mesh.vertices], dtype=np.float32)
        actual_triangles = np.asarray([tuple(face.vertices) for face in mesh.polygons], dtype=np.int32)
        actual_uv = np.asarray([tuple(loop.uv) for loop in mesh.uv_layers.active.data], dtype=np.float32)
        if not np.array_equal(actual_vertices, fitted if fitted is not None else vertices) or not np.array_equal(actual_triangles, triangles) or not np.array_equal(actual_uv, uv):
            raise RuntimeError("native output changed ordered geometry or UV identity")
        report["final_positions_sha256"] = array_hash(actual_vertices)
        report["final_matrix_world"] = np.asarray(target.matrix_world).tolist()
        report["fitted_positions_only"] = fitted is not None
    except Exception as error:
        report.update(status="failed; retained diagnostics", error=str(error))
        raise
    finally:
        restore_surface(temporary)
        for index, original in enumerate(originals):
            source.data.materials[index] = original
        if sha256(source_path) != recipe["source_sha256"] or sha256(control_path) != recipe["control_sha256"]:
            raise RuntimeError("source/control file identity changed")
        report["seconds"] = time.monotonic() - started
        (output / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
    report["seconds_before_pack"] = report["seconds"]
    try:
        source.hide_render = True
        source.hide_set(True)
        select(target)
        bpy.ops.file.pack_all()
        bpy.ops.wm.save_as_mainfile(filepath=str(output / "baked-control.blend"))
        report["packed_blend_sha256"] = sha256(output / "baked-control.blend")
    except Exception as error:
        report.update(status="failed; retained diagnostics", error=str(error))
        raise
    finally:
        report["seconds"] = time.monotonic() - started
        (output / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
    print("MATERIAL_BAKE", json.dumps(report), flush=True)


if __name__ == "__main__":
    arguments = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(arguments) != 2:
        raise SystemExit("expected recipe.json new-output-directory")
    main(*arguments)
