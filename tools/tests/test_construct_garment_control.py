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
        "composed",
        "high_control",
        "decimated",
        "repaired",
        "pre_separation",
        "reduced_control",
    }
    for name, stage in receipt["stages"].items():
        if name == "composed":
            assert stage["note"] and stage["verified"] is True
            continue
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


# --- explicit source-opening cuts -------------------------------------------

def capped_prism(outline, height):
    """Extruded polygon: one closed solid with a top and a bottom cap."""
    count = len(outline)
    positions = [(x, y, 0.0) for x, y in outline] + [(x, y, height) for x, y in outline]
    triangles = []
    for index in range(count):
        following = (index + 1) % count
        low, low_next = index, following
        high, high_next = count + index, count + following
        triangles.append([low, low_next, high_next])
        triangles.append([low, high_next, high])
    for index in range(1, count - 1):
        triangles.append([count, count + index, count + index + 1])
        triangles.append([0, index + 1, index])
    return positions, triangles


def count_boundary_edges(triangles):
    """Edges with exactly one incident triangle: the declared openings."""
    counts = {}
    for face in triangles:
        for first, second in (
            (face[0], face[1]),
            (face[1], face[2]),
            (face[2], face[0]),
        ):
            key = (min(first, second), max(first, second))
            counts[key] = counts.get(key, 0) + 1
    return sum(1 for count in counts.values() if count == 1)


def cut_output():
    """A fresh directory for one derivative and its lineage file."""
    return Path(tempfile.mkdtemp(prefix="sculpto-cut-"))


def reconstruct_record(positions, triangles, record):
    """Rebuild one recorded vertex from its source triangle and weights."""
    corners = triangles[record["source_triangle"]]
    return [
        sum(
            record["weights"][corner] * positions[corners[corner]][axis]
            for corner in range(3)
        )
        for axis in range(3)
    ]


