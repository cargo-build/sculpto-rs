//! Headless Bevy proof: load the accepted actor GLB and the baked cloth GLB
//! (morph animation), render an offscreen frame, save it as PNG, exit.
//! Also provides `--headless --report --glb <path>` count reporting for consumer acceptance.
//! `--control <socket>` publishes the agent control socket (see `control.rs`).
mod bank;
mod control;
mod facets;
mod number_entry;

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::oit::{resolve::is_oit_supported, OrderIndependentTransparencySettings};
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::mesh::morph::{MorphWeights, MAX_MORPH_WEIGHTS};
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::renderer::{RenderAdapter, RenderDevice};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use serde::{Deserialize, Serialize};

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Resource)]
struct Units(usize);

#[derive(Resource)]
struct Bench(bool);

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PartReport {
    pub name: String,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub morph_targets: usize,
    pub joints: usize,
    pub material: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CountReport {
    pub profile: String,
    pub bevy_version: String,
    pub morph_limit: usize,
    pub morph_limit_verified: bool,
    pub nodes: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub materials: usize,
    pub joints: usize,
    pub parts: Vec<PartReport>,
}

#[derive(Resource, Clone, Debug)]
struct ViewerConfig {
    report_mode: bool,
    debug_scene: bool,
    interactive: bool,
    /// Seed for the review lighting: flat hides the light rig and renders
    /// unlit materials. `L` flips it at runtime in the interactive viewer.
    flat: bool,
    asset_file: String,
    /// Operator-facing status of the result on screen: `ReadyToShowProgress`
    /// when it is finished and verified, `InProgress` while it is not.
    status: String,
    /// Control socket path when `--control` is given, so the card and the
    /// agent name the same address.
    control: Option<String>,
}

/// Runtime lit/flat state; `SCULPTO_VIEWER_FLAT` seeds it and `L` flips it.
#[derive(Resource, Clone, Copy)]
struct FlatMode(bool);

/// Marks the review lights so flat mode can hide the rig without touching it.
#[derive(Component)]
struct ReviewLight;

/// Fill and rim lights for the optional three-point rig.
#[derive(Component)]
struct FillLight;

/// The directional sun casts the scene's shadow.
#[derive(Component)]
struct KeyLight;

/// Review ground: it exists only to receive the key light's shadow in the
/// interactive window, which is how a turntable turn proves the light is
/// runtime rather than baked onto the model.
#[derive(Component)]
struct ReviewGround;

/// Review rig: one key light by default; `F3` adds the fill/rim pair.
#[derive(Resource, Clone, Copy)]
struct RigMode {
    three_point: bool,
}

/// Materials flattened by the flat mode, so switching back restores them.
#[derive(Resource, Default)]
struct FlatFlipped(Vec<AssetId<StandardMaterial>>);

/// Opacity for **every** actor material while the skeleton overlay is on — body,
/// bra and panty alike. The operator's default is 85 % (2026-11-17),
/// so the figure stays readable as flesh with the bones visible through it.
///
/// The value is one number for one figure: parts are distinguished by their
/// own compositing, not by different alpha. `state.materials` reports what each
/// material actually carries.
const ACTOR_OPACITY: f32 = 0.85;

/// `B` cycles the skeleton overlay: off -> primary chain -> every driven joint.
/// Enabled at start by `SCULPTO_VIEWER_SKELETON=1` so a headless capture can
/// prove the mode.
#[derive(Resource, Clone, Copy, Default)]
struct SkelMode {
    level: u8,
}

impl SkelMode {
    fn on(&self) -> bool {
        self.level > 0
    }
}

/// Space pauses, Ctrl+, and Ctrl+. slow down and speed up the clip so a pose can
/// be studied instead of glimpsed.
#[derive(Resource)]
struct Playback {
    paused: bool,
    speed: f32,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            paused: false,
            speed: 1.0,
        }
    }
}

/// Pooled overlay entities: bones and joints are spawned once, then moved to
/// the live joint positions every frame. Spawning per frame cost more than the
/// review mode is worth.
#[derive(Resource, Default)]
struct SkelPool {
    built: bool,
    joints: Vec<Entity>,
    bones: Vec<Entity>,
}

/// Actor materials made translucent by the skeleton mode, restored on toggle.
#[derive(Resource, Default)]
struct GlassRestore(Vec<AssetId<StandardMaterial>>);

/// Marks a spawned review model so the turntable can rotate it while the key
/// light stays fixed in world space — that is what makes the shading and the
/// cast shadows sweep across the figure as it turns.
#[derive(Component)]
struct ReviewModel;

/// Direction and elevation of the review sun.
#[derive(Resource, Clone, Copy)]
struct SunOrbit {
    azimuth: f32,
    elevation: f32,
}

impl SunOrbit {
    const DEFAULT_ELEVATION_DEG: f32 = 55.0;
    const MIN_ELEVATION: f32 = 2.0_f32.to_radians();
    const MAX_ELEVATION: f32 = 83.0_f32.to_radians();

    fn new(elevation_degrees: f32) -> Self {
        let degrees = if elevation_degrees.is_finite() {
            elevation_degrees
        } else {
            Self::DEFAULT_ELEVATION_DEG
        };
        Self {
            azimuth: 3.0_f32.atan2(2.0),
            elevation: degrees
                .to_radians()
                .clamp(Self::MIN_ELEVATION, Self::MAX_ELEVATION),
        }
    }

    fn transform(self) -> Transform {
        let (height, horizontal) = self.elevation.sin_cos();
        let (east, north) = self.azimuth.sin_cos();
        let toward_sun = Vec3::new(horizontal * east, height, horizontal * north);
        Transform::IDENTITY.looking_to(-toward_sun, Vec3::Y)
    }

    fn lighting(self) -> (Color, f32) {
        let height = (self.elevation.to_degrees() / 45.0).clamp(0.0, 1.0);
        let daylight = height * height * (3.0 - 2.0 * height);
        let color = Color::linear_rgb(1.0, 0.40 + 0.60 * daylight, 0.18 + 0.82 * daylight);
        let illuminance = 3_500.0 + 3_000.0 * daylight;
        (color, illuminance)
    }
}

/// Character pose under the fixed light: Ctrl+LMB rotates the figure, Ctrl+RMB
/// moves it sideways; the default mouse keeps the camera controls.
#[derive(Resource, Clone, Copy)]
struct CharacterPose {
    yaw: f32,
    pitch: f32,
    offset: Vec3,
}

/// The model's spawn translation, so a character offset is relative to home.
#[derive(Component, Clone, Copy)]
struct ModelHome(Vec3);

/// The top-left controls card; `H` hides or shows it.
#[derive(Component)]
struct HelpOverlay;

/// Actual glTF clip index; usize::MAX selects the asset's unanimated rest.
#[derive(Resource, Clone, Copy)]
struct ClipSelection(usize);

/// Root joint held at its loaded translation, so a clip plays without carrying the
/// figure across the floor (operator, 2026-11-16: a preview walks in place — world
/// displacement hides joint motion behind a moving camera).
#[derive(Component, Clone, Copy)]
struct InPlaceRoot {
    translation: Vec3,
}

/// The control socket as a Bevy resource. The socket module stays free of
/// Bevy so its protocol tests run without an app; this is the one adapter.
#[derive(Resource)]
struct ControlHandle(control::ControlServer);

/// `SCULPTO_VIEWER_IN_PLACE=0` restores world displacement for a deliberate check.
#[derive(Resource, Clone, Copy)]
struct InPlaceEnabled(bool);

/// Tag each skeleton's topmost bone once, at its loaded translation.
fn tag_in_place_roots(
    mut commands: Commands,
    skins: Query<&SkinnedMesh>,
    transforms: Query<&Transform>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    tagged: Query<(), With<InPlaceRoot>>,
) {
    let joint: std::collections::HashSet<Entity> = skins
        .iter()
        .flat_map(|skin| skin.joints.iter().copied())
        .collect();
    let debug = std::env::var("SCULPTO_VIEWER_IN_PLACE_DEBUG").is_ok();
    // The mover is the skin's root joint, not the armature node above it: clamping
    // the armature left the hips free to carry the figure across the floor.
    for entity in &joint {
        if tagged.get(*entity).is_ok() {
            continue;
        }
        let parent_is_joint = parents
            .get(*entity)
            .map(|parent| joint.contains(&parent.parent()))
            .unwrap_or(false);
        if parent_is_joint {
            continue;
        }
        let Ok(transform) = transforms.get(*entity) else {
            continue;
        };
        if debug {
            let name = names
                .get(*entity)
                .map(|n| n.as_str().to_string())
                .unwrap_or_else(|_| format!("{entity}"));
            eprintln!("in-place: joint root {name} at {:?}", transform.translation);
        }
        commands.entity(*entity).insert(InPlaceRoot {
            translation: transform.translation,
        });
    }
}

/// Restore the root translation after the animation pass; rotation stays animated.
fn hold_roots_in_place(
    enabled: Res<InPlaceEnabled>,
    mut roots: Query<(&InPlaceRoot, &mut Transform)>,
) {
    if !enabled.0 {
        return;
    }
    for (root, mut transform) in &mut roots {
        transform.translation = root.translation;
    }
}

// --- Live reload (`--watch <dir> --stage <name>`) -------------------------
// Agents write `<dir>/<stage>.glb` plus `<dir>/<stage>.json` every few
// minutes; the viewer polls the file and swaps it in place so the operator
// watches the work happen. The filesystem half below is plain std so the
// poll, staging and sidecar logic is testable headless; the Bevy half
// (`watch_tick` / `watch_card`) swaps the scene root when the staged world
// has loaded.

/// How often to `stat` the watched file. Polling keeps the standalone viewer
/// dependency-free — no file-watcher crate and no platform feature flag —
/// and an unchanged poll is one metadata read, not a file read.
const POLL_INTERVAL_SECS: f64 = 0.5;

/// Watch directory used when `--watch` is given without a value.
const DEFAULT_WATCH_DIR: &str = "/tmp/opencode/live";

/// Minimal SHA-256 (FIPS 180-4) for the live card's content fingerprint —
/// the hash of the bytes the viewer actually staged, not a sidecar claim.
/// Hand-rolled so the standalone viewer stays dependency-free; pinned by the
/// standard vectors in the tests and cross-checked against `sha256sum` in
/// the task evidence.
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (index, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        for (i, k) in K.iter().enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, round) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(round);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

/// The 8-character display form of a digest. The card and the title show the
/// short form; the control socket reports the whole hash.
fn sha_short(sha: &str) -> &str {
    match sha.char_indices().nth(8) {
        Some((index, _)) => &sha[..index],
        None => sha,
    }
}

/// The `<stage>.json` sidecar an agent writes next to the GLB: status label,
/// sha and a one-line note. Only the note is displayed; the sha in the card
/// is always this viewer's own hash of the staged bytes, never the sidecar's.
enum SidecarNote {
    Missing,
    Unparsable,
    Empty,
    Note(String),
}

impl SidecarNote {
    fn display(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Missing => "no sidecar".into(),
            Self::Unparsable => "sidecar unparsable".into(),
            Self::Empty => "no sidecar note".into(),
            Self::Note(note) => std::borrow::Cow::Borrowed(note),
        }
    }
}

fn read_sidecar_note(path: &std::path::Path) -> SidecarNote {
    let Ok(text) = std::fs::read_to_string(path) else {
        return SidecarNote::Missing;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return SidecarNote::Unparsable;
    };
    match value.get("note").and_then(|note| note.as_str()) {
        Some(note) if !note.trim().is_empty() => SidecarNote::Note(note.trim().to_string()),
        _ => SidecarNote::Empty,
    }
}

/// The watched stage file and its staged copies. The asset server caches by
/// path, so a changed file is copied to a fresh `live-<n>.glb` in the asset
/// root before loading — the same path would never reload.
struct WatchFiles {
    dir: std::path::PathBuf,
    stage: String,
    staging_dir: std::path::PathBuf,
    seq: u64,
    /// (mtime, len) of the watched file already staged or attempted, so a
    /// failed load is not retried until the file changes again.
    seen: Option<(std::time::SystemTime, u64)>,
}

/// One staged generation: the asset-relative copy name, the sha of the bytes
/// actually copied, and the sidecar note read beside the watched file.
struct StagedFile {
    seq: u64,
    asset_name: String,
    sha: String,
    note: SidecarNote,
}

impl WatchFiles {
    fn watched_path(&self) -> std::path::PathBuf {
        self.dir.join(format!("{}.glb", self.stage))
    }

    fn sidecar_path(&self) -> std::path::PathBuf {
        self.dir.join(format!("{}.json", self.stage))
    }

    fn staged_path(&self, seq: u64) -> std::path::PathBuf {
        self.staging_dir.join(format!("live-{seq}.glb"))
    }

    /// One poll: `stat` the watched file; nothing to do unless mtime or size
    /// changed. A change is read fully, copied to `live-<seq>.glb` (write to
    /// a temp name, then rename — a reader never sees a half copy) and
    /// hashed. A torn write usually fails later at GLB parse, where the
    /// previous scene is kept; the next change retries.
    fn poll(&mut self) -> Option<StagedFile> {
        let path = self.watched_path();
        let metadata = std::fs::metadata(&path).ok()?;
        let mtime = metadata.modified().ok()?;
        let key = (mtime, metadata.len());
        if self.seen.as_ref() == Some(&key) {
            return None;
        }
        self.seen = Some(key);
        let bytes = std::fs::read(&path).ok()?;
        let seq = self.seq;
        let staged = self.staged_path(seq);
        let tmp = self.staging_dir.join(format!(".live-{seq}.tmp"));
        std::fs::write(&tmp, &bytes).ok()?;
        std::fs::rename(&tmp, &staged).ok()?;
        self.seq += 1;
        Some(StagedFile {
            seq,
            asset_name: format!("live-{seq}.glb"),
            sha: sha256_hex(&bytes),
            note: read_sidecar_note(&self.sidecar_path()),
        })
    }

    /// Drop a staged copy whose generation is no longer needed: a failed
    /// load, or the replaced generation after a successful swap.
    fn remove_staged(&self, seq: u64) {
        let _ = std::fs::remove_file(self.staged_path(seq));
    }
}

/// A load in flight.
struct PendingScene {
    seq: u64,
    asset_name: String,
    world: Handle<bevy::world_serialization::WorldAsset>,
    sha: String,
    note: SidecarNote,
    bank_slot: Option<bank::Selection>,
}

/// Live reload state; present only when `--watch` is given, so every system
/// is a no-op without it.
#[derive(Resource)]
struct WatchState {
    files: WatchFiles,
    pending: Option<PendingScene>,
    /// Scene root on screen; despawned on the next successful swap.
    current_root: Option<Entity>,
    /// Staged sequence number of the generation on screen; its copy is
    /// removed when the next generation swaps in.
    current_seq: Option<u64>,
    /// Spawn transform for a reloaded figure: setup's home and source scale,
    /// so a swapped-in body lands exactly where the first one stood.
    home: Vec3,
    scale: f32,
    /// Seconds since startup — the poll clock and the smoke-run exit timer.
    elapsed: f64,
    next_poll: f64,
    /// `SCULPTO_VIEWER_WATCH_EXIT_SECS`: headless smoke runs only.
    exit_after: Option<f64>,
    /// The generation on screen: its full sha and sidecar note.
    sha: String,
    note: String,
    /// The last attempt failed to load — keep the old scene and say so.
    failed: bool,
    /// A stage file existed at startup, so setup spawned it directly.
    ready: bool,
    /// Cached card/title line, so Text is written only on change.
    last_line: String,
}

impl WatchState {
    /// The truth line: status first (the niri focus rules match
    /// `^diagnostic · ` titles), then stage, the sha of what is on screen
    /// and the sidecar note.
    fn line(&self, status: &str) -> String {
        if self.failed {
            return format!(
                "{} · live {} {} · load failed — kept previous",
                status,
                self.files.stage,
                sha_short(&self.sha)
            );
        }
        live_line(status, &self.files.stage, sha_short(&self.sha), &self.note)
    }
}

fn live_line(status: &str, stage: &str, sha8: &str, note: &str) -> String {
    let sha8 = if sha8.is_empty() { "—" } else { sha8 };
    format!("{status} · live {stage} {sha8} · {note}")
}

