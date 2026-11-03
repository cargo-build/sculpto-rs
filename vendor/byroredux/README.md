# Vendored `byroredux-nif`

Upstream: <https://github.com/matiaszanolli/ByroRedux>
Pin: `f04c3433d32b7e12ea32df6a39d434adcc6b85b3` (2026-10-27)
License: MIT — upstream `LICENSE` preserved as `LICENSE`.
Vendored: 2026-10-30 by OpenCode `ses_e81884401ffePuGOFANlxil2z6`.

## Contents

| Path | Upstream |
|---|---|
| `nif/` | `crates/nif` |
| `core/` | `crates/core` |
| `Cargo.toml` | mini-workspace written for the vendor tree |
| `LICENSE` | upstream file, unmodified |

Only these two crates are vendored because they are the whole dependency of the
adapter: `byroredux-nif` needs `byroredux-core`, `log` and `nalgebra`; core needs
`glam`, `log`, `rayon`, `rustc-hash`, `serde`, `serde_json`,
`string-interner`, `thiserror` and `uuid`.

## Manifest adaptations (behaviour unchanged)

- Removed `[dev-dependencies]`, the `[[test]]` and `[[example]]` targets, and
  the optional `dhat` dependency plus its feature from `nif`. The vendored tree
  is a dependency, not a test workspace, and one example includes a file from
  `docs/` outside the crate.
- Added `Cargo.toml` so the upstream `{ workspace = true }` manifests resolve,
  with the exact upstream version pins.
- The Sculpto workspace declares `exclude = ["vendor/byroredux"]` so the two
  crates keep their own workspace root.

No source file was modified. A behavioural patch would be a defect to record and
work around in `sculpto-nif` instead (managed-wrapper requirement DECL0008).

## Verified

`sculpto-nif` reads the accepted female reference body
(`femalebodyastrid_0.nif`): header `0x14020007` / user version 12, shapes
`3BA` (18,436 vertices, 35,904 triangles, 49 bones), `3BA_Vagina`, `3BA_Anus`,
with a maximum weight sum of 1.0001.
