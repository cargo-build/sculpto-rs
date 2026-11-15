# boethos-catalogue

Asset, preset, face and hairstyle catalogue kinds for the
[`boethos`](../boethos) store.

The crate owns the `asset` record and a read-only indexer for supplied game
assets. It hashes regular files, lists `.7z`/`.zip`/`.rar` archives through the
host `7z`, and records only catalogue-relevant members: `FaceGeom` meshes,
`SliderPresets`, `SliderSets`, `.jslot` presets and hair meshes. For `.glb` and
`.gltf` files it also records one `clip` row per embedded `animations[]` entry
with measured channels, keyframes, duration, driven nodes and skin-joint
coverage. It never extracts or rewrites a payload.

Identity is derived from the source label and canonical host path; the file
content digest stays in `sha256`. Two files with equal bytes at different paths
remain distinct records.

```rust
use boethos::Store;
use boethos_catalogue::{IndexOptions, index_root};

let store = Store::open("assets.redb")?;
let report = index_root(
    &store,
    std::path::Path::new("/supplied/assets"),
    "supplied",
    &IndexOptions::default(),
);
println!("{} files, {} errors", report.files, report.errors.len());
# Ok::<(), boethos::StoreError>(())
```

Private asset payloads and the generated database stay outside version control.

License: GPL-3.0-only.
