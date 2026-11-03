"""Corner UVs follow the CLI's first-corner polygon fan."""
import pathlib
import subprocess
import sys
import tempfile
import unittest


class PolygonUvTest(unittest.TestCase):
    def test_quad_retains_each_source_corner(self):
        script = pathlib.Path(__file__).resolve().parents[1] / "merge_uv.py"
        with tempfile.TemporaryDirectory(prefix="sculpto-uv-fan-") as temporary:
            root = pathlib.Path(temporary)
            source = root / "source.obj"
            fitted = root / "fitted.obj"
            output = root / "out.obj"
            positions = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n"
            source.write_text(positions + "vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\n")
            fitted.write_text(positions + "f 1 2 3\nf 1 3 4\n")
            run = subprocess.run([sys.executable, str(script), "--source", str(source), "--fitted", str(fitted), "--out", str(output)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertIn("matched=2 unmatched=0", run.stdout)
            self.assertEqual([line for line in output.read_text().splitlines() if line.startswith("f ")], ["f 1/1 2/2 3/3", "f 1/1 3/3 4/4"])


if __name__ == "__main__":
    unittest.main()
