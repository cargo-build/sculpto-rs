//! Headless Bevy proof: load the accepted actor GLB and the baked cloth GLB
//! (morph animation), render an offscreen frame, save it as PNG, exit.
//! Also provides `--headless --report --glb <path>` count reporting for consumer acceptance.
use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::mesh::morph::{MorphWeights, MAX_MORPH_WEIGHTS};
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
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
}

/// Runtime lit/flat state; `SCULPTO_VIEWER_FLAT` seeds it and `L` flips it.
#[derive(Resource, Clone, Copy)]
struct FlatMode(bool);

/// Marks the review lights so flat mode can hide the rig without touching it.
#[derive(Component)]
struct ReviewLight;

/// Fill and rim lights: visible only in the optional three-point rig, so the
/// default is one key light and its honest runtime shadow.
#[derive(Component)]
struct FillLight;

/// The key light is the only default source; it casts the shadow.
#[derive(Component)]
struct KeyLight;

/// Review ground: it exists only to receive the key light's shadow in the
/// interactive window, which is how a turntable turn proves the light is
/// runtime rather than baked onto the model.
#[derive(Component)]
struct ReviewGround;

/// Review rig: one key light by default; `3` adds the fill/rim pair.
#[derive(Resource, Clone, Copy)]
struct RigMode {
    three_point: bool,
}

/// Materials flattened by the flat mode, so switching back restores them.
#[derive(Resource, Default)]
struct FlatFlipped(Vec<AssetId<StandardMaterial>>);

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

/// Key-light orbit: Shift+LMB moves the light around the figure while the
/// character stays put, so the shading and cast shadow follow the light.
#[derive(Resource, Clone, Copy)]
struct LightOrbit {
    azimuth: f32,
    elevation: f32,
    radius: f32,
}

impl LightOrbit {
    fn from_position(position: Vec3) -> Self {
        let radius = position.length().max(0.2);
        Self {
            azimuth: position.x.atan2(position.z),
            elevation: (position.y / radius).asin(),
            radius,
        }
    }

    fn position(self) -> Vec3 {
        let horizontal = self.radius * self.elevation.cos();
        Vec3::new(
            horizontal * self.azimuth.sin(),
            self.radius * self.elevation.sin(),
            horizontal * self.azimuth.cos(),
        )
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

/// Selected actor clip: `0` idle, `1` walk, `2` the actor-extra clip when
/// `SCULPTO_VIEWER_ACTOR_ANIMATION` names one. Cloth keeps its own clip.
#[derive(Resource, Clone, Copy)]
struct ClipSelection(usize);

/// Root joint held at its loaded translation, so a clip plays without carrying the
/// figure across the floor (operator, 2026-11-16: a preview walks in place — world
/// displacement hides joint motion behind a moving camera).
#[derive(Component, Clone, Copy)]
struct InPlaceRoot {
    translation: Vec3,
}

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

/// Whether `SCULPTO_VIEWER_ACTOR_ANIMATION` names an extra actor clip (index ≥ 2).
fn has_actor_extra() -> bool {
    std::env::var("SCULPTO_VIEWER_ACTOR_ANIMATION")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|index| index >= 2)
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
    sha8: String,
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
            sha8: sha256_hex(&bytes)[..8].to_string(),
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
    sha8: String,
    note: SidecarNote,
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
    /// The generation on screen: its sha and sidecar note.
    sha8: String,
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
                status, self.files.stage, self.sha8
            );
        }
        live_line(status, &self.files.stage, &self.sha8, &self.note)
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
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return None;
    }
    let chunk_len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let chunk_type = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    if chunk_type != 0x4E4F_534A || 20 + chunk_len > bytes.len() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + chunk_len]).ok()?;
    Some(json.get("animations")?.as_array()?.len())
}