def cut_tests():
    outline = [(math.cos(math.tau * i / 6) * 0.1, math.sin(math.tau * i / 6) * 0.1) for i in range(6)]
    positions, triangles = capped_prism(outline, 0.2)
    source_positions = [tuple(point) for point in positions]
    source_triangles = [list(face) for face in triangles]
    assert count_boundary_edges(triangles) == 0, "the synthetic solid starts closed"

    def cut(label, z, normal, retain):
        return {"label": label, "plane_point": [0.0, 0.0, z], "normal": normal, "retain": retain}

    # Two declared cuts open the waist and the hem without filling them.
    cuts = {
        "cuts": [
            cut("waist", 0.19, [0.0, 0.0, 1.0], "negative"),
            cut("hem", 0.01, [0.0, 0.0, -1.0], "negative"),
        ]
    }
    output = cut_output()
    cut_positions, cut_triangles, report = adapter.apply_source_cuts(
        positions, triangles, cuts, output
    )
    lineage = report["lineage"]
    print(
        "CUTS",
        json.dumps(
            {
                "cuts": [
                    {"label": row["label"], "deleted": row["operator_faces_deleted"],
                     "polygons": row["polygons_triangulated"], "cut_vertices": row["cut_vertices"]}
                    for row in report["cuts"]
                ],
                "derivative": report["derivative"],
                "removed_face_count": lineage["removed_face_count"],
                "counts": lineage["counts"],
                "areas": {
                    "removed": lineage["complete_face_removal_area"],
                    "clipped": lineage["clipped_area"],
                    "total": lineage["total_area_removed"],
                    "bound": lineage["area_error_bound"],
                },
                "tolerance": lineage["tolerance"],
                "max_error": lineage["max_reconstruction_error"],
            }
        ),
        flush=True,
    )
    # The declared cuts create real openings: the solid was closed, the derivative is not.
    assert count_boundary_edges(cut_triangles) > 0, "no opening was created"
    assert len(cut_triangles) > 0 and len(cut_positions) > len(positions) - 12
    # Both caps are gone; nothing was filled back in. The ledger is 0-based like
    # `face_source_triangles`: the prism's 12 side triangles come first, then the
    # 4 top-cap and 4 bottom-cap triangles, so the removed ids are exactly 12..19.
    assert lineage["removed_face_count"] == 8, lineage["removed_face_count"]
    assert lineage["removed_original_faces"] == list(range(12, 20)), lineage[
        "removed_original_faces"
    ]
    assert lineage["verified"] and lineage["max_reconstruction_error"] <= lineage["tolerance"]
    assert lineage["counts"]["clipped_vertices"] > 0, lineage["counts"]
    assert lineage["fill"] == "none; declared openings stay open"
    assert all(row["fill"] == "none" for row in report["cuts"])
    assert report["original"]["triangles"] == len(triangles)
    # Complete-face removal and the actual area difference are separate numbers.
    assert lineage["complete_face_removal_area"] > 0.0
    assert abs(
        lineage["complete_face_removal_area"] + lineage["clipped_area"] - lineage["total_area_removed"]
    ) <= lineage["area_error_bound"] + 1e-18
    assert lineage["total_area_removed"] > 0.0
    assert abs(
        lineage["total_area_removed"]
        - (report["original"]["area"] - report["derivative"]["area"])
    ) <= lineage["area_error_bound"]
    # The source arrays are neither mutated nor re-labelled.
    assert positions == source_positions, "the displayed source was mutated"
    assert triangles == source_triangles, "the displayed source topology was mutated"
    # The complete ordered relation is a file, and it reads back exactly.
    written = json.loads((output / lineage["file"]).read_text())
    assert written["counts"] == lineage["counts"]
    assert len(written["vertices"]) == len(cut_positions)
    assert len(written["face_source_triangles"]) == len(cut_triangles)
    assert set(written["domains"]) == {
        "derivative_vertex", "derivative_face", "source_vertex", "source_triangle", "weights",
    }
    assert all(0 <= face < len(triangles) for face in written["face_source_triangles"])
    worst = 0.0
    for index, record in enumerate(written["vertices"]):
        assert 0 <= record["source_triangle"] < len(triangles)
        assert abs(sum(record["weights"]) - 1.0) <= lineage["tolerance"]
        assert all(-lineage["tolerance"] <= weight <= 1.0 + lineage["tolerance"]
                   for weight in record["weights"])
        rebuilt = reconstruct_record(positions, triangles, record)
        target = cut_positions[index]
        worst = max(worst, max(abs(rebuilt[axis] - target[axis]) for axis in range(3)))
        assert worst <= lineage["tolerance"], (index, rebuilt, target)
    assert worst <= lineage["tolerance"]
    # A vertex is a source corner or a clipped point, never a "new source vertex".
    kinds = {record["kind"] for record in written["vertices"]}
    assert kinds <= {"source-corner", "clipped"}, kinds

    # A plane that cuts nothing is diagnosed, not accepted.
    empty = {"cuts": [cut("noop", 0.5, [0.0, 0.0, 1.0], "negative")]}
    try:
        adapter.apply_source_cuts(positions, triangles, empty, cut_output())
    except RuntimeError as error:
        assert "removed nothing" in str(error), str(error)
    else:
        raise AssertionError("a no-effect cut was accepted")

    # Retaining the wrong side empties the solid and is refused.
    wrong = {"cuts": [cut("waist", 0.5, [0.0, 0.0, 1.0], "positive")]}
    try:
        adapter.apply_source_cuts(positions, triangles, wrong, cut_output())
    except RuntimeError as error:
        assert "removed the whole surface" in str(error), str(error)
        assert "`waist`" in str(error), str(error)
    else:
        raise AssertionError("an emptying cut was accepted")
    # The opposite side keeps the whole solid: also a diagnosed no-op, not a surface.
    keep_everything = {"cuts": [cut("waist", 0.5, [0.0, 0.0, -1.0], "positive")]}
    try:
        adapter.apply_source_cuts(positions, triangles, keep_everything, cut_output())
    except RuntimeError as error:
        assert "removed nothing" in str(error), str(error)
    else:
        raise AssertionError("a cut that keeps every face was accepted")

    # A transformed and scaled solid cuts the same way: the plane is a frame fact.
    factor, offset = 2.5, (3.0, -1.0, 0.5)
    moved = [
        (x * factor + offset[0], y * factor + offset[1], z * factor + offset[2])
        for x, y, z in positions
    ]
    moved_cuts = {
        "cuts": [
            cut("waist", 0.19 * factor + offset[2], [0.0, 0.0, 1.0], "negative"),
            cut("hem", 0.01 * factor + offset[2], [0.0, 0.0, -1.0], "negative"),
        ]
    }
    _, moved_triangles, moved_report = adapter.apply_source_cuts(
        moved, triangles, moved_cuts, cut_output()
    )
    assert len(moved_triangles) == len(cut_triangles)
    moved_lineage = moved_report["lineage"]
    assert moved_lineage["counts"]["clipped_vertices"] == lineage["counts"]["clipped_vertices"]
    assert moved_lineage["removed_face_count"] == lineage["removed_face_count"]
    assert moved_lineage["verified"], moved_lineage["max_reconstruction_error"]
    assert moved_lineage["tolerance"] > lineage["tolerance"], "tolerance follows the extent"
    # Trimmed area scales with the squared factor.
    ratio = moved_lineage["total_area_removed"] / lineage["total_area_removed"]
    assert abs(ratio - factor * factor) <= 1e-3, ratio
    print("CUT_OWNERSHIP ok", flush=True)


