"""Focused tests for the optional protected native reduction backend.

Run with the built executable:

    SCULPTO_PROTECTED_REDUCE=<build>/protected_reduce \\
      python3 -B tools/tests/test_protected_reduce.py

Without the variable the file reports `PROTECTED_REDUCE_SKIP` and exits 0, because the
native toolchain is not a prerequisite of the Rust gate. Fixtures are synthetic and
generated here; no garment asset is read or written.
"""

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


BINARY = os.environ.get("SCULPTO_PROTECTED_REDUCE")


def write_obj(path, positions, triangles):
    lines = [f"v {x:.17g} {y:.17g} {z:.17g}" for x, y, z in positions]
    lines.extend(f"f {a + 1} {b + 1} {c + 1}" for a, b, c in triangles)
    Path(path).write_text("\n".join(lines) + "\n")


def read_obj(path):
    positions, triangles = [], []
    for line in Path(path).read_text().splitlines():
        if line.startswith("v "):
            positions.append(tuple(float(value) for value in line.split()[1:4]))
        elif line.startswith("f "):
            triangles.append(tuple(int(value.split("/")[0]) - 1 for value in line.split()[1:]))
    return positions, triangles


def tube(stations=32, sides=12, radius=0.05):
    """An open tube: two boundary loops, no caps, 2*sides*(stations-1) triangles."""
    positions = []
    for station in range(stations):
        height = station / (stations - 1)
        for side in range(sides):
            angle = math.tau * side / sides
            positions.append((radius * math.cos(angle), radius * math.sin(angle), height))
    triangles = []
    for station in range(stations - 1):
        for side in range(sides):
            following = (side + 1) % sides
            near = station * sides
            far = (station + 1) * sides
            triangles.append((near + side, near + following, far + following))
            triangles.append((near + side, far + following, far + side))
    return positions, triangles


def crossing_sheets():
    """Two clean sheets that cross: the guards must not add crossings."""
    positions = []
    triangles = []
    for sheet in range(2):
        base = len(positions)
        for row in range(6):
            for column in range(6):
                u = column / 5.0 - 0.5
                v = row / 5.0 - 0.5
                if sheet == 0:
                    positions.append((u, v, 0.0))
                else:
                    positions.append((u * 0.4, 0.0, v * 0.4))
        for row in range(5):
            for column in range(5):
                corner = base + row * 6 + column
                triangles.append((corner, corner + 1, corner + 7))
                triangles.append((corner, corner + 7, corner + 6))
    return positions, triangles


def boundary_loops(triangles):
    counts = {}
    for triangle in triangles:
        for first, second in (
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ):
            key = (min(first, second), max(first, second))
            counts[key] = counts.get(key, 0) + 1
    boundary = [edge for edge, count in counts.items() if count == 1]
    adjacency = {}
    for first, second in boundary:
        adjacency.setdefault(first, set()).add(second)
        adjacency.setdefault(second, set()).add(first)
    seen = set()
    loops = 0
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
        loops += 1
    return loops


def nondegenerate(positions, triangles):
    for face in triangles:
        if len(set(face)) != 3:
            return False
        a, b, c = (positions[index] for index in face)
        if not all(math.isfinite(value) for point in (a, b, c) for value in point):
            return False
        ab = [b[axis] - a[axis] for axis in range(3)]
        ac = [c[axis] - a[axis] for axis in range(3)]
        cross = (
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        )
        if math.sqrt(sum(component * component for component in cross)) <= 1e-14:
            return False
    return True


def run(arguments, directory, timeout=120):
    return subprocess.run(
        [BINARY, *arguments], capture_output=True, text=True, timeout=timeout, check=False
    )


def reduce(directory, name, positions, triangles, target, tolerance=0.01, timeout=60):
    source = Path(directory) / f"{name}.obj"
    write_obj(source, positions, triangles)
    output = Path(directory) / f"{name}-out"
    arguments = [
        "--input",
        str(source),
        "--target",
        str(target),
        "--tolerance",
        str(tolerance),
        "--timeout",
        str(timeout),
        "--output",
        str(output),
    ]
    result = run(arguments, directory)
    return source, output, result


