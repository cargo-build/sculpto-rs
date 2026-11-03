# Sculpto

GPL-3.0-only Rust authoring tools for character bodies and outfits. The pure
core reads BodySlide data, bakes morphs, imports NIF meshes, prepares and fits
garment surfaces, evaluates skeletal GLB actors and packs baked cloth motion.
Blender, Bevy and Unity adapters are optional; building the core requires none
of them. The complete raw-garment-to-finished-outfit workflow is experimental.

## Build and use

Rust 1.99 or newer, edition 2024:

```sh
cargo build -j 2
cargo test -j 2
just gate
cargo run -p sculpto-cli -- help
```

Examples with explicitly supplied local assets:

```sh
target/debug/sculpto actor sample --input actor.glb --part BaseShape --rest --out body.obj
target/debug/sculpto actor rig --input actor.glb --part BaseShape --part ArcherTrousers \
  --joint "NPC COM [COM ]" --joint "NPC L Hand [LHnd]" --rest --out rig.json
target/debug/sculpto actor pack-cloth --input actor.glb --targets targets.glb \
  --node TunicSimulationRender --clip Run --schedule schedule.json --out dressed.glb
```

Actor sampling distinguishes asset defaults from animation time zero and
preserves explicit part/primitive identity. Cloth packing preserves the
actor's other geometry, materials and skeletal animation; it does not qualify
fit, collisions or periodic motion. The current packing consumer supports
2–256 targets. Base textures must be embedded or self-contained data URIs.

`actor rig` samples the same explicit parts/pose and additionally reports the
reduced skin correspondence used by fitting adapters:

```text
sculpto actor rig --input actor.glb --part NAME [--part NAME ...] \
  --joint NAME [--joint NAME ...] (--rest | --clip NAME --time SECONDS) --out rig.json
```

The JSON output is one document in glTF Y-up world space with f64 values:
`pose`, combined `vertices`/`triangles`, per-primitive `parts` ranges, the
retained `rig` nodes (original glTF node id, name, rig-local `parent`,
`requested`/`connector` role and column-major `world_column_major` transform
from the same pose evaluation as the vertices), the caller-ordered
`requested` name→node→rig mapping and sparse per-vertex `influences`
(`bindings` with retained `rig` index and the original accumulated `weight`;
zero-weight source influences are omitted and weights are never
renormalized). Requested joints are exact unique node names, resolved in the
real node graph; the retained rig keeps the hierarchy paths connecting them
through their lowest common ancestor, ordered parent-before-child. Selections
spanning different roots, influences without a retained ancestor, unskinned
selected parts and ambiguous/repeated/missing joint names are explicit
errors. The known subset matches `actor sample` (triangles, one
JOINTS_0/WEIGHTS_0 influence set, embedded dense accessors); validation runs
before any output file is created, and the output file is overwritten like
`actor sample`'s OBJ/report (not an atomic transaction).

Real-data tests are optional and read an explicitly supplied private fixture:

```sh
SCULPTO_BODYSLIDE_HOME=/path/to/BodySlide cargo test -p sculpto-core --test real_data
```

## Layout

| Path | Responsibility |
|---|---|
| `crates/sculpto-core` | BodySlide document and morph-container formats |
| `crates/sculpto-bake` | Deterministic morph application |
| `crates/sculpto-nif` | NIF import boundary |
| `crates/sculpto-mesh` | Geometry, fitting, actor motion and GLB packing |
| `crates/sculpto-cli` | Command-line client |
| `tools/` | Optional host adapters, diagnostics and source metrics |
| `vendor/byroredux` | Pinned dependency subset with its own notices |

The Blender slider panel takes explicit `bodies=...` and `garments=...`
catalog paths. Private body, garment, texture and animation assets are not
distributed with this repository. Generated models and renders belong in a
separate output directory.

### Bake original garment materials onto a control