def partial_cut_tests():
    """A plane that clips one triangle without removing any complete face."""
    # Two triangles forming a square; the plane cuts a corner off the first one.
    positions = [
        (0.0, 0.0, 0.0),
        (1.0, 0.0, 0.0),
        (1.0, 1.0, 0.0),
        (0.0, 1.0, 0.0),
    ]
    triangles = [[0, 1, 2], [0, 2, 3]]
    output = cut_output()
    # Keep x + y <= 1.9, which shaves the (1, 1) corner off triangle 1 only.
    clip = {
        "cuts": [
            {
                "label": "corner",
                "plane_point": [0.95, 0.95, 0.0],
                "normal": [1.0, 1.0, 0.0],
                "retain": "negative",
            }
        ]
    }
    _, cut_triangles, report = adapter.apply_source_cuts(
        positions, triangles, clip, output
    )
    lineage = report["lineage"]
    print(
        "PARTIAL_CUT",
        json.dumps(
            {
                "removed_faces": lineage["removed_face_count"],
                "removed_area": lineage["complete_face_removal_area"],
                "clipped_area": lineage["clipped_area"],
                "total": lineage["total_area_removed"],
                "bound": lineage["area_error_bound"],
                "faces": len(cut_triangles),
            }
        ),
        flush=True,
    )
    # Both source triangles survive in part: nothing is removed whole.
    assert lineage["removed_face_count"] == 0, lineage["removed_face_count"]
    assert lineage["removed_original_faces"] == []
    assert lineage["complete_face_removal_area"] == 0.0
    # The clipped corner is the actual area difference, and it is inside the bound.
    triangle_area = 0.5
    assert 0.0 < lineage["clipped_area"] < 2 * triangle_area
    assert abs(
        lineage["clipped_area"] - lineage["total_area_removed"]
    ) <= lineage["area_error_bound"]
    assert lineage["verified"]
    written = json.loads((output / lineage["file"]).read_text())
    assert written["counts"]["removed_face_count"] if "removed_face_count" in written["counts"] else True
    assert all(0 <= face < len(triangles) for face in written["face_source_triangles"])
    print("PARTIAL_CUT_OWNERSHIP ok", flush=True)


def known_face_provenance_tests():
    """Provenance is the owning source face, never the nearest unrelated edge."""
    # Two parallel sheets closer than the interpolation tolerance: a distance search
    # cannot tell them apart, but every face carries its own source triangle.
    gap = 2e-7
    positions = [
        (0.0, 0.0, gap),
        (1.0, 0.0, gap),
        (1.0, 1.0, gap),
        (0.0, 0.0, 0.0),
        (1.0, 0.0, 0.0),
        (1.0, 1.0, 0.0),
    ]
    triangles = [[0, 1, 2], [3, 4, 5]]
    output = cut_output()
    cuts = {
        "cuts": [
            {
                "label": "both-sheets",
                "plane_point": [0.5, 0.0, 0.0],
                "normal": [1.0, 0.0, 0.0],
                "retain": "negative",
            }
        ]
    }
    _, cut_triangles, report = adapter.apply_source_cuts(positions, triangles, cuts, output)
    written = json.loads((output / report["lineage"]["file"]).read_text())
    upper = [record for record in written["vertices"] if record["source_triangle"] == 0]
    lower = [record for record in written["vertices"] if record["source_triangle"] == 1]
    assert upper and lower, "both sheets are cut and both keep their own relation"
    for record in upper:
        assert abs(reconstruct_record(positions, triangles, record)[2] - gap) <= 1e-9, record
    for record in lower:
        assert abs(reconstruct_record(positions, triangles, record)[2]) <= 1e-9, record
    # Every triangle is attributed to the sheet its own faces belong to.
    sources = set(written["face_source_triangles"])
    assert sources == {0, 1}, sources
    assert report["lineage"]["verified"]
    print("KNOWN_FACE_PROVENANCE ok", flush=True)