def expect_failure(directory, arguments, expected, name):
    """`expected` is one message fragment or a tuple of acceptable fragments."""
    wanted = (expected,) if isinstance(expected, str) else expected
    result = run(arguments, directory)
    assert result.returncode == 1, (name, result.returncode, result.stdout, result.stderr)
    assert any(fragment in result.stderr for fragment in wanted), (name, result.stderr)
    print(f"PROTECTED_REDUCE_FAILURE ok {name}", flush=True)


def igl_identity(script, tree):
    """Run the identity script in script mode and return (digest, file count)."""
    result = subprocess.run(
        ["cmake", f"-DIGL_INCLUDE_DIR={tree}", "-P", str(script)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    # CMake NOTICE messages go to stderr; read both streams.
    for line in (result.stdout + result.stderr).splitlines():
        if line.startswith("IGL_TREE_SHA256="):
            parts = dict(token.split("=", 1) for token in line.split())
            return parts["IGL_TREE_SHA256"], int(parts["IGL_TREE_FILE_COUNT"])
    raise AssertionError(f"no identity line in {result.stdout + result.stderr!r}")


def provenance_tests(directory, script, tree):
    """The binary reports a measured content identity, and it tracks the content."""
    identity, count = igl_identity(script, tree)
    with tempfile.TemporaryDirectory(prefix="sculpto-igl-fixture-") as temporary:
        fixture = Path(temporary) / "igl"
        fixture.mkdir()
        (fixture / "alpha.h").write_text("// alpha\n")
        (fixture / "beta.cpp").write_text("// beta\n")
        first, first_count = igl_identity(script, Path(temporary))
        again, _ = igl_identity(script, Path(temporary))
        assert first == again, "the identity must be deterministic"
        assert first_count == 2, first_count
        (fixture / "beta.cpp").write_text("// beta changed\n")
        changed, _ = igl_identity(script, Path(temporary))
        assert changed != first, "altered content must change the recorded identity"
    print(
        "PROTECTED_REDUCE_PROVENANCE ok",
        json.dumps({"fixture_identity": first[:16], "tree_identity": identity[:16], "files": count}),
        flush=True,
    )
    return identity, count


def main():
    if not BINARY:
        print("PROTECTED_REDUCE_SKIP: set SCULPTO_PROTECTED_REDUCE to the built executable")
        return
    assert Path(BINARY).is_file(), BINARY
    tools = Path(__file__).resolve().parents[1]
    script = tools / "native" / "igl_identity.cmake"
    tree = Path(os.environ["SCULPTO_LIBIGL_INCLUDE"])
    identity, identity_files = provenance_tests(Path(tempfile.mkdtemp(prefix="sculpto-igl-")), script, tree)
    with tempfile.TemporaryDirectory(prefix="sculpto-protected-reduce-") as temporary:
        directory = Path(temporary)

        # 1. An open surface keeps its two openings and stays finite.
        positions, triangles = tube()
        source, output, result = reduce(directory, "tube", positions, triangles, target=120)
        assert result.returncode == 0, (result.returncode, result.stderr)
        receipt = json.loads((output / "result.json").read_text())
        candidate_positions, candidate_triangles = read_obj(output / "candidate.obj")
        assert receipt["target_reached"] is True, receipt
        assert receipt["cost"] == "qslim", receipt
        assert "qslim" in receipt["operation"], receipt
        assert receipt["stop_reason"] == "target", receipt
        assert receipt["libigl"]["content_sha256"] == identity, (receipt["libigl"], identity)
        assert receipt["libigl"]["file_count"] == identity_files, receipt["libigl"]
        assert receipt["libigl"]["qualified_reference"] == "b41f1944a703653ae0da81e6588650ba14c270ea"
        assert "not measured" in receipt["libigl"]["qualified_reference_note"]
        assert receipt["source_faces"] == len(triangles)
        assert receipt["candidate_faces"] == len(candidate_triangles)
        assert receipt["candidate_vertices"] == len(candidate_positions)
        assert abs(len(candidate_triangles) - 120) <= 2, len(candidate_triangles)
        assert boundary_loops(triangles) == 2, boundary_loops(triangles)
        assert boundary_loops(candidate_triangles) == 2, boundary_loops(candidate_triangles)
        assert nondegenerate(candidate_positions, candidate_triangles)
        guards = receipt["guards"]
        assert guards["attempts"] >= guards["collapses"] > 0, guards
        for name, value in guards.items():
            assert isinstance(value, int) and value >= 0, (name, value)
        assert guards["blocked_upstream"] + guards["blocked_fan"] + guards["blocked_source"] > 0, guards
        print(
            "PROTECTED_REDUCE_OPEN ok",
            json.dumps(
                {
                    "faces": [len(triangles), len(candidate_triangles)],
                    "loops": [2, boundary_loops(candidate_triangles)],
                    "guards": guards,
                    "seconds": round(receipt["seconds"], 3),
                }
            ),
            flush=True,
        )

        # 1b. There is exactly one declared cost, and the removed dual-cost switch is
        #     refused as an unknown option rather than silently ignored.
        result = run(
            [
                "--input",
                str(source),
                "--target",
                "120",
                "--tolerance",
                "0.01",
                "--timeout",
                "10",
                "--output",
                str(directory / "cost-out"),
                "--cost",
                "shortest-edge",
            ],
            directory,
        )
        assert result.returncode == 1, (result.returncode, result.stderr)
        assert "unknown option" in result.stderr, result.stderr
        print("PROTECTED_REDUCE_COST ok", json.dumps({"cost": "qslim only"}), flush=True)

        # 2. Smoke only: an already-crossing input still yields valid output. This is
        #    NOT the protection proof — an aggregate pair count can fall while new
        #    crossings appear, and a helper that skips shared-vertex pairs cannot see a
        #    local fold. The proof is `protected_reduce_guard_test`, which states one
        #    shared-vertex fold analytically against the production guard.
        positions, triangles = crossing_sheets()
        source, output, result = reduce(directory, "crossing", positions, triangles, target=40)
        assert result.returncode == 0, (result.returncode, result.stderr)
        receipt = json.loads((output / "result.json").read_text())
        candidate_positions, candidate_triangles = read_obj(output / "candidate.obj")
        assert nondegenerate(candidate_positions, candidate_triangles)
        assert receipt["guards"]["attempts"] >= receipt["guards"]["collapses"], receipt["guards"]
        print(
            "PROTECTED_REDUCE_CROSSING_SMOKE ok",
            json.dumps(
                {
                    "note": "valid output on already-crossing input; protection proof is the native guard test",
                    "faces": [len(triangles), len(candidate_triangles)],
                    "guards": receipt["guards"],
                }
            ),
            flush=True,
        )

        # 3. Invalid geometry and arguments fail explicitly, before any candidate.
        good = directory / "good.obj"
        write_obj(good, *tube(stations=8, sides=6))
        prefix = ["--input", str(good), "--tolerance", "0.01", "--timeout", "10"]
        expect_failure(
            directory,
            [*prefix, "--target", "0", "--output", str(directory / "bad-1")],
            "--target",
            "zero target",
        )
        expect_failure(
            directory,
            ["--input", str(good), "--target", "10", "--tolerance", "0", "--timeout", "10",
             "--output", str(directory / "bad-2")],
            "--tolerance",
            "zero tolerance",
        )
        expect_failure(
            directory,
            ["--input", str(good), "--target", "10", "--tolerance", "0.01", "--timeout", "-1",
             "--output", str(directory / "bad-3")],
            "--timeout",
            "negative timeout",
        )
        expect_failure(
            directory,
            ["--input", str(good), "--target", "abc", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(directory / "bad-4")],
            "--target",
            "non-numeric target",
        )
        expect_failure(
            directory,
            [*prefix, "--output", str(directory / "bad-5")],
            "--target",
            "missing target",
        )
        quad = directory / "quad.obj"
        quad.write_text("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n")
        expect_failure(
            directory,
            ["--input", str(quad), "--target", "1", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(directory / "bad-6")],
            "only triangles are supported",
            "quad face",
        )
        bad_index = directory / "index.obj"
        bad_index.write_text("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n")
        expect_failure(
            directory,
            ["--input", str(bad_index), "--target", "1", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(directory / "bad-7")],
            "out of range",
            "index out of range",
        )
        nan = directory / "nan.obj"
        nan.write_text("v 0 0 nan\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
        expect_failure(
            directory,
            ["--input", str(nan), "--target", "1", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(directory / "bad-8")],
            ("nonfinite", "vertex record"),
            "nonfinite vertex",
        )
        repeated = directory / "repeated.obj"
        repeated.write_text("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 1 2\n")
        expect_failure(
            directory,
            ["--input", str(repeated), "--target", "1", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(directory / "bad-9")],
            "repeats a corner",
            "repeated corner",
        )
        expect_failure(
            directory,
            ["--input", str(good), "--target", "99", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(directory / "bad-10")],
            "below the input",
            "target at or above input",
        )
        # The supported face form is bare positive indices only.
        for name, body, fragment in [
            ("slashed", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/1/1 2/2/1 3/3/1\n", "bare positive"),
            ("negative", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -1 2 3\n", "bare positive"),
            ("signed", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf +1 2 3\n", "bare positive"),
            ("letters", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf a b c\n", "bare positive"),
            ("zero", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n", "out of range"),
            ("two", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2\n", "2 corners"),
            ("five", "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nv 0 0 1\nf 1 2 3 4 5\n", "5 corners"),
        ]:
            fixture = directory / f"face-{name}.obj"
            fixture.write_text(body)
            expect_failure(
                directory,
                ["--input", str(fixture), "--target", "1", "--tolerance", "0.01", "--timeout", "10",
                 "--output", str(directory / f"face-{name}")],
                fragment,
                f"face form {name}",
            )
        existing = directory / "existing"
        existing.mkdir()
        expect_failure(
            directory,
            ["--input", str(good), "--target", "20", "--tolerance", "0.01", "--timeout", "10",
             "--output", str(existing)],
            "fresh",
            "existing output",
        )

        # 4. The deadline stops the run, keeps the checkpoint and reports why.
        positions, triangles = tube(stations=200, sides=24)
        source = directory / "large.obj"
        write_obj(source, positions, triangles)
        output = directory / "deadline-out"
        result = run(
            ["--input", str(source), "--target", "50", "--tolerance", "0.01", "--timeout", "0.001",
             "--output", str(output)],
            directory,
            timeout=120,
        )
        assert result.returncode == 124, (result.returncode, result.stderr)
        receipt = json.loads((output / "result.json").read_text())
        assert receipt["timed_out"] is True, receipt
        assert receipt["stop_reason"] in {"deadline", "signal"}, receipt
        assert receipt["target_reached"] is False, receipt
        checkpoint = output / "candidate.obj"
        assert checkpoint.is_file(), "the checkpoint must survive a deadline"
        positions_out, triangles_out = read_obj(checkpoint)
        assert nondegenerate(positions_out, triangles_out)
        print(
            "PROTECTED_REDUCE_DEADLINE ok",
            json.dumps({"checkpoint_faces": len(triangles_out), "stop_reason": receipt["stop_reason"]}),
            flush=True,
        )

    print("PROTECTED_REDUCE ok", flush=True)


if __name__ == "__main__":
    main()
