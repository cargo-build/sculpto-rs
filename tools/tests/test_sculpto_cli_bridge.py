"""The add-on's command bridge: argv shape, payload builders, envelope parsing.

Runs with plain `python3 -m unittest` — no Blender, no build. The one optional
case dispatches the real `sculpto` binary when `target/debug/sculpto` exists.

The bridge is imported by file path because the `sculpto_garment` package
`__init__` registers Blender classes and needs `bpy`; the bridge itself is
pure stdlib.
"""

import importlib.util
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

BRIDGE = (
    Path(__file__).resolve().parents[1]
    / "blender_addon"
    / "sculpto_garment"
    / "cli_bridge.py"
)
spec = importlib.util.spec_from_file_location("sculpto_cli_bridge", BRIDGE)
cli_bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cli_bridge)

ROOT = Path(__file__).resolve().parents[2]
SCULPTO = ROOT / "target" / "debug" / "sculpto"

# A fake `sculpto`: prints the envelope from $FAKE_ENVELOPE and exits with
# $FAKE_EXIT, so the bridge's subprocess handling is exercised for real.
STUB = """#!{}
import os, sys
sys.stdout.write(os.environ["FAKE_ENVELOPE"])
sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
""".format(sys.executable)

OK_ENVELOPE = '{"ok": true, "result": {"vertices": 4, "triangles": 2}}\n'
ERROR_ENVELOPE = '{"ok": false, "error": {"kind": "not_found", "code": "shape", "message": "no shape"}}\n'


class _Recorder:
    """Operator stand-in: captures every report call for exact assertions."""

    def __init__(self):
        self.reports = []

    def report(self, level, message):
        self.reports.append((set(level), message))


