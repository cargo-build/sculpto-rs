"""The add-on's one decision table, awaiting its command ids (F03).

DECL0002 puts every measure/normalise/decide rule behind a command id. The
registry (`crates/sculpto-engine/src/lib.rs` `COMMANDS`) closes over version,
commands list, bake, outfit.fit and the export commands — none of the garment
rules the dress flow needs — so the values stay here, named once, each entry
labelled with the proposed id that will own it. `pipeline.py` consumes the
table and keeps only Blender data construction; the ids are proposed, not
registered, and `tools/tests/test_sculpto_rules.py` fails the day one
appears, prompting the add-on copy's deletion instead of a second owner.

This module must stay `bpy`-free: the tests run with plain python3.
"""

# The action both panel operators preselect.
DEFAULT_CLIP = "Combat_Idle"

# Gap: `garment.offset` — clearance the reconstructed surface keeps from the
# body (pipeline.build_garment), meters.
GARMENT_OFFSET = 0.008
# Gap: `garment.offset` — extra clearance at boundary rings so openings do
# not cut into the body when the mesh deforms with the armature, meters.
BOUNDARY_RELIEF = 0.004

# Gap: `garment.region` — a face stays in the band when at least this many
# of its vertices sit between the hem and collar lines
# (pipeline.select_region).
REGION_MAJORITY = 2
# Gap: `garment.region` — faces whose mean |x| sits beyond this many meters
# from the sagittal plane drop for arm-excluding kinds (hanging arms).
REGION_ARM_LIMIT_M = 0.21

# Gap: `garment.project` — UV assigned to ray-cast misses, and the
# world-space cast direction (front relief faces -y).
PROJECT_MISS_UV = (0.5, 0.5)
PROJECT_AXIS = (0.0, -1.0, 0.0)

# Gap: `garment.place` — the raw sits this far in front of the body's front
# surface, lifted by this height fraction (pipeline.place_raw).
PLACE_FRONT_GAP_M = 0.05
PLACE_FRONT_RISE_FRAC = 0.02

# One row per garment class the add-on exposes: the panel's enum items and
# the placement/region decisions derive from the same entry (F04).
GARMENT_KINDS = (
    {
        "id": "upper-body",
        "label": "Upper body",
        "tip": "Tunic, shirt, bodice",
        # Gap: `garment.place` — relief width (incl. sleeves) scales to
        # 1.15 x the reference body width; collar at 0.87 x height; hem
        # follows the raw's own proportions.
        "width_basis": "body-width",
        "width_factor": 1.15,
        "collar_frac": 0.87,
        "hem_from_sole": False,
        "sole_offset_m": None,
        "floor_clamp_m": None,
        # Gap: `garment.region` — upper kinds keep arm faces.
        "arm_exclusion": False,
    },
    {
        "id": "trousers",
        "label": "Trousers",
        "tip": "Waist-to-ankle legs",
        # Gap: `garment.place` — width 1.1 x (0.42 x height, hip width
        # approx); collar at 0.62 x height; hem may not pass the floor.
        "width_basis": "height",
        "width_factor": 1.1 * 0.42,
        "collar_frac": 0.62,
        "hem_from_sole": False,
        "sole_offset_m": None,
        "floor_clamp_m": 0.02,
        # Gap: `garment.region` — the band overlaps hanging arms.
        "arm_exclusion": True,
    },
    {
        "id": "footwear",
        "label": "Footwear",
        "tip": "Boots and shoes (left foot)",
        # Gap: `garment.place` — sole width 2.4 x (0.10 x height, one foot
        # sole length, generous); sole rests on the floor and the shaft
        # rises by the raw's own proportions.
        "width_basis": "height",
        "width_factor": 2.4 * 0.10,
        "collar_frac": None,
        "hem_from_sole": True,
        "sole_offset_m": 0.01,
        "floor_clamp_m": None,
        # Gap: `garment.region` — the band overlaps hanging arms.
        "arm_exclusion": True,
    },
)

# F03 gap register: proposed command id -> the rule it will own. No row is
# registered today (`crates/sculpto-engine/src/lib.rs` `COMMANDS`).
RULE_GAPS = {
    "garment.offset": "surface offsets (build_garment)",
    "garment.place": "class placement fractions (place_raw)",
    "garment.region": "band cut with arm exclusion (select_region)",
    "garment.project": "texture projection fill and miss snap (project_texture)",
    "garment.weights": "nearest-vertex weight transfer and unpose inversion (wire_animation)",
}


def kind(kind_id):
    """The table row for `kind_id`, or None when the add-on does not expose it."""
    return next((entry for entry in GARMENT_KINDS if entry["id"] == kind_id), None)


def enum_items():
    """(identifier, name, description) triples for EnumProperty items."""
    return [(entry["id"], entry["label"], entry["tip"]) for entry in GARMENT_KINDS]
