"""Headless add-on test: import, export and one classified failure (F07).

Blender (the pinned system binary):

    /run/current-system/sw/bin/blender --background --python \
      tools/blender_addon/tests/test_addon_headless.py

Plain `python3` skips with a clear message: every assertion needs Blender's
`bpy`. The sculpto binary is discovered like the add-on discovers it
($SCULPTO_BIN, then target/debug/sculpto): the add-on owns no format reader
(DECL0002). Exit 0 pass, exit 1 fail, each failure printed as one FAIL line.
"""

import os
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools" / "blender_addon"))

try:
    import bpy
except ModuleNotFoundError:
    print(
        "SKIP: Blender is absent — run it as "
        "/run/current-system/sw/bin/blender --background --python "
        "tools/blender_addon/tests/test_addon_headless.py"
    )
    sys.exit(0)

import sculpto_garment  # noqa: E402

from sculpto_garment import cli_bridge, pipeline  # noqa: E402

SCULPTO = ROOT / "target" / "debug" / "sculpto"
NIF = (
    ROOT
    / "crates"
    / "sculpto-cli"
    / "tests"
    / "fixtures"
    / "tier1"
    / "ShapeData"
    / "Tier1"
    / "Body.nif"
)


class Recorder:
    """Operator stand-in: captures every report call for exact assertions."""

    def __init__(self):
        self.reports = []

    def report(self, level, message):
        self.reports.append((set(level), message))


def check(condition, message):
    if not condition:
        print(f"FAIL: {message}")
        sys.exit(1)


def main():
    binary = os.environ.get("SCULPTO_BIN") or str(SCULPTO)
    check(Path(binary).is_file(), f"sculpto binary is missing: {binary} (build it first)")
    check(NIF.is_file(), f"Tier-1 fixture is missing: {NIF}")
    cli = cli_bridge.SculptoCli(binary=binary)
    work = tempfile.TemporaryDirectory(prefix="sculpto-addon-test-")

    # 1. Import: the add-on's NIF path builds real Blender data. import_nif
    #    asserts its vertex/triangle counts against the envelope itself, so
    #    reaching the checks below means the counts agreed.
    raw, _material = pipeline.import_nif(str(NIF), Recorder(), cli=cli)
    check(raw is not None and raw.type == "MESH", "the import produced no mesh")
    vertices = len(raw.data.vertices)
    check(vertices > 0, "the imported mesh is empty")

    # 2. Export: the registered operator converts the fixture through
    #    bpy.ops and leaves the artifact on disk.
    sculpto_garment.register()
    local_nif = Path(work.name) / "Body.nif"
    local_nif.write_bytes(NIF.read_bytes())
    result = bpy.ops.sculpto_garment.export_nif(filepath=str(local_nif), target="le")
    check(result == {"FINISHED"}, f"the export operator did not finish: {result}")
    artifact = Path(work.name) / "Body_le.nif"
    check(artifact.is_file(), f"the converted NIF is missing: {artifact}")

    # 3. Failure path: a corrupt NIF is refused. The expected kind/message
    #    and exit code come from the same command through the bridge, so the
    #    assertion covers exactly the surfacing: the real execute() reports
    #    `kind: message` and cancels instead of raising.
    bad = Path(work.name) / "bad.nif"
    bad.write_bytes(b"not a NIF")
    try:
        cli.run(
            "export.nif",
            cli_bridge.export_nif_payload(bad, Path(work.name) / "bad_le.nif", "le"),
        )
        check(False, "the engine accepted a corrupt NIF")
    except cli_bridge.CommandError as error:
        expected = f"{error.kind}: {error.message}"
        check(error.exit_code == 2, f"a refused NIF must exit 2, got {error.exit_code}")
    recorder = Recorder()
    stand_in = SimpleNamespace(filepath=str(bad), target="le", shape="", report=recorder.report)
    result = sculpto_garment.SCULPTO_OT_export_nif.execute(stand_in, None)
    check(result == {"CANCELLED"}, f"a bad NIF must cancel, got {result}")
    check(
        recorder.reports == [({"ERROR"}, expected)],
        f"the bad-NIF report is {recorder.reports!r}, expected [({{'ERROR'}}, {expected!r})]",
    )

    exported_bytes = artifact.stat().st_size
    work.cleanup()
    print(
        f"OK: import {vertices} vertices, export {exported_bytes} bytes, "
        f"bad NIF reported {expected!r}"
    )
    sys.exit(0)


try:
    main()
except SystemExit:
    raise
except Exception as error:  # noqa: BLE001 - one FAIL line either way
    print(f"FAIL: {type(error).__name__}: {error}")
    sys.exit(1)
