# Sculpto implementation

Public GPL-3.0-only Rust 2024 engine. Agents author through discoverable headless
commands and structured results. Use the declared toolchain,
own shared behavior in Rust and keep host adapters thin. Keep mod/model payloads
private. Label generated PNGs when publishing geometry review evidence.

## Supplied assets — required first stop

- Start at `~/src/oss/gamedev/assets/`. Read its instructions and
  `from-skyrim-community/README.md` before download or absence claims. Pass resolved
  pack paths to every asset delegate. Bijin and HIMBO Mike are primary fitting targets.
- Supply private host fixtures explicitly. The standalone engine builds and tests
  without the private wrapper or asset payloads.
- Use jj for mutations and `git diff` for reads. Confirm the task bookmark before
  edits/builds. `jj describe` → `jj new`. Land on public `main` after checks.
- `cargo build`, at most two jobs. `just gate` runs fmt, Clippy `-D warnings` and tests.
  Parsing changes also run `just test-real /path/to/BodySlide` on the explicit fixture.
- In `sculpto-rs-wrapper/repos/sculpto-rs`, follow wrapper AGENTS,
  issue, rules and map. Update affected bindings and metrics. Run wrapper `just gate`
  before integration. Concurrent source writers use
  `repos/jj-workspaces/sculpto-rs-<task>` in the wrapper. Workspace name = directory
  basename. Keep the stable build path when possible. Wrapper writes use its primary
  copy and `main`.

## Start here — engine help

```sh
cargo build -p sculpto-cli -j 2
target/debug/sculpto --help
target/debug/sculpto commands --json
target/debug/sculpto run --cmd version
cargo build -p boethos-cli -j 2
target/debug/boethos --help
```

`--help` → argv. `commands --json` → closed registry, schemas, `implemented` and
`mutates`. `run --cmd ID --input JSON` and stdio `mcp` use the registry. For actor,
control and UV coverage inspect [argv](crates/sculpto-cli/src/main.rs) and
[registry](crates/sculpto-engine/src/lib.rs) directly. `boethos` owns the supplied-asset
catalogue. Query it before walking archives.

