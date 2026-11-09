"""The add-on's rules table: one copy, gaps declared, panel items derived.

F03/F04/F07 seed: the table in
tools/blender_addon/sculpto_garment/rules.py owns every garment decision the
add-on still makes, each labelled with the proposed command id that will own
it. Runs with plain `python3 -m unittest` — no Blender: the module is pure
data. The optional registry case dispatches the real `sculpto commands`
output when `target/debug/sculpto` exists and fails the day a gap id becomes
registered, because the add-on copy behind it must then be deleted, not
doubled.
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

# The panel exposed exactly these choices before the table existed; the one
# table must keep them (F04: one table-driven definition, stable surface).
EXPECTED_ITEMS = (
    ("upper-body", "Upper body", "Tunic, shirt, bodice"),
    ("trousers", "Trousers", "Waist-to-ankle legs"),
    ("footwear", "Footwear", "Boots and shoes (left foot)"),
)


class KindTableTest(unittest.TestCase):
    def test_panel_items_come_from_one_table(self):
        self.assertEqual(tuple(rules.enum_items()), EXPECTED_ITEMS)

    def test_kind_ids_are_unique(self):
        ids = [entry["id"] for entry in rules.GARMENT_KINDS]
        self.assertEqual(len(ids), len(set(ids)))

    def test_every_kind_carries_place_and_region_values(self):
        for entry in rules.GARMENT_KINDS:
            self.assertIn(entry["width_basis"], ("body-width", "height"))
            self.assertGreater(entry["width_factor"], 0.0)
            self.assertIn(entry["hem_from_sole"], (True, False))
            self.assertIsInstance(entry["arm_exclusion"], bool)

    def test_unknown_kind_has_no_rule(self):
        self.assertIsNone(rules.kind("hat"))
        self.assertIsNone(rules.kind(""))

    def test_upper_body_keeps_arms(self):
        self.assertFalse(rules.kind("upper-body")["arm_exclusion"])

    def test_leg_and_foot_kinds_exclude_arms(self):
        for kind_id in ("trousers", "footwear"):
            self.assertTrue(rules.kind(kind_id)["arm_exclusion"])

    def test_placement_fractions_match_the_documented_values(self):
        upper = rules.kind("upper-body")
        self.assertEqual(upper["width_basis"], "body-width")
        self.assertEqual(upper["width_factor"], 1.15)
        self.assertEqual(upper["collar_frac"], 0.87)
        self.assertIsNone(upper["floor_clamp_m"])
        trousers = rules.kind("trousers")
        self.assertAlmostEqual(trousers["width_factor"], 1.1 * 0.42)
        self.assertEqual(trousers["collar_frac"], 0.62)
        self.assertEqual(trousers["floor_clamp_m"], 0.02)
        footwear = rules.kind("footwear")
        self.assertAlmostEqual(footwear["width_factor"], 2.4 * 0.10)
        self.assertTrue(footwear["hem_from_sole"])
        self.assertEqual(footwear["sole_offset_m"], 0.01)


class RuleGapTest(unittest.TestCase):
    def test_offsets_are_the_documented_constants(self):
        self.assertEqual(rules.GARMENT_OFFSET, 0.008)
        self.assertEqual(rules.BOUNDARY_RELIEF, 0.004)

    def test_projection_miss_snaps_to_the_documented_uv(self):
        self.assertEqual(rules.PROJECT_MISS_UV, (0.5, 0.5))
        self.assertEqual(rules.PROJECT_AXIS, (0.0, -1.0, 0.0))

    def test_region_majority_and_arm_limit_are_named(self):
        self.assertEqual(rules.REGION_MAJORITY, 2)
        self.assertEqual(rules.REGION_ARM_LIMIT_M, 0.21)

    def test_front_placement_constants_are_named(self):
        self.assertEqual(rules.PLACE_FRONT_GAP_M, 0.05)
        self.assertEqual(rules.PLACE_FRONT_RISE_FRAC, 0.02)

    def test_gap_register_covers_every_pending_rule(self):
        self.assertEqual(
            set(rules.RULE_GAPS),
            {
                "garment.offset",
                "garment.place",
                "garment.region",
                "garment.project",
                "garment.weights",
            },
        )

    @unittest.skipUnless(SCULPTO.is_file(), "target/debug/sculpto is not built")
    def test_gap_ids_are_still_absent_from_the_registry(self):
        out = subprocess.run(
            [str(SCULPTO), "commands", "--json"],
            capture_output=True,
            text=True,
            check=True,
        )
        registered = {row["id"] for row in json.loads(out.stdout)["commands"]}
        self.assertFalse(
            set(rules.RULE_GAPS) & registered,
            "a gap id is now registered: delete the add-on copy behind it",
        )


if __name__ == "__main__":
    unittest.main()