fn main() {
    let mut options = parse_args();
    let (asset_dir, mut asset_file) = if let Some(ref glb) = options.glb_path {
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
            sha8: String::new(),
            note: String::new(),
            failed: false,
            ready: false,
            last_line: String::new(),
        };
        if let Some(staged) = state.files.poll() {
            eprintln!(
                "viewer: live start {} sha {} sidecar {}",
                staged.asset_name,
                staged.sha8,
                staged.note.display()
            );
            asset_file = staged.asset_name;
            state.sha8 = staged.sha8;
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
    let requested_clip = std::env::var("SCULPTO_VIEWER_ACTOR_ANIMATION")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let clip_note = match (requested_clip, glb_animation_count(&asset_path)) {
        (Some(index), Some(count)) if count > 0 && index >= count => {
            let fallback = count - 1;
            std::env::set_var("SCULPTO_VIEWER_ACTOR_ANIMATION", fallback.to_string());
            eprintln!(
                "viewer: requested actor clip {index}, file carries {count} — showing {fallback}"
            );
            format!(" [no clip {index}: file has {count}, showing {fallback}]")
        }
        _ => String::new(),
    };

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
                        title: format!(
                            "{} — Sculpto cloth review — drag orbit (Ctrl+drag figure), Shift+drag light, RMB pan, wheel zoom, R reset, L lit/flat, 3 rig, B bones, Space pause, Ctrl+,/. speed, H help",
                            headline
                        ),
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
    app.insert_resource(FlatFlipped::default())
        .insert_resource(ClipSelection(
            std::env::var("SCULPTO_VIEWER_ACTOR_ANIMATION")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        ))
        .insert_resource(InPlaceEnabled(
            std::env::var("SCULPTO_VIEWER_IN_PLACE").map_or(true, |value| value != "0"),
        ))
        .insert_resource(LightOrbit::from_position(Vec3::new(3.0, 5.0, 2.0)))
        .insert_resource(CharacterPose {
            yaw: env_f32("SCULPTO_VIEWER_TURNTABLE_YAW", 0.0),
            pitch: 0.0,
            offset: Vec3::ZERO,
        })
        .insert_resource(FlatMode(config.flat))
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
            brightness: env_f32(
                "SCULPTO_VIEWER_AMBIENT",
                if options.interactive { 60.0 } else { 0.0 },
            ),
            ..default()
        })
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
    if config.interactive && keys.just_pressed(KeyCode::Digit3) {
        rig.three_point = !rig.three_point;
    }
}