cut_tests()
partial_cut_tests()
known_face_provenance_tests()
def y_solid():
    """A closed Y-shaped trouser solid: pelvis box over two leg boxes.

    Built from shared quads so the surface is a single closed manifold with one
    waist cap, two cuff caps and an open notch between the legs: the synthetic
    analogue of a capped trouser sculpt.
    """
    bm = bmesh.new()
    corners = {}

    def vertex(point):
        if point not in corners:
            corners[point] = bm.verts.new(point)
        return corners[point]

    def quad(points):
        bm.faces.new([vertex(point) for point in points])

    low, hip, top = 0.0, 3.0, 6.0
    left, inner_left, inner_right, right = -4.0, -2.6, 2.6, 4.0
    back, front = 0.6, -0.6
    for near, far in ((left, inner_left), (inner_right, right)):
        quad([(near, front, low), (far, front, low), (far, back, low), (near, back, low)])
        quad([(near, front, low), (far, front, low), (far, front, hip), (near, front, hip)])
        quad([(near, back, low), (far, back, low), (far, back, hip), (near, back, hip)])
        quad([(near, front, low), (near, back, low), (near, back, hip), (near, front, hip)])
        quad([(far, front, low), (far, back, low), (far, back, hip), (far, front, hip)])
    for near, far in ((left, inner_left), (inner_left, inner_right), (inner_right, right)):
        quad([(near, front, hip), (far, front, hip), (far, front, top), (near, front, top)])
        quad([(near, back, hip), (far, back, hip), (far, back, top), (near, back, top)])
    quad([(left, front, hip), (left, back, hip), (left, back, top), (left, front, top)])
    quad([(right, front, hip), (right, back, hip), (right, back, top), (right, front, top)])
    for near, far in ((left, inner_left), (inner_left, inner_right), (inner_right, right)):
        quad([(near, front, top), (far, front, top), (far, back, top), (near, back, top)])
    quad(
        [
            (inner_left, front, hip),
            (inner_right, front, hip),
            (inner_right, back, hip),
            (inner_left, back, hip),
        ]
    )
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bm.verts.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    bm.verts.index_update()
    positions = [tuple(vertex.co) for vertex in bm.verts]
    triangles = [[corner.index for corner in face.verts] for face in bm.faces]
    bm.free()
    return positions, triangles


def welded_loops(positions, triangles):
    """Exact-position boundary loops, as the native core identifies them."""
    weld = {}
    ids = []
    for point in positions:
        key = tuple(round(value, 9) for value in point)
        weld.setdefault(key, len(weld))
        ids.append(weld[key])
    counts = {}
    for face in triangles:
        corners = [ids[index] for index in face]
        if len(set(corners)) < 3:
            continue
        for first, second in (
            (corners[0], corners[1]),
            (corners[1], corners[2]),
            (corners[2], corners[0]),
        ):
            key = (min(first, second), max(first, second))
            counts[key] = counts.get(key, 0) + 1
    adjacency = {}
    for (first, second), count in counts.items():
        if count != 1:
            continue
        adjacency.setdefault(first, set()).add(second)
        adjacency.setdefault(second, set()).add(first)
    position = {}
    for index, welded in enumerate(ids):
        position.setdefault(welded, positions[index])
    seen = set()
    loops = []
    for start in adjacency:
        if start in seen:
            continue
        stack = [start]
        component = set()
        while stack:
            node = stack.pop()
            if node in component:
                continue
            component.add(node)
            stack.extend(adjacency[node])
        seen |= component
        points = [position[node] for node in component]
        # The core's contract: the edge-length-weighted midpoint of the boundary curve,
        # so this measurement reports the same quantity the core matches and never a
        # vertex arithmetic mean.
        weight = 0.0
        weighted = [0.0, 0.0, 0.0]
        for (first, second), count in counts.items():
            if count != 1 or first not in component:
                continue
            left = position[first]
            right = position[second]
            length = math.dist(left, right)
            if length == 0.0:
                continue
            weight += length
            for axis in range(3):
                weighted[axis] += 0.5 * (left[axis] + right[axis]) * length
        center = (
            [value / weight for value in weighted] if weight > 0.0 else list(points[0])
        )
        loops.append(
            {
                "vertices": len(component),
                "degrees": sorted({len(adjacency[node]) for node in component}),
                "points": points,
                "center": center,
            }
        )
    return loops