/// Marks the live-status line in the controls card.
#[derive(Component)]
struct WatchLine;

struct CliOptions {
    report: bool,
    headless: bool,
    debug_scene: bool,
    interactive: bool,
    glb_path: Option<String>,
    units: usize,
    bench: bool,
    uncapped: bool,
    /// `--watch <dir>`: live-reload the actor from `<dir>/<stage>.glb`.
    watch: Option<std::path::PathBuf>,
    /// `--stage <name>`: the stage file name to watch (required with --watch).
    stage: Option<String>,
    /// `--control <socket>`: publish the agent control socket at this path.
    control: Option<std::path::PathBuf>,
    bank: Option<std::path::PathBuf>,
}

/// The asset the viewer is showing: identity for the control socket.
///
/// `sha256` is the hash of the file's bytes at load time — or, in watch mode,
/// of the generation on screen — so an agent can prove which artifact it reads
/// instead of trusting a path.
#[derive(Resource)]
struct AssetProfile {
    path: std::path::PathBuf,
    file: String,
    sha256: String,
    /// A scene mesh from this file is on screen. A spawned scene root alone is
    /// not evidence: the root exists before its scene parses.
    loaded: bool,
    /// A newer watch generation is staged and still loading, so the digest
    /// above describes the generation on screen, not the one arriving.
    pending: bool,
    /// The newest staged file did not parse; the previous scene is still shown.
    failed: bool,
    /// Staged generation index on screen, so a reload of identical bytes still
    /// moves the reported file name to the copy actually loaded.
    seq: Option<u64>,
    /// Scene mesh entities counted while waiting for `loaded`.
    meshes: usize,
    scale: f32,
    /// Animation clips the file on screen actually carries. The player graph
    /// always holds idle/walk nodes, so its length is not this number.
    clips: Option<usize>,
}

fn parse_args() -> CliOptions {
    let mut args = std::env::args().skip(1);
    let mut options = CliOptions {
        report: std::env::var("SCULPTO_VIEWER_REPORT").is_ok(),
        headless: std::env::var("SCULPTO_VIEWER_HEADLESS").is_ok(),
        debug_scene: std::env::var("SCULPTO_VIEWER_DEBUG_SCENE").is_ok(),
        interactive: std::env::var("SCULPTO_VIEWER_INTERACTIVE").is_ok(),
        glb_path: std::env::var("SCULPTO_VIEWER_GLB")
            .ok()
            .or_else(|| std::env::var("SCULPTO_VIEWER_ASSET").ok()),
        units: env_usize("SCULPTO_VIEWER_UNITS", 1),
        bench: std::env::var("SCULPTO_VIEWER_BENCH").is_ok(),
        uncapped: std::env::var("SCULPTO_VIEWER_UNCAPPED").is_ok(),
        watch: std::env::var("SCULPTO_VIEWER_WATCH")
            .ok()
            .map(std::path::PathBuf::from),
        stage: std::env::var("SCULPTO_VIEWER_STAGE").ok(),
        control: std::env::var("SCULPTO_VIEWER_CONTROL")
            .ok()
            .map(std::path::PathBuf::from),
        bank: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--report" => options.report = true,
            "--headless" => options.headless = true,
            "--debug-scene" => options.debug_scene = true,
            "--interactive" => options.interactive = true,
            "--bench" => options.bench = true,
            "--uncapped" => options.uncapped = true,
            // A bare `--watch` uses the default staging directory; a following
            // flag is not swallowed as a directory value.
            "--watch" => {
                let value = args.next().filter(|value| !value.starts_with('-'));
                options.watch = Some(value.map_or_else(
                    || std::path::PathBuf::from(DEFAULT_WATCH_DIR),
                    std::path::PathBuf::from,
                ));
            }
            "--stage" => {
                if let Some(val) = args.next() {
                    options.stage = Some(val);
                }
            }
            "--control" => {
                if let Some(val) = args.next().filter(|value| !value.starts_with('-')) {
                    options.control = Some(std::path::PathBuf::from(val));
                }
            }
            "--bank" => {
                options.bank = Some(
                    args.next()
                        .filter(|value| !value.starts_with('-'))
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| {
                            eprintln!("viewer: --bank requires a manifest path");
                            std::process::exit(2);
                        }),
                );
            }
            "--glb" => {
                if let Some(val) = args.next() {
                    options.glb_path = Some(val);
                }
            }
            "--asset" => {
                if let Some(val) = args.next() {
                    options.glb_path = Some(val);
                }
            }
            "--units" => {
                if let Some(val) = args.next().and_then(|v| v.parse().ok()) {
                    options.units = val;
                }
            }
            _ => {}
        }
    }
    options
}

/// Number of animations a GLB carries, read from its JSON chunk.
///
/// A requested clip index the file does not have used to play nothing and leave
/// the figure in its rest pose, which reads as "the clip is broken" rather than
/// "the index is wrong". Reporting the real count keeps the preview honest.
fn glb_animation_count(path: &std::path::Path) -> Option<usize> {
    glb_animation_count_bytes(&std::fs::read(path).ok()?)
}

/// The animation count of already-read GLB bytes, so a startup hash and a
/// clip count cost one file read, not two.
fn glb_animation_count_bytes(bytes: &[u8]) -> Option<usize> {
    glb_animation_names_bytes(bytes).map(|names| names.len())
}

fn glb_animation_names_bytes(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return None;
    }
    let chunk_len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let chunk_type = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    if chunk_type != 0x4E4F_534A || 20 + chunk_len > bytes.len() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + chunk_len]).ok()?;
    // A GLB without an `animations` array carries zero clips; reporting `null`
    // there would read as "unknown" to an agent.
    Some(
        json.get("animations")
            .and_then(|value| value.as_array())
            .map(|animations| {
                animations
                    .iter()
                    .enumerate()
                    .map(|(index, animation)| {
                        animation
                            .get("name")
                            .and_then(|name| name.as_str())
                            .map_or_else(|| format!("Animation{index}"), str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn main() {
    let mut options = parse_args();
    let bank = options.bank.as_ref().map(|path| {
        if options.watch.is_some()
            || options.glb_path.is_some()
            || options.report
            || options.units != 1
        {
            eprintln!(
                "viewer: --bank requires a single live actor, without --watch/--glb/--report"
            );
            std::process::exit(2);
        }
        bank::Bank::load(path).unwrap_or_else(|error| {
            eprintln!("viewer: bank: {error}");
            std::process::exit(2);
        })
    });
    let (mut asset_dir, mut asset_file) = if let Some(ref glb) = options.glb_path {
        let p = std::path::Path::new(glb);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        };
        let parent = abs.parent().unwrap_or(std::path::Path::new("."));
        let filename = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.glb")
            .to_string();
        (parent.to_string_lossy().to_string(), filename)
    } else {
        (
            "assets".to_string(),
            env_or("SCULPTO_VIEWER_ASSET", "combined.glb"),
        )
    };
    let bank_staging = bank.as_ref().map(|bank| {
        let initial = &bank.slots[0];
        options.watch = initial.asset.parent().map(std::path::Path::to_path_buf);
        options.stage = initial
            .asset
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned());
        let dir = std::env::temp_dir().join(format!("sculpto-viewer-bank-{}", std::process::id()));
        asset_dir = dir.display().to_string();
        dir
    });

    // The ordinary proof runs at 60 Hz so captures land on stable ticks. The
    // units benchmark and report modes must not be capped: a 16.7 ms sleep would
    // hide the real playback cost or slow down tests.
    let interval = if options.uncapped || options.report {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs_f64(1.0 / 60.0)
    };

    // Live reload: `--watch <dir> --stage <name>` shows `<dir>/<stage>.glb`
    // and re-loads it whenever the file changes, with the stage, sha and
    // sidecar note in the title and card. The watched file names the asset
    // itself, so --glb/--asset and --report are refused together with it.
    if options.report && options.watch.is_some() {
        eprintln!("viewer: --watch is a live window mode and cannot run with --report");
        std::process::exit(2);
    }
    let watch_state = if let Some(ref dir) = options.watch {
        if options.glb_path.is_some() {
            eprintln!(
                "viewer: --watch names the asset itself; do not combine it with --glb/--asset"
            );
            std::process::exit(2);
        }
        let stage = match options.stage.clone() {
            Some(stage) => stage,
            None => {
                eprintln!("viewer: --watch needs --stage <name> (agents write <dir>/<stage>.glb)");
                std::process::exit(2);
            }
        };
        if stage.is_empty() || stage.contains('/') || stage.contains('\\') || stage.contains("..") {
            eprintln!("viewer: --stage must be a plain file name, got {stage:?}");
            std::process::exit(2);
        }
        if std::fs::create_dir_all(dir).is_err() {
            eprintln!(
                "viewer: watch directory {} cannot be created",
                dir.display()
            );
            std::process::exit(2);
        }
        // Bevy's file asset source resolves a relative `file_path` against
        // BEVY_ASSET_ROOT, then CARGO_MANIFEST_DIR, then the executable's
        // directory (bevy_asset's `get_base_path`) — stage into that same
        // root, or `asset_server.load("live-N.glb")` looks somewhere else.
        let staging_root = if let Ok(root) = std::env::var("BEVY_ASSET_ROOT") {
            std::path::PathBuf::from(root)
        } else if let Ok(root) = std::env::var("CARGO_MANIFEST_DIR") {
            std::path::PathBuf::from(root)
        } else {
            match std::env::current_exe() {
                Ok(exe) => match exe.parent() {
                    Some(dir) => dir.to_path_buf(),
                    None => {
                        eprintln!("viewer: cannot resolve the executable's directory");
                        std::process::exit(2);
                    }
                },
                Err(error) => {
                    eprintln!("viewer: cannot resolve the asset root: {error}");
                    std::process::exit(2);
                }
            }
        };
        let staging_dir = staging_root.join(&asset_dir);
        if std::fs::create_dir_all(&staging_dir).is_err() {
            eprintln!(
                "viewer: staging directory {} cannot be created",
                staging_dir.display()
            );
            std::process::exit(2);
        }
        // The staged copies are process-local artifacts, never sources.
        let _ = std::fs::write(staging_dir.join(".gitignore"), "*\n");
        let mut state = WatchState {
            files: WatchFiles {
                dir: dir.clone(),
                stage: stage.clone(),
                staging_dir,
                seq: 0,
                seen: None,
            },
            pending: None,
            current_root: None,
            current_seq: None,
            home: Vec3::ZERO,
            scale: env_f32("SCULPTO_VIEWER_SCALE", 1.0),
            elapsed: 0.0,
            next_poll: POLL_INTERVAL_SECS,
            exit_after: std::env::var("SCULPTO_VIEWER_WATCH_EXIT_SECS")
                .ok()
                .and_then(|value| value.parse().ok()),
            sha: String::new(),
            note: String::new(),
            failed: false,
            ready: false,
            last_line: String::new(),
        };
        if let Some(staged) = state.files.poll() {
            eprintln!(
                "viewer: live start {} sha {} sidecar {}",
                staged.asset_name,
                sha_short(&staged.sha),
                staged.note.display()
            );
            asset_file = staged.asset_name;
            state.sha = staged.sha;
            state.note = staged.note.display().to_string();
            state.ready = true;
            state.current_seq = Some(0);
        } else {
            state.note = format!("waiting for {stage}.glb");
        }
        Some(state)
    } else {
        None
    };
    if options.watch.is_some() && options.units != 1 {
        eprintln!("viewer: --watch shows one stage file; units forced to 1");
        options.units = 1;
    }

    // A clip index the file does not carry is a caller error, not a broken clip.
    // Say it in the status line and fall back to the last clip that exists.
    let asset_path = std::path::Path::new(&asset_dir).join(&asset_file);
    let mut requested_clip = bank
        .as_ref()
        .map(|bank| bank.slots[0].clip.unwrap_or(usize::MAX))
        .or_else(|| {
            std::env::var("SCULPTO_VIEWER_ACTOR_ANIMATION")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
        });
    let clip_note = match (requested_clip, glb_animation_count(&asset_path)) {
        (Some(index), Some(count)) if bank.is_none() && count > 0 && index >= count => {
            let fallback = count - 1;
            requested_clip = Some(fallback);
            std::env::set_var("SCULPTO_VIEWER_ACTOR_ANIMATION", fallback.to_string());
            eprintln!(
                "viewer: requested actor clip {index}, file carries {count} — showing {fallback}"
            );
            format!(" [no clip {index}: file has {count}, showing {fallback}]")
        }
        _ => String::new(),
    };

    // The control socket drives a live window or a headless capture; `--report`
    // prints its counts and exits before a socket could answer once.
    if options.control.is_some() && options.report {
        eprintln!("viewer: --control needs a running viewer; it cannot combine with --report");
        std::process::exit(2);
    }
    // A missing asset used to render a black frame that looked like a result.
    // Say it at startup instead; `--watch` legitimately waits for its first file.
    if options.watch.is_none() && !asset_path.exists() {
        eprintln!(
            "viewer: asset not found: {} (pass --glb <path> or set SCULPTO_VIEWER_ASSET)",
            asset_path.display()
        );
        std::process::exit(2);
    }
    // Identity of what is on screen. Watch mode already hashed the staged
    // generation it loaded; the single-asset modes hash the file once here.
    let (digest_path, digest, clips) = match watch_state.as_ref() {
        Some(watch) if !watch.sha.is_empty() => {
            let path = watch.files.staging_dir.join(
                watch
                    .current_seq
                    .map_or_else(|| asset_file.clone(), |seq| format!("live-{seq}.glb")),
            );
            let clips = glb_animation_count(&path);
            (path, watch.sha.clone(), clips)
        }
        _ => match std::fs::read(&asset_path) {
            Ok(bytes) => (
                asset_path.clone(),
                sha256_hex(&bytes),
                glb_animation_count_bytes(&bytes),
            ),
            Err(_) => (asset_path.clone(), String::new(), None),
        },
    };
    let asset_profile = AssetProfile {
        path: digest_path,
        file: asset_file.clone(),
        sha256: digest,
        loaded: false,
        pending: false,
        failed: false,
        seq: None,
        meshes: 0,
        scale: env_f32("SCULPTO_VIEWER_SCALE", 1.0),
        clips,
    };
    let control_server = match options.control.as_deref() {
        Some(path) => match control::start(path) {
            Ok(server) => {
                eprintln!("viewer: control socket {}", server.path.display());
                Some(server)
            }
            Err(error) => {
                eprintln!(
                    "viewer: control socket {} cannot be created: {error}",
                    path.display()
                );
                std::process::exit(2);
            }
        },
        None => None,
    };
    let control_path = control_server
        .as_ref()
        .map(|server| server.path.display().to_string());

    let config = ViewerConfig {
        report_mode: options.report,
        debug_scene: options.debug_scene,
        interactive: options.interactive,
        flat: std::env::var("SCULPTO_VIEWER_FLAT").is_ok(),
        asset_file,
        status: format!(
            "{}{}",
            std::env::var("SCULPTO_VIEWER_STATUS").unwrap_or_else(|_| "InProgress".to_string()),
            clip_note
        ),
        control: control_path,
    };
    let asset_plugin = AssetPlugin {
        file_path: asset_dir,
        ..Default::default()
    };

    let mut app = App::new();
    // The live line carries the truth (status, stage, sha, sidecar note) and
    // stays status-first, so the niri focus rules keep matching the title.
    let headline = match watch_state.as_ref() {
        Some(watch) => watch.line(&config.status),
        None => config.status.clone(),
    };
    if options.interactive {
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: match options.bank.is_some() {
                            true => format!("Sculpto cloth review — {headline}"),
                            false => format!("{headline} — Sculpto cloth review"),
                        },
                        resolution: (1280, 900).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default())
                .set(asset_plugin),
        );
    } else {
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(interval))
            .add_plugins(
                DefaultPlugins
                    .set(WindowPlugin {
                        primary_window: None,
                        primary_cursor_options: None,
                        exit_condition: bevy::window::ExitCondition::DontExit,
                        close_when_requested: false,
                    })
                    .set(ImagePlugin::default())
                    .set(asset_plugin),
            );
    }

    if let Some(state) = watch_state {
        app.insert_resource(state);
    }
    if let Some(server) = control_server {
        app.insert_resource(ControlHandle(server));
    }
    if let Some(bank) = bank {
        app.insert_resource(bank);
    }
    app.insert_resource(asset_profile)
        .insert_resource(ActorMaterials::default())
        .insert_resource(FlatFlipped::default())
        .insert_resource(ClipSelection(requested_clip.unwrap_or(0)))
        .insert_resource(InPlaceEnabled(
            std::env::var("SCULPTO_VIEWER_IN_PLACE").map_or(true, |value| value != "0"),
        ))
        .insert_resource(SunOrbit::new(env_f32(
            "SCULPTO_VIEWER_SUN_ELEVATION_DEG",
            SunOrbit::DEFAULT_ELEVATION_DEG,
        )))
        .insert_resource(CharacterPose {
            yaw: env_f32("SCULPTO_VIEWER_TURNTABLE_YAW", 0.0),
            pitch: 0.0,
            offset: Vec3::ZERO,
        })
        .insert_resource(FlatMode(config.flat))
        .init_resource::<number_entry::NumberEntry>()
        .insert_resource(facets::FaceNormals(
            std::env::var_os("SCULPTO_VIEWER_FACE_NORMALS").is_some_and(|value| value != "0"),
        ))
        .insert_resource(RigMode {
            three_point: std::env::var("SCULPTO_VIEWER_THREE_POINT").is_ok(),
        })
        .insert_resource(SkelMode {
            // 1 = primary chain, 2 = every driven joint; any other value is off.
            // The operator's rule: never show a translucent body without its
            // bones. The overlay is on by default; `B` cycles it and
            // SCULPTO_VIEWER_SKELETON=0 is the explicit opt-out.
            level: std::env::var("SCULPTO_VIEWER_SKELETON")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(2),
        })
        .insert_resource(Playback::default())
        .insert_resource(SkelPool::default())
        .insert_resource(GlassRestore::default())
        .insert_resource(Units(options.units))
        .insert_resource(Bench(options.bench))
        .insert_resource(config)
        .register_type::<Transform>()
        .register_type::<GlobalTransform>()
        .register_type::<bevy::transform::components::TransformTreeChanged>()
        .register_type::<bevy::ecs::hierarchy::ChildOf>()
        .register_type::<bevy::ecs::hierarchy::Children>()
        .register_type::<bevy::gltf::GltfExtras>()
        .register_type::<bevy::gltf::GltfSceneExtras>()
        .register_type::<Mesh3d>()
        .register_type::<MeshMaterial3d<StandardMaterial>>()
        .register_type::<bevy::camera::visibility::Visibility>()
        .register_type::<bevy::animation::AnimationPlayer>()
        .register_type::<bevy::animation::AnimationTargetId>()
        .register_type::<bevy::animation::AnimatedBy>()
        .register_type::<bevy::camera::primitives::Aabb>()
        .register_type::<bevy::gltf::GltfMaterialExtras>()
        .register_type::<bevy::gltf::GltfMeshName>()
        .register_type::<bevy::gltf::GltfMaterialName>()
        .register_type::<bevy::gltf::GltfSceneName>()
        .register_type::<bevy::mesh::skinning::SkinnedMesh>()
        .register_type::<bevy::mesh::skinning::SkinnedMeshBounds>()
        .register_type::<bevy::camera::visibility::DynamicSkinnedMeshBounds>()
        .register_type::<bevy::mesh::morph::MeshMorphWeights>()
        .register_type::<bevy::gltf::GltfMeshExtras>()
        .register_type::<bevy::mesh::morph::MorphWeights>()
        .register_type::<Name>()
        .register_type::<Visibility>()
        .register_type::<InheritedVisibility>()
        .register_type::<ViewVisibility>()
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.88, 0.93, 1.0),
            brightness: env_f32("SCULPTO_VIEWER_AMBIENT", 125.0),
            ..default()
        })
        .init_resource::<SkelJointsInfo>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                report_counts,
                advance_animation,
                select_clip,
                capture_once,
                benchmark,
                debug_scene,
                debug_playback,
                fit_camera_to_scene,
                orbit_controls,
                light_controls,
                toggle_flat,
                apply_flat_mode,
                toggle_rig,
                toggle_skeleton,
                apply_skeleton_mode,
                draw_skeleton,
                apply_review_visibility,
                character_controls,
                apply_character_pose,
                toggle_help,
            ),
        )
        // Separate call: the tuple above is already at Bevy's arity limit.
        .add_systems(Update, (watch_tick, watch_card))
        .add_systems(
            Update,
            number_entry::read
                .before(bank_keyboard)
                .before(select_clip)
                .before(hover_joint_name),
        )
        .add_systems(Update, bank_keyboard.before(watch_tick))
        .add_systems(Update, clip_label.after(watch_tick))
        .add_systems(Update, hover_joint_name)
        .add_systems(Update, facets::update)
        .add_systems(Update, track_asset_loaded)
        .add_systems(Update, collect_actor_materials.after(apply_skeleton_mode))
        // The socket reads joints after the overlay refreshed them and writes
        // the camera, the clip and the playback the frame's own systems read.
        .add_systems(
            Update,
            (control_service,)
                .after(draw_skeleton)
                .before(control_playback)
                .before(apply_playback)
                .before(orbit_controls),
        )
        .add_systems(Update, (control_playback, apply_playback))
        .add_systems(Update, tag_in_place_roots)
        // After the animation pass, before transform propagation, so the held
        // translation is what the skin actually renders this frame.
        .add_systems(
            PostUpdate,
            hold_roots_in_place
                .after(bevy::app::AnimationSystems)
                .before(bevy::transform::TransformSystems::Propagate),
        )
        .run();
    if let Some(dir) = bank_staging {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Ctrl+LMB turns the figure and Ctrl+RMB moves it sideways; the default mouse
/// keeps the camera controls. The light stays world-fixed, so a character move
/// changes the shading and the cast shadow.
fn character_controls(
    config: Res<ViewerConfig>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    pose: Option<ResMut<CharacterPose>>,
    cameras: Query<&Transform, With<Camera3d>>,
) {
    if !config.interactive {
        return;
    }
    let Some(mut pose) = pose else {
        return;
    };
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if !control {
        return;
    }
    let mut delta = Vec2::ZERO;
    for event in motion.read() {
        delta += event.delta;
    }
    if buttons.pressed(MouseButton::Left) {
        pose.yaw -= delta.x * 0.005;
        pose.pitch = (pose.pitch - delta.y * 0.005).clamp(-1.2, 1.2);
    }
    if buttons.pressed(MouseButton::Right) {
        let Ok(camera) = cameras.single() else {
            return;
        };
        let right = camera.rotation * Vec3::X;
        let up = camera.rotation * Vec3::Y;
        pose.offset += right * (delta.x * 0.002) - up * (delta.y * 0.002);
    }
}

/// Applies the character pose every frame. The model's home is its spawn
/// translation, so an offset moves it relative to where the scene put it.
fn apply_character_pose(
    pose: Res<CharacterPose>,
    mut models: Query<(&mut Transform, &ModelHome), With<ReviewModel>>,
) {
    let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, pose.yaw, pose.pitch, 0.0);
    for (mut transform, home) in &mut models {
        transform.rotation = rotation;
        transform.translation = home.0 + pose.offset;
    }
}

