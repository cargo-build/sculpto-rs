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
