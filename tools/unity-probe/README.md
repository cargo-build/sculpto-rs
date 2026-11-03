# unity-probe

Headless Unity check that the exported GLB actually reaches the Unity side of
the delivery pipeline: the probe imports the actor and cloth GLBs with
glTFast and reports meshes, vertex counts, blend shapes, clips and skinned
renderers as JSON.

## Run

```sh
ACTOR_GLB=<character.glb> CLOTH_GLB=<C1-cloth-idle.glb> ./run.sh
```

The script creates a throwaway project under
`/tmp/sculpto-classify/cloth/unity-probe`, adds `com.unity.cloud.gltfast`
6.14.0, copies both GLBs, runs `ImportProbe.Run` and prints the
`IMPORT_PROBE {...}` line from the Editor log. Private payloads stay in `/tmp`.

## NixOS launch facts (verified 2026-11-02)

- Launch through the Hub's FHS env:
  `/nix/store/46z4ihvk6g5hxx6426ihrr99vv74lc16-unityhub-fhs-env-3.19.5/bin/unityhub-fhs-env`
  — the bare `Editor/Unity` binary cannot load `libGL.so.1`.
- The shader compiler needs `libtinfo.so.6`; pass
  `LD_LIBRARY_PATH=$HOME/.local/state/unity-mcp/runtime/ncurses-64/lib`,
  otherwise batchmode aborts with
  `Shader compiler initialization error ... 0x80000008`.
- `-nographics` is enough for import/inspection work.

## Observed results (2026-11-02, cloth GLB from `export_cloth_glb.py`)

| Asset | Meshes | Blend shapes | Clips | Skinned renderers |
|---|---|---|---|---|
| `C1-cloth-idle.glb` | 2 (45,534 / 89,085 verts) | 30 each | 2 × 5.83 s | 2 |
| `character.glb` (accepted actor) | — none surfaced — | — | — | — |

The cloth asset imports with its full morph set and clips; the pre-existing
actor GLB reimports through the same importer but exposes no sub-assets to
`AssetDatabase.LoadAllAssetsAtPath`, and the Editor logs no error — recorded
as an importer-side observation, not a defect of the cloth export.