/// `L` flips lit/flat in the interactive viewer; the capture path keeps its
/// seeded state.
fn toggle_flat(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut flat: ResMut<FlatMode>,
) {
    if config.interactive && keys.just_pressed(KeyCode::KeyL) {
        flat.0 = !flat.0;
    }
}

/// `3` adds the fill/rim pair to the single key light; the default rig stays
/// one source so the runtime shading and shadow are not filled away.
fn toggle_rig(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut rig: ResMut<RigMode>,
) {
    if config.interactive && keys.just_pressed(KeyCode::F3) {
        rig.three_point = !rig.three_point;
    }
}

#[derive(Component)]
struct SkelJoint;

/// Active overlay joint names + world positions, refreshed each frame for hover picking.
#[derive(Resource, Default)]
struct SkelJointsInfo(Vec<(String, Vec3)>);

/// UI label showing the hovered joint's name.
#[derive(Component)]
struct HoverLabel;

#[derive(Component)]
struct SkelBone;

/// `B` toggles the skeleton overlay; the body turns translucent so the bones
/// read through it without hiding the surface they drive.
fn toggle_skeleton(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut skel: ResMut<SkelMode>,
) {
    if config.interactive && keys.just_pressed(KeyCode::KeyB) {
        skel.level = (skel.level + 1) % 3;
    }
}

/// Space pauses the clip; Ctrl+, and Ctrl+. halve and double the playback rate.
fn control_playback(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut playback: ResMut<Playback>,
) {
    if !config.interactive {
        return;
    }
    if keys.just_pressed(KeyCode::Space) {
        playback.paused = !playback.paused;
    }
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if control && keys.just_pressed(KeyCode::Comma) {
        playback.speed = (playback.speed * 0.5).max(0.05);
    }
    if control && keys.just_pressed(KeyCode::Period) {
        playback.speed = (playback.speed * 2.0).min(4.0);
    }
}

/// Apply the playback state to every animation player that exists.
fn apply_playback(playback: Res<Playback>, mut players: Query<&mut AnimationPlayer>) {
    for mut player in &mut players {
        if playback.paused {
            player.pause_all();
        } else {
            player.resume_all();
        }
        for (_, animation) in player.playing_animations_mut() {
            animation.set_speed(playback.speed);
        }
    }
}

/// Alpha the actor materials while the overlay is on and restore them after.
/// glTF materials stream in after startup, so the "on" branch runs every frame.
#[allow(clippy::type_complexity)]
fn apply_skeleton_mode(
    skel: Res<SkelMode>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut restore: ResMut<GlassRestore>,
    actors: Query<
        &MeshMaterial3d<StandardMaterial>,
        (Without<ReviewGround>, Without<SkelJoint>, Without<SkelBone>),
    >,
) {
    if skel.on() {
        for handle in &actors {
            let id = handle.id();
            if let Some(mut material) = materials.get_mut(id) {
                // The default opacity is shared so the body
                // reads as a body while the bones still show through it.
                if material.base_color.alpha() != ACTOR_OPACITY
                    || material.alpha_mode != AlphaMode::Blend
                {
                    material.base_color = material.base_color.with_alpha(ACTOR_OPACITY);
                    material.alpha_mode = AlphaMode::Blend;
                }
                if !restore.0.contains(&id) {
                    restore.0.push(id);
                }
            }
        }
    } else if !restore.0.is_empty() {
        for id in restore.0.drain(..) {
            if let Some(mut material) = materials.get_mut(id) {
                material.base_color = material.base_color.with_alpha(1.0);
                material.alpha_mode = AlphaMode::Opaque;
            }
        }
    }
}

/// Build the overlay once, then move it to the live joint positions every frame:
/// a sphere per joint and a cylinder from each joint to its parent bone.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn draw_skeleton(
    mut commands: Commands,
    skel: Res<SkelMode>,
    mut pool: ResMut<SkelPool>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut info: ResMut<SkelJointsInfo>,
    nodes: Query<(Entity, &Name, &GlobalTransform, Option<&ChildOf>)>,
    skins: Query<&SkinnedMesh>,
    mut overlay: Query<(&mut Transform, &mut Visibility), Or<(With<SkelJoint>, With<SkelBone>)>>,
) {
    if !pool.built {
        let joint_mesh = meshes.add(Sphere::new(0.007));
        let bone_mesh = meshes.add(Cylinder::new(0.003, 1.0));
        let joint_material = materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.82, 0.15),
            unlit: true,
            ..default()
        });
        let bone_material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.95, 1.0),
            unlit: true,
            ..default()
        });
        for _ in 0..160 {
            pool.joints.push(
                commands
                    .spawn((
                        Mesh3d(joint_mesh.clone()),
                        MeshMaterial3d(joint_material.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                        SkelJoint,
                    ))
                    .id(),
            );
            pool.bones.push(
                commands
                    .spawn((
                        Mesh3d(bone_mesh.clone()),
                        MeshMaterial3d(bone_material.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                        SkelBone,
                    ))
                    .id(),
            );
        }
        pool.built = true;
    }
    let is_bone = |name: &str| {
        (name.starts_with("NPC ") || name.starts_with("CME ") || name.starts_with("mixamorig"))
            && !name.ends_with("Mesh")
    };
    // Only joints the skin actually drives, plus their ancestors: the SSE
    // skeleton also carries tail, wing and HDT bones that sit metres outside a
    // humanoid body and hide the real chain.
    let mut driven: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    for skin in &skins {
        driven.extend(skin.joints.iter().copied());
    }
    let parents: std::collections::HashMap<Entity, Option<Entity>> = nodes
        .iter()
        .map(|(entity, _, _, parent)| (entity, parent.map(|p| p.parent())))
        .collect();
    let mut frontier: Vec<Entity> = driven.iter().copied().collect();
    while let Some(current) = frontier.pop() {
        if let Some(Some(parent)) = parents.get(&current) {
            if driven.insert(*parent) {
                frontier.push(*parent);
            }
        }
    }
    // Level 1 keeps the chain an animator reasons about: pelvis, spine, neck,
    // head, clavicles, arms, hands, thighs, calves, feet and toes. Twist bones
    // and finger chains only appear at level 2.
    let primary = |name: &str| {
        !name.contains("Twist")
            && !name.contains("Finger")
            && [
                "Pelvis", "Spine", "Neck", "Head", "Clavicle", "UpperArm", "Forearm", "Hand",
                "Thigh", "Calf", "Foot", "Toe",
            ]
            .iter()
            .any(|key| name.contains(key))
    };
    // Skeleton scaffolding, not body bones: `NPC Root` sits at the actor origin and
    // `NPC COM` a metre above it, so the bar between them crosses the whole figure
    // as a stray vertical line. The joints stay drawn; only a bar leaving them does
    // not.
    let is_anchor = |name: &str| name.ends_with("[Root]") || name.ends_with("[COM ]");
    let anchors: std::collections::HashSet<Entity> = nodes
        .iter()
        .filter(|(_, name, _, _)| is_anchor(name.as_str()))
        .map(|(entity, _, _, _)| entity)
        .collect();
    let mut bones: Vec<(Entity, Vec3, Option<Entity>)> = Vec::new();
    for (entity, name, transform, parent) in &nodes {
        if is_bone(name.as_str())
            && driven.contains(&entity)
            && (skel.level > 1 || primary(name.as_str()))
        {
            bones.push((entity, transform.translation(), parent.map(|p| p.parent())));
        }
    }
    let name_of: std::collections::HashMap<Entity, String> = nodes
        .iter()
        .map(|(e, n, _, _)| (e, n.as_str().to_string()))
        .collect();
    info.0 = bones
        .iter()
        .filter_map(|(e, p, _)| name_of.get(e).map(|n| (n.clone(), *p)))
        .collect();
    if !skel.on() {
        info.0.clear();
        for entity in pool.joints.iter().chain(pool.bones.iter()) {
            if let Ok((_, mut visibility)) = overlay.get_mut(*entity) {
                *visibility = Visibility::Hidden;
            }
        }
        return;
    }
    let positions: std::collections::HashMap<Entity, Vec3> = bones
        .iter()
        .map(|(entity, position, _)| (*entity, *position))
        .collect();
    for (index, (_, position, parent)) in bones.iter().enumerate() {
        if let Some(entity) = pool.joints.get(index) {
            if let Ok((mut transform, mut visibility)) = overlay.get_mut(*entity) {
                transform.translation = *position;
                transform.rotation = Quat::IDENTITY;
                transform.scale = Vec3::ONE;
                *visibility = Visibility::Inherited;
            }
        }
        let Some(entity) = pool.bones.get(index) else {
            continue;
        };
        let target = parent
            .filter(|parent| !anchors.contains(parent))
            .and_then(|parent| positions.get(&parent))
            .copied();
        if let (Some(start), Ok((mut transform, mut visibility))) =
            (target, overlay.get_mut(*entity))
        {
            let delta = *position - start;
            let length = delta.length().max(1e-5);
            transform.translation = start + delta * 0.5;
            transform.rotation = Quat::from_rotation_arc(Vec3::Y, delta / length);
            transform.scale = Vec3::new(1.0, length, 1.0);
            *visibility = Visibility::Inherited;
        } else if let Ok((_, mut visibility)) = overlay.get_mut(*entity) {
            *visibility = Visibility::Hidden;
        }
    }
    for entity in pool
        .joints
        .iter()
        .skip(bones.len())
        .chain(pool.bones.iter().skip(bones.len()))
    {
        if let Ok((_, mut visibility)) = overlay.get_mut(*entity) {
            *visibility = Visibility::Hidden;
        }
    }
}