| Task | Exact entry and boundary |
|---|---|
| BodySlide | `inspect PATH`, `presets`, `sliders`. OSP, presets, OSD/BSD, family and ordered vertex IDs. [Core](crates/sculpto-core/src/lib.rs) |
| Review GLB | `viewer.export` / `sculpto viewer export`. Metres (71.4286 units/m), palette mesh per part. Review only. [Writer](crates/sculpto-gltf/src/review.rs) |
| Shapes | `bake`, `batch`. Set, preset and weight endpoint. [Bake](crates/sculpto-bake/src/lib.rs) |
| Native I/O | `export-obj`, `export-nif`, `export.nif`, `export.bundle`, `export.pair`. Supported subsets. [NIF](crates/sculpto-nif/src/lib.rs) · [GLB](crates/sculpto-gltf/src/lib.rs) |
| Garment intake | `garment fit`, `follow`, `aura`, `classify`, `intake`. Authored topology, rest frame and declared prep. [Mesh](crates/sculpto-mesh/src/lib.rs) |
| Densify | `mesh densify` / `mesh.densify`. Midpoint subdivision, optional boundary-locked smoothing. Preserve parts/boundaries. Measure counts and drift. Diagnostic only. [Source](crates/sculpto-mesh/src/densify.rs) |
| Dress raw garment | `garment dress` / `outfit.dress`. Seam merge → registration → subdivision → contact band → poke repair. Placement can change authored ease. [Source](crates/sculpto-mesh/src/dress.rs) |
| Component seam | `component.seam_fit`. Original vertex targets and same-frame lengths. Preserve render order. Qualify anatomy and skin transfer separately. [Source](crates/sculpto-mesh/src/seam.rs) |
| Geometry checks | `garment check`, `depth`, `cover`, `proxies`, `stack`. Distinguish intersections, local normal-side depth by facing and DECL0015 contact-point cover. Verify containment and render evidence separately. `stack` uses declared `pairs`; disjoint bounds under fallback order require a manifest correction. [Checks](crates/sculpto-mesh/src/check.rs) · [Stack](crates/sculpto-mesh/src/layer_stack.rs) |
| Control | `garment control-exterior`, `control-repair`, `control-cuts`, `control-separate`, `control-check`. Strict profile, lineage, bounded repairs and refusal diagnostics. [Source](crates/sculpto-mesh/src/control.rs) |
| UV/material | `garment uv-check`. Check atlas, source-ray coverage and appearance. [UV](crates/sculpto-mesh/src/uv.rs) · [Native bake](README.md#bake-original-garment-materials-onto-a-control) |
| Skin | `actor skin` uses surface distance. Shells inherit body. Leaf bones carry no weights. `actor rebind` transfers. [Author](crates/sculpto-mesh/src/rig/author.rs) · [Verb](crates/sculpto-engine/src/skin.rs) |
| Actor samples | `actor sample`, `actor rig`. Explicit parts, REST or clip/time. Report finite sampled clearance. [CLI](crates/sculpto-cli/src/actor.rs) · [Reader](crates/sculpto-mesh/src/actor/mod.rs) |
| Joint checks | `actor joints` checks type/axis/range with cited values and labelled envelopes. Non-Mixamo digit axes advisory until measured per body. [Code](crates/sculpto-engine/src/joints.rs) · [Schema](crates/sculpto-engine/src/joints.toml) |
| Dressed motion | `actor pack-cloth`. Preserve actor. Qualify cloth motion, collisions and loop closure separately. [Source](crates/sculpto-gltf/src/pack.rs) |
| Outfit selection | `outfit inspect --nif FILE --manifest FILE`. Named parts/regions for rigging or Blender Cloth. [Source](crates/sculpto-mesh/src/outfit.rs) |
| Asset catalogue | `boethos index`, `list`, `get`, `stats`, `export`. Host `7z`, supported archives exclude RAR, DB and exports outside VCS. [Catalogue](crates/boethos-catalogue/src/lib.rs) · [CLI](crates/boethos-cli/src/main.rs) |

`garment trim` / `strip` delete triangles in derived output. Qualify a construction-preserving
fit on geometry with intact topology.

## Architecture and change ownership

| Owner | Scope and verification |
|---|---|
| [sculpto-core](crates/sculpto-core/src/lib.rs) | Neutral mesh/rig/skin, BodySlide, morphs and validation. `crates/sculpto-core/tests` |
| [sculpto-bake](crates/sculpto-bake/src/lib.rs) | Deterministic preset offsets. `crates/sculpto-bake/tests` |
| [sculpto-mesh](crates/sculpto-mesh/src/lib.rs) | Fit, control, remap, contacts and actor evaluation. `crates/sculpto-mesh/tests` |
| [sculpto-nif](crates/sculpto-nif/src/lib.rs) | Native I/O, skin-space projection and byroredux boundary. `crates/sculpto-nif/tests` |
| [sculpto-gltf](crates/sculpto-gltf/src/lib.rs) | GLB, append-only cloth packing. `crates/sculpto-gltf/tests` |
| [sculpto-engine](crates/sculpto-engine/src/lib.rs) | Registry, schemas, operations and classified errors. `crates/sculpto-engine/tests` |
| [sculpto-cli](crates/sculpto-cli/src/main.rs) | Argv and stdio. `crates/sculpto-cli/tests` |
| [tools](tools/) | Optional Blender/reduction/viewer adapters. `tripo-split`: one GLB per figure + catalogue manifest. [Recipes](README.md), `tools/tests` |

Share rules in the owning Rust crate. For managed work, wrapper `doc/map/README.md` owns
entity bindings and function metrics.

## Investigate 3D work by coordinates

1. Record digest, selected parts, coordinate frame, units/scale, matrix, REST or clip/time
   and ordered topology IDs.
2. Check [source-local/world frames](README.md#bake-original-garment-materials-onto-a-control).
   Apply the source matrix once. Infer axes from data.
3. Inspect bounds/counts → named regions → failed triangle/vertex IDs → matched crop.
   Keep numeric/visual evidence in one frame. Retain evaluated-to-source lineage after
   topology changes. Managed Blender queries use garment-authoring's spatial-inspection reference.

## Worker verification

Implementer → build → focused tests → repair. INPUT names workspace, target dir, commands,
fixtures and acceptance gates. Heavy work follows QUEUE.md inside a task-owned unit and
acquires the shared lock once. Record pending handles, source snapshot and exact result.
Coordinator composes evidence and runs destination gates.

## Remaining work — not a shipped guarantee

Raw garment → accepted outfit remains experimental. Managed roadmap and parity ledger
own priorities, BAS/CLS residuals and acceptance.

- FIT-08: real Apachii/Sentinel pose, fit and clearance.
- BLD-01–06/09/10: zap/clamp/UV, clipping repair, batch, partitions/material/morph fidelity,
  `export.tri`.
- RIG-04/05: posed NIF/GLB round trips and supported-family binding acceptance.
- SHP-06: native Outfit Studio multi-shape acceptance. Shipped:
  `export.bundle` / `export-nif --all-shapes`.
- MOD-02–04/06: material/texture completeness and native outfit load.
- ARC-01/02/04–06: archetypes, attachments and qualified one-button adaptation.
- Future: authoring surfaces, engine session/undo/invalidation, hairstyle workflow.

Toolkit scope excludes gameplay, animation authoring and world building.