#[derive(Component)]
struct SkelJoint;

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
            if restore.0.contains(&id) {
                continue;
            }
            if let Some(mut material) = materials.get_mut(id) {
                material.base_color = material.base_color.with_alpha(0.16);
                material.alpha_mode = AlphaMode::Blend;
                restore.0.push(id);
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
    if !skel.on() {
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
    /// The cloth clip, when `SCULPTO_VIEWER_CLOTH_ANIMATION` names one. It plays
    /// alongside whatever actor clip is selected, on its own node.
    cloth: Option<AnimationNodeIndex>,
}

/// The actor idle/walk clips (plus the optional extra and cloth clips) as one
/// graph. Shared by startup and every live swap, so a reloaded file plays its
/// own clips under the same `0`/`1` keys the operator already set.
fn build_player_graph(
    asset_server: &AssetServer,
    graphs: &mut Assets<AnimationGraph>,
    asset_file: &str,
) -> PlayerGraph {
    let actor_idle = asset_server.load(format!("{asset_file}#Animation0"));
    let actor_walk = asset_server.load(format!("{asset_file}#Animation1"));
    let (mut graph, actor_idle_node) = AnimationGraph::from_clip(actor_idle);
    let actor_walk_node = graph.add_clip(actor_walk, 1.0, graph.root);
    // A separate cloth clip exists only when the file carries one and the
    // caller names it. Defaulting it to `Animation0` played the actor's own
    // idle clip a second time at full weight, blending two clips on the same
    // skeleton — the figure trembled and the walk was diluted by half.
    let mut nodes = vec![actor_idle_node, actor_walk_node];
    // `SCULPTO_VIEWER_ACTOR_ANIMATION=N` with N ≥ 2 adds the named actor clip
    // (for example an adapted Havok spike) as node 2. `select_clip` reaches it
    // with the `2` key; cloth keeps its own slot behind it.
    if has_actor_extra() {
        let extra_index = std::env::var("SCULPTO_VIEWER_ACTOR_ANIMATION")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2);
        let extra_clip = asset_server.load(format!("{asset_file}#Animation{extra_index}"));
        nodes.push(graph.add_clip(extra_clip, 1.0, graph.root));
    }
    let cloth = std::env::var("SCULPTO_VIEWER_CLOTH_ANIMATION")
        .ok()
        .map(|index| {
            let cloth_clip = asset_server.load(format!("{asset_file}#Animation{index}"));
            graph.add_clip(cloth_clip, 1.0, graph.root)
        });
    PlayerGraph {
        handle: graphs.add(graph),
        nodes,
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
    mut watch: Option<ResMut<WatchState>>,
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
    if let Some(handle) = target_handle {
        camera.insert(bevy::camera::RenderTarget::Image(handle.into()));
    } else {
        commands.insert_resource(OrbitCamera::looking_at(camera_eye, camera_target));
    }
    // One key light by default: the shading and the cast shadow are computed
    // at runtime, so turning the figure under it changes both. `3` adds the
    // fill/rim pair when the far side is too dark to read.
    let rig = [
        (10_000.0_f32, Vec3::new(3.0, 5.0, 2.0), true),
        (3_500.0, Vec3::new(-3.0, 2.0, -2.5), false),
        (6_000.0, Vec3::new(0.0, 3.0, -4.0), false),
    ];
    for (index, (illuminance, position, key)) in rig.into_iter().enumerate() {
        let mut entity = commands.spawn((
            DirectionalLight {
                illuminance,
                shadow_maps_enabled: key && std::env::var("SCULPTO_VIEWER_NO_SHADOWS").is_err(),
                ..default()
            },
            Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y),
            ReviewLight,
        ));
        match index {
            0 => entity.insert(KeyLight),
            1 | 2 => entity.insert(FillLight),
            _ => unreachable!(),
        };
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
                    "Ctrl+LMB turn figure · Ctrl+RMB move sideways",
                    "Shift+LMB move key light",
                    if has_actor_extra() {
                        "0 idle · 1 walk · 2 attack"
                    } else {
                        "0 idle · 1 walk"
                    },
                    if has_actor_extra() {
                        "0 idle · 1 walk · 2 attack · Space pause · Ctrl+, slower · Ctrl+. faster"
                    } else {
                        "0 idle · 1 walk · Space pause · Ctrl+, slower · Ctrl+. faster"
                    },
                    "R reset · L lit/flat · 3 rig · B bones (chain/all) · H help",
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
            });
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
        let distance = offset.length().max(0.2);
        let yaw = offset.x.atan2(offset.z);
        let pitch = (offset.y / distance).asin();
        Self {
            target,
            yaw,
            pitch,
            distance,
            min_distance: 0.2,
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
        self.min_distance = (distance * 0.05).max(0.01);
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
    meshes: Query<
        (&bevy::camera::primitives::Aabb, &GlobalTransform),
        (With<Mesh3d>, Without<ReviewGround>),
    >,
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

/// Interactive-only orbit camera: LMB drag orbits, RMB drag pans, wheel zooms,
/// R resets. It never runs in the headless capture path.
fn orbit_controls(
    config: Res<ViewerConfig>,
    orbit: Option<ResMut<OrbitCamera>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
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
        orbit.yaw -= delta.x * 0.005;
        orbit.pitch = (orbit.pitch - delta.y * 0.005).clamp(-1.45, 1.45);
    }
    if buttons.pressed(MouseButton::Right) && !moving_character {
        let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let right = rotation * Vec3::X;
        let up = rotation * Vec3::Y;
        let pan_scale = orbit.distance * 0.0015;
        orbit.target += (-right * delta.x + up * delta.y) * pan_scale;
    }
    if zoom.abs() > f32::EPSILON {
        orbit.distance =
            (orbit.distance * (1.0 - zoom * 0.12)).clamp(orbit.min_distance, orbit.max_distance);
    }

    let Ok(mut transform) = cameras.single_mut() else {
        return;
    };
    let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    transform.translation = orbit.target + rotation * Vec3::Z * orbit.distance;
    transform.look_at(orbit.target, Vec3::Y);
}

