"""Native source-guided control reconstruction: positive, rejection and invariance runs.

Blender: --background --factory-startup -t 2 --python-exit-code 1
--python tools/tests/test_construct_garment_control.py

Requires the CLI build (`cargo build -p sculpto-cli`) at target/debug/sculpto.
"""

import json
import math
from pathlib import Path
import sys
import tempfile

import bmesh
import bpy
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import construct_garment_control as adapter  # noqa: E402
from construct_garment_control import main, sha256  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
SCULPTO = ROOT / "target" / "debug" / "sculpto"


def shell_geometry(stations, sides, outer, inner, offset, scale):
    """A closed double-walled tube: outward wall, inner lining and two rims."""
    def ring(station, radius):
        return [
            (
                offset[0] + scale * radius * np.cos(2 * np.pi * side / sides),
                offset[1] + scale * radius * np.sin(2 * np.pi * side / sides),
                offset[2] + scale * station,
            )
            for side in range(sides)
        ]

    vertices = []
    outer_index = []
    inner_index = []
    for station in stations:
        outer_index.append(list(range(len(vertices), len(vertices) + sides)))
        vertices.extend(ring(station, outer))
        inner_index.append(list(range(len(vertices), len(vertices) + sides)))
        vertices.extend(ring(station, inner))
    triangles = []
    for station in range(len(stations) - 1):
        for side in range(sides):
            following = (side + 1) % sides
            triangles.extend(
                [
                    [
                        outer_index[station][side],
                        outer_index[station][following],
                        outer_index[station + 1][following],
                    ],
                    [
                        outer_index[station][side],
                        outer_index[station + 1][following],
                        outer_index[station + 1][side],
                    ],
                    [
                        inner_index[station][side],
                        inner_index[station + 1][following],
                        inner_index[station][following],
                    ],
                    [
                        inner_index[station][side],
                        inner_index[station + 1][side],
                        inner_index[station + 1][following],
                    ],
                ]
            )
    last = len(stations) - 1
    for side in range(sides):
        following = (side + 1) % sides
        triangles.extend(
            [
                [
                    outer_index[0][side],
                    inner_index[0][side],
                    inner_index[0][following],
                ],
                [
                    outer_index[0][side],
                    inner_index[0][following],
                    outer_index[0][following],
                ],
                [
                    outer_index[last][side],
                    inner_index[last][following],
                    inner_index[last][side],
                ],
                [
                    outer_index[last][side],
                    outer_index[last][following],
                    inner_index[last][following],
                ],
            ]
        )
    return vertices, triangles


def build_shell(name, offset=(0.0, 0.0, 0.0), scale=1.0, stations=None, sides=10, drop=None):
    stations = stations or [-0.5, -0.25, 0.0, 0.25, 0.5]
    vertices, triangles = shell_geometry(stations, sides, 0.2, 0.1, offset, scale)
    if drop is not None:
        triangles.pop(drop)
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], triangles)
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    return obj, stations, offset, scale


def profile(offset, scale, stations, *, anchor_shift=0.0, sides=10):
    low = offset[2] + scale * stations[0]
    high = offset[2] + scale * stations[-1]
    below = low - 0.3 * scale
    above = high + 0.3 * scale
    return {
        "frame": "source-local-z-up",
        "guide_segments": [
            [
                [offset[0], offset[1], below],
                [offset[0], offset[1], low],
            ],
            [
                [offset[0], offset[1], low],
                [offset[0], offset[1], high],
            ],
            [
                [offset[0], offset[1], high],
                [offset[0], offset[1], above],
            ],
        ],
        "apertures": [
            {
                "role": "low opening",
                "anchor": [offset[0], offset[1], low + anchor_shift],
                "radius": 0.05 * scale,
            },
            {
                "role": "high opening",
                "anchor": [offset[0], offset[1], high],
                "radius": 0.05 * scale,
            },
        ],
        "source_repair": {
            "max_incidental_loops": 4,
            "collapse_below_edges": 0,
            "max_fill_edges": 64,
            "max_tip_removal_share": 0.05,
        },
        "reduced_repair": {
            "max_incidental_loops": 4,
            "collapse_below_edges": 10,
            "max_fill_edges": 64,
            "max_tip_removal_share": 0.05,
        },
        "reduction": {
            "target_triangles": int(60 * sides / 10),
            "minimum_triangles": int(30 * sides / 10),
            "maximum_triangles": int(70 * sides / 10),
        },
        "coverage": {"tolerance": 0.02 * scale, "minimum": 0.5},
        "contact": {
            "mode": "separate",
            "clearance": 0.000002 * scale,
            "max_displacement": 0.0001 * scale,
            "max_pairs": 1,
        },
    }