/// One visibility rule for the review scene: flat hides everything, the
/// default rig shows only the key, and `3` shows the fill/rim pair as well.
/// The ground is interactive-only and hides in flat so the silhouette stays
/// against the black backdrop.
#[allow(clippy::type_complexity, clippy::if_same_then_else)]
fn apply_review_visibility(
    flat: Res<FlatMode>,
    rig: Res<RigMode>,
    mut lights: Query<
        (
            &mut Visibility,
            Option<&KeyLight>,
            Option<&FillLight>,
            Option<&ReviewGround>,
        ),
        Or<(With<ReviewLight>, With<ReviewGround>)>,
    >,
) {
    for (mut visibility, key, fill, ground) in &mut lights {
        let wanted = if flat.0 {
            Visibility::Hidden
        } else if ground.is_some() {
            Visibility::Inherited
        } else if key.is_some() {
            Visibility::Inherited
        } else if fill.is_some() && !rig.three_point {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Flat review renders unlit materials: silhouette, seams and layer colours,
/// with nothing that reads as a defect. Runs every frame because glTF
/// materials stream in after startup.
fn apply_flat_mode(
    flat: Res<FlatMode>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut flipped: ResMut<FlatFlipped>,
) {
    if flat.0 {
        // Remember which materials we flattened so `L` can restore the lit
        // look; leaving them unlit made the second press look like a no-op.
        for (id, material) in materials.iter_mut() {
            if !material.unlit {
                material.unlit = true;
                flipped.0.push(id);
            }
        }
    } else {
        for id in flipped.0.drain(..) {
            if let Some(mut material) = materials.get_mut(id) {
                material.unlit = false;
            }
        }
    }
}

#[derive(Resource)]
struct Target(Handle<Image>);

#[derive(Resource)]
struct GltfRoot(Handle<bevy::gltf::Gltf>);

/// Graph (actor bones + cloth morph clips) to attach to the scene's players.
#[derive(Resource)]
struct PlayerGraph {
    handle: Handle<AnimationGraph>,
    nodes: Vec<AnimationNodeIndex>,
    names: Vec<String>,
    /// The cloth clip, when `SCULPTO_VIEWER_CLOTH_ANIMATION` names one. It plays
    /// alongside whatever actor clip is selected, on its own node.
    cloth: Option<AnimationNodeIndex>,
}

/// Every actual clip gets its own node; a clipless rest asset gets an empty graph.
fn build_player_graph(
    asset_server: &AssetServer,
    graphs: &mut Assets<AnimationGraph>,
    asset_file: &str,
    names: Vec<String>,
) -> PlayerGraph {
    let clips =
        (0..names.len()).map(|index| asset_server.load(format!("{asset_file}#Animation{index}")));
    let (mut graph, nodes) = AnimationGraph::from_clips(clips);
    // A separate cloth clip exists only when the file carries one and the
    // caller names it. Defaulting it to `Animation0` played the actor's own
    // idle clip a second time at full weight, blending two clips on the same
    // skeleton — the figure trembled and the walk was diluted by half.
    let cloth = std::env::var("SCULPTO_VIEWER_CLOTH_ANIMATION")
        .ok()
        .map(|index| {
            let cloth_clip = asset_server.load(format!("{asset_file}#Animation{index}"));
            graph.add_clip(cloth_clip, 1.0, graph.root)
        });
    PlayerGraph {
        handle: graphs.add(graph),
        nodes,
        names,
        cloth,
    }
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    units: Res<Units>,
    config: Res<ViewerConfig>,
    profile: Res<AssetProfile>,
    sun: Res<SunOrbit>,
    bank: Option<Res<bank::Bank>>,
    mut watch: Option<ResMut<WatchState>>,
    render_adapter: Option<Res<RenderAdapter>>,
    render_device: Option<Res<RenderDevice>>,
) {
    // A live watch with no file yet spawns nothing until the first poll
    // stages one; everything else behaves exactly as before.
    let watch_ready = watch.as_ref().is_none_or(|watch| watch.ready);
    let mut target_handle = None;
    if !config.interactive {
        let mut image = Image::new(
            Extent3d {
                width: 960,
                height: 900,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![0; 960 * 900 * 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.texture_descriptor.usage =
            bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT
                | bevy::render::render_resource::TextureUsages::COPY_SRC
                | bevy::render::render_resource::TextureUsages::COPY_DST
                | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING;
        let handle = images.add(image);
        commands.insert_resource(Target(handle.clone()));
        target_handle = Some(handle);
    }
    if config.debug_scene {
        let at_cap = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS], None).is_ok();
        let over_cap = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS + 1], None)
            .err()
            .map(|error| error.to_string());
        info!(
            "viewer: morph_max={MAX_MORPH_WEIGHTS} at_cap_ok={at_cap} over_cap_error={over_cap:?}"
        );
    }
    // Lab fixtures are authored in BodySlide units (71.4286 per metre), so the
    // fixed review camera sits inside them. Scaling the loaded scene keeps the
    // camera, lights and orbit speeds in metres for any source unit.
    let scene_scale = env_f32("SCULPTO_VIEWER_SCALE", 1.0);
    let count = units.0.max(1);
    let columns = (count as f32).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let spacing = 0.9_f32;
    let (camera_eye, camera_target) = if count > 1 {
        let extent = (columns.max(rows) as f32 - 1.0) * spacing + 1.5;
        (
            Vec3::new(extent * 1.4, 1.4, extent * 1.8),
            Vec3::new(0.0, 0.9, 0.0),
        )
    } else {
        (Vec3::new(2.2, 1.3, 2.8), Vec3::new(0.0, 0.9, 0.0))
    };
    let mut camera = commands.spawn((
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_translation(camera_eye).looking_at(camera_target, Vec3::Y),
    ));
    if config.interactive {
        camera.insert(Projection::Perspective(PerspectiveProjection {
            near: INSPECTION_NEAR_PLANE,
            ..default()
        }));
    }
    if !config.report_mode
        && std::env::var_os("SCULPTO_VIEWER_SORT_TRANSPARENCY").is_none_or(|value| value != "0")
        && render_adapter
            .zip(render_device)
            .is_some_and(|(adapter, device)| is_oit_supported(&adapter, &device, false))
    {
        camera.insert((Msaa::Off, OrderIndependentTransparencySettings::default()));
    }
    if let Some(handle) = target_handle {
        camera.insert(bevy::camera::RenderTarget::Image(handle.into()));
    }
    // Both modes carry the orbit state: the interactive window drives it with
    // the mouse, the headless capture leaves it at the fixed review view, and
    // the control socket can move either one through the same resource.
    commands.insert_resource(OrbitCamera::looking_at(camera_eye, camera_target));
    let (color, illuminance) = sun.lighting();
    commands.spawn((
        DirectionalLight {
            color,
            illuminance,
            shadow_maps_enabled: std::env::var("SCULPTO_VIEWER_NO_SHADOWS").is_err(),
            ..default()
        },
        sun.transform(),
        ReviewLight,
        KeyLight,
    ));
    for (illuminance, position) in [
        (3_500.0, Vec3::new(-3.0, 2.0, -2.5)),
        (6_000.0, Vec3::new(0.0, 3.0, -4.0)),
    ] {
        commands.spawn((
            DirectionalLight {
                illuminance,
                ..default()
            },
            Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y),
            ReviewLight,
            FillLight,
        ));
    }
    if config.interactive || std::env::var("SCULPTO_VIEWER_GROUND").is_ok() {
        // A matte ground receives the key's shadow. It is off in the archived
        // headless captures unless `SCULPTO_VIEWER_GROUND` asks for the shadow
        // proof, so the contact-sheet backdrop stays black.
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(4.0, 0.02, 4.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.06, 0.06, 0.07),
                perceptual_roughness: 0.95,
                ..default()
            })),
            Transform::from_xyz(0.0, -0.01, 0.0),
            ReviewGround,
        ));
    }

    // GltfRoot feeds the report path only, and watch mode refuses --report;
    // skipping it here also lets generation 0 unload after its last swap.
    if watch.is_none() {
        let gltf_handle: Handle<bevy::gltf::Gltf> = asset_server.load(config.asset_file.clone());
        commands.insert_resource(GltfRoot(gltf_handle));
    }

    if !config.report_mode && watch_ready {
        commands.insert_resource(build_player_graph(
            &asset_server,
            &mut graphs,
            &config.asset_file,
            std::fs::read(&profile.path)
                .ok()
                .and_then(|bytes| glb_animation_names_bytes(&bytes))
                .unwrap_or_default(),
        ));
    }

    if watch_ready {
        let scene = asset_server.load(format!("{}#Scene0", config.asset_file));
        for index in 0..count {
            let column = index % columns;
            let row = index / columns;
            let x = (column as f32 - (columns as f32 - 1.0) * 0.5) * spacing;
            let z = (row as f32 - (rows as f32 - 1.0) * 0.5) * spacing;
            let home = Vec3::new(x, 0.0, z);
            let root = commands
                .spawn((
                    bevy::world_serialization::WorldAssetRoot(scene.clone()),
                    Transform::from_translation(home).with_scale(Vec3::splat(scene_scale)),
                    ModelHome(home),
                    ReviewModel,
                ))
                .id();
            if let Some(watch) = watch.as_mut() {
                watch.current_root = Some(root);
            }
        }
    }
    if config.interactive {
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(12.0),
                    top: Val::Px(12.0),
                    padding: UiRect::all(Val::Px(10.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                HelpOverlay,
            ))
            .with_children(|card| {
                let (status_colour, status_line) = match config.status.split(" · ").next() {
                    Some("ReadyToShowProgress") => {
                        (Color::srgb(0.18, 0.80, 0.44), config.status.as_str())
                    }
                    _ => (Color::srgb(0.95, 0.77, 0.20), config.status.as_str()),
                };
                card.spawn((
                    Text::new(status_line),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(status_colour),
                ));
                for line in [
                    "Drag LMB orbit · RMB pan · wheel zoom",
                    "W/S approach or retreat · A/D move sideways",
                    "Ctrl+LMB turn figure · Ctrl+RMB move sideways",
                    "Shift+LMB aim sun · lower is warmer",
                    if bank.as_ref().is_some_and(|bank| bank.inspection.is_some()) {
                        "0 A-pose · 1 T-pose · Ctrl+0 disassembled · Ctrl+number 1.35s"
                    } else if bank.is_some() {
                        "0–9 select bank slot · Ctrl+number 1.35s"
                    } else {
                        "0–9 select clip · Ctrl+number 1.35s"
                    },
                    "Space pause · Ctrl+, slower · Ctrl+. faster",
                    "R reset · L lit/flat · N face normals · F3 rig · B bones · H help",
                    "Hover node = name",
                ] {
                    card.spawn((
                        Text::new(line),
                        TextFont {
                            font_size: bevy::text::FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(Color::WHITE),
                    ));
                }
                card.spawn((
                    Text::new("Loading…"),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(14.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.77, 0.20)),
                    ClipLine,
                ));
                if let Some(watch) = watch.as_mut() {
                    let line = watch.line(&config.status);
                    watch.last_line = line.clone();
                    card.spawn((
                        Text::new(line),
                        TextFont {
                            font_size: bevy::text::FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(Color::WHITE),
                        WatchLine,
                    ));
                }
                if let Some(control) = config.control.as_deref() {
                    card.spawn((
                        Text::new(format!("control: {control}")),
                        TextFont {
                            font_size: bevy::text::FontSize::Px(12.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.55, 0.85, 1.0)),
                    ));
                }
            });
        commands
            .spawn(Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(22.0),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.82, 0.15)),
                    HoverLabel,
                    Visibility::Hidden,
                ));
            });
    }
}

/// Hover the cursor over a yellow joint to read its name (top-center label).
/// Picks the joint whose world position lies closest to the cursor ray within a
/// small radius, so the operator can identify nodes while placing them.
fn hover_joint_name(
    config: Res<ViewerConfig>,
    number: Res<number_entry::NumberEntry>,
    skel: Res<SkelMode>,
    info: Res<SkelJointsInfo>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window>,
    mut labels: Query<(&mut Text, &mut Visibility), With<HoverLabel>>,
) {
    let Ok((mut text, mut vis)) = labels.single_mut() else {
        return;
    };
    if !number.digits().is_empty() {
        text.0 = format!("Ctrl · {}", number.digits());
        *vis = Visibility::Inherited;
        return;
    }
    if !config.interactive || !skel.on() || info.0.is_empty() {
        *vis = Visibility::Hidden;
        return;
    }
    let Ok(window) = windows.single() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Ok((camera, cam_tf)) = cameras.single() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        *vis = Visibility::Hidden;
        return;
    };
    let dir = ray.direction.normalize();
    let mut best: Option<(f32, &str)> = None;
    for (name, pos) in &info.0 {
        let d = *pos - ray.origin;
        let along = d.dot(dir);
        if along <= 0.0 {
            continue;
        }
        let perp = (d - dir * along).length();
        if perp < 0.012 && best.is_none_or(|(b, _)| perp < b) {
            best = Some((perp, name.as_str()));
        }
    }
    if let Some((_, name)) = best {
        text.0 = name.to_string();
        *vis = Visibility::Inherited;
    } else {
        *vis = Visibility::Hidden;
    }
}