def y_cut_tests():
    """Waist plus two cuff cuts on a synthetic Y-shaped trouser solid."""
    positions, triangles = y_solid()
    source_positions = [tuple(point) for point in positions]
    source_triangles = [list(face) for face in triangles]
    assert count_boundary_edges(triangles) == 0, "the synthetic Y solid starts closed"
    assert welded_loops(positions, triangles) == [], "the synthetic Y solid has no opening"

    waist = {"label": "waist", "plane_point": [0.0, 0.0, 5.7], "normal": [0.0, 0.0, 1.0], "retain": "negative"}
    # Each cuff plane is tilted toward its own leg, so at the other leg it passes
    # below the solid and removes nothing.
    left = {
        "label": "left-cuff",
        "plane_point": [-3.3, 0.0, 0.4],
        "normal": [-0.09, 0.0, -0.996],
        "retain": "negative",
    }
    right = {
        "label": "right-cuff",
        "plane_point": [3.3, 0.0, 0.4],
        "normal": [0.09, 0.0, -0.996],
        "retain": "negative",
    }
    output = cut_output()
    cut_positions, cut_triangles, report = adapter.apply_source_cuts(
        positions, triangles, {"cuts": [waist, left, right]}, output
    )
    loops = welded_loops(cut_positions, cut_triangles)
    print(
        "Y_CUTS",
        json.dumps(
            {
                "loops": [
                    {"vertices": loop["vertices"], "degrees": loop["degrees"],
                     "center": [round(value, 6) for value in loop["center"]]}
                    for loop in loops
                ],
                "removed_face_count": report["lineage"]["removed_face_count"],
                "counts": report["lineage"]["counts"],
                "max_error": report["lineage"]["max_reconstruction_error"],
            }
        ),
        flush=True,
    )
    # Exactly the three declared passages, each a closed rim with no pinch.
    assert len(loops) == 3, [loop["vertices"] for loop in loops]
    assert all(loop["degrees"] == [2] for loop in loops), loops
    # One rim per declared plane: every vertex of the rim lies on that plane.
    matched = []
    for declared in (waist, left, right):
        point = declared["plane_point"]
        normal = declared["normal"]
        length = math.sqrt(sum(component * component for component in normal))
        unit = [component / length for component in normal]
        on_plane = [
            loop
            for loop in loops
            if all(
                abs(
                    sum(unit[axis] * (vertex[axis] - point[axis]) for axis in range(3))
                )
                <= 1e-5
                for vertex in loop["points"]
            )
        ]
        assert len(on_plane) == 1, (declared["label"], [loop["vertices"] for loop in loops])
        matched.append(id(on_plane[0]))
    assert len(set(matched)) == 3, "each declared cut owns its own rim"
    # The three caps are gone and nothing was filled back in.
    lineage = report["lineage"]
    # Six waist-cap triangles plus two triangles per cuff cap, and the 0-based ids name
    # exactly the faces that sit on a cap (every corner above the waist plane or below a
    # cuff plane), never a side-wall face.
    assert lineage["removed_face_count"] == 10, lineage["removed_face_count"]
    removed = lineage["removed_original_faces"]
    assert len(set(removed)) == len(removed), removed
    for index in removed:
        heights = [positions[vertex][2] for vertex in triangles[index]]
        assert all(height > 5.6 for height in heights) or all(
            height < 0.5 for height in heights
        ), (index, heights)
    assert lineage["verified"] and lineage["fill"] == "none; declared openings stay open"
    assert lineage["total_area_removed"] > 0.0
    # Every one of the three rims is carried by the complete relation, read back.
    written = json.loads((output / lineage["file"]).read_text())
    for record in written["vertices"]:
        assert 0 <= record["source_triangle"] < len(triangles)
        rebuilt = reconstruct_record(positions, triangles, record)
        target = cut_positions[written["vertices"].index(record)]
        assert max(abs(rebuilt[axis] - target[axis]) for axis in range(3)) <= lineage["tolerance"]
    assert positions == source_positions and triangles == source_triangles
    assert [row["label"] for row in report["cuts"]] == ["waist", "left-cuff", "right-cuff"]
    print("Y_CUT_OWNERSHIP ok", flush=True)


