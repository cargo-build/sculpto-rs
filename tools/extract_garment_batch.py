"""Partition a static Tripo bundle without changing its surfaces (@req DECL0013).

Run in Blender: --background --factory-startup --python-exit-code 1
--python tools/extract_garment_batch.py -- recipe.json output-directory

Recipe: {"source": "/absolute/model.fbx", "source_sha256": "...",
"mesh": "node", "parts": [{"id": "name", "bounds": [xmin,xmax,zmin,zmax]}]}.
Bounds use Blender world X/Z after native FBX/glTF import. Every source vertex
must belong to exactly one rectangle, and every triangle must stay in one part.
Output is one packed parts.blend plus an identity receipt; no fit is implied.
"""

import hashlib
import json
from pathlib import Path
import sys
import time

import bpy
import numpy as np


def values(collection, field, width=1, dtype=np.float32):
    result = np.empty(len(collection) * width, dtype=dtype)
    collection.foreach_get(field, result)
    return result.reshape(-1, width) if width != 1 else result


def partition(obj, parts):
    """Reject incomplete or cutting selections before creating any output objects."""
    mesh = obj.data
    if obj.modifiers or mesh.shape_keys or obj.vertex_groups:
        raise ValueError("batch extraction accepts static unbound source meshes")
    if not len(mesh.polygons) or np.any(values(mesh.polygons, "loop_total", dtype=np.int32) != 3):
        raise ValueError("batch extraction requires nonempty triangle geometry")
    if len({part["id"] for part in parts}) != len(parts):
        raise ValueError("part IDs must be unique")
    positions = values(mesh.vertices, "co", 3)
    transform = np.asarray(obj.matrix_world, dtype=np.float64)
    world = positions @ transform[:3, :3].T + transform[:3, 3]
    owners = np.full(len(positions), -1, dtype=np.int32)
    for index, part in enumerate(parts):
        if not part["id"] or "/" in part["id"] or "\\" in part["id"]:
            raise ValueError("part ID must be a nonempty local object name")
        bounds = part["bounds"]
        if len(bounds) != 4 or not np.all(np.isfinite(bounds)):
            raise ValueError("bounds must contain four finite numbers")
        x0, x1, z0, z1 = bounds
        if not (x0 < x1 and z0 < z1):
            raise ValueError("bounds must have positive area")
        selected = (world[:, 0] >= x0) & (world[:, 0] <= x1) & (world[:, 2] >= z0) & (world[:, 2] <= z1)
        if not selected.any() or np.any(owners[selected] != -1):
            raise ValueError(f"empty or overlapping selection: {part['id']}")
        owners[selected] = index
    if np.any(owners < 0):
        raise ValueError(f"uncovered source vertices: {np.count_nonzero(owners < 0)}")
    corners = values(mesh.loops, "vertex_index", dtype=np.int32).reshape(-1, 3)
    face_owners = owners[corners]
    crossing = np.any(face_owners != face_owners[:, :1], axis=1)
    if crossing.any():
        raise ValueError(f"selection cuts source faces: {np.count_nonzero(crossing)}")
    return positions, corners, owners, face_owners[:, 0]


