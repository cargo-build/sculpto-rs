"""Headless operator run: import, preset, fit and export, all through sculpto.

Blender: --background --factory-startup --python-exit-code 1 \
  --python tools/blender_addon/headless_run.py -- [--out DIR] [--bin PATH]

Runs the operator command chain on the CMD_GATE Tier-1 fixture (a synthetic
NIF written by `sculpto_nif::write::se_shape::write_se_shape`): `export.obj`
imports the NIF through the command and asserts the Blender vertex count
against the envelope, `bake` applies the fixture preset, `outfit.fit` binds
the shape onto the baked body, `export.nif` converts the NIF to the LE
target. Every artifact is written by the CLI — this script never writes a
mesh format itself. Prints exactly one JSON summary line; exits non-zero on
any refusal or count mismatch.
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from sculpto_garment import cli_bridge, pipeline  # noqa: E402

import bpy  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "crates" / "sculpto-cli" / "tests" / "fixtures" / "tier1"
NIF = FIXTURE / "ShapeData" / "Tier1" / "Body.nif"
PROJECT = FIXTURE / "SliderSets" / "Tier1.osp"
PRESETS = FIXTURE / "SliderPresets" / "Tier1.xml"
# The set/preset/shape names the fixture pins (tests/fixtures/tier1/README.md).
SET = "Tier1 Body"
PRESET = "Tier1 Default"
SHAPE = "Tier1 Body"
ENDPOINT = "big"


def script_arguments():
    """Values after the `--` separator Blender leaves in sys.argv."""
    argv = sys.argv
    values = {}
    if "--" in argv:
        rest = argv[argv.index("--") + 1 :]
        for index, flag in enumerate(rest):
            if flag in ("--out", "--bin") and index + 1 < len(rest):
                values[flag[2:]] = rest[index + 1]
    return values


def import_artifact(path, expected_vertices, expected_triangles, name):
    """Build Blender data from a CLI OBJ artifact and assert the counts.

    `expected_triangles` is None for envelopes that report vertices only
    (`outfit.fit`); the face count is then recorded, not asserted.
    """
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=str(path))
    imported = [o for o in bpy.data.objects if o not in before and o.type == "MESH"]
    if len(imported) != 1:
        raise RuntimeError(f"{path}: expected one mesh object, got {len(imported)}")
    obj = imported[0]
    obj.name = name
    vertices, triangles = len(obj.data.vertices), len(obj.data.polygons)
    if vertices != expected_vertices or (
        expected_triangles is not None and triangles != expected_triangles
    ):
        raise RuntimeError(
            f"{path}: imported {vertices} vertices / {triangles} faces, "
            f"the envelope reported {expected_vertices} / {expected_triangles}"
        )
    return {"imported_vertices": vertices, "imported_faces": triangles}


def main():
    values = script_arguments()
    work = Path(values.get("out") or tempfile.mkdtemp(prefix="sculpto-headless-"))
    work.mkdir(parents=True, exist_ok=True)
    cli = cli_bridge.SculptoCli(binary=values.get("bin"))

    summary = {
        "ok": True,
        "binary": cli.binary,
        "fixture": str(FIXTURE),
        "work": str(work),
        "steps": {},
    }
    try:
        if not NIF.is_file():
            raise RuntimeError(f"Tier-1 fixture is missing: {NIF}")

        # Import through the model: the envelope's counts must match Blender.
        import_obj = work / "import.obj"
        result = cli.run("export.obj", cli_bridge.export_obj_payload(NIF, import_obj))
        summary["steps"]["import"] = {
            "cmd": "export.obj",
            "shape": result["shape"],
            "vertices": result["vertices"],
            "triangles": result["triangles"],
            **import_artifact(
                import_obj, result["vertices"], result["triangles"], "SculptoHeadlessImport"
            ),
        }

        # Apply the fixture preset: bake the slider set to an OBJ.
        baked_obj = work / "baked.obj"
        result = cli.run(
            "bake",
            cli_bridge.bake_payload(
                PROJECT, PRESETS, SET, PRESET, ENDPOINT, baked_obj, shape=SHAPE, home=FIXTURE
            ),
        )
        summary["steps"]["preset"] = {
            "cmd": "bake",
            "vertices": result["vertices"],
            "triangles": result["triangles"],
            "sliders": result["sliders"],
            **import_artifact(
                baked_obj, result["vertices"], result["triangles"], "SculptoHeadlessBaked"
            ),
        }

        # Fit: bind the NIF shape onto the baked body. The fixture carries one
        # shape, so garment and authored body are the same shape and the
        # offsets come out ~0; a real garment/body pair needs a two-shape NIF
        # (tests/fixtures/tier1/README.md).
        fitted_obj = work / "fitted.obj"
        result = cli.run(
            "outfit.fit", cli_bridge.fit_payload(NIF, SHAPE, SHAPE, baked_obj, fitted_obj)
        )
        summary["steps"]["fit"] = {
            "cmd": "outfit.fit",
            "garment": result["garment"],
            "reference": result["reference"],
            "vertices": result["vertices"],
            "offset_mean": result["offset_mean"],
            "offset_max": result["offset_max"],
            # The fit envelope reports vertices only (no triangles), so the
            # face count is recorded unasserted here.
            **import_artifact(fitted_obj, result["vertices"], None, "SculptoHeadlessFitted"),
        }

        # Export through the command: LE conversion, no Blender NIF writer.
        converted_nif = work / "converted_le.nif"
        result = cli.run("export.nif", cli_bridge.export_nif_payload(NIF, converted_nif, "le"))
        artifact_bytes = converted_nif.stat().st_size
        if artifact_bytes != result["bytes"]:
            raise RuntimeError(
                f"{converted_nif}: {artifact_bytes} bytes on disk, "
                f"the envelope reported {result['bytes']}"
            )
        summary["steps"]["export"] = {
            "cmd": "export.nif",
            "target": result["target"],
            "vertices": result["vertices"],
            "triangles": result["triangles"],
            "bytes": artifact_bytes,
        }

        # The produced files must match the plain-CLI run: the same bake via
        # the argv surface must write identical bytes.
        parity_obj = work / "baked-argv.obj"
        argv_run = subprocess.run(
            [
                cli.binary,
                "bake",
                "--project", str(PROJECT),
                "--presets", str(PRESETS),
                "--set", SET,
                "--preset", PRESET,
                "--endpoint", ENDPOINT,
                "--shape", SHAPE,
                "--home", str(FIXTURE),
                "--out", str(parity_obj),
            ],
            capture_output=True,
            text=True,
            timeout=cli.timeout,
        )
        if argv_run.returncode != 0:
            raise RuntimeError(f"argv bake failed: {argv_run.stderr.strip()}")
        if baked_obj.read_bytes() != parity_obj.read_bytes():
            raise RuntimeError("the command-surface bake does not match the argv bake")
        summary["argv_bake_matches"] = True
        summary["artifacts"] = [
            str(import_obj),
            str(baked_obj),
            str(fitted_obj),
            str(converted_nif),
            str(parity_obj),
        ]
    except Exception as error:  # noqa: BLE001 - one summary line either way
        summary["ok"] = False
        summary["error"] = f"{type(error).__name__}: {error}"
        print(json.dumps(summary))
        sys.exit(1)
    print(json.dumps(summary))
    sys.exit(0)


main()
