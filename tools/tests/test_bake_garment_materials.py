"""Native source/frame, PBR, rejection and fitted UV identity regression."""

import json
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

import bpy
import numpy as np
from mathutils import Euler, Vector

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from bake_garment_materials import main, sha256
import bake_garment_materials as baker
import garment_uv_atlas as atlas

arguments = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(arguments) != 1:
    raise SystemExit("expected explicit sculpto executable argument")
binary = Path(arguments[0]).resolve()


def rejected(root, recipe, name, expected, existing=False):
    path = root / (name + ".json")
    path.write_text(json.dumps(recipe))
    output = root / name
    if existing:
        output.mkdir()
    try:
        main(path, output)
    except (ValueError, FileExistsError) as error:
        assert expected in str(error), str(error)
    else:
        raise AssertionError(f"accepted {name}")
    if not existing:
        assert not output.exists(), f"invalid {name} created output"


with tempfile.TemporaryDirectory(prefix="sculpto-material-native-") as temporary:
    root = Path(temporary)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    mesh = bpy.data.meshes.new("Source")
    positions = np.array([[-1, -1, 0], [1, -1, 0], [1, 1, 0], [-1, 1, 0]], dtype=np.float32)
    triangles = np.array([[0, 1, 2], [0, 2, 3]], dtype=np.int32)
    mesh.from_pydata(positions.tolist(), [], triangles.tolist())
    mesh.uv_layers.new().data.foreach_set("uv", [0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1])
    obj = bpy.data.objects.new("OriginalGarment", mesh)
    bpy.context.collection.objects.link(obj)
    obj.location = (3, 2, 1)
    obj.rotation_euler = Euler((0.1, 0.2, 0.3))
    material = bpy.data.materials.new("PBR")
    material.use_nodes = True
    mesh.materials.append(material)
    shader = material.node_tree.nodes.get("Principled BSDF")
    shader.inputs["Roughness"].default_value = 0.3
    shader.inputs["Metallic"].default_value = 0.7
    shader.inputs["Emission Strength"].default_value = 0
    shader.inputs["IOR"].default_value = 1.8
    image = bpy.data.images.new("Color", width=8, height=8)
    image.pixels[:] = [0.8, 0.1, 0.05, 1] * 64
    image.filepath_raw = str(root / "source-color.png")
    image.file_format = "PNG"
    image.save()
    image.source = "FILE"
    image.pack()
    node = material.node_tree.nodes.new("ShaderNodeTexImage")
    node.image = image
    material.node_tree.links.new(node.outputs["Color"], shader.inputs["Base Color"])
    normal_image = bpy.data.images.new("TangentNormal", width=8, height=8)
    normal_image.colorspace_settings.name = "Non-Color"
    normal_image.pixels[:] = [0.7, 0.5, 0.9582576, 1] * 64
    normal_image.filepath_raw = str(root / "source-normal.png")
    normal_image.file_format = "PNG"
    normal_image.save()
    normal_image.source = "FILE"
    normal_image.pack()
    normal_texture = material.node_tree.nodes.new("ShaderNodeTexImage")
    normal_texture.image = normal_image
    normal_node = material.node_tree.nodes.new("ShaderNodeNormalMap")
    material.node_tree.links.new(normal_texture.outputs["Color"], normal_node.inputs["Color"])
    material.node_tree.links.new(normal_node.outputs["Normal"], shader.inputs["Normal"])
    source = root / "source.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(source))
    np.savez(root / "control.npz", vertices=positions, triangles=triangles)
    fitted_local = positions.copy()
    fitted_local[:, 2] = [0, 0.1, 0.2, 0]
    bpy.context.view_layer.update()
    fitted_world = np.asarray([obj.matrix_world @ Vector(point) for point in fitted_local], dtype=np.float32)
    fitted = fitted_world[:, [0, 2, 1]].copy()
    fitted[:, 2] *= -1
    np.savez(root / "fitted.npz", vertices=fitted, triangles=triangles)
    recipe = dict(source=str(source), source_sha256=sha256(source), object=obj.name,
                  sculpto=str(binary), sculpto_sha256=sha256(binary),
                  control=str(root / "control.npz"), control_sha256=sha256(root / "control.npz"),
                  control_frame="blender-source-local-z-up", resolution=32, margin=2, atlas_padding_px=2,
                  frame_max_distance=0.001, cage_extrusion=0.01, max_ray_distance=0.02)
    path = root / "recipe.json"
    path.write_text(json.dumps(recipe))
    main(path, root / "baked")
    receipt = json.loads((root / "baked/receipt.json").read_text())
    assert receipt["coverage"]["uncovered_texels"] == 0
    assert receipt["atlas"]["operation"] == "retain"
    assert receipt["atlas"]["before_sha256"] == receipt["atlas"]["after_sha256"]
    assert receipt["atlas_padding"] == receipt["atlas"]["padding"] == dict(pixels=2, resolution=32, fraction=2 / 32)
    assert receipt["coverage"]["footprint_texels"] > 100
    assert receipt["frame"]["vertex_nearest_distance"]["max"] < 1e-5
    for channel, expected in (("roughness", 0.3), ("metallic", 0.7)):
        values = np.array(bpy.data.images["BakedControl_" + channel].pixels[:]).reshape(-1, 4)
        mask = np.array(bpy.data.images["BakedControl_footprint"].pixels[:]).reshape(-1, 4)[:, 0] > 0.5
        assert np.max(np.abs(values[mask, 0] - expected)) < 0.01, (channel, values[mask, 0])
    color = np.array(bpy.data.images["BakedControl_basecolor"].pixels[:]).reshape(-1, 4)[mask, :3]
    assert np.all(color[:, 0] > 0.7) and np.all(color[:, 1] < 0.15) and np.all(color[:, 2] < 0.1)
    normals = np.array(bpy.data.images["BakedControl_normal"].pixels[:]).reshape(-1, 4)[mask, :3]
    assert np.all(normals[:, 2] > 0.9) and np.all(np.linalg.norm(normals[:, :2] - 0.5, axis=1) > 0.15)
    fitted_recipe = dict(recipe, fitted=str(root / "fitted.npz"), fitted_sha256=sha256(root / "fitted.npz"), fitted_frame="sculpto-world-y-up")
    path.write_text(json.dumps(fitted_recipe))
    main(path, root / "fitted")
    second = json.loads((root / "fitted/receipt.json").read_text())
    assert second["uv_sha256"] == receipt["uv_sha256"]
    assert all(second["maps"][channel]["sha256"] == receipt["maps"][channel]["sha256"] for channel in ("basecolor", "roughness", "metallic", "normal"))
    bpy.ops.wm.open_mainfile(filepath=str(root / "fitted/baked-control.blend"))
    actual = bpy.data.objects["BakedControl"].data
    assert np.array_equal(np.array([v.co[:] for v in actual.vertices], dtype=np.float32), fitted)
    assert np.array_equal(np.array([f.vertices[:] for f in actual.polygons]), triangles)
    actual_world = np.asarray([bpy.data.objects["BakedControl"].matrix_world @ v.co for v in actual.vertices])
    assert np.max(np.abs(actual_world - fitted_world)) < 1e-6
    actual_shader = actual.materials[0].node_tree.nodes.get("Principled BSDF")
    assert abs(actual_shader.inputs["IOR"].default_value - 1.8) < 1e-6
    assert sha256(source) == recipe["source_sha256"]
    class PaddingObserved(Exception):
        pass
    for resolution, padding_px in ((32, 2), (64, 2), (64, 0)):
        margin = min(padding_px, 2)
        expected = dict(pixels=padding_px, resolution=resolution, fraction=padding_px / resolution)
        def capture_initial(target, executable, output, record, padding):
            assert padding == expected
            uv = atlas.coordinates(target.data)
            assert abs(float(uv.min()) - expected["fraction"]) < 1e-6
            raise PaddingObserved
        propagation = dict(recipe, resolution=resolution, margin=margin, atlas_padding_px=padding_px)
        path.write_text(json.dumps(propagation))
        output = root / f"padding-{resolution}-{padding_px}"
        with patch.object(baker, "qualify_atlas", capture_initial):
            try:
                main(path, output)
            except PaddingObserved:
                pass
            else:
                raise AssertionError("padding observation did not stop before baking")
        observed = json.loads((output / "receipt.json").read_text())
        assert observed["atlas_padding"] == expected
        assert observed["native_settings"]["island_margin"] == expected["fraction"]
        assert observed["maps"] == {}
    with patch.object(bpy.ops.wm, "read_factory_settings", side_effect=AssertionError("invalid padding mutated native state")):
        rejected(root, {key: value for key, value in recipe.items() if key != "atlas_padding_px"}, "missing-padding", "requires atlas_padding_px")
        for index, value in enumerate((True, False, None, "2", 2.0, 0.5, float("nan"), float("inf"), -1, 1, 3)):
            rejected(root, dict(recipe, atlas_padding_px=value), f"invalid-padding-{index}", "atlas_padding_px must be an integer")
    rejected(root, recipe, "existing", "refusing to overwrite", existing=True)
    rejected(root, {key: value for key, value in recipe.items() if key != "sculpto"}, "missing-cli", "requires sculpto")
    rejected(root, {key: value for key, value in recipe.items() if key != "sculpto_sha256"}, "missing-cli-hash", "requires sculpto")
    rejected(root, dict(recipe, sculpto_sha256="0" * 64), "wrong-cli-hash", "sculpto binary digest")
    rejected(root, dict(recipe, object="absent"), "missing-object", "source mesh not found")
    rejected(root, dict(recipe, control_frame="guess"), "invalid-frame", "control_frame")
    rejected(root, dict(recipe, control_frame="blender-world-z-up"), "wrong-frame", "declared frame exceeds")
    np.savez(root / "reordered.npz", vertices=fitted, triangles=triangles[::-1])
    rejected(root, dict(fitted_recipe, fitted_frame=None), "missing-fitted-frame", "fitted_frame is required")
    rejected(root, dict(fitted_recipe, fitted=str(root / "reordered.npz"), fitted_sha256=sha256(root / "reordered.npz")), "bad-topology", "ordered topology")
    bpy.ops.wm.open_mainfile(filepath=str(source))
    source_image = bpy.data.images["Color"]
    source_image.unpack(method="REMOVE")
    source_image.filepath = str(root / "absent.png")
    missing = root / "missing.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(missing))
    rejected(root, dict(recipe, source=str(missing), source_sha256=sha256(missing)), "missing-image", "missing source image")
    bpy.ops.wm.open_mainfile(filepath=str(source))
    material = bpy.data.objects["OriginalGarment"].data.materials[0]
    shader = material.node_tree.nodes.get("Principled BSDF")
    noise = material.node_tree.nodes.new("ShaderNodeTexNoise")
    material.node_tree.links.new(noise.outputs["Color"], shader.inputs["Base Color"])
    unsupported = root / "unsupported.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(unsupported))
    rejected(root, dict(recipe, source=str(unsupported), source_sha256=sha256(unsupported)), "unsupported-graph", "unsupported source channel graph")
    original_qualify = baker.qualify_atlas
    def failed_atlas(target, binary, output, record, padding):
        target.data.uv_layers.active.data.foreach_set("uv", [0, 0, .4, 0, 0, .4] * 2)
        original_qualify(target, binary, output, record, padding)
    path.write_text(json.dumps(recipe))
    with patch.object(baker, "qualify_atlas", failed_atlas), patch.object(atlas, "pack_once", return_value=None):
        try:
            main(path, root / "atlas-refused")
        except ValueError as error:
            assert "after one native repack" in str(error)
        else:
            raise AssertionError("unqualified atlas reached baking")
    failure = json.loads((root / "atlas-refused/receipt.json").read_text())
    assert failure["maps"] == {} and failure["status"] == "failed; retained diagnostics"
    assert not (root / "atlas-refused/footprint.png").exists()
    assert not (root / "atlas-refused/baked-control.blend").exists()
    assert sha256(source) == recipe["source_sha256"]
    print("PASS: required pixel padding before native mutation; propagation at two resolutions; native PBR/frame/fitted identity and refusal boundaries")