def split(obj, parts):
    """Copy indexed surface data and retain source IDs for independent read-back."""
    positions, corners, owners, face_owners = partition(obj, parts)
    original = obj.data
    uvs = {layer.name: values(layer.data, "uv", 2) for layer in original.uv_layers}
    normals = values(original.corner_normals, "vector", 3) if original.has_custom_normals else None
    materials = values(original.polygons, "material_index", dtype=np.int32)
    smooth = values(original.polygons, "use_smooth", dtype=np.bool_)
    result = []
    for index, part in enumerate(parts):
        vertex_ids = np.flatnonzero(owners == index).astype(np.int32)
        face_ids = np.flatnonzero(face_owners == index).astype(np.int32)
        if not len(face_ids):
            raise ValueError(f"part contains no faces: {part['id']}")
        corner_ids = (3 * face_ids[:, None] + np.arange(3)).ravel()
        remap = np.full(len(positions), -1, dtype=np.int32)
        remap[vertex_ids] = np.arange(len(vertex_ids))
        mesh = bpy.data.meshes.new(part["id"])
        mesh.vertices.add(len(vertex_ids))
        mesh.vertices.foreach_set("co", positions[vertex_ids].ravel())
        mesh.loops.add(len(corner_ids))
        mesh.loops.foreach_set("vertex_index", remap[corners[face_ids]].ravel())
        mesh.polygons.add(len(face_ids))
        mesh.polygons.foreach_set("loop_start", np.arange(len(face_ids), dtype=np.int32) * 3)
        mesh.polygons.foreach_set("loop_total", np.full(len(face_ids), 3, dtype=np.int32))
        for material in original.materials:
            mesh.materials.append(material)
        mesh.polygons.foreach_set("material_index", materials[face_ids])
        mesh.polygons.foreach_set("use_smooth", smooth[face_ids])
        mesh.update(calc_edges=True)
        for name, uv in uvs.items():
            mesh.uv_layers.new(name=name).data.foreach_set("uv", uv[corner_ids].ravel())
        if original.uv_layers.active:
            mesh.uv_layers.active_index = original.uv_layers.active_index
        for layer in original.uv_layers:
            mesh.uv_layers[layer.name].active_render = layer.active_render
        if normals is not None:
            mesh.normals_split_custom_set(normals[corner_ids].tolist())
            # Blender re-encodes custom normals; retain the exact source vectors.
            mesh.attributes.new("source_corner_normal", "FLOAT_VECTOR", "CORNER").data.foreach_set(
                "vector", normals[corner_ids].ravel())
        for name, domain, ids in [("source_vertex", "POINT", vertex_ids), ("source_face", "FACE", face_ids)]:
            mesh.attributes.new(name, "INT", domain).data.foreach_set("value", ids)
        child = bpy.data.objects.new(part["id"], mesh)
        bpy.context.collection.objects.link(child)
        child.matrix_world = obj.matrix_world.copy()
        result.append(child)
    return result


def main(recipe_path, output):
    started = time.monotonic()
    recipe = json.loads(recipe_path.read_text())
    source = Path(recipe["source"])
    with source.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if digest != recipe["source_sha256"]:
        raise ValueError("source SHA256 differs from the recipe")
    if output.exists():
        raise FileExistsError(f"refusing to overwrite {output}")
    bpy.ops.wm.read_factory_settings(use_empty=True)
    if source.suffix.lower() == ".fbx":
        bpy.ops.wm.fbx_import(filepath=str(source))
    elif source.suffix.lower() in (".gltf", ".glb"):
        bpy.ops.import_scene.gltf(filepath=str(source))
    else:
        raise ValueError("source must be FBX or glTF/GLB")
    obj = bpy.data.objects.get(recipe["mesh"])
    if obj is None or obj.type != "MESH":
        raise ValueError(f"source mesh not found: {recipe['mesh']}")
    meshes = [item for item in bpy.context.scene.objects if item.type == "MESH"]
    if meshes != [obj]:
        raise ValueError("recipe must identify a single-mesh bundle")
    source_faces = len(obj.data.polygons)
    children = split(obj, recipe["parts"])
    assert sum(len(item.data.polygons) for item in children) == source_faces
    bpy.data.objects.remove(obj, do_unlink=True)
    output.mkdir(parents=True)
    bpy.ops.file.pack_all()
    bpy.ops.wm.save_as_mainfile(filepath=str(output / "parts.blend"))
    report = dict(recipe=recipe, recipe_sha256=hashlib.sha256(recipe_path.read_bytes()).hexdigest(),
                  source_faces=source_faces, seconds=time.monotonic() - started,
                  status="partitioned with source correspondence; preparation and fit unqualified",
                  parts=[dict(id=item.name, vertices=len(item.data.vertices), faces=len(item.data.polygons)) for item in children])
    (output / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
    print("BATCH_EXTRACTED", json.dumps(report), flush=True)


if __name__ == "__main__":
    arguments = sys.argv[sys.argv.index("--") + 1:]
    if len(arguments) != 2:
        raise SystemExit("expected recipe.json output-directory")
    main(Path(arguments[0]), Path(arguments[1]))