y_cut_tests()



# --- end-to-end native cut pipeline -----------------------------------------

Y_CUTS = [
    {
        "label": "waist-cap",
        "plane_point": [0.0, 0.0, 5.7],
        "normal": [0.0, 0.0, 1.0],
        "retain": "negative",
    },
    {
        "label": "left-cuff-cap",
        "plane_point": [-3.3, 0.0, 0.4],
        "normal": [-0.09, 0.0, -0.996],
        "retain": "negative",
    },
    {
        "label": "right-cuff-cap",
        "plane_point": [3.3, 0.0, 0.4],
        "normal": [0.09, 0.0, -0.996],
        "retain": "negative",
    },
]


def retained_source_share(positions, cuts):
    """Share of source vertices the declared cuts leave in place, by plane."""
    kept = 0
    for point in positions:
        removed = False
        for cut in cuts:
            normal = cut["normal"]
            origin = cut["plane_point"]
            side = sum(normal[axis] * (point[axis] - origin[axis]) for axis in range(3))
            if (cut["retain"] == "negative" and side > 0.0) or (
                cut["retain"] == "positive" and side < 0.0
            ):
                removed = True
        kept += 0 if removed else 1
    return kept / len(positions)


def y_cut_recipe(root, *, target, minimum, maximum, surface="boundary", anchor_shift=(0.0, 0.0, 0.0), coverage_minimum=0.3):
    """The Y solid saved with UVs and a material, plus a cut profile and recipe."""
    root.mkdir(parents=True, exist_ok=True)
    blend = root / "SyntheticTrousers.blend"
    bpy.ops.wm.read_factory_settings(use_empty=True)
    positions, triangles = y_solid()
    mesh = bpy.data.meshes.new("SyntheticTrousers")
    mesh.from_pydata(positions, [], triangles)
    mesh.update()
    uv = mesh.uv_layers.new(name="UVMap")
    for loop in mesh.loops:
        point = positions[loop.vertex_index]
        uv.data[loop.index].uv = (point[0] / 8.0 + 0.5, point[2] / 6.0)
    material = bpy.data.materials.new("synthetic_cloth")
    material.use_nodes = True
    mesh.materials.append(material)
    obj = bpy.data.objects.new("SyntheticTrousers", mesh)
    bpy.context.collection.objects.link(obj)
    bpy.ops.wm.save_as_mainfile(filepath=str(blend))

    # The opening anchors are measured from the same native cut the pipeline runs.
    measured = Path(tempfile.mkdtemp(prefix="sculpto-cut-rims-"))
    cut_positions, cut_triangles, cut_report = adapter.apply_source_cuts(
        positions, triangles, {"cuts": Y_CUTS}, measured
    )
    rims = welded_loops(cut_positions, cut_triangles)
    assert len(rims) == 3, [rim["vertices"] for rim in rims]
    waist = max(rims, key=lambda rim: rim["center"][2])
    cuffs = sorted((rim for rim in rims if rim is not waist), key=lambda rim: rim["center"][0])
    openings = [
        ("waist", waist["center"]),
        ("left cuff", cuffs[0]["center"]),
        ("right cuff", cuffs[1]["center"]),
    ]
    profile_path = root / "SyntheticTrousers.json"
    profile_path.write_text(
        json.dumps(
            {
                "frame": "source-local-z-up",
                "surface": surface,
                "guide_segments": [
                    [[0.0, 0.0, 2.0], [0.0, 0.0, 7.0]],
                    [[0.0, 0.0, 2.0], [-3.3, 0.0, -1.0]],
                    [[0.0, 0.0, 2.0], [3.3, 0.0, -1.0]],
                ],
                "apertures": [
                    {
                        "role": role,
                        "anchor": [
                            round(center[axis] + anchor_shift[axis], 6) for axis in range(3)
                        ],
                        # The declared opening tolerance: the source stage matches exactly,
                        # and the reduced stage may move a rim by a fraction of a unit.
                        "radius": 0.5,
                    }
                    for role, center in openings
                ],
                "source_repair": {
                    "max_incidental_loops": 0,
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
                    "target_triangles": target,
                    "minimum_triangles": minimum,
                    "maximum_triangles": maximum,
                },
                "coverage": {"tolerance": 0.01, "minimum": coverage_minimum},
                "contact": {
                    "mode": "reject",
                    "clearance": 0.000001,
                    "max_displacement": 0.000001,
                    "max_pairs": 1,
                },
                "source_cuts": Y_CUTS,
            },
            indent=2,
        )
    )
    recipe = {
        "source": str(blend),
        "source_sha256": sha256(blend),
        "profile": str(profile_path),
        "profile_sha256": sha256(profile_path),
        "object": "SyntheticTrousers",
        "sculpto": str(SCULPTO),
    }
    recipe_path = root / "SyntheticTrousers-recipe.json"
    recipe_path.write_text(json.dumps(recipe, indent=2))
    return recipe_path, recipe, cut_report