/// Shift+LMB moves the key light around the figure; the character stays put so
/// a shadow comparison is a light change, not a pose change.
fn light_controls(
    config: Res<ViewerConfig>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    orbit: Option<ResMut<LightOrbit>>,
    mut lights: Query<&mut Transform, With<KeyLight>>,
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
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
        && buttons.pressed(MouseButton::Left)
    {
        orbit.azimuth -= delta.x * 0.005;
        orbit.elevation = (orbit.elevation - delta.y * 0.005).clamp(-0.2, 1.45);
    }
    let Ok(mut transform) = lights.single_mut() else {
        return;
    };
    transform.translation = orbit.position();
    transform.look_at(Vec3::ZERO, Vec3::Y);
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
    let selection = selection.0.min(graph.nodes.len().saturating_sub(1));
    let cloth = graph.cloth;
    for (entity, mut player) in &mut players {
        player.play(graph.nodes[selection]).repeat();
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
    keys: Res<ButtonInput<KeyCode>>,
    graph: Option<Res<PlayerGraph>>,
    mut selection: ResMut<ClipSelection>,
    mut players: Query<&mut AnimationPlayer>,
) {
    if !config.interactive {
        return;
    }
    let wanted = if keys.just_pressed(KeyCode::Digit0) {
        Some(0)
    } else if keys.just_pressed(KeyCode::Digit1) {
        Some(1)
    } else if keys.just_pressed(KeyCode::Digit2)
        && graph.as_ref().is_some_and(|g| g.nodes.len() > 2)
    {
        Some(2)
    } else {
        None
    };
    let Some(wanted) = wanted else {
        return;
    };
    if selection.0 == wanted {
        return;
    }
    selection.0 = wanted;
    let Some(graph) = graph else { return };
    for mut player in &mut players {
        player.stop_all();
        player.play(graph.nodes[wanted]).repeat();
        if let Some(cloth) = graph.cloth {
            player.play(cloth).repeat();
        }
    }
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
    if bench.0 || config.report_mode || config.debug_scene || config.interactive || watch.is_some()
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
fn watch_tick(
    mut commands: Commands,
    time: Res<Time>,
    mut watch: Option<ResMut<WatchState>>,
    asset_server: Res<AssetServer>,
    worlds: Res<Assets<bevy::world_serialization::WorldAsset>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut exit: MessageWriter<AppExit>,
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
        let world = asset_server.load(format!("{}#Scene0", staged.asset_name));
        info!(
            "viewer: live staged {} sha {} sidecar {}",
            staged.asset_name,
            staged.sha8,
            staged.note.display()
        );
        watch.pending = Some(PendingScene {
            seq: staged.seq,
            asset_name: staged.asset_name,
            world,
            sha8: staged.sha8,
            note: staged.note,
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
        ));
        watch.sha8 = pending.sha8.clone();
        watch.note = pending.note.display().to_string();
        watch.failed = false;
        // Free the replaced generation: its staged copy is gone from disk and
        // its asset unloads once nothing holds a strong handle any more.
        if let Some(old_seq) = watch.current_seq.replace(pending.seq) {
            watch.files.remove_staged(old_seq);
        }
        info!(
            "viewer: live swap to {} sha {} — camera, orbit, lights, bones and playback state kept",
            pending.asset_name, pending.sha8
        );
    } else if failed {
        let pending = watch.pending.take().expect("checked failed above");
        watch.files.remove_staged(pending.seq);
        watch.failed = true;
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
) {
    let Some(watch) = watch.as_mut() else {
        return;
    };
    let line = watch.line(&config.status);
    if watch.last_line == line {
        return;
    }
    for mut text in &mut lines {
        text.0 = line.clone();
    }
    if let Ok(mut window) = windows.single_mut() {
        window.title = format!("{line} — Sculpto live review");
    }
    watch.last_line = line;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer_config(interactive: bool, flat: bool) -> ViewerConfig {
        ViewerConfig {
            report_mode: false,
            debug_scene: false,
            interactive,
            flat,
            asset_file: String::new(),
            status: "InProgress".to_string(),
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
    fn key_three_toggles_the_rig() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(RigMode { three_point: false })
            .add_systems(Update, toggle_rig);

        press(&mut app, KeyCode::Digit3);
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
