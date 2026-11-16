# cloth-viewer

Headless Bevy proof harness for the baked cloth GLB. Loads the actor GLB and
the `export_cloth_glb.py` output, renders one offscreen frame and writes
`/tmp/sculpto-classify/cloth/bevy_cloth.png`.

`--interactive` opens the same scene in a window so a baked result can be
orbited and inspected instead of captured from one fixed view: drag **LMB** to
orbit, **RMB** to pan, wheel to zoom, **R** to reset. The review window is the
fast inspection loop; the 13-view contact sheet remains the fit evidence.

The interactive camera frames the loaded scene from its world bounds: once
geometry exists it measures the AABB, puts the orbit target at its centre and
derives the opening distance and the zoom range from that radius, so a 1.8 m
character and the 100-unit Khronos Fox both open readable and zoom out to the
whole model. Framing never rescales the asset — `SCULPTO_VIEWER_SCALE` remains
the source-unit conversion. `SCULPTO_VIEWER_AUTOFIT=0` keeps the fixed review
camera; headless captures always use the fixed camera.

Standalone crate (not a workspace member): it pulls Bevy, which is unrelated
to the sculpto crates.

```sh
cargo build
cp <character.glb> <cloth.glb> target/debug/assets/
LD_LIBRARY_PATH=/run/opengl-driver/lib:<64-bit vulkan-loader>/lib \
VK_ICD_FILENAMES=/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json \
WGPU_BACKEND=vulkan ./target/debug/cloth-viewer
```

The build needs the window, X11 and Vulkan development inputs that NixOS does
not expose in a plain shell. Build and run inside the shipped dev shell
(`flake.nix`), which also exports the runtime libraries, so no hand-assembled
`LD_LIBRARY_PATH` is needed:

```sh
nix develop -c cargo build -j 2
nix develop -c ./target/debug/cloth-viewer --interactive \
  --glb ~/Models/sculpto-cloth-fixture-20261115/peanut-r105.glb
```

