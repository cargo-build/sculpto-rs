# Sculpto implementation

This independent repository contains the GPL-3.0-only implementation. Use
Rust 2024 and the declared toolchain; keep core behavior in Rust and optional
host adapters thin. Private asset payloads and renders never belong in VCS.

Use jj for mutations and readable git diff. Confirm the task feature before
editing/building; after jj describe immediately jj new. This implementation
is published publicly; its managed wrapper stays private. Land on main and
push after the required checks. Use at most two Cargo jobs;
on the shared host, heavy work uses the operator's owned-unit/QUEUE procedure.

`just gate` checks formatting, Clippy with -D warnings and workspace tests.
Parsing changes also run `just test-real /path/to/BodySlide` with the explicit
host fixture. Build with cargo build, not only cargo check.

In the managed `sculpto-rs-wrapper/repos/sculpto-rs` checkout, the wrapper's
AGENTS.md, issue journal, rules and source map govern coordinated work. Keep
their affected bindings/metrics current and run the wrapper's full `just gate`
before integration. Each independent source writer uses the wrapper's ignored
`repos/jj-workspaces/sculpto-rs-<task>` container; wrapper writers use
`repos/jj-workspaces/wrapper-<task>`. The workspace name equals its directory
basename. Standalone build/test does not require the wrapper.

## Start here — engine help

Run from this checkout, using the freshly built binary:

```sh
cargo build -p sculpto-cli -j 2
target/debug/sculpto --help
target/debug/sculpto commands --json
target/debug/sculpto run --cmd version
```

`--help` covers argv families. `commands --json` covers the closed engine
registry, including input schemas, `implemented` and `mutates`; `run --cmd ID
--input JSON` and stdio `mcp` use that registry. Actor sampling, control-surface
and UV commands also exist on argv and are not all registered for JSON/MCP.
Inspect the actual [argv dispatcher](crates/sculpto-cli/src/main.rs) and
[registry](crates/sculpto-engine/src/lib.rs) before claiming transport parity.
Subcommand help is uneven; missing subcommand help does not prove absence.