class BridgeTest(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory(prefix="sculpto-bridge-test-")
        self.addCleanup(self.work.cleanup)
        stub = Path(self.work.name) / "fake-sculpto"
        stub.write_text(STUB)
        stub.chmod(stub.stat().st_mode | stat.S_IEXEC)
        self.stub = str(stub)

    def tearDown(self):
        for name in ("FAKE_ENVELOPE", "FAKE_EXIT", "SCULPTO_BIN"):
            os.environ.pop(name, None)

    def test_run_argv_is_exact(self):
        argv = cli_bridge.build_run_argv(
            "/opt/sculpto", "export.obj", {"nif": "a.nif", "out": "b.obj"}
        )
        self.assertEqual(
            argv,
            ["/opt/sculpto", "run", "--cmd", "export.obj", "--input", '{"nif": "a.nif", "out": "b.obj"}'],
        )

    def test_run_argv_without_payload_omits_input(self):
        self.assertEqual(
            cli_bridge.build_run_argv("sculpto", "version", None),
            ["sculpto", "run", "--cmd", "version"],
        )

    def test_payload_builders_carry_the_documented_keys(self):
        self.assertEqual(
            cli_bridge.export_obj_payload("a.nif", "b.obj"),
            {"nif": "a.nif", "out": "b.obj"},
        )
        self.assertEqual(
            cli_bridge.export_obj_payload("a.nif", "b.obj", shape="S"),
            {"nif": "a.nif", "out": "b.obj", "shape": "S"},
        )
        self.assertEqual(
            cli_bridge.export_nif_payload("a.nif", "b.nif", "le"),
            {"nif": "a.nif", "out": "b.nif", "target": "le"},
        )
        self.assertEqual(
            cli_bridge.bake_payload("p.osp", "s.xml", "Set", "Preset", "big", "o.obj", home="h"),
            {
                "project": "p.osp",
                "presets": "s.xml",
                "set": "Set",
                "preset": "Preset",
                "endpoint": "big",
                "out": "o.obj",
                "home": "h",
            },
        )
        self.assertEqual(
            cli_bridge.fit_payload("a.nif", "G", "B", "body.obj", "fit.obj"),
            {"nif": "a.nif", "shape": "G", "body_shape": "B", "body": "body.obj", "out": "fit.obj"},
        )

    def test_parse_envelope_returns_the_result_member(self):
        self.assertEqual(
            cli_bridge.parse_envelope(OK_ENVELOPE), {"vertices": 4, "triangles": 2}
        )

    def test_parse_envelope_raises_the_engine_error(self):
        with self.assertRaises(cli_bridge.CommandError) as raised:
            cli_bridge.parse_envelope(ERROR_ENVELOPE)
        self.assertEqual(raised.exception.kind, "not_found")
        self.assertEqual(raised.exception.code, "shape")
        self.assertEqual(raised.exception.message, "no shape")

    def test_parse_envelope_rejects_broken_stdout(self):
        for garbage in ("", "not json at all", '{"no_envelope": 1}', "[1, 2]"):
            with self.assertRaises(cli_bridge.EnvelopeError):
                cli_bridge.parse_envelope(garbage)

    def test_run_parses_a_real_success_envelope(self):
        os.environ["FAKE_ENVELOPE"] = OK_ENVELOPE
        result = cli_bridge.SculptoCli(binary=self.stub).run("export.obj", {"nif": "x"})
        self.assertEqual(result["vertices"], 4)

    def test_run_maps_an_error_envelope_to_the_engine_error(self):
        os.environ["FAKE_ENVELOPE"] = ERROR_ENVELOPE
        os.environ["FAKE_EXIT"] = "2"
        with self.assertRaises(cli_bridge.CommandError) as raised:
            cli_bridge.SculptoCli(binary=self.stub).run("export.obj", {})
        self.assertEqual(raised.exception.exit_code, 2)
        self.assertEqual(raised.exception.kind, "not_found")

    def test_run_rejects_an_ok_envelope_with_a_failure_exit(self):
        os.environ["FAKE_ENVELOPE"] = OK_ENVELOPE
        os.environ["FAKE_EXIT"] = "1"
        with self.assertRaises(cli_bridge.EnvelopeError):
            cli_bridge.SculptoCli(binary=self.stub).run("version", None)

    def test_run_cli_reports_the_engine_kind_and_message(self):
        os.environ["FAKE_ENVELOPE"] = ERROR_ENVELOPE
        os.environ["FAKE_EXIT"] = "2"
        recorder = _Recorder()
        result = cli_bridge.run_cli(
            recorder, "export.obj", {}, cli=cli_bridge.SculptoCli(binary=self.stub)
        )
        self.assertIsNone(result)
        self.assertEqual(recorder.reports, [({"ERROR"}, "not_found: no shape")])

    def test_run_cli_reports_a_broken_envelope_without_raising(self):
        os.environ["FAKE_ENVELOPE"] = "not json at all"
        recorder = _Recorder()
        result = cli_bridge.run_cli(
            recorder, "version", None, cli=cli_bridge.SculptoCli(binary=self.stub)
        )
        self.assertIsNone(result)
        self.assertEqual(len(recorder.reports), 1)
        level, message = recorder.reports[0]
        self.assertEqual(level, {"ERROR"})
        self.assertTrue(message.startswith("protocol: "), message)

    def test_run_cli_reports_a_missing_binary_without_raising(self):
        recorder = _Recorder()
        result = cli_bridge.run_cli(
            recorder,
            "version",
            None,
            cli=cli_bridge.SculptoCli(binary=str(Path(self.work.name) / "absent-sculpto")),
        )
        self.assertIsNone(result)
        self.assertEqual(len(recorder.reports), 1)
        level, message = recorder.reports[0]
        self.assertEqual(level, {"ERROR"})
        self.assertTrue(message.startswith("launch: "), message)

    def test_run_cli_returns_the_result_and_reports_nothing_on_success(self):
        os.environ["FAKE_ENVELOPE"] = OK_ENVELOPE
        recorder = _Recorder()
        result = cli_bridge.run_cli(
            recorder, "export.obj", {"nif": "x"}, cli=cli_bridge.SculptoCli(binary=self.stub)
        )
        self.assertEqual(result["vertices"], 4)
        self.assertEqual(recorder.reports, [])

    def test_find_binary_prefers_the_environment(self):
        os.environ["SCULPTO_BIN"] = "/somewhere/sculpto"
        self.assertEqual(cli_bridge.find_binary(root=ROOT), "/somewhere/sculpto")

    @unittest.skipUnless(SCULPTO.is_file(), "target/debug/sculpto is not built")
    def test_real_binary_dispatches_version(self):
        result = cli_bridge.SculptoCli().run("version")
        self.assertTrue(str(result["version"]).startswith("0."), result)


if __name__ == "__main__":
    unittest.main()
