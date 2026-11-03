# cloth-viewer

Headless Bevy proof harness for the baked cloth GLB. Loads the actor GLB and
the `export_cloth_glb.py` output, renders one offscreen frame and writes
`/tmp/sculpto-classify/cloth/bevy_cloth.png`.

Standalone crate (not a workspace member): it pulls Bevy, which is unrelated
to the sculpto crates.

```sh
cargo build
cp <character.glb> <cloth.glb> target/debug/assets/
LD_LIBRARY_PATH=/run/opengl-driver/lib:<64-bit vulkan-loader>/lib \
VK_ICD_FILENAMES=/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json \
WGPU_BACKEND=vulkan ./target/debug/cloth-viewer
```

Two animation facts learned the hard way (2026-11-02, after every capture
turned out to be a frozen still): Bevy's facade feature `bevy_animation`
does **not** enable `bevy_gltf/bevy_animation` — without the `animation`
facade feature the glTF loader creates no `AnimationClip` assets and
`file.glb#AnimationN` handles stay phantom; and the glTF loader spawns its
**own** `AnimationPlayer` inside each scene, with every `AnimationTarget`
bound to that internal player — a player added on the `SceneRoot` wrapper
entity drives zero targets. The viewer therefore uses one scene instance and
one graph (actor + cloth clips as parallel nodes) attached to the scene's
own players. Verify captures with `md5sum`: a pose series with one unique
hash per view is a still, not a proof.

NixOS notes learned the hard way: the Vulkan loader must be the 64-bit store
build (`readelf -h` — some `vulkan-loader` store paths are 32-bit), the
windowless app needs `ScheduleRunnerPlugin` (winit is the default runner),
and the offscreen `Image` target needs `RENDER_ATTACHMENT | COPY_SRC` usage.
glTF scenes need the reflect registrations listed in `main.rs` when building
Bevy with a minimal feature set.

The asset, the two clip indices and the output PNG are env-driven because
every combined GLB renumbers its clips (the cloth weight track is animation
0, actor clips follow alphabetically): `SCULPTO_VIEWER_ASSET` (default
`combined.glb`), `SCULPTO_VIEWER_ACTOR_ANIMATION` (default `4`),
`SCULPTO_VIEWER_CLOTH_ANIMATION` (default `0`) and `SCULPTO_VIEWER_PNG`
(default `/tmp/sculpto-classify/cloth/bevy_combined_240.png`). Print the
index list from the GLB's JSON chunk before a capture.

Series mode replaces the single shot:
`SCULPTO_VIEWER_SERIES=<start>:<count>:<stride>` captures one pose every
`stride` ticks from `start` (≥ 240 — earlier frames are black while assets
load), `SCULPTO_VIEWER_VIEWS=front,side,back,quarter` shoots each pose from
several cameras on consecutive ticks, `SCULPTO_VIEWER_DIR` receives
`pose<tick>_<view>.png`. One view per tick, so views of a pose differ by
~1/60 s of animation.