The optional native Blender adapter accepts a digest-pinned original extracted
`.blend` object and a control NPZ containing `vertices` (Nx3) and ordered
`triangles` (Mx3). Bake in the original garment frame before fitting:

```sh
blender --background --factory-startup -t 2 --python-exit-code 1 \
  --python tools/bake_garment_materials.py -- recipe.json /new/output-directory
```

Example recipe (paths are explicitly supplied local fixtures):

```json
{
  "source": "/assets/original-parts.blend",
  "source_sha256": "<SHA256 of original-parts.blend>",
  "object": "OriginalGarment",
  "control": "/assets/control.npz",
  "control_sha256": "<SHA256 of control.npz>",
  "control_frame": "sculpto-source-local-y-up",
  "resolution": 2048,
  "margin": 8,
  "frame_max_distance": 0.002,
  "cage_extrusion": 0.0015,
  "max_ray_distance": 0.003
}
```

`sculpto-source-local-y-up` means control `(x,y,z)` maps to source-local Blender
`(x,-z,y)`, then through the original object's actual world matrix.
`blender-source-local-z-up` uses source-local Blender coordinates directly;
`blender-world-z-up` uses explicit Blender world coordinates;
`sculpto-world-y-up` maps world `(x,y,z)` to Blender world `(x,-z,y)` without
applying the source object's transform. Ray distances and
the required nearest-surface frame tolerance use Blender world units. Establish
the frame from the source recipe and transforms; proximity alone cannot prove
it. Shell inner walls can catch rays even when coverage is complete.

The supported source graph is a static, opaque direct Principled surface with
constant or UV-image base color, roughness and metallic; RGB Separate Color is
supported for packed scalar maps. A tangent Normal Map may feed Normal.
Other shader graphs, linked UV transforms, transparency, volume, displacement,
coat, transmission, subsurface, sheen, anisotropy and emission reject explicitly.
Other constant Principled settings are preserved; conflicting values across
source materials reject because one target shader cannot represent them. Source
images must be packed or readable at their original paths. The source file and
its material datablocks are retained; temporary graph edits affect copies.

Output contains packed `baked-control.blend`, four PBR PNG maps, native white
`footprint.png`/`coverage.png` diagnostic passes and `receipt.json`. Coverage
counts uncovered texels **inside the actual native UV footprint**, with zero
diagnostic margin. The receipt records transforms, source/control/image hashes,
ordered topology and UV identities, settings, timings and native ray witnesses.
Measured misses remain reported; a produced bake does not qualify appearance,
fit, body clearance or animation. Failed bake diagnostics remain in their new
output directory. Existing output directories are rejected.

Optional `fitted` and `fitted_sha256` fields require an explicit `fitted_frame`
from the same four frame names. Actor-world Y-up fitted output uses
`sculpto-world-y-up`; it must not inherit the original source object's transform.
The adapter requires exactly the same vertex count and ordered triangles,
bakes on the original source-frame positions, then applies the fitted positions
and their declared frame while retaining UVs and maps. It performs no fit or
geometry repair.

Run the focused native regression with:

```sh
blender --background --factory-startup -t 2 --python-exit-code 1 \
  --python tools/tests/test_bake_garment_materials.py
```

### Reconstruct a source-guided control surface

A source may be a thick sheet with lining and sealed rims, or a capped solid.
The profile selects guide-facing extraction (`surface: "guide"`) or retention
of an identified solid boundary (`surface: "boundary"`). The latter preserves
concave folds and may need explicit opening cuts. Rust validates the profile
and construction plans; the Blender adapter performs native cuts and mesh
operations while preserving the original source.

The full adapter command below applies optional cuts before extraction. These
individual stage commands expect the appropriate prepared input:

