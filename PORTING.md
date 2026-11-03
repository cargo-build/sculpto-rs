# Porting notes: source-guided control construction

The frozen prototype (`construct_upper_control.py`, `verify_physical_boundaries.py`
under `/home/wave/Models/sculpto-upper-physical-boundaries-01a19e92`) is
characterization evidence: it shows what the rule is and what it produced. This
document records what the Rust implementation kept, what it deliberately changed,
and what it verified.

## API mapping

| Prototype | Rust |
|---|---|
| `bmesh.ops.weld_verts` on `tuple(v.co)` | `control::SourceSurface::identify` (exact-position topology, source lineage via `topology::position_topology`) |
| `closest_pt_on_seg` + loop over `skeleton_segments` | `control::GuideSegment::{closest}` and `control::Guide::nearest` (validated segments, first minimum wins) |
| `f.normal.dot(radial) > 0.0` face filter | `control::exterior_triangles` (winding normal, radial from the guide) |
| component BFS over `e.link_faces`, keep largest | `control::largest_component` (union-find over shared edges, deterministic tie-break) |
| `clean_non_manifold_tips` | `control::non_manifold_tips` (keeps the largest face fan, returns the rest) |
| `find_boundary_loops` | `control::boundary_loops` (ordered walk, `closed` flag, stable order) |
| "the four largest loops are apertures" | `control::resolve_apertures` against declared profile anchors and radii |
| `bmesh.ops.triangle_fill` / `holes_fill` / `collapse` | `RepairAction::{Fill, Collapse}` planned in core, executed by the adapter |
| `target_budget / current_tris` | `control::ReductionPlan::plan` plus `ReductionLimits` bounds |
| `compute_deviations` (one direction) | `control::measure::{deviation, deviations}` (both directions, coverage at the declared tolerance) |
| `is_manifold`, chest ray, corridor anchors | `control::measure::ControlDiagnostics::qualify` (manifold disk, edges, components, apertures, Euler, contacts, coincident vertices) |

`intake::strip_interior` keeps its own API and tests. It solves a different
problem: one body-derived axis with nested section bands after registration. The
multi-axis source guide has to act before registration, so sharing it would have
changed behavior. The one primitive that did move is `topology::position_topology`,
which now returns `Identical { positions, triangles, vertex_sources }`; the extra
field is provenance, and the existing callers' behavior is unchanged.

## Conformance repairs after primary review

Primary reproduced three acceptance defects and a lineage omission. All four are
fixed here, and each is covered by a test that runs the real CLI boundary:

1. **Final triangle bounds are enforced.** `ReductionPlan::verify` (which nothing
   called) is removed; `ControlDiagnostics::qualify` now rejects a control outside
   `reduction.minimum_triangles … maximum_triangles`, so `control-check` and the
   native consumer share one enforcement path.
2. **Source-stage closure limits are enforced before native work.** `repair` is
   replaced by `source_repair` and `reduced_repair` (no compatibility alias);
   `control-exterior` checks the source-stage loops with the same rule the reduced
   stage uses and returns limit-checked `actions`, which the adapter executes
   instead of filling every incidental loop. Declared source limits are the measured
   operations: Archer 4 loops / 16 edges, Crossbowman 43 loops / 164 edges; the
   reduced stage keeps 4 / 10 / 64.
3. **Directed winding is part of qualification.** `ControlDiagnostics` counts edges
   whose two incident faces traverse them in the same direction and `qualify`
   rejects them. Generated patches are oriented against the retained boundary after
   each fill (the adapter reports the flip count); no retained source face is ever
   flipped to hide an invalid input.
4. **Lineage is complete.** `ExteriorOutcome` carries `selected_faces` (one source
   face per returned triangle) and `removals` (source face + reason, grouped in the
   plan), and the plan serializes every `vertex_sources` entry instead of an
   eight-value preview. QEM output is recorded as an operation with pre/post stage
   digests and counts; it is never labelled as original faces.

## Bounded local contact separation

`sculpto_mesh::control::contact` owns the mechanism; `garment control-separate` is a
thin adapter. Witness detection is `check::self_intersection_witnesses`, which is
also what `self_intersection_pairs` counts, so listing and counting cannot drift.
The policy is profile data (`reject`, or `separate` with clearance, maximum
displacement and maximum pair count). A separation refuses duplicate faces,
coupled or excessive pairs, a required translation beyond the bound, an incident
face that would invert or collapse, and any residual contact. The measured Archer
case uses clearance 2e-6 with a 1e-4 displacement bound; the resulting translation
was 6.954e-5 m (67.5 µm overlap + 2 µm clearance) moving 4 vertices.

## Deliberate differences

1. **Apertures are profile data, not "the four largest loops".** Each declared
   role has an anchor and radius. Two loops inside one radius are
   `AmbiguousAperture`; a declared opening with no loop is `UnknownAperture`. The
   prototype filled everything past the fourth loop, so a split real opening would
   have been closed as debris.
2. **No silent exception suppression.** Loop closure tries `triangle_fill`, then
   `holes_fill`; both messages are retained and a double failure raises. The
   prototype's `except: pass` chain could report success after closing nothing.
3. **No dummy-body witness.** The prototype wrote a 0.01 m sphere far from the
   garment so `sculpto garment check`/`intake` would accept a "body". That is
   manufactured whole-body evidence, and the port does not reproduce it. Control
   self-validation (`ControlDiagnostics`) needs no body; a real body check stays
   with the fit backend.