def write_recipe(root, name, offset, scale, *, anchor_shift=0.0, sides=10, drop=None):
    blend = root / f"{name}.blend"
    bpy.ops.wm.read_factory_settings(use_empty=True)
    obj, stations, offset, scale = build_shell(name, offset, scale, sides=sides, drop=drop)
    bpy.ops.wm.save_as_mainfile(filepath=str(blend))
    profile_path = root / f"{name}.json"
    profile_path.write_text(
        json.dumps(profile(offset, scale, stations, anchor_shift=anchor_shift, sides=sides))
    )
    recipe = {
        "source": str(blend),
        "source_sha256": sha256(blend),
        "profile": str(profile_path),
        "profile_sha256": sha256(profile_path),
        "object": name,
        "sculpto": str(SCULPTO),
    }
    recipe_path = root / f"{name}-recipe.json"
    recipe_path.write_text(json.dumps(recipe, indent=2))
    return recipe_path, recipe


def expect_failure(root, recipe, name, expected, existing=False):
    recipe_path = root / f"{name}-recipe.json"
    recipe_path.write_text(json.dumps(recipe))
    output = root / name
    if existing:
        output.mkdir()
    try:
        main(recipe_path, output)
    except (ValueError, RuntimeError, FileExistsError) as error:
        assert expected in str(error), f"{name}: {error}"
    else:
        raise AssertionError(f"{name} was accepted")
    return output