```sh
sculpto garment control-cuts --profile profile.json --report cut-plan.json
sculpto garment control-exterior --source shell.obj --profile profile.json \
  --out selected.obj --plan exterior-plan.json
sculpto garment control-repair --control decimated.obj --profile profile.json \
  --out repaired.obj --plan repair-plan.json
sculpto garment control-separate --control repaired.obj --profile profile.json \
  --out reduced.obj --report separation.json
sculpto garment control-check --control reduced.obj --source shell.obj \
  --profile profile.json --report validation.json
```

`control-cuts` validates and reports planes; it does not itself modify geometry.
Its `plane_frame` is always `source-local-z-up`, while `declared_frame` records
the profile's input frame. The host rejects an unexpected plane frame.
`control-exterior` applies the declared surface policy, keeps the largest connected shell, prunes
corner-flare faces at pinch vertices, names the profile's apertures, and returns
the source-stage closure actions *after* checking them against `source_repair`.
Each fill includes a required `closure` kind: an existing interior edge between
nonadjacent rim vertices selects a boundary patch instead of free chords.
The native adapter captures all generated triangles, propagates source winding,
and verifies the complete patch. A rejected attempt is rolled back before one
recorded alternative operator; failure of both preserves diagnostics and stops.
`control-repair` plans the post-decimation closure of corner flares and incidental
boundary loops against `reduced_repair`. `control-separate` applies the profile's
single bounded local contact separation, if one is declared. `control-check`
measures the finished surface and enforces the acceptance conjunction: one
manifold disk shell, consistent directed winding, exactly the declared apertures,
the genus-0 Euler characteristic, zero coincident vertices, zero residual contacts,
the declared triangle bounds and the declared bidirectional coverage.

The profile is data, not a search space:

```json
{
  "frame": "source-local-z-up",
  "surface": "guide",
  "source_cuts": [],
  "guide_segments": [[[0.0, 0.0, -0.55], [0.0, 0.0, 0.5]]],
  "apertures": [
    {"role": "hem", "anchor": [0.0, 0.0, -0.5], "radius": 0.02},
    {"role": "collar", "anchor": [0.0, 0.0, 0.5], "radius": 0.02}
  ],
  "source_repair": {
    "max_incidental_loops": 4,
    "collapse_below_edges": 0,
    "max_fill_edges": 16,
    "max_tip_removal_share": 0.05
  },
  "reduced_repair": {
    "max_incidental_loops": 4,
    "collapse_below_edges": 10,
    "max_fill_edges": 64,
    "max_tip_removal_share": 0.05
  },
  "reduction": {
    "target_triangles": 1800,
    "minimum_triangles": 1200,
    "maximum_triangles": 1890
  },
  "coverage": {"tolerance": 0.005, "minimum": 0.8},
  "contact": {
    "mode": "separate",
    "clearance": 0.000002,
    "max_displacement": 0.0001,
    "max_pairs": 1
  }
}
```

`source_repair` and `reduced_repair` are separate because the two stages work at
different resolutions; each is checked *before* its native fills or collapses, so
an out-of-limit stage is refused instead of silently expanded. `contact` is either
`mode: "reject"` (any rest contact fails) or `mode: "separate"` with a
finite clearance, a maximum vertex displacement and a maximum pair count, all in
source units. A separation uses one geometry-derived minimum separating axis, moves
the smaller face in full and its immediate neighbours at half strength, and is
refused when the required translation exceeds the bound, an incident face would
invert or collapse, or any contact remains.
Both modes require all four fields. Under `reject`, the three numeric values
are retained in the receipt but no displacement is attempted.

`guide_segments` and every anchor share one declared frame: `source-local-z-up`
(the extracted object's own coordinates) or `sculpto-display-y-up` (the Y-up OBJ
frame). A declared display point `(x, y, z)` converts to source-local
`(x, -z, y)`; the inverse source-to-display map is `(x, z, -y)`. Missing required
fields and unknown fields are errors. Omitted `surface` means `guide`; omitted
`source_cuts` means no cuts. An aperture is a declared garment landmark: two boundary loops inside one
radius are ambiguous, and an opening with no loop is reported — never filled. No
model name, asset path or vertex number is part of the algorithm.
Opening centers use edge-length-weighted midpoints of the complete boundary,
so subdividing the same curve does not change its identity. Role names are
garment-local labels; anatomical sides and facing require separate actor
registration.

