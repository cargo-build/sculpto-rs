# cloth-viewer

Headless Bevy proof harness for the baked cloth GLB. Loads the actor GLB and
the `export_cloth_glb.py` output, renders one offscreen frame and writes
`/tmp/sculpto-classify/cloth/bevy_cloth.png`.

`--interactive` opens the same scene in a review window. Drag **LMB** to
orbit or **RMB** to pan. Scroll to zoom, or press **R** to reset. Hold **W/S**
to travel forward/back along the view. The **LMB** orbit radius shrinks on
approach. Hold W to continue past the old pivot. Hold **A/D** to move the
view left/right. The keys work during a drag. WASD and RMB use the fitted
opening distance as their speed reference at every zoom level. Keyboard
travel covers 1.71125 opening distances per second and RMB panning covers
0.00513375 per pixel. The interactive near clipping plane is 1 mm.
LMB rotation is 0.005 radians per pixel at every distance.
The 13-view contact sheet remains the fit evidence.

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

## Agent control socket (`--control <path>`)

An LLM or script inspects and steers a **running** window — interactive or
headless — over one Unix stream socket: one JSON object per line in, one out.
The socket is `0600` in a directory this call creates `0700`; an existing
directory keeps its own mode, and an existing file or symlink at the path is
never replaced. A second viewer on the same path is refused (`another viewer is
listening on …`) and only a socket nobody serves is reclaimed, so launch each
window with its own fresh task-owned path (`<task dir>/viewer.sock`). The path
is printed to stderr and shown in the controls card.

A connection that sends nothing for `15 s` is dropped and the next caller is
served: the protocol is request/reply, so one request per connection is the
intended use. An unread reply is bounded at `5 s`.

```sh
nix develop -c ./target/debug/cloth-viewer --interactive \
  --glb fixture.glb --control /tmp/opencode/task/viewer.sock
printf '{"cmd":"state"}\n' | socat - UNIX-CONNECT:/tmp/opencode/task/viewer.sock
```

`{"cmd":"help"}` lists every command and its fields. `state` answers:

```json
{"ok":true,"protocol":1,
 "asset":{"file":"ybot-A-fingers.glb","path":"…","sha256":"dae19324…6b8",
          "present":true,"loaded":true,"pending":false,"failed":false,
          "meshes":2,"scale":1.0},
 "status":"InProgress",
 "clip":{"index":0,"count":0,"paused":false,"speed":1.0},
 "skeleton":2,
 "camera":{"mode":"orbit","target":[0,0.9,0],"yaw":0.66,"pitch":0.11,
           "distance":3.58,"position":[2.2,1.3,2.8]},
 "joint_count":65,"joints":[{"name":"mixamorig:Hips","position":[0,1.02,0]}],
 "material_count":3,
 "materials":[{"mesh":"3BA","material":"bra","asset":1,"alpha":0.85,
               "blend":"Blend","color":[0.18,0.8,0.44],"unlit":false}]}
```

- **Identity is the hash of the bytes on screen, qualified by three flags.**
  In `--watch` mode that is the generation currently displayed
  (`live-<n>.glb`), so a staged file that fails to parse leaves the previous
  digest in place with `failed: true`; otherwise it is the file
  `--glb`/`--asset` named, with `failed` set when that file does not parse.
  `pending: true` means a newer staged generation is still loading and the
  digest describes the one you are looking at. `loaded` is **scene readiness**,
  not intent: it is true only once at least one scene mesh from that generation
  is in the world (`meshes` counts them), so a spawned root or a staged file is
  never reported as a parsed scene.
- **`clip.count` is the file's animation count**, not the player graph's length:
  the graph always holds idle/walk nodes, so a clipless fixture must refuse
  `{"cmd":"clip"}` instead of playing a phantom handle.
- **`joints` are world positions** of the overlay's joints, so they exist only
  with the skeleton on (`level` 1 or 2) and only for `mixamorig`/`NPC`/`CME`
  named nodes. Ask for one joint by name: exact, then case-insensitive, then
  unique substring — `"ForeArm"` answers with both candidates instead of
  guessing.

| request | effect |
|---|---|
| `{"cmd":"state"}` | the document above |
| `{"cmd":"joint","name":"LeftForeArm"}` | one position, same lookup |
| `{"cmd":"view","name":"front"}` | `front`/`back`/`left`/`right`/`quarter`/`top`/`below`; `left`/`right` are the figure's own sides (the actor faces +Z, so `left` puts the camera on +X) |
| `{"cmd":"view","yaw":0.8,"pitch":0.1,"distance":1.5,"target":"Head"}` | explicit angles around the current or a named target |
| `{"cmd":"target","joint":"Pelvis"}` / `{"cmd":"target","point":[0,0.9,0]}` | move the orbit target |
| `{"cmd":"clip","index":1}` | restart the chosen actor clip; the cloth clip keeps running |
| `{"cmd":"slot","index":3}` | select a posed asset or named clip in `--bank` mode |
| `{"cmd":"inspection"}` | select the optional disassembled rest asset in a bank |
| `{"cmd":"playback","paused":true,"speed":0.25}` | pause and slow the clip (`0.05..4`) |
| `{"cmd":"seek","seconds":0.7}` | pause the loaded actor clip at the requested time within its duration |
| `{"cmd":"skeleton","level":2}` | `0` off, `1` primary chain, `2` every driven joint |
| `{"cmd":"capture","path":"/abs/shot.png"}` | screenshot of the window (interactive) or the render target (headless); absolute `.png`/`.jpg` path, parent directories created — the file lands a frame later, poll it |
| `{"cmd":"quit"}` | answer, then close the process |

