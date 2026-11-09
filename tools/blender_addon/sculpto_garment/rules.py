"""The add-on's garment class table: identity only, rules live in the core.

DECL0002 puts every measure/normalise/decide rule behind a command id. The five
rule groups the dress flow needs are registered commands now, implemented in
`crates/sculpto-mesh/src/garment.rs`:

    outfit.offsets     surface offsets and boundary relief  (build_garment)
    outfit.place       class placement fractions            (place_raw)
    outfit.band        band cut with arm exclusion          (select_region)
    outfit.projection  projection fill and miss snap        (project_texture)
    outfit.weights     weight transfer and unpose inversion (wire_animation)

This module keeps what the panel needs to *name* a class and nothing numeric:
the id, the label and the tooltip. `pipeline.py` calls the commands; this module
must stay `bpy`-free so the tests run with plain python3.
"""

# The action both panel operators preselect.
DEFAULT_CLIP = "Combat_Idle"

# One row per garment class the add-on exposes. The panel's enum items and the
# per-class rule parameters both derive from the row's id: the numbers belong to
# the core, keyed by the same string (F04 owns the shared panel driver).
GARMENT_KINDS = (
    {
        "id": "upper-body",
        "label": "Upper body",
        "tip": "Tunic, shirt, bodice",
    },
    {
        "id": "trousers",
        "label": "Trousers",
        "tip": "Waist-to-ankle legs",
    },
    {
        "id": "footwear",
        "label": "Footwear",
        "tip": "Boots and shoes (left foot)",
    },
)

# The rule groups the add-on dispatches, and the pipeline step that calls each.
RULE_COMMANDS = {
    "outfit.offsets": "surface offsets and boundary relief (build_garment)",
    "outfit.place": "class placement fractions (place_raw)",
    "outfit.band": "band cut with arm exclusion (select_region)",
    "outfit.projection": "projection fill and miss snap (project_texture)",
    "outfit.weights": "weight transfer and unpose inversion (wire_animation)",
}


def kind(kind_id):
    """The table row for `kind_id`, or None when the add-on does not expose it."""
    return next((entry for entry in GARMENT_KINDS if entry["id"] == kind_id), None)


def enum_items():
    """(identifier, name, description) triples for EnumProperty items."""
    return [(entry["id"], entry["label"], entry["tip"]) for entry in GARMENT_KINDS]
