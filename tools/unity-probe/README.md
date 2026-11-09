# unity-probe

Headless Unity check that exported GLBs reach the Unity side of the delivery
pipeline: the probe imports GLBs with `glTFast` 6.14.0 and reports meshes, vertex
counts, blend shapes, clips, skinned renderers and nodes as JSON.

## Run

To probe specific GLBs:
```sh
./run.sh /path/to/model1.glb /path/to/model2.glb
```

Or using legacy environment variables:
```sh
ACTOR_GLB=<character.glb> CLOTH_GLB=<C1-cloth-idle.glb> ./run.sh
```

Or with default `sculpto_gltf` fixtures generated automatically:
```sh
./run.sh
```

The script creates or reuses a throwaway project under
`/tmp/sculpto-classify/cloth/unity-probe`, adds `com.unity.cloud.gltfast`
6.14.0, copies the GLBs into `Assets/Garments/`, runs `ImportProbe.Run` and
prints the `IMPORT_PROBE {...}` line from the Editor log. Output is also written
to `/tmp/sculpto-classify/cloth/unity-import-report.json`.

## NixOS launch facts (verified 2026-11-02, 2026-11-09)

- Launch through the Hub's FHS env:
  `/nix/store/46z4ihvk6g5hxx6426ihrr99vv74lc16-unityhub-fhs-env-3.19.5/bin/unityhub-fhs-env`
  — the bare `Editor/Unity` binary cannot load `libGL.so.1`.
- The shader compiler needs `libtinfo.so.6`; pass
  `LD_LIBRARY_PATH=$HOME/.local/state/unity-mcp/runtime/ncurses-64/lib`,
  otherwise batchmode aborts with
  `Shader compiler initialization error ... 0x80000008`.
- `-nographics` is enough for import/inspection work.
- When running under the shared laptop resource queue (`~/nix-config/QUEUE.md`),
  acquire the lock inside a transient systemd user unit:
  ```sh
  systemd-run --user --scope --unit=agent-heavy-unity-probe-$$ -- \
    flock /home/wave/.local/state/agent-resources/heavy.lock \
    nice -n 10 ionice -c3 ./run.sh [GLB_FILES...]
  ```

## Consumer acceptance results (E08 vs Bevy E07)

| Asset | Source | Unity Meshes | Unity Verts | Unity BlendShapes | Unity Clips | Unity Skinned / Mesh Renderers | Bevy Match (E07) |
|---|---|---|---|---|---|---|---|
| `tier1.glb` | `sculpto_gltf::write_glb` | 1 | 4 | 0 | 0 | 0 skinned / 1 mesh | Meshes=1, Verts=4, Tris=2, Materials=0 |
| `synthetic.glb` | `sculpto_gltf::write_glb` | 1 | 3 | 0 | 0 | 1 skinned / 0 mesh | Meshes=1, Verts=3, Tris=1, Joints=2 |
| `morph.glb` | `sculpto_gltf::write_glb` | 1 | 3 | 2 ("Smile", "Widen") | 0 | 1 skinned / 0 mesh | Morphs=2 ("Smile", "Widen") |
| `C1-cloth-idle.glb` | Legacy python bake | 2 | 134,619 (45,534 / 89,085) | 60 (30 each) | 2 (5.83 s) | 2 skinned / 0 mesh | N/A (legacy benchmark) |
| `character.glb` | Legacy actor fixture | 0 | 0 | 0 | 0 | 0 | Known glTFast importer refusal |