with tempfile.TemporaryDirectory(prefix="sculpto-control-native-") as temporary:
    root = Path(temporary)

    # 1. Positive run on an explicit thick shell.
    recipe_path, recipe = write_recipe(root, "SyntheticTunic", (0.0, 0.0, 0.0), 1.0)
    output = root / "case-positive"
    main(recipe_path, output)
    receipt = json.loads((output / "receipt.json").read_text())
    assert receipt["status"] == "constructed", receipt.get("error")
    assert receipt["exterior"]["exterior_triangles"] == 4 * 10 * 2
    assert receipt["exterior"]["interior_removed"] == 4 * 10 * 2 + 40
    assert [match["role"] for match in receipt["apertures"]] == [
        "low opening",
        "high opening",
    ]
    validation = receipt["validation"]
    assert validation["qualification"]["passed"], validation["qualification"]
    assert validation["measured"]["self_contacts"] == 0
    assert len(validation["measured"]["loops"]) == 2
    for name in (
        "high_control.npz",
        "high_control.obj",
        "reduced_control.npz",
        "reduced_control.obj",
        "receipt.json",
        "validation.json",
    ):
        assert (output / name).is_file(), name
    reduced = np.load(output / "reduced_control.npz")
    assert reduced["vertices"].shape[1] == 3
    assert int(reduced["triangles"].max()) < len(reduced["vertices"])
    assert receipt["decimation"]["ratio"] < 1.0
    assert len(receipt["renders"]) == 4
    selected = receipt["stages"]["selected"]
    assert receipt["lineage"]["vertex_sources"] == selected["vertices"]
    assert receipt["lineage"]["selected_faces"] == selected["triangles"]
    assert set(receipt["stages"]) == {
        "selected",
        "high_control",
        "decimated",
        "repaired",
        "pre_separation",
        "reduced_control",
    }
    for stage in receipt["stages"].values():
        assert len(stage["sha256"]) == 64 and stage["triangles"] > 0
    assert receipt["separation"]["applied"] is False, "the synthetic shell is clean"
    assert receipt["validation"]["verdict"]["passed"] is True

    # 2. The same shell under another name, offset and scale decides the same way.
    moved_recipe, _ = write_recipe(root, "UnnamedCell", (3.0, -1.5, 0.75), 2.0)
    moved = root / "case-moved"
    main(moved_recipe, moved)
    moved_receipt = json.loads((moved / "receipt.json").read_text())
    assert moved_receipt["status"] == "constructed", moved_receipt.get("error")
    assert moved_receipt["exterior"]["exterior_triangles"] == receipt["exterior"]["exterior_triangles"]
    assert moved_receipt["exterior"]["interior_removed"] == receipt["exterior"]["interior_removed"]
    assert [match["role"] for match in moved_receipt["apertures"]] == [
        "low opening",
        "high opening",
    ]
    assert moved_receipt["source_geometry"]["triangles"] == receipt["source_geometry"]["triangles"]

    # 3. Two runs of one recipe produce byte-identical control arrays.
    duplicate = root / "case-duplicate"
    main(recipe_path, duplicate)
    assert (duplicate / "reduced_control.npz").read_bytes() == (
        output / "reduced_control.npz"
    ).read_bytes()

    # 4. Source-stage closure limits are enforced before any native fill.
    holed_recipe, holed = write_recipe(root, "HoledTunic", (0.0, 0.0, 0.0), 1.0, drop=40)
    strict = json.loads(Path(holed["profile"]).read_text())
    strict["source_repair"]["max_incidental_loops"] = 0
    strict_path = root / "HoledStrict.json"
    strict_path.write_text(json.dumps(strict))
    strict_recipe = dict(holed)
    strict_recipe["profile"] = str(strict_path)
    strict_recipe["profile_sha256"] = sha256(strict_path)
    (root / "HoledStrict-recipe.json").write_text(json.dumps(strict_recipe))
    try:
        main(root / "HoledStrict-recipe.json", root / "case-holed-strict")
    except RuntimeError as error:
        assert "non-aperture boundary loops" in str(error), str(error)
    else:
        raise AssertionError("source-stage closure limits were not enforced")

    holed_output = root / "case-holed"
    main(holed_recipe, holed_output)
    holed_receipt = json.loads((holed_output / "receipt.json").read_text())
    assert holed_receipt["status"] == "constructed", holed_receipt.get("error")
    assert holed_receipt["exterior"]["tip_triangles_removed"] >= 0
    assert len(holed_receipt["incidental"]) == 1
    assert len(holed_receipt["high_control"]["fills"]) == 1
    filled = holed_receipt["high_control"]["fills"][0]
    assert filled["operator"] in {"triangle_fill", "holes_fill+triangulate"}
    assert filled["closure"] in {"triangulated", "boundary"}
    assert filled["faces"] >= 1
    assert filled["faces"] >= 1
    assert holed_receipt["validation"]["verdict"]["passed"] is True

    # 5. An existing output directory is rejected before anything is written.
    expect_failure(
        root,
        recipe,
        "case-existing",
        "already exists",
        existing=True,
    )
    assert sorted(path.name for path in (root / "case-existing").iterdir()) == []

    # 5. A digest mismatch is refused.
    tampered = dict(recipe, profile_sha256="0" * 64)
    expect_failure(root, tampered, "case-digest", "digest does not match")

    # 6. A wrong aperture anchor keeps the diagnostics and never claims a control.
    shifted_recipe, _ = write_recipe(
        root, "ShiftedTunic", (0.0, 0.0, 0.0), 1.0, anchor_shift=0.2
    )
    failed = expect_failure(
        root,
        json.loads(shifted_recipe.read_text()),
        "case-shifted",
        "no boundary loop lies within",
    )
    failed_receipt = json.loads((failed / "receipt.json").read_text())
    assert failed_receipt["status"].startswith("failed")
    assert "no boundary loop lies within" in failed_receipt["error"]
    assert not (failed / "reduced_control.npz").exists()

print("NATIVE_CONTROL_REGRESSION ok", flush=True)


# --- native patch ownership -------------------------------------------------

def hex_hole_mesh():
    """A flat surface with a hexagonal hole: 6 boundary edges, one face each."""
    bm = bmesh.new()
    inner = [
        bm.verts.new((0.1 * math.cos(math.tau * i / 6), 0.1 * math.sin(math.tau * i / 6), 0.0))
        for i in range(6)
    ]
    outer = [
        bm.verts.new((0.2 * math.cos(math.tau * i / 6), 0.2 * math.sin(math.tau * i / 6), -0.05))
        for i in range(6)
    ]
    for i in range(6):
        following = (i + 1) % 6
        bm.faces.new([inner[i], inner[following], outer[following], outer[i]])
    bm.verts.ensure_lookup_table()
    bm.edges.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    bm.verts.index_update()
    return bm, list(range(6))


def mesh_state(bm):
    bm.verts.ensure_lookup_table()
    bm.edges.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    return {
        "vertices": len(bm.verts),
        "edges": len(bm.edges),
        "faces": len(bm.faces),
        "idents": (set(bm.verts), set(bm.edges), set(bm.faces)),
    }


def boundary_loop_edges(bm, vertices):
    bm.verts.ensure_lookup_table()
    edges = []
    for first, second in zip(vertices, vertices[1:] + vertices[:1]):
        start, end = bm.verts[first], bm.verts[second]
        edge = next(
            (candidate for candidate in start.link_edges if candidate.other_vert(start) is end),
            None,
        )
        assert edge is not None, f"no loop edge {first}-{second}"
        edges.append(edge)
    return edges