The shell puts `/run/opengl-driver/lib` (the host's GL/Vulkan driver) first when
it exists; set `VK_ICD_FILENAMES`/`WGPU_BACKEND` only for a non-default backend.
An `.envrc` with `use flake` loads the same shell on `cd` when direnv is on.

`SCULPTO_VIEWER_SCALE` multiplies the loaded scene before the fixed camera and
lights see it. BodySlide-authored fixtures are in lab units (71.4286 per
metre), so the peanut bake needs `SCULPTO_VIEWER_SCALE=0.014`; a metre-scale
GLB keeps the default `1.0`. The scale applies to `--report` too, where it
does not change counts.

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
`SCULPTO_VIEWER_CLOTH_ANIMATION` (no default: the extra clip loads only when
this is set, so an unnamed `Animation0` is not blended over the actor) and
`SCULPTO_VIEWER_PNG` (default `/tmp/sculpto-classify/cloth/bevy_combined_240.png`). Print the
index list from the GLB's JSON chunk before a capture.

`SCULPTO_VIEWER_DEBUG_SCENE=1` replaces the capture with a consumer-limit
audit on the resolved Bevy 0.20.x: it probes the morph-target gate (256 accepted,
257 rejected), prints one line per loaded mesh with vertex count, morph targets,
material presence and skin joints, and takes two playback snapshots (active
clips, elapsed time, summed morph weights) before exiting. This is the command
behind the A00B numbers.

## 0.17.3 compatibility

The pre-0.20 viewer is retained in jj history at revision `f280a6a3` (its
manifest pins `bevy = "=0.17.3"`) as a compatibility check, never as the
production reference. Reproduce it without disturbing this checkout:

```sh
jj workspace add --revision f280a6a3 /tmp/sculpto-bevy-017
cargo build --manifest-path /tmp/sculpto-bevy-017/tools/bevy-viewer/Cargo.toml
jj workspace forget sculpto-bevy-017
```

Series mode replaces the single shot:
`SCULPTO_VIEWER_SERIES=<start>:<count>:<stride>` captures one pose every
`stride` ticks from `start` (≥ 240 — earlier frames are black while assets
load), `SCULPTO_VIEWER_VIEWS=front,side,back,quarter` shoots each pose from
several cameras on consecutive ticks, `SCULPTO_VIEWER_DIR` receives
`pose<tick>_<view>.png`. One view per tick, so views of a pose differ by
~1/60 s of animation.

## Live reload (`--watch <dir> --stage <name>`)

Agents write `<dir>/<stage>.glb` plus `<dir>/<stage>.json` every few minutes;
the viewer polls the file and swaps it in place, so the operator watches the
work happen instead of reading about it:

```sh
nix develop -c ./target/debug/cloth-viewer --interactive \
  --watch /tmp/opencode/live --stage weights-0164
```

`--watch` without a value uses `/tmp/opencode/live` (`SCULPTO_VIEWER_WATCH` /
`SCULPTO_VIEWER_STAGE` are the env forms). One well-known file per stage: the
sidecar carries `{"status", "sha256", "note"}` and only its one-line `note` is
displayed — the sha in the card is always this viewer's own hash of the bytes
it actually staged, never the sidecar's claim. A missing sidecar says
`no sidecar`; an unparsable one says `sidecar unparsable`.

Every `0.5 s` the viewer `stat`s the file (polling keeps the crate
dependency-free — no file-watcher, no feature flag). An mtime or size change
is copied to a fresh `live-<n>.glb` inside the asset root (write + rename, so
a reader never sees a half copy) and loaded as a new asset; when it loads, the
new scene root spawns and the previous one despawns. The camera, orbit, zoom,
light rig, lit/flat, bones level, turntable pose and pause/speed all live in
resources that outlive the swap; the clip graph is rebuilt from the new file,
so `0`/`1` keep their meaning and playback restarts at 0. A corrupt or
half-written GLB fails its load: the previous scene stays, the card says
`load failed — kept previous`, and the next change retries. The title and the
card's live line carry the status first (`diagnostic · live <stage>
<sha256[:8]> · <note>`), matching the niri focus rules at every swap.

`--watch` cannot combine with `--report` (that path exits through a fixed
headless route) or `--glb` (the watched file names the asset itself), and it
shows exactly one figure (`--units` is forced to 1). Staged copies and their
`.gitignore` live under the asset root (`BEVY_ASSET_ROOT`, else
`CARGO_MANIFEST_DIR`, else the executable's directory + `assets/`) and are
removed as generations advance. `SCULPTO_VIEWER_WATCH_EXIT_SECS=<n>` exits a
headless `--watch` run after n seconds — the smoke knob behind the task's
headless proof, never needed for a real window. An idle poll is one `stat`
(measured 0.4 µs, twice a second); a change costs one read, one SHA-256 of
the bytes (21 ms for 4 MiB) and one write, off the hot path.

## Review lighting

Lighting is runtime PBR, not baked into the exported GLB (`viewer export` writes
albedo and normals only). The interactive viewer defaults to **one key light** with
`shadow_maps_enabled`, and spawns a matte ground so the cast shadow is visible. The
default mouse keeps the camera controls (LMB orbit, RMB pan). **Ctrl+LMB turns the
figure**, **Ctrl+RMB moves it sideways** under the fixed light, and **Shift+LMB
moves the key light**; every one of those changes the shading and the cast shadow.
`3` toggles the optional three-point rig (fill and rim) when the far side is too dark
to read; `SCULPTO_VIEWER_THREE_POINT` seeds it. `L` toggles lit/flat; flat hides the
whole rig and ground and renders every material unlit, so a review shows the
silhouette, the seams and the layer palette without a shadow that can be mistaken
for a defect. The capture path keeps whatever `SCULPTO_VIEWER_FLAT` seeds and stays
on the black backdrop (no ground); `SCULPTO_VIEWER_GROUND=1` adds the ground and
`SCULPTO_VIEWER_TURNTABLE_YAW` seeds the figure pose for a shadow comparison.
`0` selects the idle clip and `1` the walk clip of a rigged GLB (the actor graph
loads `Animation0` and `Animation1`; the cloth clip keeps running). A top-left
controls card lists the keys and `H` hides or shows it. Keyboard events need the
window focused — agent-opened preview windows start unfocused by design, so click
the figure once.