/// `H` hides or shows the controls card. Agent-opened windows start unfocused,
/// so the first key press needs one click on the figure.
fn toggle_help(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut card: Query<&mut Visibility, With<HelpOverlay>>,
) {
    if !config.interactive || !keys.just_pressed(KeyCode::KeyH) {
        return;
    }
    for mut visibility in &mut card {
        *visibility = match *visibility {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

const INSPECTION_NEAR_PLANE: f32 = 0.001;
/// Retain a well-defined look direction as keyboard travel carries the pivot forward.
const MIN_ORBIT_RADIUS: f32 = 5.0 * INSPECTION_NEAR_PLANE;

/// Orbit state for the interactive review window.
///
/// `looking_at` seeds the state from the same eye/target the headless camera
/// uses, so the first interactive frame matches the captured review view.
#[derive(Resource)]
struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
    home: (Vec3, f32, f32, f32),
}

impl OrbitCamera {
    fn looking_at(eye: Vec3, target: Vec3) -> Self {
        let offset = eye - target;
        let distance = offset.length().max(MIN_ORBIT_RADIUS);
        let yaw = offset.x.atan2(offset.z);
        let pitch = (offset.y / distance).asin();
        Self {
            target,
            yaw,
            pitch,
            distance,
            min_distance: MIN_ORBIT_RADIUS,
            max_distance: 30.0,
            home: (target, yaw, pitch, distance),
        }
    }

    /// Frame the measured scene once, keeping the review direction but widening
    /// the zoom range with the asset. A 100-unit Fox must not inherit the
    /// human-scale 30-unit cap.
    fn fit(&mut self, target: Vec3, distance: f32) {
        self.target = target;
        self.distance = distance;
        self.max_distance = (distance * 20.0).max(30.0);
        self.home = (target, self.yaw, self.pitch, distance);
    }

    fn reset(&mut self) {
        let (target, yaw, pitch, distance) = self.home;
        self.target = target;
        self.yaw = yaw;
        self.pitch = pitch;
        self.distance = distance;
    }
}

/// Frame the first loaded scene from its world bounds so a 1.8 m character and
/// a 100-unit Fox both open at a readable distance. The model itself is never
/// rescaled; `SCULPTO_VIEWER_SCALE` still controls source-unit conversion.
/// Interactive only, so headless captures keep their fixed review camera.
#[allow(clippy::type_complexity)]
fn fit_camera_to_scene(
    config: Res<ViewerConfig>,
    mut fitted: Local<bool>,
    orbit: Option<ResMut<OrbitCamera>>,
    meshes: Query<(&bevy::camera::primitives::Aabb, &GlobalTransform), SceneMeshFilter>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut grounds: Query<&mut Transform, (With<ReviewGround>, Without<Camera3d>)>,
) {
    if *fitted || !config.interactive || env_f32("SCULPTO_VIEWER_AUTOFIT", 1.0) == 0.0 {
        return;
    }
    let Some(mut orbit) = orbit else {
        return;
    };
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (aabb, transform) in &meshes {
        let center = aabb.center;
        let half = aabb.half_extents;
        for x in [-1.0_f32, 1.0] {
            for y in [-1.0_f32, 1.0] {
                for z in [-1.0_f32, 1.0] {
                    let local = center + Vec3A::new(x * half.x, y * half.y, z * half.z);
                    let world = transform.transform_point(local.into());
                    min = min.min(world);
                    max = max.max(world);
                }
            }
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return;
    }
    let radius = (max - min).length() * 0.5;
    if radius < 1e-6 {
        return;
    }
    let target = (min + max) * 0.5;
    let distance = radius / (std::f32::consts::FRAC_PI_8).tan() * 1.15;
    orbit.fit(target, distance);
    if let Ok(mut camera) = cameras.single_mut() {
        let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        camera.translation = target + rotation * Vec3::Z * distance;
        camera.look_at(target, Vec3::Y);
    }
    let width = (max.x - min.x).max(max.z - min.z).max(radius * 2.0);
    for mut ground in &mut grounds {
        ground.translation = Vec3::new(target.x, min.y - 0.01, target.z);
        ground.scale = Vec3::new(width / 4.0, 1.0, width / 4.0);
    }
    *fitted = true;
    info!("viewer: auto-fit target={target:?} radius={radius:.3} distance={distance:.3}");
}

/// Scale pan and keyboard travel by the fitted opening distance.
const CAMERA_TRANSLATION_GAIN: f32 = 3.4225;

/// Interactive orbit, pan and zoom with view-relative WASD travel.
fn orbit_controls(
    config: Res<ViewerConfig>,
    orbit: Option<ResMut<OrbitCamera>>,
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    (mut motion, mut wheel): (MessageReader<MouseMotion>, MessageReader<MouseWheel>),
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    if !config.interactive {
        return;
    }
    let Some(mut orbit) = orbit else {
        return;
    };
    let mut delta = Vec2::ZERO;
    for event in motion.read() {
        delta += event.delta;
    }
    let mut zoom = 0.0;
    for event in wheel.read() {
        zoom += match event.unit {
            bevy::input::mouse::MouseScrollUnit::Line => event.y,
            bevy::input::mouse::MouseScrollUnit::Pixel => event.y / 50.0,
        };
    }

    if keys.just_pressed(KeyCode::KeyR) {
        orbit.reset();
    }
    let moving_light = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let moving_character = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if buttons.pressed(MouseButton::Left) && !moving_light && !moving_character {
        let sensitivity = 0.005;
        orbit.yaw -= delta.x * sensitivity;
        orbit.pitch = (orbit.pitch - delta.y * sensitivity).clamp(-1.45, 1.45);
    }
    let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    let travel_scale = orbit.home.3 * CAMERA_TRANSLATION_GAIN;
    if buttons.pressed(MouseButton::Right) && !moving_character {
        let right = rotation * Vec3::X;
        let up = rotation * Vec3::Y;
        let pan_scale = travel_scale * 0.0015;
        orbit.target += (-right * delta.x + up * delta.y) * pan_scale;
    }
    if zoom.abs() > f32::EPSILON {
        orbit.distance =
            (orbit.distance * (1.0 - zoom * 0.12)).clamp(orbit.min_distance, orbit.max_distance);
    }

    let axis = |positive, negative| {
        f32::from(i8::from(keys.pressed(positive)) - i8::from(keys.pressed(negative)))
    };
    let direction = Vec2::new(
        axis(KeyCode::KeyD, KeyCode::KeyA),
        axis(KeyCode::KeyS, KeyCode::KeyW),
    );
    let travel = direction.normalize_or_zero() * (0.5 * travel_scale * time.delta_secs());
    let radius = orbit.distance;
    if travel.x != 0.0 {
        orbit.target += rotation * Vec3::X * travel.x;
    }
    if travel.y != 0.0 {
        let requested = radius + travel.y;
        let next = requested.clamp(orbit.min_distance, orbit.max_distance);
        orbit.target += rotation * Vec3::Z * (requested - next);
        orbit.distance = next;
    }

    let Ok(mut transform) = cameras.single_mut() else {
        return;
    };
    transform.translation = orbit.target + rotation * Vec3::Z * orbit.distance;
    transform.look_at(orbit.target, Vec3::Y);
}

/// Shift+LMB aims the sun and grades its light by elevation.
fn light_controls(
    config: Res<ViewerConfig>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    mut sun: ResMut<SunOrbit>,
    mut lights: Query<(&mut Transform, &mut DirectionalLight), With<KeyLight>>,
) {
    if !config.interactive {
        return;
    }
    let delta: Vec2 = motion.read().map(|event| event.delta).sum();
    if delta == Vec2::ZERO
        || !keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
        || !buttons.pressed(MouseButton::Left)
    {
        return;
    }
    let Ok((mut transform, mut light)) = lights.single_mut() else {
        return;
    };
    sun.azimuth -= delta.x * 0.005;
    sun.elevation =
        (sun.elevation - delta.y * 0.005).clamp(SunOrbit::MIN_ELEVATION, SunOrbit::MAX_ELEVATION);
    *transform = sun.transform();
    (light.color, light.illuminance) = sun.lighting();
}

#[allow(clippy::too_many_arguments)]
fn report_counts(
    config: Res<ViewerConfig>,
    gltf_root: Option<Res<GltfRoot>>,
    asset_server: Res<AssetServer>,
    gltf_assets: Res<Assets<bevy::gltf::Gltf>>,
    gltf_mesh_assets: Res<Assets<bevy::gltf::GltfMesh>>,
    mesh_assets: Res<Assets<Mesh>>,
    gltf_skin_assets: Res<Assets<bevy::gltf::GltfSkin>>,
    mut frames: Local<u32>,
) {
    if !config.report_mode {
        return;
    }
    *frames += 1;
    let Some(root) = gltf_root else { return };
    let state = asset_server.load_state(&root.0);
    if state.is_failed() {
        eprintln!(
            "REPORT_ERROR: failed to load glTF asset {}: {state:?}",
            config.asset_file
        );
        unsafe {
            libc::_exit(1);
        }
    }
    let Some(gltf) = gltf_assets.get(&root.0) else {
        if *frames > 300 {
            eprintln!(
                "REPORT_ERROR: timed out waiting for glTF asset {} to load (state={state:?})",
                config.asset_file
            );
            unsafe {
                libc::_exit(2);
            }
        }
        return;
    };

    let mut total_vertices = 0;
    let mut total_triangles = 0;
    let mut parts = Vec::new();
    let total_joints: usize = gltf
        .skins
        .iter()
        .filter_map(|handle| gltf_skin_assets.get(handle))
        .map(|skin| skin.joints.len())
        .sum();

    for (mesh_idx, mesh_handle) in gltf.meshes.iter().enumerate() {
        let Some(gltf_mesh) = gltf_mesh_assets.get(mesh_handle) else {
            return;
        };
        for (prim_idx, prim) in gltf_mesh.primitives.iter().enumerate() {
            let Some(mesh) = mesh_assets.get(&prim.mesh) else {
                return;
            };
            let verts = mesh.count_vertices();
            let tris = mesh.indices().map_or(verts / 3, |ind| ind.len() / 3);
            let morphs = mesh.morph_target_names().map_or(0, <[String]>::len);
            total_vertices += verts;
            total_triangles += tris;
            parts.push(PartReport {
                name: if !prim.name.is_empty() {
                    prim.name.clone()
                } else {
                    gltf_mesh.name.clone()
                },
                mesh_index: mesh_idx,
                primitive_index: prim_idx,
                vertices: verts,
                triangles: tris,
                morph_targets: morphs,
                joints: total_joints,
                material: prim.material.is_some(),
            });
        }
    }

    let at_cap_ok = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS], None).is_ok();
    let over_cap_err = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS + 1], None).is_err();

    let report = CountReport {
        profile: "bevy".to_string(),
        bevy_version: "0.20.1".to_string(),
        morph_limit: MAX_MORPH_WEIGHTS,
        morph_limit_verified: at_cap_ok && over_cap_err,
        nodes: gltf.nodes.len(),
        meshes: gltf.meshes.len(),
        vertices: total_vertices,
        triangles: total_triangles,
        materials: gltf.materials.len(),
        joints: total_joints,
        parts,
    };

    use std::io::Write;
    let json = serde_json::to_string_pretty(&report).unwrap();
    println!("{json}");
    let _ = std::io::stdout().flush();
    unsafe {
        // Skip glibc atexit handlers that crash inside NVIDIA driver cleanup.
        libc::_exit(0);
    }
}

fn advance_animation(
    mut commands: Commands,
    config: Res<ViewerConfig>,
    graph: Option<Res<PlayerGraph>>,
    selection: Res<ClipSelection>,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    if config.report_mode {
        return;
    }
    // The glTF scene spawns its players asynchronously; attach our graph and
    // start both clips the moment each player appears.
    let Some(graph) = graph else { return };
    let selected = graph.nodes.get(selection.0).copied();
    let cloth = graph.cloth;
    for (entity, mut player) in &mut players {
        if let Some(node) = selected {
            player.play(node).repeat();
        }
        if let Some(cloth) = cloth {
            player.play(cloth).repeat();
        }
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.handle.clone()));
        info!("viewer: started 1 animation players");
    }
}

/// `0` idle, `1` walk: stop the player and restart the chosen actor clip while
/// the cloth clip keeps running.
fn select_clip(
    config: Res<ViewerConfig>,
    number: Res<number_entry::NumberEntry>,
    graph: Option<Res<PlayerGraph>>,
    mut selection: ResMut<ClipSelection>,
    mut players: Query<(&mut AnimationPlayer, &AnimationGraphHandle)>,
    bank: Option<Res<bank::Bank>>,
) {
    if !config.interactive || bank.is_some() {
        return;
    }
    let Some(graph) = graph else { return };
    let Some(bank::Selection::Slot(wanted)) = number
        .pick
        .and_then(|p| p.resolve(graph.nodes.len(), false))
    else {
        return;
    };
    if selection.0 == wanted {
        return;
    }
    selection.0 = wanted;
    start_actor_clip(&graph, wanted, &mut players);
}

/// Keyboard and socket requests stage a fresh scene, including for a return to rest.
fn request_slot(
    bank: &mut bank::Bank,
    watch: &mut WatchState,
    index: bank::Selection,
) -> Result<(), String> {
    let slot = bank
        .slot(index)
        .ok_or_else(|| format!("bank has no selection {}", index.value()))?;
    let bytes =
        std::fs::read(&slot.asset).map_err(|error| format!("{}: {error}", slot.asset.display()))?;
    let names = glb_animation_names_bytes(&bytes).ok_or("bank asset is not a valid GLB")?;
    if slot
        .clip
        .is_some_and(|clip| names.get(clip) != slot.clip_name.as_ref())
    {
        return Err(format!(
            "selection {index:?}: expected clip is absent from the asset"
        ));
    }
    watch.files.dir = slot
        .asset
        .parent()
        .ok_or("bank asset has no parent")?
        .to_path_buf();
    watch.files.stage = slot
        .asset
        .file_stem()
        .ok_or("bank asset has no stem")?
        .to_string_lossy()
        .into_owned();
    watch.files.seen = None;
    watch.next_poll = 0.0;
    bank.requested = Some(index);
    bank.error = None;
    Ok(())
}

fn bank_keyboard(
    config: Res<ViewerConfig>,
    number: Res<number_entry::NumberEntry>,
    bank: Option<ResMut<bank::Bank>>,
    watch: Option<ResMut<WatchState>>,
) {
    if !config.interactive {
        return;
    }
    let (Some(mut bank), Some(mut watch)) = (bank, watch) else {
        return;
    };
    let Some(selected) = number
        .pick
        .and_then(|p| p.resolve(bank.slots.len(), bank.inspection.is_some()))
    else {
        return;
    };
    if let Err(error) = request_slot(&mut bank, &mut watch, selected) {
        bank.error = Some(error);
    }
}

#[derive(Component)]
struct ClipLine;

fn clip_label(
    bank: Option<Res<bank::Bank>>,
    selection: Res<ClipSelection>,
    graph: Option<Res<PlayerGraph>>,
    profile: Res<AssetProfile>,
    mut lines: Query<&mut Text, With<ClipLine>>,
) {
    let line = match bank {
        Some(bank) => match (&bank.error, bank.requested) {
            (Some(error), _) => error.clone(),
            (_, Some(index)) => format!("Loading {}", bank.caption(index)),
            _ => bank.caption(bank.current),
        },
        None => graph
            .as_ref()
            .and_then(|graph| graph.names.get(selection.0))
            .map_or_else(
                || "Rest pose".into(),
                |name| format!("{} · {name}", selection.0),
            ),
    };
    let line = if profile.loaded {
        line
    } else {
        format!("{line} · loading")
    };
    lines.iter_mut().for_each(|mut text| {
        if text.0 != line {
            text.0.clone_from(&line);
        }
    });
}

/// Camera positions for the review views; the glTF actor faces +Z, so the
/// +Z camera is the front view and (2.2, ., 2.8) the three-quarter front.
fn view_location(name: &str) -> Vec3 {
    match name {
        "front" => Vec3::new(0.0, 1.3, 2.8),
        "side" => Vec3::new(2.8, 1.3, 0.0),
        "back" => Vec3::new(0.0, 1.3, -2.8),
        _ => Vec3::new(2.2, 1.3, 2.8),
    }
}

fn parse_series(spec: &str) -> Option<(u32, u32, u32)> {
    let mut parts = spec.split(':');
    let start = parts.next()?.parse().ok()?;
    let count = parts.next()?.parse().ok()?;
    let stride = parts.next().unwrap_or("1").parse().ok()?;
    (count > 0 && stride > 0).then_some((start, count, stride))
}

/// The one-shot capture and its frame-301 exit.
///
/// A `--control` session is a live session: its caller decides when it ends,
/// and the automatic capture would both overwrite `SCULPTO_VIEWER_PNG` and take
/// the process down after roughly five seconds — measured against a live
/// control session that lost its third capture request. Control mode is
/// therefore exempt, exactly like the interactive window and `--watch`; the
/// one-shot and series behaviour stays for every run without it.
#[allow(clippy::too_many_arguments)]
fn capture_once(
    config: Res<ViewerConfig>,
    target: Option<Res<Target>>,
    bench: Res<Bench>,
    watch: Option<Res<WatchState>>,
    mut frames: Local<u32>,
    mut captured: Local<bool>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if bench.0
        || config.report_mode
        || config.debug_scene
        || config.interactive
        || config.control.is_some()
        || watch.is_some()
    {
        return;
    }
    let Some(target) = target else {
        return;
    };
    *frames += 1;
    if *frames < 240 {
        return;
    }
    // Series mode: one pose every `stride` ticks, one view per following
    // tick, so a review gets 10-20 distinct poses across the clip without
    // rewinding the animation.
    if let Some((start, count, stride)) = parse_series(&env_or("SCULPTO_VIEWER_SERIES", "")) {
        let views: Vec<String> = env_or("SCULPTO_VIEWER_VIEWS", "quarter")
            .split(',')
            .map(str::to_string)
            .collect();
        let dir = env_or("SCULPTO_VIEWER_DIR", "/tmp/sculpto-classify/cloth/series");
        let last = start + (count - 1) * stride;
        if *frames >= start && *frames <= last + stride {
            let pose = (*frames - start) % stride;
            // ticks after the last pose tick belong to its trailing views only
            if pose < views.len() as u32 && *frames - pose <= last {
                let view = &views[pose as usize];
                let location = view_location(view);
                if let Ok(mut transform) = cameras.single_mut() {
                    transform.translation = location;
                    transform.look_at(Vec3::new(0.0, 0.95, 0.0), Vec3::Y);
                }
                let pose_tick = *frames - pose;
                info!(
                    "viewer: series pose {} view {} at tick {}",
                    pose_tick, view, *frames
                );
                commands
                    .spawn(Screenshot::image(target.0.clone()))
                    .observe(save_to_disk(format!("{dir}/pose{pose_tick:04}_{view}.png")));
            }
        }
        if *frames > last + stride + views.len() as u32 + 30 {
            info!("viewer: series done, exiting at {}", *frames);
            exit.write(AppExit::Success);
        }
        return;
    }
    if *captured {
        if *frames > 300 {
            info!("viewer: captured at 240, exiting at {}", *frames);
            exit.write(AppExit::Success);
        }
        return;
    }
    *captured = true;
    info!("viewer: requesting screenshot at frame {}", *frames);
    commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(save_to_disk(env_or(
            "SCULPTO_VIEWER_PNG",
            "/tmp/sculpto-classify/cloth/bevy_combined_240.png",
        )));
}