def cut_pipeline_tests():
    """Capped Y solid -> declared cuts -> retained boundary -> qualified control."""
    with tempfile.TemporaryDirectory(prefix="sculpto-cuts-") as temporary:
        root = Path(temporary)
        # A boxy solid has no spare triangles to remove without distortion, so the
        # reduction plan is exercised at ratio 1.0; real QEM reduction stays covered by
        # the retained thick-shell case below.
        recipe_path, recipe, measured = y_cut_recipe(root, target=60, minimum=10, maximum=100)
        source_digest = recipe["source_sha256"]
        output = root / "control"
        main(recipe_path, output)
        receipt = json.loads((output / "receipt.json").read_text())
        assert receipt["status"] == "constructed", receipt.get("error")
        print(
            "CUT_PIPELINE",
            json.dumps(
                {
                    "surface": receipt["surface_policy"],
                    "apertures": [match["role"] for match in receipt["apertures"]],
                    "incidental": len(receipt["incidental"]),
                    "exterior": receipt["exterior"]["exterior_triangles"],
                    "interior_removed": receipt["exterior"]["interior_removed"],
                    "composed": {
                        key: receipt["stages"]["composed"][key]
                        for key in (
                            "selected_vertices",
                            "clipped_vertices",
                            "composed_faces",
                            "max_reconstruction_error",
                        )
                    },
                    "reduction": receipt["decimation"]["ratio"],
                    "loops": [
                        loop["edges"] for loop in receipt["validation"]["measured"]["loops"]
                    ],
                    "euler": receipt["validation"]["measured"]["euler"],
                    "coverage": receipt["validation"]["deviations"]["source_exterior_to_control"][
                        "coverage"
                    ],
                }
            ),
            flush=True,
        )
        # 1. The declared policy kept the whole cut shell: no guide-facing removal.
        assert receipt["surface_policy"] == "boundary"
        assert receipt["exterior"]["interior_removed"] == 0
        # 2. Exactly the three declared openings, nothing incidental.
        assert [match["role"] for match in receipt["apertures"]] == [
            "waist",
            "left cuff",
            "right cuff",
        ]
        assert receipt["incidental"] == []
        loops = receipt["validation"]["measured"]["loops"]
        assert len(loops) == 3, loops
        assert receipt["validation"]["measured"]["euler"] == -1
        # 3. Complete provenance is a file and composes back to the original source.
        lineage = receipt["source_stage"]["lineage"]
        assert (output / lineage["file"]).is_file()
        composed = receipt["stages"]["composed"]
        assert composed["verified"] is True
        assert composed["selected_vertices"] == receipt["stages"]["selected"]["vertices"]
        assert composed["clipped_vertices"] > 0
        assert composed["composed_faces"] == receipt["stages"]["selected"]["triangles"]
        assert composed["max_reconstruction_error"] <= lineage["tolerance"]
        assert composed["source_triangles"] == receipt["source_geometry"]["triangles"]
        # 4. Immutability: the saved source, its UVs and its material are untouched.
        assert sha256(Path(recipe["source"])) == source_digest
        assert receipt["source_geometry"]["unchanged"] is True
        assert receipt["source_geometry"]["uv_layers"][0]["name"] == "UVMap"
        assert receipt["source_geometry"]["materials"][0]["name"] == "synthetic_cloth"
        # 5. Bounds, winding, fan, self-contact and coverage gates pass.
        validation = receipt["validation"]
        assert validation["verdict"]["passed"] is True, validation["verdict"]
        measured_control = validation["measured"]
        assert measured_control["orientation_conflicts"] == 0
        assert measured_control["non_manifold_edges"] == 0
        assert measured_control["non_manifold_vertices"] == 0
        assert measured_control["self_contacts"] == 0
        assert measured_control["components"] == 1
        assert measured_control["coincident_vertices"] == 0
        declared = json.loads(Path(recipe["profile"]).read_text())["reduction"]
        assert (
            declared["minimum_triangles"]
            <= validation["measured"]["triangles"]
            <= declared["maximum_triangles"]
        ), (validation["measured"]["triangles"], declared)
        assert receipt["decimation"]["ratio"] == 1.0, receipt["decimation"]
        coverage = validation["deviations"]["source_exterior_to_control"]["coverage"]
        # The measured coverage is exactly the share of source vertices the declared cuts
        # remove from a coarse 24-vertex solid, not a hidden defect: the trim is recorded
        # separately and the limit is the measured declared geometry.
        expected = retained_source_share(y_solid()[0], Y_CUTS)
        assert abs(coverage - expected) <= 0.02, (coverage, expected)
        assert coverage >= 0.3, coverage
        # 6. Actual trim is reported against the original surface, not lowered away.
        assert lineage["removed_face_count"] == 10, lineage["removed_face_count"]
        assert lineage["total_area_removed"] > 0.0
        assert receipt["source_stage"]["areas"]["complete_face_removal"] > 0.0
        # 7. Output is exclusive.
        try:
            main(recipe_path, output)
        except FileExistsError as error:
            assert "already exists" in str(error), str(error)
        else:
            raise AssertionError("an existing output directory was reused")

        # 8. A declared opening that does not exist keeps its failure and diagnostics.
        shifted_path, shifted, _ = y_cut_recipe(
            root / "shifted", target=60, minimum=10, maximum=100, anchor_shift=(1.5, 0.0, 0.0)
        )
        raised = None
        try:
            main(shifted_path, root / "shifted-control")
        except RuntimeError as error:
            raised = str(error)
        assert raised is not None and "no boundary loop lies within" in raised, raised
        failed = json.loads((root / "shifted-control" / "receipt.json").read_text())
        assert failed["status"] == "failed; diagnostics retained"
        assert "UnknownAperture" in failed["error"] or "no boundary loop" in failed["error"]
        assert (root / "shifted-control" / "derivative.obj").is_file()
        assert (root / "shifted-control" / "derivative-lineage.json").is_file()

        # 9. Without the policy field the same cuts stay on the published guide route,
        #    which drops the solid's folds and cannot resolve three clean openings.
        guide_path, guide_recipe, _ = y_cut_recipe(
            root / "guide", target=60, minimum=10, maximum=100
        )
        text = json.loads(Path(guide_recipe["profile"]).read_text())
        text.pop("surface")
        Path(guide_recipe["profile"]).write_text(json.dumps(text, indent=2))
        guide_recipe["profile_sha256"] = sha256(Path(guide_recipe["profile"]))
        guide_path.write_text(json.dumps(guide_recipe, indent=2))
        raised = None
        try:
            main(guide_path, root / "guide-control")
        except RuntimeError as error:
            raised = str(error)
        assert raised is not None, "the guide route accepted a capped solid"
        print("CUT_PIPELINE_OWNERSHIP ok", flush=True)


cut_pipeline_tests()