| Goal | Existing entry / source | Boundary |
|---|---|---|
| Inspect BodySlide data | `inspect PATH`, `presets`, `sliders`; [core](crates/sculpto-core/src/lib.rs) | OSP, presets, OSD/BSD; family and ordered vertex identity matter |
| Bake shapes | `bake`, `batch`; [bake](crates/sculpto-bake/src/lib.rs) | Explicit set, preset and weight endpoint; a body bake is not a finished outfit |
| Import/export assets | `export-obj`, registry `export.nif` / `export.pair`; [NIF](crates/sculpto-nif/src/lib.rs), [GLB writer](crates/sculpto-gltf/src/lib.rs) | Supported subsets; do not infer every native format feature |
| Fit and classify garments | `garment fit`, `follow`, `aura`, `classify`, `intake`; [mesh](crates/sculpto-mesh/src/lib.rs) | Authored topology/rest frame and declared preparation route |
| Measure geometric defects | `garment check`, `cover`, `proxies`; [checks](crates/sculpto-mesh/src/check.rs) | Intersection counts and sampled coverage have different meanings |
| Construct a control | `garment control-exterior`, `control-repair`, `control-cuts`, `control-separate`, `control-check`; [control](crates/sculpto-mesh/src/control.rs) | Strict profile, source lineage, explicit bounded repairs; refusal retains diagnostics |
| Qualify UVs and retain appearance | `garment uv-check`; [UV](crates/sculpto-mesh/src/uv.rs), [native baking guide](README.md#bake-original-garment-materials-onto-a-control) | A valid atlas does not establish source-ray coverage or appearance |
| Sample actor geometry/rig/motion | `actor sample`, `actor rig`; [actor CLI](crates/sculpto-cli/src/actor.rs), [actor reader](crates/sculpto-mesh/src/actor/mod.rs) | Explicit parts and REST or clip/time; finite samples do not prove continuous clearance |
| Export a dressed animation | `actor pack-cloth`; [packing](crates/sculpto-gltf/src/pack.rs) | Preserves the actor; does not qualify cloth motion, collisions or loop closure |
| Choose ordinary rigging / Blender Cloth | `outfit inspect --nif FILE --manifest FILE`; [outfit](crates/sculpto-mesh/src/outfit.rs) | Named parts/regions; selection is distinct from simulation |

`garment trim` / `strip` are destructive geometry operations on derived output;
triangle deletion is not successful construction-preserving fitting.

## Architecture and change ownership

| Owner | Responsibility | Direct consumers / verification |
|---|---|---|
| [sculpto-core](crates/sculpto-core/src/lib.rs) | Neutral mesh/rig/skin model; BodySlide documents, morph data and format validation | bake, mesh, NIF, GLB; `crates/sculpto-core/tests` |
| [sculpto-bake](crates/sculpto-bake/src/lib.rs) | Deterministic preset-to-offset application | engine bake/build; `crates/sculpto-bake/tests` |
| [sculpto-mesh](crates/sculpto-mesh/src/lib.rs) | Fitting, control topology, weight transfer/remap, contacts, actor evaluation | engine and thin host adapters; `crates/sculpto-mesh/tests` |
| [sculpto-nif](crates/sculpto-nif/src/lib.rs) | Native import/write, skin-space projection; vendored byroredux boundary | engine exports; `crates/sculpto-nif/tests` |
| [sculpto-gltf](crates/sculpto-gltf/src/lib.rs) | GLB writing and append-only cloth packing | engine/actor exports; `crates/sculpto-gltf/tests` |
| [sculpto-engine](crates/sculpto-engine/src/lib.rs) | Closed command catalogue, schemas, classified errors and operations | CLI/run/MCP; `crates/sculpto-engine/tests` |
| [sculpto-cli](crates/sculpto-cli/src/main.rs) | argv and stdio transports | users/adapters; `crates/sculpto-cli/tests` |
| [tools](tools/) | Optional Blender adapters, native reduction, viewers and diagnostics | [recipes and restrictions](README.md); `tools/tests` |

Put shared rules in the owning Rust crate; keep adapters thin. This is a
package/command navigator, not an exhaustive function map. In managed work,
the wrapper's `doc/map/README.md` owns entity bindings and measured functions.

## Investigate 3D work by coordinates

Before comparing meshes, record artifact digest, selected objects/parts,
coordinate frame, units/scale, object matrix, REST or clip/time and ordered
topology identity. [Material-frame definitions](README.md#bake-original-garment-materials-onto-a-control)
distinguish source-local and world coordinates: applying a source matrix twice
can produce a false clearance pass. Do not infer axes from screen orientation.

Start with object bounds/counts, then named regions and failing triangle/vertex
IDs. Keep numeric witnesses in the same frame as the visual crop. Blender's
evaluated mesh gives modifier/pose output; its indices are not automatically
source IDs after topology changes. Preserve the adapter's lineage maps.
Use the managed garment-authoring skill's spatial-inspection reference for
region queries and Blender MCP. Standalone adapters remain usable without it.

## Worker verification

Implementers build, run focused tests and repair their own changes. Cargo is
allowed for delegates. On the shared operator host, follow QUEUE.md for heavy
commands inside task-owned units; small checks need no heavy slot. Never nest
the shared lock. Name workspace, target directory, tests and fixtures in each
assignment; preserve the tested source snapshot and exact command/result.
The coordinator integrates and runs destination gates, reusing worker evidence
where integration does not invalidate it. A waiting check remains pending.

## Remaining work — not a shipped guarantee

The raw-garment-to-accepted-outfit workflow is experimental. The managed
wrapper's roadmap and engine-parity task ledger own priorities/status; update
this summary when those contracts materially change:

- FIT-08: real Apachii/Sentinel garment pose, fit and clearance acceptance.
- BLD-01–06/09/10: integrated zap/clamp/UV builds, clipping repair, batch,
  partitions/material/morph fidelity and the `export.tri` command surface.
- RIG-04/05: complete posed NIF/GLB round trips and supported-family binding
  acceptance. Existing rig/remap code is a foundation, not that acceptance.
- MOD-02–04/06: material/texture completeness and native end-to-end outfit load.
- ARC-01/02/04–06: archetypes, attachments and qualified one-button adaptation.
- Authoring surfaces, engine session/undo/invalidation and hairstyle workflow
  remain future work; existing Blender prototypes do not establish acceptance.

Gameplay, animation authoring and world building are outside this toolkit.
