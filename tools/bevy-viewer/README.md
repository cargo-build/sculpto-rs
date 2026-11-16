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

## Reference overlay and the census

`Y` draws a reference skeleton over the loaded one and `M` switches the drawing
between dots+lines and cone/octahedral bones. Nothing is drawn unless two
censuses agree, because a window that calls a native 126-joint rig "Mixamo" is a
defect, not a preference:

- the loaded skin must census `65 joints · 65 mixamorig · 0 native`;
- the reference file (`SCULPTO_VIEWER_REFERENCE`, default
  `~/Models/sculpto-jokes-20261115/ybot_walking.glb`) must census the same;
- otherwise the card says `reference: unavailable — the loaded file is a native
  carrier` (or which file failed which rule) and no geometry appears.

The card always prints the census: `skeleton: 126 joints · 0 mixamorig · 126
native · native carrier`. The word **Mixamo** appears only at `65 / 65`.

The overlay is placed by the seat similarity — measured yaw and tilt from the two
flesh frames, scale from the pelvis→head plus pelvis→toe span, hips on hips —
never by the reference file's own units (Mixamo's armature carries scale 0.01
and would otherwise draw a hundred times too large). Only the *matched* joints
are drawn, and each pair's distance in millimetres is printed beside it; the card
carries `Δ max` and `mean`.

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

## Pose mode

`P` turns pose mode on and off, and `T` poses the loaded skeleton to a reference
file's bone **directions** — the Mixamo T — without writing a file. A pose is a
rotation **delta on the file's rest**, never an absolute local write:
`local.rotation = delta * rest.rotation`, and translation and scale are copied
from the rest untouched. A rotation cannot move a child's bone offset or inflate
it, so the skin neither detaches nor scales, and a joint nobody touched is
written back as the captured rest bits. `Shift+R` puts the whole body back on
the file's rest.

| Key | Effect |
|---|---|
| `P` | pose mode on/off (off plays the clips again) |
| `T` | Mixamo T from the reference skeleton, live |
| `Tab` / `Shift+Tab` | next / previous joint of the selected limb chain |
| `R` / `Shift+R` | reset the selected joint / every joint to the file rest |
| drag `LMB` | turn the selected joint about the **camera's** axes |
| drag `Shift+LMB` | turn it about the **joint's own** axes |
| arrows | 5° steps, `←`/`→` about the camera, `↑`/`↓` about the joint |
| drag `RMB` | orbit the camera (pose mode takes the left button) |
| wheel, `Ctrl+LMB`, `Ctrl+RMB` | zoom, turn the figure, move it sideways |

The selected joint is a magenta marker with its bone name drawn beside it, so a
3 mm bone does not have to be clicked exactly. `Tab` walks the limb of the
current selection (clavicle → hand, thigh → toe) ordered by hierarchy depth.

`T` reads `SCULPTO_VIEWER_REFERENCE` or, when unset,
`~/Models/sculpto-jokes-20261115/ybot_walking.glb`. The body frame is
**measured**, not assumed: both skeletons give a left axis from the two thighs
and an up axis from thigh → calf, and the transfer between those frames is the
yaw the seat lanes found by other means. `T` refuses instead of guessing when a
skeleton has no thigh pair, and a direction-only aim leaves the roll of each
bone where the file put it.

A headless capture can open on a pose, which is how a still of a defect is made:

```sh
SCULPTO_VIEWER_HEADLESS=1 SCULPTO_VIEWER_POSE_MODE=1 \
SCULPTO_VIEWER_POSE_JOINT='NPC L UpperArm' SCULPTO_VIEWER_POSE_ROTATE='0,0,-25' \
SCULPTO_VIEWER_POSE_TPOSE=1 SCULPTO_VIEWER_PNG=/tmp/opencode/pose.png \
  ./target/debug/cloth-viewer --headless --glb ~/Models/sculpto-tpose-20261116/emily-tpose.glb
```

`SCULPTO_VIEWER_POSE_ROTATE=x,y,z` is degrees about the camera's axes, applied
to `SCULPTO_VIEWER_POSE_JOINT` (a substring of a palette name). The seeded pose
is logged, so a capture carries the numbers that produced it.