Camera scalars and target coordinates are validated after the conversion to
`f32`: `1e100` is a finite `f64` and an infinite `f32`, so it is refused with a
named range error before any camera state changes.

Errors are one line with the reason and the current bound:
`{"ok":false,"error":"\"ForeArm\" matches 2 joints: … — use a longer name"}`.
A malformed line, an unknown command, an out-of-range clip or a relative capture
path never reach the ECS and never kill the window.

A plain capture's fixed review camera is untouched by the socket; `view`/`target`
move the interactive orbit (and the headless camera) only when asked. `--control`
cannot combine with `--report`, which prints counts and exits before a socket
could answer once. A missing asset now fails at startup (`exit 2`,
`viewer: asset not found: <path>`) instead of rendering a black frame that looks
like a result.

**A control session lives until `quit`.** Headless runs without `--control` keep
their one-shot behaviour — capture at frame 240 into `SCULPTO_VIEWER_PNG`, exit
at frame 301, or the `SCULPTO_VIEWER_SERIES` sweep — and that exit took a live
control session down together with an unanswered third capture (measured on the
2026-11-17 Emily knee session: `captured at 240, exiting at 301`). With
`--control` those automatic paths are skipped, so the process stays up for as
long as the caller needs; nothing else about the render changes. The capture
answer is a *queue* acknowledgement, not a finished file: read the PNG until its
`IEND` chunk arrives before treating the view as evidence.

`quit` leaves through `_exit` after the reply is on the wire: this host's NVIDIA
driver segfaults inside the glibc atexit handlers that a normal teardown runs
(the report path skips them the same way). A window closed by the compositor is
unaffected.

## Skeleton overlay and body opacity

The overlay is on at level 2 by default (`B` cycles off → primary chain → every
driven joint; `SCULPTO_VIEWER_SKELETON=0` is the only opt-out). While it is on,
**every actor material** — body, bra and panty alike — renders at **85 %
opacity** (`ACTOR_OPACITY` in `src/main.rs`, the operator's default from
2026-11-17, previously 16 %): flesh stays readable and the bones show through
it. Turning the overlay off restores full opacity. There is no opacity control
beyond that default and no socket command for it.

One number for one figure, but not one appearance: a part worn in front of the
body composites two blended layers over the background where the bare body
composites one, so underwear can read denser or more saturated at the same
alpha. That is transparency compositing, not a different setting — `state`'s
`materials` array reports the alpha, blend mode, colour, material name and
material-store index of every drawn actor material, which is how the equality is
checked instead of judged from a still.

The review camera uses depth-sorted transparency on supported GPUs.
`SCULPTO_VIEWER_SORT_TRANSPARENCY=0` selects the ordinary blend path for comparison.
`N` switches between smooth normals and per-triangle normals on body and clothing.
`SCULPTO_VIEWER_FACE_NORMALS=1` starts in the faceted view. The faceted mesh copies
skin influences and morph offsets onto each triangle corner and computes the face
normal for each target. The control socket exposes the active clip time in
`state.clip.seconds` and the current mesh weights in `state.morph_weights`.

## Review lighting

The viewer supplies a directional sun and subtle cool skylight for every
lit review. The sun starts at 55° with white light. **Shift+LMB** aims it.
Lower it toward the horizon for warmer, dimmer light and longer shadows.
The matte ground receives the shadow in an interactive review. **Ctrl+LMB**
turns the figure and **Ctrl+RMB** moves it sideways under the same sun.

`F3` adds the optional fill and rim and `SCULPTO_VIEWER_THREE_POINT` seeds
that rig. `L` switches to unlit materials against a black background for
seam, silhouette and palette checks. `SCULPTO_VIEWER_FLAT` seeds that view.

Headless captures use the same sunlight. Set
`SCULPTO_VIEWER_SUN_ELEVATION_DEG` within 2–83 for a repeatable starting
angle. `SCULPTO_VIEWER_AMBIENT` sets the ambient brightness.
`SCULPTO_VIEWER_GROUND=1` adds the matte ground and
`SCULPTO_VIEWER_TURNTABLE_YAW` sets the opening figure angle.
`0` selects the idle clip and `1` the walk clip of a rigged GLB (the actor graph
loads `Animation0` and `Animation1`; the cloth clip keeps running).
`--interactive --bank <manifest.json>` accepts a version-1 manifest with a nonempty
`slots` array. Each entry names a GLB asset, a display name and either a clip name or
`null` for rest. Plain digits select slots 0–9 immediately. Hold Ctrl while typing a
slot number. The viewer shows the digits and selects the slot 1.35 seconds after the
last digit, including after Ctrl is released. Numbers beyond the bank select the last
slot. An optional top-level `inspection` entry has the same fields and a null clip.
Ctrl+0 opens this asset and reports `bank.current = "inspection"`. Plain 0 opens slot 0.
The socket's numeric slot command requires a valid index.

In the Emily bank, 0 opens A-pose, 1 opens T-pose and 2–9 play named Mixamo clips.
Click the figure for keyboard focus. H toggles the controls card.