4. **No Laplacian relaxation fallback.** The prototype ran up to 15 iterations of
   whole-mesh Laplacian displacement on self-contact witnesses. The port refuses:
   contacts terminate qualification with retained diagnostics. This is not a
   cosmetic difference — see the archer finding below.
5. **Corner-flare pruning is bounded and reported.** `max_tip_removal_share` is
   profile data; the prototype removed flares with no limit.
6. **Reduction is plan-plus-bounds.** The ratio is computed from the high control
   and profile bounds; the result must land between `minimum_triangles` and
   `maximum_triangles`, so a decimation that silently under-reduced fails.
7. **The checker binary is recipe data.** The prototype hardcoded one absolute
   path to `sculpto`.
8. **Explicit validation instead of derived flags.** `is_qualified` in the
   prototype was a conjunction of Python checks including chest ray hits and
   corridor anchors at fixed coordinates. The port keeps the model-independent
   part (manifold disk, one component, exact opening set, Euler characteristic,
   contacts, coincident vertices, bidirectional coverage) and drops the
   model-specific ray/corridor probes, which belong to a body-fitted check that
   this stage cannot honestly perform without a body.
9. **One exterior rule.** The prototype's constructor used `dot > 0.0` while its
   verifier classified source exterior vertices with `normalized().dot(...) > 0.05`.
   The port uses the single construction rule for both, so the measured source
   coverage set is not identical to the frozen verifier's.
10. **Failures write diagnostics first.** `control-check` writes its report and
    only then returns the failure; the adapter keeps the receipt with
    `status: "failed; diagnostics retained"`.

## Preserved quirks

- A face exactly perpendicular to the radial direction (`dot == 0`) is dropped,
  as in the prototype's strict `> 0.0` comparison.
- A welded triangle with two coincident corners carries no surface and is dropped
  before classification; the count is reported as `welded_degenerate_removed`.
- Component ties keep the lowest first triangle; loop ties keep the lowest vertex.
- Aperture matching assigns in profile order and consumes each loop once.

## Verification

| Check | Command | Result |
|---|---|---|
| Core rule, invalid/ambiguous input, translation/scale invariance, self-contact rejection | `cargo test -p sculpto-mesh --test control` | 18 passed |
| Profile parsing and frame conversion | `cargo test -p sculpto-mesh --test control_profile` | 9 passed |
| CLI decisions and gates | `cargo test -p sculpto-cli --test garment_control` | 5 passed |
| Native adapter (positive, source-limit refusal, closed-loop fill with orientation, other name/scale, determinism, exclusive output, digest mismatch, wrong anchor) | `blender -b --factory-startup -t 2 --python-exit-code 1 --python tools/tests/test_construct_garment_control.py` | exit 0, `NATIVE_CONTROL_REGRESSION ok` (`logs/native-regression.log`) |
| Contact separation (clean no-op, one crossing, bound refusal, coupled pairs, reject policy, duplicate faces, transformed/reindexed geometry) | `cargo test -p sculpto-mesh --test contact` | 9 passed |
| Source gate (`fmt --check`, clippy `-D warnings`, workspace tests) | `just gate` | exit 0, 32 test binaries ok (`logs/source-gate.log`) |

Matched source/control views are rendered before `control-check`, so a rejected
control keeps its review images next to the diagnostics.

### Pinned garment results

Both pinned objects were run through the same implementation and profile schema.

- **humans/crossbowman `HC3_green-tunic`**: accepted. High control 71632v/142030f
  (identical to the frozen receipt), openings 618/246/200/174 edges before
  reduction, reduced control 1064v/1810f, four apertures 173/57/46/46 edges,
  Euler −2, one manifold component, zero coincident vertices, zero self contacts.
  Deviations: control→source mean 0.215 mm, p95 0.523 mm; source exterior→control
  mean 0.217 mm, p95 0.505 mm, coverage 99.96% at 5 mm.
- **elves/archer `archer-tunic-front`**: **rejected** with retained diagnostics.
  High control 15805v/31084f and reduced 1055v/1799f reproduce the frozen receipt
  exactly, including openings 210/41/34/30. `control-check` reports one
  nonadjacent triangle pair intersecting at rest, so qualification stops. The
  frozen "PASS" is not reproducible without the removed fallback: its own receipt
  records `self_pairs: 1` at iteration 0 and a 1.902 mm whole-mesh Laplacian
  displacement before the second iteration reports zero. Replaying the frozen
  script against the current CLI reproduces the same numbers
  (`replay/archer/receipt.json`). The declared policy — no smoothing ladder,
  explicit failure with diagnostics — is what the port does.

Reduced topology is not the frozen topology: 1064/1810 against 1058/1798 for the
crossbowman, and the face/vertex order changed. Any binding built on the old
arrays is invalid and must be rebuilt; the reference comparison in the receipt
shows mean disagreement 7.7 µm and maximum 1.04 mm against the frozen crossbowman
control.

## Limitations

- A positive route exists for one of the two pinned shells only. The second shell
  needs a decision about self-contacts that this assignment may not invent:
  either a measured local repair with its own acceptance evidence, or accepting a
  reconstruction that the frozen pipeline only qualified through smoothing.
- Coverage and aperture anchors are garment-profile data. A new garment needs its
  own guide and landmarks; nothing here claims an unseen garment, other garment
  categories or whole-outfit readiness.
- The control is a reconstruction, not a fit. No downstream fitting, cloth
  dynamics, material transfer or runtime export was exercised.