/// Per-material evidence for the control socket: what each drawn actor material
/// carries after the overlay pass has run.
#[derive(Resource, Default)]
struct ActorMaterials(Vec<control::MaterialEntry>);

/// Read every actor material back from the assets once per frame. Cheap (a
/// handful of entities) and honest: it reports the values the renderer uses,
/// including `unlit` in flat mode, instead of restating the setting.
#[allow(clippy::type_complexity)] // the four name/material components are the evidence
fn collect_actor_materials(
    actors: Query<
        (
            Option<&Name>,
            Option<&bevy::gltf::GltfMeshName>,
            Option<&bevy::gltf::GltfMaterialName>,
            &MeshMaterial3d<StandardMaterial>,
        ),
        (
            With<Mesh3d>,
            Without<ReviewGround>,
            Without<SkelJoint>,
            Without<SkelBone>,
        ),
    >,
    materials: Res<Assets<StandardMaterial>>,
    mut out: ResMut<ActorMaterials>,
) {
    let entries: Vec<control::MaterialEntry> = actors
        .iter()
        .filter_map(|(name, mesh_name, material_name, handle)| {
            let material = materials.get(handle.id())?;
            let srgba = material.base_color.to_srgba();
            Some(control::MaterialEntry {
                mesh: name
                    .map(|name| name.as_str().to_string())
                    .or_else(|| mesh_name.map(|name| name.0.clone()))
                    .unwrap_or_else(|| "<unnamed mesh>".to_string()),
                material: material_name
                    .map(|name| name.0.clone())
                    .unwrap_or_else(|| "<unnamed material>".to_string()),
                asset: materials.iter().position(|(id, _)| id == handle.id()),
                alpha: material.base_color.alpha(),
                blend: format!("{:?}", material.alpha_mode),
                color: [srgba.red, srgba.green, srgba.blue],
                unlit: material.unlit,
            })
        })
        .collect();
    out.0 = entries;
}

/// A drawn scene mesh: not the overlay pool, not the review ground. Those exist
/// without any asset, so counting them would fake readiness.
type SceneMeshFilter = (
    With<Mesh3d>,
    Without<SkelJoint>,
    Without<SkelBone>,
    Without<ReviewGround>,
);

/// Reflect the asset on screen into the profile the control socket reports.
/// Watch mode stages a new copy per generation, so a changed digest also
/// refreshes the file name, the path and the clip count the same frame.
///
/// `loaded` is scene readiness, not intent: a spawned scene root exists before
/// its scene parses, and a pending watch generation is not on screen yet.
fn track_asset_loaded(
    asset_server: Res<AssetServer>,
    gltf: Option<Res<GltfRoot>>,
    watch: Option<Res<WatchState>>,
    meshes: Query<(), SceneMeshFilter>,
    mut profile: ResMut<AssetProfile>,
) {
    // A scene mesh is the one signal that something parsed and reached the
    // renderer; the overlay pool and the review ground are excluded because
    // they exist without any loaded asset.
    let scene_meshes = meshes.iter().count();
    let (parsed, pending, failed) = match watch.as_ref() {
        Some(watch) => {
            // The generation index moves on every swap, including one whose bytes
            // are identical; the identity work only repeats for a new digest.
            if watch.current_seq != profile.seq {
                profile.seq = watch.current_seq;
                profile.file = watch
                    .current_seq
                    .map_or_else(|| profile.file.clone(), |seq| format!("live-{seq}.glb"));
                profile.path = watch.files.staging_dir.join(&profile.file);
                if watch.sha != profile.sha256 && !watch.sha.is_empty() {
                    profile.sha256 = watch.sha.clone();
                    profile.clips = glb_animation_count(&profile.path);
                }
            }
            // `current_seq` names the generation on screen and survives a failed
            // swap, so a rejected file does not erase the identity of what is
            // still displayed.
            (
                watch.current_seq.is_some(),
                watch.pending.is_some(),
                watch.failed,
            )
        }
        None => {
            let state = gltf.as_ref().map(|gltf| asset_server.load_state(&gltf.0));
            (
                state.as_ref().is_some_and(|state| state.is_loaded()),
                false,
                state.as_ref().is_some_and(|state| state.is_failed()),
            )
        }
    };
    profile.pending = pending;
    profile.failed = failed;
    profile.meshes = scene_meshes;
    // `loaded` describes the generation the digest names, so a pending load
    // keeps it false until that generation is the one on screen.
    profile.loaded = parsed && !pending && scene_meshes > 0;
}

/// Orbit angles for the named views. `left`/`right` are the figure's own sides:
/// the actor faces +Z, so its left is +X and the camera for `left` sits there.
fn view_angles(name: &str) -> (f32, f32) {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    match name {
        "front" => (0.0, 0.12),
        "back" => (PI, 0.12),
        "left" => (FRAC_PI_2, 0.12),
        "right" => (-FRAC_PI_2, 0.12),
        "quarter" => (FRAC_PI_4, 0.12),
        "top" => (0.0, 1.25),
        "below" => (0.0, -0.65),
        _ => (0.0, 0.12),
    }
}

/// Move the camera to the orbit state. In the interactive window
/// `orbit_controls` writes the same values from the same resource, so the two
/// paths cannot disagree; the headless capture camera is otherwise untouched,
/// which keeps a plain capture on its fixed review view.
fn place_camera(
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    cameras: &mut Query<&mut Transform, With<Camera3d>>,
) {
    let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, yaw, pitch, 0.0);
    if let Ok(mut camera) = cameras.single_mut() {
        camera.translation = target + rotation * Vec3::Z * distance;
        camera.look_at(target, Vec3::Y);
    }
}

/// Start the chosen actor clip on every player while the cloth clip keeps
/// running. Shared by the keyboard path and the control socket.
fn start_actor_clip(
    graph: &PlayerGraph,
    index: usize,
    players: &mut Query<(&mut AnimationPlayer, &AnimationGraphHandle)>,
) {
    let Some(node) = graph.nodes.get(index).copied() else {
        return;
    };
    for (mut player, _) in players
        .iter_mut()
        .filter(|(_, bound)| bound.0 == graph.handle)
    {
        player.stop_all();
        player.play(node).repeat();
        if let Some(cloth) = graph.cloth {
            player.play(cloth).repeat();
        }
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct ControlScene<'w, 's> {
    joints: Res<'w, SkelJointsInfo>,
    materials: Res<'w, ActorMaterials>,
    graphs: Res<'w, Assets<AnimationGraph>>,
    clips: Res<'w, Assets<AnimationClip>>,
    morphs: Query<'w, 's, (&'static Name, &'static MorphWeights)>,
}

/// The agent control socket: drain the queued requests and answer each one in
/// this frame. The socket thread only moves JSON strings; every mutation and
/// every read of ECS state happens here, on the schedule that owns it.
#[allow(clippy::too_many_arguments)]
fn control_service(
    mut commands: Commands,
    server: Option<Res<ControlHandle>>,
    config: Res<ViewerConfig>,
    profile: Res<AssetProfile>,
    target: Option<Res<Target>>,
    orbit: Option<ResMut<OrbitCamera>>,
    mut selection: ResMut<ClipSelection>,
    mut playback: ResMut<Playback>,
    mut skel: ResMut<SkelMode>,
    scene: ControlScene,
    graph: Option<Res<PlayerGraph>>,
    mut players: Query<(&mut AnimationPlayer, &AnimationGraphHandle)>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut bank: Option<ResMut<bank::Bank>>,
    mut watch: Option<ResMut<WatchState>>,
) {
    let Some(server) = server else {
        return;
    };
    let Ok(requests) = server.0.requests.lock() else {
        return;
    };
    let mut orbit = orbit;
    // The graph always carries idle/walk nodes, so its length is not the file's
    // clip count: a file with no animations must refuse `clip` instead of
    // playing a phantom handle.
    let clip_count = profile.clips;
    while let Ok(request) = requests.try_recv() {
        let joints: Vec<(String, [f32; 3])> = scene
            .joints
            .0
            .iter()
            .map(|(name, position)| (name.clone(), [position.x, position.y, position.z]))
            .collect();
        let response = match request.command {
            control::Command::Help => control::help_json(),
            control::Command::State => {
                let snapshot = control::Snapshot {
                    asset_file: profile.file.clone(),
                    asset_path: profile.path.display().to_string(),
                    sha256: profile.sha256.clone(),
                    loaded: profile.loaded,
                    pending: profile.pending,
                    failed: profile.failed,
                    meshes: profile.meshes,
                    scale: profile.scale,
                    status: config.status.clone(),
                    clip_seconds: graph
                        .as_ref()
                        .filter(|_| profile.loaded && !profile.pending)
                        .and_then(|g| {
                            let node = *g.nodes.get(selection.0)?;
                            players
                                .iter()
                                .filter(|(_, bound)| bound.0 == g.handle)
                                .find_map(|(player, _)| {
                                    player.animation(node).map(|a| a.seek_time())
                                })
                        }),
                    morph_weights: scene
                        .morphs
                        .iter()
                        .map(|(name, w)| (name.to_string(), w.weights().to_vec()))
                        .collect(),
                    clip_index: graph
                        .as_ref()
                        .and_then(|graph| graph.nodes.get(selection.0).map(|_| selection.0)),
                    clip_name: graph
                        .as_ref()
                        .and_then(|graph| graph.names.get(selection.0).cloned()),
                    bank: bank.as_ref().map(|bank| bank.state()),
                    clip_count,
                    paused: playback.paused,
                    speed: playback.speed,
                    skeleton: skel.level,
                    camera_mode: if orbit.is_some() { "orbit" } else { "fixed" },
                    camera_target: orbit
                        .as_ref()
                        .map_or([0.0; 3], |orbit| orbit.target.to_array()),
                    camera_yaw: orbit.as_ref().map_or(0.0, |orbit| orbit.yaw),
                    camera_pitch: orbit.as_ref().map_or(0.0, |orbit| orbit.pitch),
                    camera_distance: orbit.as_ref().map_or(0.0, |orbit| orbit.distance),
                    camera_position: cameras
                        .iter()
                        .next()
                        .map_or([0.0; 3], |transform| transform.translation.to_array()),
                    joints: joints.clone(),
                    materials: scene.materials.0.clone(),
                };
                snapshot.to_json()
            }
            control::Command::Joint { name } => match control::find_joint(&joints, &name) {
                Ok((found, position)) => control::ok_json(serde_json::json!({
                    "joint": { "name": found, "position": position },
                })),
                Err(error) => control::error_json(&error),
            },
            control::Command::View {
                name,
                yaw,
                pitch,
                distance,
                target,
            } => {
                let mut message = None;
                if let Some(name) = target.as_deref() {
                    match control::find_joint(&joints, name) {
                        Ok((_, position)) => {
                            if let Some(orbit) = orbit.as_mut() {
                                orbit.target = Vec3::from(*position);
                            }
                        }
                        Err(error) => message = Some(error),
                    }
                }
                match (message, orbit.as_mut()) {
                    (Some(error), _) => control::error_json(&error),
                    (None, None) => {
                        control::error_json("no orbit camera in this mode; camera left unchanged")
                    }
                    (None, Some(orbit)) => {
                        if let Some(name) = name.as_deref() {
                            let (view_yaw, view_pitch) = view_angles(name);
                            orbit.yaw = view_yaw;
                            orbit.pitch = view_pitch;
                        }
                        if let Some(yaw) = yaw {
                            orbit.yaw = yaw;
                        }
                        if let Some(pitch) = pitch {
                            orbit.pitch = pitch;
                        }
                        if let Some(distance) = distance {
                            orbit.distance = distance;
                        }
                        place_camera(
                            orbit.target,
                            orbit.yaw,
                            orbit.pitch,
                            orbit.distance,
                            &mut cameras,
                        );
                        control::ok_json(serde_json::json!({
                            "camera": {
                                "target": orbit.target.to_array(),
                                "yaw": orbit.yaw,
                                "pitch": orbit.pitch,
                                "distance": orbit.distance,
                            },
                        }))
                    }
                }
            }
            control::Command::Target { joint, point } => {
                let resolved = match (joint.as_deref(), point) {
                    (Some(name), _) => control::find_joint(&joints, name)
                        .map(|(_, position)| Vec3::from(*position)),
                    (None, Some(point)) => Ok(Vec3::from(point)),
                    (None, None) => Err("target needs \"joint\" or \"point\"".to_string()),
                };
                match (resolved, orbit.as_mut()) {
                    (Err(error), _) => control::error_json(&error),
                    (Ok(_), None) => control::error_json("no orbit camera in this mode"),
                    (Ok(target), Some(orbit)) => {
                        orbit.target = target;
                        place_camera(
                            orbit.target,
                            orbit.yaw,
                            orbit.pitch,
                            orbit.distance,
                            &mut cameras,
                        );
                        control::ok_json(serde_json::json!({
                            "camera": { "target": target.to_array(), "distance": orbit.distance },
                        }))
                    }
                }
            }
            control::Command::Slot { index } => match (bank.as_mut(), watch.as_mut()) {
                (Some(bank), Some(watch)) => {
                    match request_slot(bank, watch, bank::Selection::Slot(index)) {
                        Ok(()) => control::ok_json(serde_json::json!({"requested_slot": index})),
                        Err(error) => control::error_json(&error),
                    }
                }
                _ => control::error_json("slot requires --bank"),
            },
            control::Command::Inspection => match (bank.as_mut(), watch.as_mut()) {
                (Some(bank), Some(watch)) => {
                    match request_slot(bank, watch, bank::Selection::Inspection) {
                        Ok(()) => control::ok_json(serde_json::json!({"requested":"inspection"})),
                        Err(e) => control::error_json(&e),
                    }
                }
                _ => control::error_json("inspection requires --bank"),
            },
            control::Command::Clip { .. } if bank.is_some() => {
                control::error_json("use slot to select an entry in --bank mode")
            }
            control::Command::Clip { index } => match (clip_count, graph.as_ref()) {
                (None, _) | (Some(0), _) => {
                    control::error_json("no animation clips are loaded; the file carries none")
                }
                (Some(count), None) => control::error_json(&format!(
                    "clip {index} cannot be selected without a player graph (file has {count})"
                )),
                (Some(_), Some(_))
                    if index >= graph.as_ref().map_or(0, |graph| graph.nodes.len()) =>
                {
                    control::error_json(&format!(
                        "clip {index} has no player node; the viewer holds {} actor clips",
                        graph.as_ref().map_or(0, |graph| graph.nodes.len())
                    ))
                }
                (Some(count), Some(_)) if index >= count => control::error_json(&format!(
                    "clip {index} out of range; the file carries {count} (0..{})",
                    count - 1
                )),
                (Some(_), Some(graph)) => {
                    selection.0 = index;
                    start_actor_clip(graph, index, &mut players);
                    control::ok_json(serde_json::json!({ "clip": index }))
                }
            },
            control::Command::Seek { seconds } => {
                use bevy::animation::graph::AnimationNodeType;
                let clip = graph
                    .as_ref()
                    .filter(|_| profile.loaded && !profile.pending)
                    .and_then(|g| {
                        let node = *g.nodes.get(selection.0)?;
                        let entry = scene.graphs.get(&g.handle)?.get(node)?;
                        let AnimationNodeType::Clip(handle) = &entry.node_type else {
                            return None;
                        };
                        Some((node, scene.clips.get(handle)?.duration(), &g.handle))
                    });
                match clip {
                    Some((_, duration, _)) if seconds > duration => {
                        control::error_json(&format!("time exceeds clip duration {duration}"))
                    }
                    Some((node, _, handle))
                        if players.iter().any(|(p, bound)| {
                            bound.0 == *handle && p.animation(node).is_some()
                        }) =>
                    {
                        for (mut player, _) in
                            players.iter_mut().filter(|(_, bound)| bound.0 == *handle)
                        {
                            if let Some(a) = player.animation_mut(node) {
                                a.pause().set_seek_time(seconds);
                            }
                        }
                        playback.paused = true;
                        control::ok_json(serde_json::json!({"seconds":seconds,"paused":true}))
                    }
                    _ => control::error_json("seek requires a loaded, selected clip"),
                }
            }
            control::Command::Playback { paused, speed } => {
                if let Some(paused) = paused {
                    playback.paused = paused;
                }
                if let Some(speed) = speed {
                    playback.speed = speed;
                }
                control::ok_json(serde_json::json!({
                    "paused": playback.paused,
                    "speed": playback.speed,
                }))
            }
            control::Command::Skeleton { level } => {
                skel.level = level;
                control::ok_json(serde_json::json!({
                    "skeleton": skel.level,
                    "note": "the overlay rebuilds on the next frame",
                }))
            }
            control::Command::Capture { path } => {
                let request_sent = match (config.interactive, target.as_ref()) {
                    (true, _) => {
                        commands
                            .spawn(Screenshot::primary_window())
                            .observe(save_to_disk(path.clone()));
                        true
                    }
                    (false, Some(target)) => {
                        commands
                            .spawn(Screenshot::image(target.0.clone()))
                            .observe(save_to_disk(path.clone()));
                        true
                    }
                    (false, None) => false,
                };
                if request_sent {
                    control::ok_json(serde_json::json!({
                        "capture": path,
                        "note": "queued; the PNG is written after the next rendered frame",
                    }))
                } else {
                    control::error_json("no render target to capture in this mode")
                }
            }
            control::Command::Quit => {
                // The socket thread leaves the process after it has written
                // this answer (`control::serve_connection`), skipping the
                // driver teardown that segfaults on this host.
                control::ok_json(serde_json::json!({ "quitting": true }))
            }
        };
        let _ = request.reply.send(response);
    }
}

/// One-shot scene dump (SCULPTO_VIEWER_DEBUG_SCENE=1): every mesh entity
/// with its glTF name, visibility components, bounding box and world
/// transform — catches assets that load silently but never draw. When
/// `SCULPTO_VIEWER_DEBUG_SCENE` is set it also reports the loader facts the
/// glTF writer must satisfy: vertex count, morph targets, material and skin
/// joints.
#[allow(clippy::type_complexity)]
fn debug_scene(
    config: Res<ViewerConfig>,
    mut done: Local<bool>,
    meshes: Query<
        (
            Option<&Name>,
            Option<&bevy::gltf::GltfMeshName>,
            Option<&Visibility>,
            Option<&InheritedVisibility>,
            Option<&bevy::camera::primitives::Aabb>,
            &GlobalTransform,
            &Mesh3d,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&bevy::mesh::skinning::SkinnedMesh>,
        ),
        With<Mesh3d>,
    >,
    assets: Res<Assets<Mesh>>,
) {
    if *done || config.report_mode || !config.debug_scene {
        return;
    }
    if meshes.is_empty() {
        return; // scene spawns asynchronously; wait for it
    }
    *done = true;
    for (name, gltf_name, vis, inherited, aabb, transform, mesh3d, material, skin) in &meshes {
        let mesh = assets.get(&mesh3d.0);
        let morphs = mesh
            .and_then(Mesh::morph_target_names)
            .map_or(0, <[String]>::len);
        info!(
            "mesh name={:?} gltf={:?} vis={:?} inherited={} verts={} morph_targets={} material={} joints={} aabb={} at {:?}",
            name.map(|n| n.to_string()),
            gltf_name.map(|g| g.0.clone()),
            vis.copied(),
            inherited
                .copied()
                .map(|v| format!("{}", v.get()))
                .unwrap_or_else(|| "ABSENT".into()),
            mesh.map_or(0, Mesh::count_vertices),
            morphs,
            material.is_some(),
            skin.map_or_else(|| "none".into(), |s| s.joints.len().to_string()),
            aabb.map(|a| format!("{:.2}..={:.2}", a.min(), a.max()))
                .unwrap_or_else(|| "NONE".into()),
            transform.translation()
        );
    }
}

/// Two playback snapshots on the debug run: the count of active clips and the
/// elapsed time prove animation is advancing, the summed morph weights prove
/// the cloth clip reaches the targets rather than only the skeleton.
fn debug_playback(
    config: Res<ViewerConfig>,
    mut tick: Local<u32>,
    players: Query<&AnimationPlayer>,
    weights: Query<&MorphWeights>,
    mut exit: MessageWriter<AppExit>,
) {
    if config.report_mode || !config.debug_scene {
        return;
    }
    *tick += 1;
    if players.is_empty() || weights.is_empty() {
        *tick = 249; // the scene is still loading; sample once it is ready
        return;
    }
    if *tick != 250 && *tick != 290 {
        return;
    }
    let active: usize = players
        .iter()
        .map(|player| player.playing_animations().count())
        .sum();
    let elapsed: Vec<f32> = players
        .iter()
        .flat_map(|player| player.playing_animations().map(|(_, a)| a.elapsed()))
        .collect();
    let sums: Vec<f32> = weights.iter().map(|w| w.weights().iter().sum()).collect();
    info!(
        "viewer: playback tick={} players={} active_clips={} elapsed={:?} morph_weight_sums={:?}",
        *tick,
        players.iter().count(),
        active,
        elapsed,
        sums
    );
    if *tick == 290 {
        info!("viewer: debug snapshots done, exiting");
        exit.write(AppExit::Success);
    }
}

/// Units benchmark: N scene instances, a warmup, then p50/p95 frame time and
/// the process RSS. It exits through the same `AppExit` path as the captures.
fn benchmark(
    time: Res<Time>,
    units: Res<Units>,
    bench: Res<Bench>,
    mut frames: Local<u32>,
    mut samples: Local<Vec<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    if !bench.0 {
        return;
    }
    *frames += 1;
    if *frames > 240 {
        samples.push(time.delta_secs());
    }
    if *frames <= 540 {
        return;
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f32::total_cmp);
    let mean = sorted.iter().sum::<f32>() / sorted.len().max(1) as f32;
    let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let p95 = sorted
        .get((sorted.len() as f32 * 0.95) as usize)
        .copied()
        .unwrap_or(0.0);
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .map(|line| line.trim().to_owned())
        })
        .unwrap_or_else(|| "VmRSS: unknown".to_owned());
    info!(
        "BENCH units={} ticks={} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} {}",
        units.0,
        sorted.len(),
        mean * 1000.0,
        p50 * 1000.0,
        p95 * 1000.0,
        rss
    );
    exit.write(AppExit::Success);
}

