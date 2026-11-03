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
target/debug/sculpto actor pack-cloth --input actor.glb --targets targets.glb \
  --node TunicSimulationRender --clip Run --schedule schedule.json --out dressed.glb
```

Actor sampling distinguishes asset defaults from animation time zero and
preserves explicit part/primitive identity. Cloth packing preserves the
actor's other geometry, materials and skeletal animation; it does not qualify
fit, collisions or periodic motion. The current packing consumer supports
2–256 targets. Base textures must be embedded or self-contained data URIs.

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
