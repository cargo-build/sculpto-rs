"""The add-on's class table: identity only, every rule id registered.

F03 moved the five garment rule groups into the core
(`crates/sculpto-mesh/src/garment.rs`) behind `outfit.offsets`,
`outfit.place`, `outfit.band`, `outfit.projection` and `outfit.weights`.
`rules.py` now carries what the panel needs to name a class and nothing
numeric; the optional registry case dispatches the real `sculpto commands`
output and fails if an id disappears, because the add-on would then have to
carry the rule again.

Runs with plain `python3 -m unittest` — no Blender: the module is pure data.
"""

import importlib.util
import json
import subprocess
import unittest
from pathlib import Path

RULES = (
    Path(__file__).resolve().parents[1]
    / "blender_addon"
    / "sculpto_garment"
    / "rules.py"
)
spec = importlib.util.spec_from_file_location("sculpto_rules", RULES)
rules = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rules)

ROOT = Path(__file__).resolve().parents[2]
SCULPTO = ROOT / "target" / "debug" / "sculpto"

# The panel exposed exactly these choices before the rules moved; the one table
# must keep them (F04: one table-driven definition, stable surface).
EXPECTED_ITEMS = (
    ("upper-body", "Upper body", "Tunic, shirt, bodice"),
    ("trousers", "Trousers", "Waist-to-ankle legs"),
    ("footwear", "Footwear", "Boots and shoes (left foot)"),
)

# The rule groups the dress flow dispatches, and the pipeline step behind each.
EXPECTED_COMMANDS = {
    "outfit.offsets": "surface offsets and boundary relief (build_garment)",
    "outfit.place": "class placement fractions (place_raw)",
    "outfit.band": "band cut with arm exclusion (select_region)",
    "outfit.projection": "projection fill and miss snap (project_texture)",
    "outfit.weights": "weight transfer and unpose inversion (wire_animation)",
}


class KindTableTest(unittest.TestCase):
    def test_panel_items_come_from_one_table(self):
        self.assertEqual(tuple(rules.enum_items()), EXPECTED_ITEMS)

    def test_kind_ids_are_unique(self):
        ids = [entry["id"] for entry in rules.GARMENT_KINDS]
        self.assertEqual(len(ids), len(set(ids)))

    def test_the_table_carries_identity_only(self):
        for entry in rules.GARMENT_KINDS:
            self.assertEqual(set(entry), {"id", "label", "tip"})

    def test_unknown_kind_has_no_rule(self):
        self.assertIsNone(rules.kind("hat"))
        self.assertIsNone(rules.kind(""))

    def test_no_numeric_garment_rule_is_left_in_the_table(self):
        numbers = {
            name: value
            for name, value in vars(rules).items()
            if not name.startswith("_")
            and isinstance(value, (int, float))
            and not isinstance(value, bool)
        }
        self.assertEqual(numbers, {}, "a numeric garment rule returned to the add-on")


class RuleCommandTest(unittest.TestCase):
    def test_commands_cover_every_pipeline_step(self):
        self.assertEqual(rules.RULE_COMMANDS, EXPECTED_COMMANDS)

    def test_command_values_name_the_pipeline_function(self):
        for description in rules.RULE_COMMANDS.values():
            self.assertTrue(description.endswith(")"), description)

    @unittest.skipUnless(SCULPTO.is_file(), "target/debug/sculpto is not built")
    def test_rule_ids_are_registered_in_the_core(self):
        out = subprocess.run(
            [str(SCULPTO), "commands", "--json"],
            capture_output=True,
            text=True,
            check=True,
        )
        registered = {row["id"] for row in json.loads(out.stdout)["commands"]}
        missing = set(rules.RULE_COMMANDS) - registered
        self.assertFalse(
            missing,
            f"a rule id is no longer registered: {sorted(missing)}; the add-on "
            "would have to carry the numbers again",
        )


if __name__ == "__main__":
    unittest.main()