/// The live reload state machine. Poll on a fixed cadence; stage a changed
/// file; when the staged world finishes loading, swap it in — spawn the new
/// root, despawn the old one, rebuild the clip graph from the new file — so
/// the camera, orbit, zoom, light rig, bones level and playback state all
/// survive untouched. A failed load (a half-written GLB) keeps the previous
/// scene alive and reports `load failed` until the next change.
#[allow(clippy::too_many_arguments)]
fn watch_tick(
    mut commands: Commands,
    time: Res<Time>,
    mut watch: Option<ResMut<WatchState>>,
    asset_server: Res<AssetServer>,
    worlds: Res<Assets<bevy::world_serialization::WorldAsset>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut exit: MessageWriter<AppExit>,
    mut selection: ResMut<ClipSelection>,
    mut bank: Option<ResMut<bank::Bank>>,
) {
    let Some(watch) = watch.as_mut() else {
        return;
    };
    watch.elapsed += time.delta_secs_f64();
    if let Some(limit) = watch.exit_after {
        if watch.elapsed >= limit {
            info!("viewer: live smoke limit {limit}s reached, exiting");
            exit.write(AppExit::Success);
            return;
        }
    }
    if watch.elapsed < watch.next_poll {
        return;
    }
    watch.next_poll = watch.elapsed + POLL_INTERVAL_SECS;

    if let Some(staged) = watch.files.poll() {
        if let Some(old) = watch.pending.take() {
            watch.files.remove_staged(old.seq);
        }
        let world = asset_server.load(format!("{}#Scene0", staged.asset_name));
        info!(
            "viewer: live staged {} sha {} sidecar {}",
            staged.asset_name,
            sha_short(&staged.sha),
            staged.note.display()
        );
        watch.pending = Some(PendingScene {
            seq: staged.seq,
            asset_name: staged.asset_name,
            world,
            sha: staged.sha,
            note: staged.note,
            bank_slot: bank.as_ref().and_then(|bank| bank.requested),
        });
    }

    let ready = watch
        .pending
        .as_ref()
        .is_some_and(|pending| worlds.get(&pending.world).is_some());
    let failed = watch
        .pending
        .as_ref()
        .is_some_and(|pending| asset_server.load_state(&pending.world).is_failed());
    if ready {
        let pending = watch.pending.take().expect("checked ready above");
        let home = watch.home;
        let scale = watch.scale;
        let new_root = commands
            .spawn((
                bevy::world_serialization::WorldAssetRoot(pending.world.clone()),
                Transform::from_translation(home).with_scale(Vec3::splat(scale)),
                ModelHome(home),
                ReviewModel,
            ))
            .id();
        if let Some(old) = watch.current_root {
            commands.entity(old).despawn();
        }
        watch.current_root = Some(new_root);
        commands.insert_resource(build_player_graph(
            &asset_server,
            &mut graphs,
            &pending.asset_name,
            std::fs::read(watch.files.staged_path(pending.seq))
                .ok()
                .and_then(|bytes| glb_animation_names_bytes(&bytes))
                .unwrap_or_default(),
        ));
        if let (Some(bank), Some(index)) = (bank.as_mut(), pending.bank_slot) {
            selection.0 = bank
                .slot(index)
                .expect("staged bank slot")
                .clip
                .unwrap_or(usize::MAX);
            bank.current = index;
            bank.requested = None;
            bank.error = None;
        }
        watch.sha = pending.sha.clone();
        watch.note = pending.note.display().to_string();
        watch.failed = false;
        // Free the replaced generation: its staged copy is gone from disk and
        // its asset unloads once nothing holds a strong handle any more.
        if let Some(old_seq) = watch.current_seq.replace(pending.seq) {
            watch.files.remove_staged(old_seq);
        }
        info!(
            "viewer: live swap to {} sha {} — camera, orbit, lights, bones and playback state kept",
            pending.asset_name,
            sha_short(&pending.sha)
        );
    } else if failed {
        let pending = watch.pending.take().expect("checked failed above");
        watch.files.remove_staged(pending.seq);
        watch.failed = true;
        if let Some(bank) = bank.as_mut() {
            bank.requested = None;
            bank.error = Some("Load failed; previous slot kept".into());
        }
        eprintln!(
            "viewer: live load failed for {} — keeping the previous scene",
            pending.asset_name
        );
    }
}