For a capped cylindrical source, replace the two policy fields in the example
with explicit cuts that leave the intended passages open:

```json
{
  "surface": "boundary",
  "source_cuts": [
    {"label": "lower opening", "plane_point": [0, 0, -0.5], "normal": [0, 0, 1], "retain": "positive"},
    {"label": "upper opening", "plane_point": [0, 0, 0.5], "normal": [0, 0, 1], "retain": "negative"}
  ]
}
```

Each cut has a unique label, a finite plane point, a finite nonzero normal and
an explicit retained half-space. Normals are normalized independently of their
magnitude. Cuts act on a private derivative; the original source and its maps
remain intact. Source coverage still includes deliberately removed cap areas.

`control-check` writes its report and then fails when the declaration is not met;
the report's `verdict` combines aperture/topology/winding/bounds qualification
with coverage. Diagnostics are always written before the failure is reported.

The native adapter runs the whole stage list for one digest-pinned object:

```sh
blender --background --factory-startup -t 2 --python-exit-code 1 \
  --python tools/construct_garment_control.py -- recipe.json /new/output-directory
```

```json
{
  "source": "/assets/extracted-parts.blend",
  "source_sha256": "<SHA256>",
  "profile": "/assets/profile.json",
  "profile_sha256": "<SHA256>",
  "object": "ExtractedGarment",
  "sculpto": "/build/target/debug/sculpto"
}
```

The adapter triangulates the source read-only, validates and applies any declared
opening cuts, writes the selected input for the CLI, applies the planned triangle
fills, the declared decimation ratio, the planned collapses and
fills, and re-runs `control-check` on the result. Output contains `high_control.{obj,npz}`,
`reduced_control.{obj,npz}`, the two plan files, `separation.json`,
`validation.json`, matched source/control renders and `receipt.json`. NPZ carries
ordered `vertices` (Nx3, display frame) and `triangles` (Mx3). The exterior plan
holds the full lineage the review asked for — every selected source face, every
source vertex per selected vertex, and every removed source face grouped by reason
— and the adapter records each native stage (file digest, counts), each generated
patch (operator, edges, faces, orientation flips), and the QEM record (ratio,
pre/post counts and digests) *without* labelling reduced triangles as original
faces. The receipt also keeps the source object's matrix, source/profile digests,
UV and material identities, physical openings, bidirectional deviations, coverage,
timings and an optional `reference`/`reference_sha256` comparison. `receipt.status` is authoritative: a failed run keeps its
artifacts for diagnosis and its control is not qualified. The source file,
materials and UVs are never modified.

With cuts, `derivative-lineage.json` stores every derivative vertex's known
source triangle and barycentric coordinates, plus every derivative face's source
triangle. Indices are zero-based. The adapter reads these relations back and
composes selected geometry to the original source; complete-face removal and
clipped area are reported separately. Freeze the digest-pinned executable before
rebuilding it if the exact recipe must remain replayable.

Run the focused native regression with:

```sh
cargo build -p sculpto-cli
blender --background --factory-startup -t 2 --python-exit-code 1 \
  --python tools/tests/test_construct_garment_control.py
```

## License and provenance

First-party code is [GPL-3.0-only](LICENSE). Third-party source retains its
own license and notices. This implementation was extracted, without source
loss, from the local wrapper snapshot
`a482b60f449161aea742f72a0cf218fa1ae186ca`; wrapper history retains the preceding
development record. The public repository is
[cargo-build/sculpto-rs](https://github.com/cargo-build/sculpto-rs).

This is an independent implementation repository. The managed wrapper owns
coordination issues, private host catalogs and the source map; those are not
required for standalone builds.