def patch_tests():
    # 1. Six-vertex ring ownership: the triangulated patch is fully captured.
    bm, ring = hex_hole_mesh()
    loop_edges = boundary_loop_edges(bm, ring)
    assert all(len(edge.link_faces) == 1 for edge in loop_edges)
    report = []
    adapter.fill_loop(bm, ring, report, "boundary")
    record = report[0]
    assert record["closure"] == "boundary" and record["fallback"] is False
    assert record["faces"] == 4, f"expected 4 triangles, got {record['faces']}"
    assert all(len(edge.link_faces) == 2 for edge in loop_edges)
    assert all(face.is_valid for face in bm.faces)
    # Winding: every interior edge is traversed oppositely by its two faces.
    bm.edges.ensure_lookup_table()
    for edge in bm.edges:
        if len(edge.link_faces) == 2:
            first, second = (adapter.edge_direction(face, edge) for face in edge.link_faces)
            assert not (first[0] is second[0] and first[1] is second[1]), "winding conflicts"
    bm.free()

    # Orientation repair happens before verification, including every new triangle.
    bm, ring = hex_hole_mesh()
    original_apply = adapter.apply_patch

    def reversed_patch(bm_mesh, kind, edges):
        before_faces = set(bm_mesh.faces)
        original_apply(bm_mesh, kind, edges)
        for face in bm_mesh.faces:
            if face not in before_faces:
                face.normal_flip()

    adapter.apply_patch = reversed_patch
    try:
        oriented = []
        adapter.fill_loop(bm, ring, oriented, "boundary")
        assert oriented[0]["flipped"] == 4
        assert not oriented[0]["fallback"]
    finally:
        adapter.apply_patch = original_apply
        bm.free()

    # 2. A rejected patch is rolled back to the exact pre-existing geometry.
    bm, ring = hex_hole_mesh()
    loop_edges = boundary_loop_edges(bm, ring)
    before = mesh_state(bm)
    original_verify = adapter.verify_patch

    def rejecting(*_args, **_kwargs):
        raise RuntimeError("injected verification rejection")

    adapter.verify_patch = rejecting
    try:
        try:
            adapter.fill_loop(bm, ring, [], "boundary")
        except RuntimeError as error:
            assert "verified attempts" in str(error), str(error)
        else:
            raise AssertionError("a rejected patch was accepted")
    finally:
        adapter.verify_patch = original_verify
    after = mesh_state(bm)
    assert after["faces"] == before["faces"], "rollback left generated faces"
    assert after["edges"] == before["edges"], "rollback left generated edges"
    assert after["vertices"] == before["vertices"], "rollback left generated vertices"
    assert after["idents"][2] == before["idents"][2], "rollback replaced pre-existing faces"
    assert all(edge.is_valid and len(edge.link_faces) == 1 for edge in loop_edges)
    bm.free()

    # 3. An exception raised after partial creation still owns and rolls back it.
    bm, ring = hex_hole_mesh()
    loop_edges = boundary_loop_edges(bm, ring)
    before = mesh_state(bm)
    original_apply = adapter.apply_patch

    def failing(bm_mesh, kind, edges):
        original_apply(bm_mesh, kind, edges)
        raise RuntimeError("injected after partial creation")

    adapter.apply_patch = failing
    try:
        try:
            adapter.fill_loop(bm, ring, [], "triangulated")
        except RuntimeError as error:
            assert "verified attempts" in str(error), str(error)
        else:
            raise AssertionError("a partial patch was accepted")
    finally:
        adapter.apply_patch = original_apply
    after = mesh_state(bm)
    assert (after["faces"], after["edges"], after["vertices"]) == (
        before["faces"],
        before["edges"],
        before["vertices"],
    ), "partial mutation survived rollback"
    assert after["idents"][2] == before["idents"][2]
    assert all(edge.is_valid and len(edge.link_faces) == 1 for edge in loop_edges)
    bm.free()

    # 4. An unknown closure label is an explicit error, not a silent default.
    bm, ring = hex_hole_mesh()
    try:
        adapter.fill_loop(bm, ring, [], "nonsense")
    except RuntimeError as error:
        assert "closure `nonsense`" in str(error), str(error)
    else:
        raise AssertionError("an unknown closure label was accepted")
    bm.free()

patch_tests()
print("PATCH_OWNERSHIP ok", flush=True)