/// Reflect the live line in the controls card and the window title. Status
/// first, so the niri focus rules keep matching `^diagnostic · ` titles even
/// while the sha and note change under it.
fn watch_card(
    config: Res<ViewerConfig>,
    mut watch: Option<ResMut<WatchState>>,
    mut lines: Query<&mut Text, With<WatchLine>>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    bank: Option<Res<bank::Bank>>,
) {
    let Some(watch) = watch.as_mut() else {
        return;
    };
    let line = match bank.as_ref() {
        Some(bank) => format!("{} · {}", config.status, bank.caption(bank.current)),
        None => watch.line(&config.status),
    };
    if watch.last_line == line {
        return;
    }
    for mut text in &mut lines {
        text.0 = line.clone();
    }
    if let Ok(mut window) = windows.single_mut() {
        window.title = match bank.is_some() {
            true => format!("Sculpto cloth review — {line}"),
            false => format!("{line} — Sculpto live review"),
        };
    }
    watch.last_line = line;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn viewer_config(interactive: bool, flat: bool) -> ViewerConfig {
        ViewerConfig {
            report_mode: false,
            debug_scene: false,
            interactive,
            flat,
            asset_file: String::new(),
            status: "InProgress".to_string(),
            control: None,
        }
    }

    fn press(app: &mut App, key: KeyCode) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.clear();
        input.release_all();
        input.press(key);
    }

    #[test]
    fn l_toggles_flat_and_back() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(FlatMode(false))
            .add_systems(Update, toggle_flat);

        press(&mut app, KeyCode::KeyL);
        app.update();
        assert!(app.world().resource::<FlatMode>().0, "first L enters flat");

        press(&mut app, KeyCode::KeyL);
        app.update();
        assert!(
            !app.world().resource::<FlatMode>().0,
            "second L returns to lit"
        );
    }

    #[test]
    fn l_is_ignored_when_not_interactive() {
        let mut app = App::new();
        app.insert_resource(viewer_config(false, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(FlatMode(false))
            .add_systems(Update, toggle_flat);

        press(&mut app, KeyCode::KeyL);
        app.update();
        assert!(!app.world().resource::<FlatMode>().0);
    }

    #[test]
    fn flat_restores_every_material_it_flattened() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(FlatMode(false))
            .insert_resource(Assets::<StandardMaterial>::default())
            .init_resource::<FlatFlipped>()
            .add_systems(Update, apply_flat_mode);
        let id = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default())
            .id();

        app.world_mut().resource_mut::<FlatMode>().0 = true;
        app.update();
        app.update();
        assert!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(id)
                .expect("material")
                .unlit,
            "flat renders unlit"
        );
        assert_eq!(
            app.world().resource::<FlatFlipped>().0.len(),
            1,
            "a material is remembered once, not once per frame"
        );

        app.world_mut().resource_mut::<FlatMode>().0 = false;
        app.update();
        assert!(
            !app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(id)
                .expect("material")
                .unlit,
            "lit restores what flat flattened"
        );
        assert!(app.world().resource::<FlatFlipped>().0.is_empty());
    }

    #[test]
    fn default_rig_shows_the_key_only() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(FlatMode(false))
            .insert_resource(RigMode { three_point: false })
            .add_systems(Update, apply_review_visibility);
        let key = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewLight, KeyLight))
            .id();
        let fill = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewLight, FillLight))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(key).expect("key"),
            Visibility::Inherited
        );
        assert_eq!(
            *app.world().get::<Visibility>(fill).expect("fill"),
            Visibility::Hidden,
            "one source by default: the fill would flatten the shadow"
        );
    }

    #[test]
    fn three_point_shows_fill_and_flat_hides_everything() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(FlatMode(false))
            .insert_resource(RigMode { three_point: true })
            .add_systems(Update, apply_review_visibility);
        let fill = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewLight, FillLight))
            .id();
        let ground = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewGround))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(fill).expect("fill"),
            Visibility::Inherited
        );

        app.world_mut().resource_mut::<FlatMode>().0 = true;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(fill).expect("fill"),
            Visibility::Hidden
        );
        assert_eq!(
            *app.world().get::<Visibility>(ground).expect("ground"),
            Visibility::Hidden,
            "flat keeps the black backdrop for the silhouette"
        );
    }

    #[test]
    fn f3_toggles_the_rig_without_consuming_slot_three() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(RigMode { three_point: false })
            .add_systems(Update, toggle_rig);

        press(&mut app, KeyCode::Digit3);
        app.update();
        assert!(!app.world().resource::<RigMode>().three_point);
        press(&mut app, KeyCode::F3);
        app.update();
        assert!(app.world().resource::<RigMode>().three_point);
    }

    #[test]
    fn h_hides_and_shows_the_help_card() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, toggle_help);
        let card = app
            .world_mut()
            .spawn((Visibility::Inherited, HelpOverlay))
            .id();

        press(&mut app, KeyCode::KeyH);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(card).expect("card"),
            Visibility::Hidden
        );

        press(&mut app, KeyCode::KeyH);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(card).expect("card"),
            Visibility::Inherited
        );
    }

    #[test]
    fn sunlight_is_white_high_up_and_warmer_and_dimmer_near_the_horizon() {
        let (high, high_lux) = SunOrbit::new(55.0).lighting();
        let (middle, middle_lux) = SunOrbit::new(25.0).lighting();
        let (low, low_lux) = SunOrbit::new(6.0).lighting();
        let [high, middle, low] = [high, middle, low].map(|color| color.to_linear());

        assert_eq!([high.red, high.green, high.blue], [1.0; 3]);
        assert!(low.blue < low.green && low.green < low.red);
        assert!(low.blue < middle.blue && middle.blue < high.blue);
        assert!(low_lux < middle_lux && middle_lux < high_lux);
        assert_eq!(SunOrbit::new(-10.0).elevation, SunOrbit::MIN_ELEVATION);
        assert_eq!(SunOrbit::new(90.0).elevation, SunOrbit::MAX_ELEVATION);
    }

    #[test]
    fn shift_drag_updates_sun_direction_and_lighting_in_the_same_frame() {
        let mut app = App::new();
        let high = SunOrbit::new(55.0);
        app.insert_resource(viewer_config(true, false))
            .insert_resource(high)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<MouseMotion>()
            .add_systems(Update, light_controls);
        let sun = app
            .world_mut()
            .spawn((KeyLight, DirectionalLight::default(), high.transform()))
            .id();
        press(&mut app, KeyCode::ShiftLeft);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(0.0, (55.0_f32 - 6.0).to_radians() / 0.005),
        });
        app.update();

        let light = app.world().get::<DirectionalLight>(sun).unwrap();
        let direction = app.world().get::<Transform>(sun).unwrap().rotation * Vec3::NEG_Z;
        assert!((direction.y + 6.0_f32.to_radians().sin()).abs() < 1e-5);
        assert!(light.color.to_linear().blue < 0.25);
        assert!(light.illuminance < 4_000.0);
    }

    #[test]
    fn character_pose_rotates_and_offsets_from_home() {
        let mut app = App::new();
        app.insert_resource(CharacterPose {
            yaw: 0.75,
            pitch: 0.0,
            offset: Vec3::new(0.1, 0.2, 0.0),
        })
        .add_systems(Update, apply_character_pose);
        let model = app
            .world_mut()
            .spawn((
                Transform::default(),
                ModelHome(Vec3::new(1.0, 0.0, 2.0)),
                ReviewModel,
            ))
            .id();

        app.update();
        let transform = app.world().get::<Transform>(model).expect("model");
        assert!(
            (transform.rotation.to_euler(bevy::math::EulerRot::YXZ).0 - 0.75).abs() < 1e-4,
            "Ctrl+LMB turns the figure"
        );
        assert_eq!(
            transform.translation,
            Vec3::new(1.1, 0.2, 2.0),
            "Ctrl+RMB offsets the figure from its home"
        );
    }

    fn orbit_test_app(interactive: bool) -> (App, Entity) {
        let mut app = App::new();
        let eye = Vec3::new(0.0, 1.0, 3.0);
        app.insert_resource(viewer_config(interactive, false))
            .insert_resource(OrbitCamera::looking_at(eye, Vec3::Y))
            .init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<MouseMotion>()
            .add_message::<MouseWheel>()
            .add_systems(Update, orbit_controls);
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), Transform::from_translation(eye)))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(100));
        (app, camera)
    }

    #[test]
    fn wasd_moves_toward_or_sideways_from_the_orbit_target() {
        for (key, target_offset, radius) in [
            (KeyCode::KeyW, Vec3::ZERO, 3.0 - 0.513375),
            (KeyCode::KeyA, Vec3::NEG_X * 0.513375, 3.0),
            (KeyCode::KeyS, Vec3::ZERO, 3.0 + 0.513375),
            (KeyCode::KeyD, Vec3::X * 0.513375, 3.0),
        ] {
            let (mut app, camera) = orbit_test_app(true);
            press(&mut app, key);
            app.update();

            let target = Vec3::Y + target_offset;
            let orbit = app.world().resource::<OrbitCamera>();
            assert!(orbit.target.abs_diff_eq(target, 1e-5));
            assert!((orbit.distance - radius).abs() < 1e-5);
            let eye = app.world().get::<Transform>(camera).unwrap().translation;
            assert!(eye.abs_diff_eq(target + radius * Vec3::Z, 1e-5));

            press(&mut app, KeyCode::KeyR);
            app.update();
            let orbit = app.world().resource::<OrbitCamera>();
            assert_eq!(orbit.target, Vec3::Y);
            assert_eq!(orbit.distance, 3.0);
        }
    }

    #[test]
    fn mouse_orbit_uses_the_smaller_radius_while_wasd_moves_diagonally() {
        let (mut app, camera) = orbit_test_app(true);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::KeyW);
            keys.press(KeyCode::KeyD);
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(-std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_6) / 0.005,
        });
        app.update();

        let diagonal = std::f32::consts::FRAC_1_SQRT_2;
        let target = Vec3::Y - Vec3::Z * (0.513375 * diagonal);
        let radius = 3.0 - 0.513375 * diagonal;
        let orbit = app.world().resource::<OrbitCamera>();
        assert!(orbit.target.abs_diff_eq(target, 1e-5));
        assert!((orbit.distance - radius).abs() < 1e-5);
        let eye_offset = Vec3::new(3.0_f32.sqrt() / 2.0, -0.5, 0.0) * radius;
        let eye = app.world().get::<Transform>(camera).unwrap().translation;
        assert!(eye.abs_diff_eq(target + eye_offset, 1e-5));
    }

    #[test]
    fn mouse_orbit_keeps_the_same_angular_sensitivity_at_every_radius() {
        for radius in [6.0, 3.0, 1.5, 0.75] {
            let (mut app, _) = orbit_test_app(true);
            app.world_mut().resource_mut::<OrbitCamera>().distance = radius;
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .press(MouseButton::Left);
            app.world_mut().write_message(MouseMotion {
                delta: Vec2::new(40.0, -20.0),
            });
            app.update();

            let orbit = app.world().resource::<OrbitCamera>();
            assert!((orbit.yaw + 0.2).abs() < 1e-5);
            assert!((orbit.pitch - 0.1).abs() < 1e-5);
            assert_eq!(orbit.distance, radius);
        }
    }

    #[test]
    fn right_button_pan_uses_the_translation_gain_at_each_radius() {
        for radius in [0.75, 3.0] {
            let (mut app, _) = orbit_test_app(true);
            app.world_mut().resource_mut::<OrbitCamera>().distance = radius;
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .press(MouseButton::Right);
            app.world_mut().write_message(MouseMotion {
                delta: Vec2::new(40.0, -20.0),
            });
            app.update();

            let orbit = app.world().resource::<OrbitCamera>();
            let target = Vec3::Y + Vec3::new(-0.61605, -0.308025, 0.0);
            assert!(orbit.target.abs_diff_eq(target, 1e-5));
            assert_eq!((orbit.yaw, orbit.pitch, orbit.distance), (0.0, 0.0, radius));
        }
    }

    #[test]
    fn wasd_travels_the_full_world_distance_across_both_orbit_limits() {
        for (interactive, key, steps, delta_ms, radius, displacement) in [
            (true, KeyCode::KeyW, 1, 100, 3.0, -0.513375),
            (true, KeyCode::KeyW, 4, 25, 0.09572126, -0.513375),
            (true, KeyCode::KeyW, 1, 100, 0.09572126, -0.513375),
            (true, KeyCode::KeyW, 1, 100, MIN_ORBIT_RADIUS, -0.513375),
            (true, KeyCode::KeyS, 1, 100, 29.9, 0.513375),
            (false, KeyCode::KeyW, 1, 100, 3.0, 0.0),
        ] {
            let (mut app, _) = orbit_test_app(interactive);
            app.world_mut().resource_mut::<OrbitCamera>().distance = radius;
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_millis(delta_ms));
            press(&mut app, key);
            for _ in 0..steps {
                app.update();
            }
            let orbit = app.world().resource::<OrbitCamera>();
            let expected_eye_z: f32 = radius + displacement;
            let expected_radius = expected_eye_z.clamp(MIN_ORBIT_RADIUS, 30.0);
            assert!((orbit.distance - expected_radius).abs() < 1e-5);
            let eye = orbit.target + Vec3::Z * orbit.distance;
            assert!(eye.abs_diff_eq(Vec3::Y + Vec3::Z * expected_eye_z, 1e-5));
        }
    }

    #[test]
    fn fit_widens_the_zoom_range_for_large_assets() {
        let mut orbit = OrbitCamera::looking_at(Vec3::new(2.2, 1.3, 2.8), Vec3::new(0.0, 0.9, 0.0));
        orbit.fit(Vec3::new(0.0, 10.0, 0.0), 400.0);
        assert_eq!(orbit.distance, 400.0);
        assert_eq!(orbit.target, Vec3::new(0.0, 10.0, 0.0));
        assert!(
            orbit.max_distance >= 400.0,
            "a 100-unit asset must zoom out past its opening distance"
        );
        assert!(orbit.min_distance > 0.0);
        orbit.reset();
        assert_eq!(orbit.distance, 400.0, "R restores the fitted frame");
    }

    #[test]
    fn holding_w_walks_through_an_offset_focus_in_the_reported_archer_view() {
        let (mut app, camera) = orbit_test_app(true);
        let target = Vec3::new(-0.35721472, 0.7243093, 0.96193457);
        let yaw = -19.45375;
        let pitch = -0.16969746;
        let forward = Quat::from_euler(bevy::math::EulerRot::YXZ, yaw, pitch, 0.0) * Vec3::NEG_Z;
        let opening = 1.9144252;
        let former_wall = 0.09572126;
        {
            let mut orbit = app.world_mut().resource_mut::<OrbitCamera>();
            orbit.fit(target, opening);
            orbit.yaw = yaw;
            orbit.pitch = pitch;
            orbit.distance = former_wall;
        }
        let mut previous_eye = target - former_wall * forward;
        press(&mut app, KeyCode::KeyW);
        for _ in 0..3 {
            app.update();
            let eye = app.world().get::<Transform>(camera).unwrap().translation;
            assert!((eye - previous_eye).abs_diff_eq(forward * (opening * 0.171125), 1e-5));
            assert!((eye - target).dot(forward) > 0.0);
            previous_eye = eye;
        }
    }

    #[test]
    fn a_reloaded_material_keeps_the_current_overlay_opacity() {
        let mut app = App::new();
        app.insert_resource(SkelMode { level: 2 })
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<GlassRestore>()
            .add_systems(Update, apply_skeleton_mode);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut().spawn(MeshMaterial3d(handle.clone()));
        app.update();
        *app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&handle)
            .unwrap() = StandardMaterial::default();
        app.update();
        let material = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap();
        assert_eq!(material.base_color.alpha(), ACTOR_OPACITY);
        assert_eq!(material.alpha_mode, AlphaMode::Blend);
        assert_eq!(app.world().resource::<GlassRestore>().0.len(), 1);
    }

    #[test]
    fn camera_waits_for_actor_bounds_instead_of_fitting_the_overlay() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(OrbitCamera::looking_at(Vec3::new(0.0, 1.0, 3.0), Vec3::Y))
            .add_systems(Update, fit_camera_to_scene);
        app.world_mut().spawn((
            Mesh3d::default(),
            SkelJoint,
            bevy::camera::primitives::Aabb::from_min_max(Vec3::splat(-10.0), Vec3::splat(10.0)),
            GlobalTransform::default(),
        ));
        app.update();
        assert_eq!(app.world().resource::<OrbitCamera>().target, Vec3::Y);
        app.world_mut().spawn((
            Mesh3d::default(),
            bevy::camera::primitives::Aabb::from_min_max(
                Vec3::new(-0.3, 0.0, -0.1),
                Vec3::new(0.3, 1.8, 0.1),
            ),
            GlobalTransform::default(),
        ));
        app.update();
        assert!((app.world().resource::<OrbitCamera>().target.y - 0.9).abs() < 1e-5);
    }

    fn temp_watch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sculpto-viewer-watch-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp watch dir");
        dir
    }

    fn watch_files(dir: &std::path::Path) -> WatchFiles {
        WatchFiles {
            dir: dir.to_path_buf(),
            stage: "actor".to_string(),
            staging_dir: dir.join("staging"),
            seq: 0,
            seen: None,
        }
    }

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn poll_stages_a_change_once_per_write() {
        let dir = temp_watch_dir("poll");
        let mut files = watch_files(&dir);
        std::fs::create_dir_all(&files.staging_dir).expect("staging dir");

        assert!(
            files.poll().is_none(),
            "nothing to stage before the file exists"
        );

        std::fs::write(dir.join("actor.glb"), b"first").expect("write");
        let staged = files.poll().expect("first write stages");
        assert_eq!(staged.asset_name, "live-0.glb");
        assert_eq!(staged.seq, 0);
        assert_eq!(
            std::fs::read(files.staged_path(0)).expect("staged copy"),
            b"first"
        );
        assert!(files.poll().is_none(), "an unchanged file must not restage");

        std::fs::write(dir.join("actor.glb"), b"second-version").expect("write");
        let staged = files.poll().expect("a changed file stages again");
        assert_eq!(staged.asset_name, "live-1.glb");
        assert_eq!(
            std::fs::read(files.staged_path(1)).expect("staged copy"),
            b"second-version"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_notes_are_truthful() {
        let dir = temp_watch_dir("sidecar");
        let mut files = watch_files(&dir);
        std::fs::create_dir_all(&files.staging_dir).expect("staging dir");
        std::fs::write(dir.join("actor.glb"), b"body").expect("write");

        let staged = files.poll().expect("staged");
        assert!(
            matches!(staged.note, SidecarNote::Missing),
            "no sidecar file yet"
        );

        std::fs::write(
            dir.join("actor.json"),
            r#"{"status":"InProgress","sha256":"deadbeef","note":"authored M1 on eb318020"}"#,
        )
        .expect("sidecar");
        std::fs::write(dir.join("actor.glb"), b"body-v2").expect("write");
        let staged = files.poll().expect("staged");
        assert_eq!(staged.note.display(), "authored M1 on eb318020");

        std::fs::write(dir.join("actor.json"), "not json at all {").expect("sidecar");
        std::fs::write(dir.join("actor.glb"), b"body-v3").expect("write");
        let staged = files.poll().expect("staged");
        assert_eq!(staged.note.display(), "sidecar unparsable");

        std::fs::write(dir.join("actor.json"), r#"{"status":"InProgress"}"#).expect("sidecar");
        std::fs::write(dir.join("actor.glb"), b"body-v4").expect("write");
        let staged = files.poll().expect("staged");
        assert_eq!(staged.note.display(), "no sidecar note");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_line_keeps_status_first_for_niri() {
        let line = live_line(
            "diagnostic",
            "weights-0164",
            "4c585296",
            "authored M1 on eb318020",
        );
        assert!(
            line.starts_with("diagnostic · "),
            "niri focus rules match this prefix: {line}"
        );
        assert!(line.contains("weights-0164"));
        assert!(line.contains("4c585296"));
        assert!(line.contains("authored M1 on eb318020"));
        assert_eq!(
            live_line("diagnostic", "s", "", "note"),
            "diagnostic · live s — · note",
            "no sha yet shows an em dash, never an invented value"
        );
    }

    #[test]
    fn poll_cost_is_invisible() {
        // The idle path is one `stat` per poll; the change path adds one read,
        // one hash and one write. Measure both so the report quotes numbers.
        let dir = temp_watch_dir("cost");
        let path = dir.join("actor.glb");
        std::fs::write(&path, vec![0u8; 4 * 1024 * 1024]).expect("4 MiB body");

        let start = std::time::Instant::now();
        for _ in 0..2000 {
            let metadata = std::fs::metadata(&path).expect("stat");
            let _ = (metadata.modified().expect("mtime"), metadata.len());
        }
        let per_stat_us = start.elapsed().as_secs_f64() * 1e6 / 2000.0;

        let bytes = std::fs::read(&path).expect("read");
        let start = std::time::Instant::now();
        let hash = sha256_hex(&bytes);
        let hash_ms = start.elapsed().as_secs_f64() * 1e3;

        println!(
            "poll cost: stat {per_stat_us:.1} µs, sha256 of 4 MiB {hash_ms:.1} ms ({head})",
            head = &hash[..8]
        );
        assert!(
            per_stat_us < 100.0,
            "two stat polls per second must stay far below 0.5 ms of work"
        );
        assert!(
            hash_ms < 500.0,
            "the one-off change hash must not approach a frame budget"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
