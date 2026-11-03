"""Run with Blender --background --factory-startup --python-exit-code 1 --python PATH."""

from pathlib import Path
import hashlib
import json
import runpy
import sys
import tempfile

import bpy

bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)
rig_data = bpy.data.armatures.new("Rig")
rig = bpy.data.objects.new("Rig", rig_data)
bpy.context.collection.objects.link(rig)
bpy.context.view_layer.objects.active = rig
rig.select_set(True)
bpy.ops.object.mode_set(mode="EDIT")
bone = rig_data.edit_bones.new("Bone")
bone.head = (0, 0, 0)
bone.tail = (0, 0, 1)
bpy.ops.object.mode_set(mode="OBJECT")

positions = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 0.0, 1.0)]
mesh = bpy.data.meshes.new("Body")
mesh.from_pydata(positions, [], [(0, 1, 2)])
body = bpy.data.objects.new("BaseShape", mesh)
bpy.context.collection.objects.link(body)
body.vertex_groups.new(name="Bone").add([0, 1, 2], 1.0, "REPLACE")
modifier = body.modifiers.new("Rig", "ARMATURE")
modifier.object = rig
rig.pose.bones["Bone"].location = (0.5, 0.0, 0.0)
bpy.context.view_layer.update()
posed = body.evaluated_get(bpy.context.evaluated_depsgraph_get())
assert abs(posed.data.vertices[0].co.x - 0.5) < 1e-6, "fixture must actually be posed"

with tempfile.TemporaryDirectory(prefix="sculpto-rest-test-") as directory:
    output = Path(directory) / "body.obj"
    sys.argv = ["blender", "--", "mesh=BaseShape", f"out={output}"]
    runpy.run_path(str(Path(__file__).resolve().parents[1] / "export_body_obj.py"))
    actual = [tuple(map(float, line.split()[1:4])) for line in output.read_text().splitlines()
              if line.startswith("v ")]
    expected = [(x, z, -y) for x, y, z in positions]
    assert actual == expected, (actual, expected)
    assert tuple(rig.pose.bones["Bone"].location) == (0.5, 0.0, 0.0), "do not edit authored pose"

    extra_mesh = bpy.data.meshes.new("Hand")
    extra_mesh.from_pydata(positions, [], [(0, 1, 2)])
    hand = bpy.data.objects.new("Hand", extra_mesh)
    bpy.context.collection.objects.link(hand)
    hand.location = (2.0, 3.0, 4.0)
    report = Path(directory) / "body.json"
    sys.argv = ["blender", "--", "meshes=BaseShape,Hand", f"out={output}", f"report={report}"]
    runpy.run_path(str(Path(__file__).resolve().parents[1] / "export_body_obj.py"))
    lines = output.read_text().splitlines()
    points = [tuple(map(float, line.split()[1:])) for line in lines if line.startswith("v ")]
    assert points == expected + [(x + 2, z + 4, -(y + 3)) for x, y, z in positions]
    assert [line for line in lines if line.startswith("f ")] == ["f 1 2 3", "f 4 5 6"]
    receipt = json.loads(report.read_text())
    assert [(p["name"], p["vertex_start"], p["triangle_start"]) for p in receipt["parts"]] == [
        ("BaseShape", 0, 0), ("Hand", 3, 1)]
    assert receipt["obj_sha256"] == hashlib.sha256(output.read_bytes()).hexdigest()
    for selection in ["BaseShape,Missing", "BaseShape,BaseShape", "BaseShape,"]:
        rejected = Path(directory) / "rejected.obj"
        sys.argv = ["blender", "--", f"meshes={selection}", f"out={rejected}"]
        try:
            runpy.run_path(str(Path(__file__).resolve().parents[1] / "export_body_obj.py"))
        except SystemExit:
            pass
        else:
            raise AssertionError(f"invalid selection accepted: {selection}")
        assert not rejected.exists(), "invalid selection must fail before creating geometry"
print("PASS: rest transforms, named multi-part offsets and rejection before output")
