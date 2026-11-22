//! Local JSON control socket for the review viewer.
//!
//! An agent drives a running window over a Unix stream socket: one JSON object
//! per line in, one JSON object per line out. The parsing, the joint lookup and
//! the state document stay Bevy-free so `cargo test` exercises them without a
//! GPU, and the socket thread only moves strings — it never touches the ECS.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

/// Request/response schema version. A field rename or a removed command bumps
/// it, so a stale agent script fails loudly instead of silently misreading.
pub const PROTOCOL: u32 = 1;

/// One request line, including the newline. A longer line is refused rather
/// than buffered: an agent sending a file through this socket is a bug.
const MAX_LINE: u64 = 64 * 1024;

/// The app answers inside its next frame; three seconds covers a stalled frame.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// An idle connection owns the accept thread, so silence is bounded: a caller
/// that sends nothing for this long is dropped and the next connection is
/// served. The protocol is request/reply, so a connection is never worth
/// keeping open across a human pause.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// A peer that stops reading must not block the thread on its own reply.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Named camera views. `front` looks at the figure's +Z side, which is the
/// actor convention the headless review views already use.
pub const VIEWS: [&str; 7] = ["front", "back", "left", "right", "quarter", "top", "below"];

/// Playback speed bounds, the same the keyboard controls clamp to.
pub const MIN_SPEED: f32 = 0.05;
pub const MAX_SPEED: f32 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Full state document: asset identity, clip, camera, joints.
    State,
    /// Position of one joint, by exact name or unique partial name.
    Joint {
        name: String,
    },
    /// Named or explicit camera direction around the current target.
    View {
        name: Option<String>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        look_pitch: Option<f32>,
        distance: Option<f32>,
        target: Option<String>,
    },
    /// Point the camera at a named joint or an explicit world point.
    Target {
        joint: Option<String>,
        point: Option<[f32; 3]>,
    },
    Clip {
        index: usize,
    },
    Inspection,
    Slot {
        index: usize,
    },
    Seek {
        seconds: f32,
    },
    Playback {
        paused: Option<bool>,
        speed: Option<f32>,
    },
    Skeleton {
        level: u8,
    },
    Capture {
        path: String,
    },
    Quit,
    Help,
}

/// A parsed command on its way from the socket thread to the frame system.
/// The reply channel carries the finished JSON line back to the caller.
pub struct Request {
    pub command: Command,
    pub reply: Sender<String>,
}

/// The listener plus the request queue the frame system drains.
pub struct ControlServer {
    pub path: PathBuf,
    pub requests: Mutex<Receiver<Request>>,
}

/// Bind the socket and start the accept loop.
///
/// The socket is `0600`; a directory this call creates is `0700`, while an
/// existing directory keeps the permissions its owner gave it — a task
/// directory is shared with renders and logs, not owned by the socket.
///
/// The path is never clobbered: a live listener means a second viewer must
/// choose another path, and anything that is not a Unix socket (a regular
/// file, a symlink, a directory) is left exactly as found.
pub fn start(path: &Path) -> std::io::Result<ControlServer> {
    start_with_idle(path, DEFAULT_IDLE_TIMEOUT)
}

fn start_with_idle(path: &Path, idle: Duration) -> std::io::Result<ControlServer> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // Create owner-only, but never re-mode a directory that already exists.
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "{} exists and is not a Unix socket; refusing to replace it",
                    path.display()
                ),
            ));
        }
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "another viewer is listening on {}; pass a different --control path",
                        path.display()
                    ),
                ));
            }
            // Nobody is listening: a dead viewer left the inode behind, which
            // is the only case where replacing it loses nothing.
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                std::fs::remove_file(path)?;
            }
            Err(error) => return Err(error),
        }
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("viewer-control".to_string())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                serve_connection(stream, &tx, idle);
            }
        })?;
    Ok(ControlServer {
        path: path.to_path_buf(),
        requests: Mutex::new(rx),
    })
}

/// Serve one connection until the peer closes it. Sequential on purpose: the
/// review window has one operator-facing agent, and serializing keeps the
/// command order the agent sent.
fn serve_connection(stream: UnixStream, tx: &Sender<Request>, idle: Duration) {
    // Both ends are bounded: an abandoned request line or an unread reply
    // releases the single accept thread instead of wedging every later caller.
    if stream.set_read_timeout(Some(idle)).is_err()
        || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
    {
        return;
    }
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
    loop {
        let mut line = String::new();
        let read = (&mut reader).take(MAX_LINE).read_line(&mut line);
        match read {
            Ok(0) => return,
            Ok(_) if !line.ends_with('\n') => {
                let _ = write_line(&mut writer, &error_json("request too long"));
                return;
            }
            Ok(_) => {}
            Err(_) => return,
        }
        let mut quitting = false;
        let response = match parse_request(line.trim()) {
            Err(message) => error_json(&message),
            Ok(command) => {
                quitting = command == Command::Quit;
                let (reply_tx, reply_rx) = mpsc::channel();
                if tx
                    .send(Request {
                        command,
                        reply: reply_tx,
                    })
                    .is_err()
                {
                    error_json("viewer is shutting down")
                } else {
                    match reply_rx.recv_timeout(REPLY_TIMEOUT) {
                        Ok(response) => response,
                        Err(RecvTimeoutError::Timeout) => {
                            error_json("viewer did not answer within 3 s; is the window stalled?")
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            error_json("viewer closed the control")
                        }
                    }
                }
            }
        };
        if write_line(&mut writer, &response).is_err() {
            return;
        }
        if quitting {
            // The reply is on the wire, so the caller has its answer. Leave
            // through the syscall: this host's NVIDIA driver segfaults inside
            // the glibc atexit handlers that a normal teardown runs (the
            // headless report path skips them for the same reason).
            unsafe {
                libc::_exit(0);
            }
        }
    }
}

fn write_line(writer: &mut UnixStream, response: &str) -> std::io::Result<()> {
    writer.write_all(response.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Parse one request line. Every rejection names what was wrong and the current
/// value, because the caller is a language model reading one line, not a human
/// staring at a terminal.
pub fn parse_request(line: &str) -> Result<Command, String> {
    let value: Value =
        serde_json::from_str(line).map_err(|error| format!("malformed JSON: {error}"))?;
    let Some(object) = value.as_object() else {
        return Err("request must be a JSON object, e.g. {\"cmd\":\"state\"}".to_string());
    };
    let Some(cmd) = object.get("cmd") else {
        return Err("missing \"cmd\"; try {\"cmd\":\"help\"}".to_string());
    };
    let Some(cmd) = cmd.as_str() else {
        return Err("\"cmd\" must be a string".to_string());
    };
    match cmd {
        "state" => Ok(Command::State),
        "help" => Ok(Command::Help),
        "quit" => Ok(Command::Quit),
        "joint" => Ok(Command::Joint {
            name: required_string(object, "name")?,
        }),
        "view" => {
            let name = optional_string(object, "name")?;
            if let Some(name) = &name {
                if !VIEWS.contains(&name.as_str()) {
                    return Err(format!(
                        "unknown view {name:?}; known views: {}",
                        VIEWS.join(", ")
                    ));
                }
            }
            let yaw = optional_number(object, "yaw")?;
            let pitch = optional_number(object, "pitch")?;
            let look_pitch = optional_number(object, "look_pitch")?;
            let distance = optional_number(object, "distance")?;
            for (name, angle) in [("pitch", pitch), ("look_pitch", look_pitch)] {
                if angle.is_some_and(|a| a.abs() > super::MAX_ORBIT_PITCH) {
                    return Err(format!("{name} outside -1.45..1.45 rad"));
                }
            }
            if let Some(distance) = distance {
                if !(distance.is_finite() && distance > 0.0) {
                    return Err(format!("distance {distance} must be a positive length"));
                }
            }
            let target = optional_string(object, "target")?;
            if name.is_none()
                && yaw.is_none()
                && pitch.is_none()
                && look_pitch.is_none()
                && distance.is_none()
            {
                return Err(format!(
                    "view needs one of \"name\" ({}), \"yaw\", \"pitch\", \"look_pitch\", \"distance\"",
                    VIEWS.join(", ")
                ));
            }
            Ok(Command::View {
                name,
                yaw,
                pitch,
                look_pitch,
                distance,
                target,
            })
        }
        "target" => {
            let joint = optional_string(object, "joint")?;
            let point = match object.get("point") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let array = value
                        .as_array()
                        .ok_or("\"point\" must be [x, y, z] in metres")?;
                    if array.len() != 3 {
                        return Err(format!(
                            "\"point\" must hold exactly 3 numbers, got {}",
                            array.len()
                        ));
                    }
                    let mut numbers = [0.0_f32; 3];
                    for (slot, item) in numbers.iter_mut().zip(array) {
                        let number = item.as_f64().ok_or("\"point\" entries must be numbers")?;
                        *slot = finite_f32(number, "point")?;
                    }
                    Some(numbers)
                }
            };
            match (joint.is_some(), point.is_some()) {
                (true, true) => Err("give \"joint\" or \"point\", not both".to_string()),
                (false, false) => Err("target needs \"joint\" or \"point\"".to_string()),
                _ => Ok(Command::Target { joint, point }),
            }
        }
        "inspection" => Ok(Command::Inspection),
        "clip" | "slot" => {
            let index = object
                .get("index")
                .ok_or("clip needs \"index\"")?
                .as_u64()
                .ok_or("\"index\" must be a non-negative integer")?;
            let index = usize::try_from(index).map_err(|_| "index exceeds platform size")?;
            match cmd {
                "slot" => Ok(Command::Slot { index }),
                _ => Ok(Command::Clip { index }),
            }
        }
        "seek" => {
            let seconds = optional_number(object, "seconds")?.ok_or("seek requires seconds")?;
            if seconds < 0.0 {
                return Err("seek time must be nonnegative".to_owned());
            }
            Ok(Command::Seek { seconds })
        }
        "playback" => {
            let paused = match object.get("paused") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_bool().ok_or("\"paused\" must be true or false")?),
            };
            let speed = optional_number(object, "speed")?;
            if let Some(speed) = speed {
                if !(MIN_SPEED..=MAX_SPEED).contains(&speed) {
                    return Err(format!("speed {speed} outside {MIN_SPEED}..{MAX_SPEED}"));
                }
            }
            if paused.is_none() && speed.is_none() {
                return Err("playback needs \"paused\" or \"speed\"".to_string());
            }
            Ok(Command::Playback { paused, speed })
        }
        "skeleton" => {
            let level = object
                .get("level")
                .ok_or("skeleton needs \"level\" (0 off, 1 primary chain, 2 every driven joint)")?
                .as_u64()
                .ok_or("\"level\" must be 0, 1 or 2")?;
            if level > 2 {
                return Err(format!("skeleton level {level} must be 0, 1 or 2"));
            }
            Ok(Command::Skeleton { level: level as u8 })
        }
        "capture" => Ok(Command::Capture {
            path: absolute_png_path(&required_string(object, "path")?)?,
        }),
        other => Err(format!(
            "unknown command {other:?}; try {{\"cmd\":\"help\"}} for the list"
        )),
    }
}

fn required_string(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    match object.get(key) {
        None => Err(format!("missing \"{key}\"")),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(Value::String(_)) => Err(format!("\"{key}\" must not be empty")),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(Value::String(_)) => Err(format!("\"{key}\" must not be empty")),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn optional_number(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<f32>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value
                .as_f64()
                .ok_or(format!("\"{key}\" must be a number"))?;
            Ok(Some(finite_f32(number, key)?))
        }
    }
}

/// Convert after checking the value survives the narrowing: `1e100` is a
/// finite `f64` and an infinite `f32`, which would poison the camera transform
/// and every position derived from it.
fn finite_f32(number: f64, key: &str) -> Result<f32, String> {
    if !number.is_finite() {
        return Err(format!("\"{key}\" must be finite"));
    }
    let narrowed = number as f32;
    if !narrowed.is_finite() {
        return Err(format!(
            "\"{key}\" {number} is outside the f32 range; camera values are metres and radians"
        ));
    }
    Ok(narrowed)
}

/// A capture target must be an absolute PNG/JPEG path whose directory exists or
/// can be created. Checking here keeps a screenshot from failing asynchronously
/// inside the render pipeline, where the agent never sees why.
pub fn absolute_png_path(path: &str) -> Result<String, String> {
    let candidate = PathBuf::from(path);
    if !candidate.is_absolute() {
        return Err(format!("capture path {path:?} must be absolute"));
    }
    let extension = candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
        return Err(format!(
            "capture path {path:?} must end with .png, .jpg or .jpeg"
        ));
    }
    if let Some(parent) = candidate.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create capture directory {}: {error}",
                parent.display()
            )
        })?;
    }
    Ok(path.to_string())
}

/// Resolve a joint by exact name, then case-insensitively, then by unique
/// substring. The caller is usually an LLM asking for "elbow": returning every
/// candidate on an ambiguous name is the difference between one retry and a
/// guessing loop.
pub fn find_joint<'a>(
    joints: &'a [(String, [f32; 3])],
    wanted: &str,
) -> Result<&'a (String, [f32; 3]), String> {
    if joints.is_empty() {
        return Err(
            "no joints are reported; turn the overlay on with {\"cmd\":\"skeleton\",\"level\":2} \
             and load a skinned actor"
                .to_string(),
        );
    }
    if let Some(found) = joints.iter().find(|(name, _)| name == wanted) {
        return Ok(found);
    }
    let wanted_lower = wanted.to_ascii_lowercase();
    if let Some(found) = joints
        .iter()
        .find(|(name, _)| name.to_ascii_lowercase() == wanted_lower)
    {
        return Ok(found);
    }
    // A bare bone name is how an agent asks for a joint; the palette prefixes
    // it (`mixamorig:Head`). Match the last name segment before falling back to
    // a substring, so `Head` resolves to the real head while `HeadTop_End` and
    // a genuine Left/Right pair still report their candidates.
    let tail = |name: &str| {
        name.rsplit([':', '|', '/'])
            .next()
            .unwrap_or(name)
            .trim()
            .to_ascii_lowercase()
    };
    let tail_matches: Vec<&(String, [f32; 3])> = joints
        .iter()
        .filter(|(name, _)| tail(name) == wanted_lower)
        .collect();
    if tail_matches.len() == 1 {
        return Ok(tail_matches[0]);
    }
    let matches: Vec<&(String, [f32; 3])> = joints
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().contains(&wanted_lower))
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!(
            "no joint name contains {wanted:?}; state lists all {} names",
            joints.len()
        )),
        _ => {
            let listed: Vec<&str> = matches
                .iter()
                .take(8)
                .map(|(name, _)| name.as_str())
                .collect();
            let more = matches.len().saturating_sub(listed.len());
            let suffix = if more > 0 {
                format!(" (+{more} more)")
            } else {
                String::new()
            };
            Err(format!(
                "{wanted:?} matches {} joints: {}{suffix} — use a longer name",
                matches.len(),
                listed.join(", ")
            ))
        }
    }
}

/// One drawn actor material: what the review is actually rendering with.
///
/// The body and the underwear are separate glTF materials over the same mesh,
/// so "the underwear looks different" is a question about these numbers, not
/// about the overlay setting. `material` is the glTF material name
/// (`01-assembled`, `bra`, `panty`); `asset` is the index in the run's material
/// store, which keeps two identically named materials distinguishable.
#[derive(Debug, Clone, Default)]
pub struct MaterialEntry {
    pub mesh: String,
    pub material: String,
    pub asset: Option<usize>,
    pub alpha: f32,
    pub blend: String,
    pub color: [f32; 3],
    pub unlit: bool,
}

/// Everything the state document reports. Plain data so the snapshot can be
/// built from ECS resources and serialized without Bevy types.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub asset_file: String,
    pub asset_path: String,
    pub sha256: String,
    pub loaded: bool,
    /// A newer generation is staged and still loading; the digest below is the
    /// generation on screen, not the one arriving.
    pub pending: bool,
    /// The newest staged file did not parse; the scene on screen is the previous
    /// generation the digest still names.
    pub failed: bool,
    /// Scene mesh entities on screen: the evidence behind `loaded`.
    pub meshes: usize,
    pub scale: f32,
    pub status: String,
    pub clip_index: Option<usize>,
    pub clip_seconds: Option<f32>,
    pub morph_weights: Vec<(String, Vec<f32>)>,
    pub cloth: Vec<Value>,
    pub clip_name: Option<String>,
    pub bank: Option<Value>,
    pub clip_count: Option<usize>,
    pub paused: bool,
    pub speed: f32,
    pub skeleton: u8,
    pub camera_mode: &'static str,
    pub camera_target: [f32; 3],
    pub camera_yaw: f32,
    pub camera_pitch: f32,
    pub camera_look_pitch: f32,
    pub camera_distance: f32,
    pub camera_position: [f32; 3],
    pub joints: Vec<(String, [f32; 3])>,
    /// Actor materials in draw order-independent name order, overlay and ground
    /// excluded.
    pub materials: Vec<MaterialEntry>,
}

impl Snapshot {
    pub fn to_json(&self) -> String {
        let joints: Vec<Value> = self
            .joints
            .iter()
            .map(|(name, position)| {
                json!({ "name": name, "position": [position[0], position[1], position[2]] })
            })
            .collect();
        json!({
            "ok": true,
            "protocol": PROTOCOL,
            "asset": {
                "file": self.asset_file,
                "path": self.asset_path,
                "sha256": self.sha256,
                "present": !self.sha256.is_empty(),
                "loaded": self.loaded,
                "pending": self.pending,
                "failed": self.failed,
                "meshes": self.meshes,
                "scale": self.scale,
            },
            "status": self.status,
            "clip": {
                "index": self.clip_index,
                "seconds": self.clip_seconds,
                "name": self.clip_name,
                "count": self.clip_count,
                "paused": self.paused,
                "speed": self.speed,
            },
            "bank": self.bank,
            "morph_weights": self.morph_weights,
            "cloth": self.cloth,
            "skeleton": self.skeleton,
            "camera": {
                "mode": self.camera_mode,
                "target": self.camera_target,
                "yaw": self.camera_yaw,
                "pitch": self.camera_pitch,
                "look_pitch": self.camera_look_pitch,
                "distance": self.camera_distance,
                "position": self.camera_position,
            },
            "joint_count": self.joints.len(),
            "joints": joints,
            "material_count": self.materials.len(),
            "materials": self.materials.iter().map(|entry| json!({
                "mesh": entry.mesh,
                "material": entry.material,
                "asset": entry.asset,
                "alpha": entry.alpha,
                "blend": entry.blend,
                "color": entry.color,
                "unlit": entry.unlit,
            })).collect::<Vec<Value>>(),
        })
        .to_string()
    }
}

pub fn error_json(message: &str) -> String {
    json!({ "ok": false, "protocol": PROTOCOL, "error": message }).to_string()
}

pub fn ok_json(payload: Value) -> String {
    let mut object = match payload {
        Value::Object(object) => object,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("value".to_string(), other);
            map
        }
    };
    object.insert("ok".to_string(), Value::Bool(true));
    object.insert("protocol".to_string(), json!(PROTOCOL));
    Value::Object(object).to_string()
}

/// The self-description an agent reads before its first real command.
pub fn help_json() -> String {
    ok_json(json!({
        "socket": "one JSON object per line, one JSON object per line back",
        "commands": {
            "state": "asset digest, clip, camera and every joint position",
            "help": "this document",
            "joint": {"name": "exact or unique partial joint name -> position"},
            "view": {
                "name": VIEWS,
                "yaw": "radians, 0 = camera on +Z",
                "pitch": "eye angle about the pivot in radians, -1.45..1.45",
                "look_pitch": "view angle at the eye in radians, -1.45..1.45, positive looks up",
                "distance": "metres from the target",
                "target": "joint name to orbit around, same lookup as \"joint\""
            },
            "target": {"joint": "name", "point": "[x, y, z]"},
            "clip": {"index": "actor clip index; see clip.count in state"},
            "slot": {"index": "zero-based bank entry; see state.bank.slots"},
            "inspection": "open the optional disassembled rest asset (Ctrl+0)",
            "seek": {"seconds": "pause the selected actor clip at an exact time within its duration"},
            "playback": {"paused": "true|false", "speed": format!("{MIN_SPEED}..{MAX_SPEED}")},
            "skeleton": {"level": "0 off, 1 primary chain, 2 every driven joint"},
            "capture": {"path": "absolute .png path; written asynchronously, poll the file"},
            "quit": "close the viewer window"
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joints() -> Vec<(String, [f32; 3])> {
        vec![
            ("mixamorig:Hips".to_string(), [0.0, 0.9, 0.0]),
            ("mixamorig:Head".to_string(), [0.0, 1.55, 0.02]),
            ("mixamorig:HeadTop_End".to_string(), [0.0, 1.72, 0.02]),
            ("mixamorig:LeftForeArm".to_string(), [0.2, 1.1, 0.0]),
            ("mixamorig:RightForeArm".to_string(), [-0.2, 1.1, 0.0]),
            ("mixamorig:LeftHand".to_string(), [0.3, 0.9, 0.0]),
        ]
    }

    #[test]
    fn malformed_json_names_the_parse_failure() {
        let error = parse_request("{not json").expect_err("must reject");
        assert!(error.starts_with("malformed JSON:"), "{error}");
    }

    #[test]
    fn missing_and_unknown_command_are_reported() {
        assert!(parse_request("{}")
            .expect_err("must reject")
            .contains("missing \"cmd\""));
        let error = parse_request("{\"cmd\":\"explode\"}").expect_err("must reject");
        assert!(error.contains("unknown command \"explode\""), "{error}");
        assert!(error.contains("help"), "{error}");
        assert_eq!(
            parse_request("[]").expect_err("must reject"),
            "request must be a JSON object, e.g. {\"cmd\":\"state\"}"
        );
        assert_eq!(
            parse_request("{\"cmd\":3}").expect_err("must reject"),
            "\"cmd\" must be a string"
        );
    }

    #[test]
    fn valid_commands_round_trip() {
        assert_eq!(
            parse_request("{\"cmd\":\"state\"}").unwrap(),
            Command::State
        );
        assert_eq!(
            parse_request("{\"cmd\":\"clip\",\"index\":1}").unwrap(),
            Command::Clip { index: 1 }
        );
        assert_eq!(
            parse_request("{\"cmd\":\"playback\",\"paused\":true,\"speed\":0.25}").unwrap(),
            Command::Playback {
                paused: Some(true),
                speed: Some(0.25)
            }
        );
        assert_eq!(
            parse_request("{\"cmd\":\"view\",\"name\":\"front\"}").unwrap(),
            Command::View {
                name: Some("front".to_string()),
                yaw: None,
                pitch: None,
                look_pitch: None,
                distance: None,
                target: None
            }
        );
        assert_eq!(
            parse_request("{\"cmd\":\"target\",\"point\":[0.1,-0.2,1.5]}").unwrap(),
            Command::Target {
                joint: None,
                point: Some([0.1, -0.2, 1.5])
            }
        );
    }

    #[test]
    fn out_of_range_values_are_refused_with_the_bound() {
        let error = parse_request("{\"cmd\":\"playback\",\"speed\":9.0}").expect_err("must reject");
        assert!(error.contains("0.05..4"), "{error}");
        let error = parse_request("{\"cmd\":\"skeleton\",\"level\":5}").expect_err("must reject");
        assert!(error.contains("must be 0, 1 or 2"), "{error}");
        let error = parse_request("{\"cmd\":\"clip\",\"index\":-1}").expect_err("must reject");
        assert!(error.contains("non-negative integer"), "{error}");
        let error = parse_request("{\"cmd\":\"view\",\"pitch\":3.0}").expect_err("must reject");
        assert!(error.contains("-1.45"), "{error}");
        let error = parse_request("{\"cmd\":\"view\"}").expect_err("must reject");
        assert!(error.contains("view needs one of"), "{error}");
        let error =
            parse_request("{\"cmd\":\"view\",\"name\":\"bottom\"}").expect_err("must reject");
        assert!(error.contains("unknown view"), "{error}");
        let error = parse_request("{\"cmd\":\"target\",\"joint\":\"Hand\",\"point\":[0,0,0]}")
            .expect_err("must reject");
        assert!(error.contains("not both"), "{error}");
    }

    #[test]
    fn capture_paths_must_be_absolute_png() {
        let error =
            parse_request("{\"cmd\":\"capture\",\"path\":\"shot.png\"}").expect_err("must reject");
        assert!(error.contains("absolute"), "{error}");
        let error = parse_request("{\"cmd\":\"capture\",\"path\":\"/tmp/x.txt\"}")
            .expect_err("must reject");
        assert!(error.contains(".png"), "{error}");
        // The parent is created, so a fresh task directory is enough.
        let dir = std::env::temp_dir().join(format!("viewer-control-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/shot.png");
        let parsed = parse_request(&format!(
            "{{\"cmd\":\"capture\",\"path\":{:?}}}",
            path.display().to_string()
        ))
        .unwrap();
        assert_eq!(
            parsed,
            Command::Capture {
                path: path.display().to_string()
            }
        );
        assert!(
            path.parent().expect("parent").is_dir(),
            "parent dir created"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn joint_lookup_prefers_exact_then_reports_ambiguity() {
        let list = joints();
        let found = find_joint(&list, "mixamorig:Hips").expect("exact");
        assert_eq!(found.1, [0.0, 0.9, 0.0]);
        let found = find_joint(&list, "mixamorig:lefthand").expect("case-insensitive");
        assert_eq!(found.0, "mixamorig:LeftHand");
        // A bare hammer name resolves when it is unique as a name segment, even
        // though "Head" is also a substring of "HeadTop_End".
        let found = find_joint(&list, "Head").expect("unique segment");
        assert_eq!(found.0, "mixamorig:Head");
        let error = find_joint(&list, "ForeArm").expect_err("ambiguous");
        assert!(error.contains("matches 2 joints"), "{error}");
        assert!(error.contains("mixamorig:LeftForeArm"), "{error}");
        let error = find_joint(&list, "Tail").expect_err("absent");
        assert!(error.contains("no joint name contains"), "{error}");
        let error = find_joint(&[], "Hips").expect_err("no joints");
        assert!(error.contains("skeleton"), "{error}");
    }

    #[test]
    fn snapshot_reports_identity_and_geometry() {
        let snapshot = Snapshot {
            asset_file: "ybot.glb".to_string(),
            asset_path: "/tmp/ybot.glb".to_string(),
            sha256: "dae19324".to_string(),
            loaded: true,
            pending: false,
            failed: false,
            meshes: 3,
            scale: 1.0,
            status: "InProgress".to_string(),
            clip_index: Some(1),
            clip_seconds: Some(0.5),
            morph_weights: vec![],
            cloth: vec![],
            clip_name: Some("Walk".into()),
            bank: None,
            clip_count: Some(2),
            paused: true,
            speed: 0.5,
            skeleton: 2,
            camera_mode: "orbit",
            camera_target: [0.0, 0.9, 0.0],
            camera_yaw: 0.0,
            camera_pitch: 0.2,
            camera_look_pitch: 0.2,
            camera_distance: 3.5,
            camera_position: [0.0, 1.6, 3.4],
            joints: joints(),
            materials: vec![
                MaterialEntry {
                    mesh: "3BA".to_string(),
                    material: "01-assembled".to_string(),
                    asset: Some(0),
                    alpha: 0.65,
                    blend: "Blend".to_string(),
                    color: [0.91, 0.84, 0.77],
                    unlit: false,
                },
                MaterialEntry {
                    mesh: "3BA".to_string(),
                    material: "bra".to_string(),
                    asset: Some(1),
                    alpha: 0.65,
                    blend: "Blend".to_string(),
                    color: [0.18, 0.8, 0.44],
                    unlit: false,
                },
            ],
        };
        let value: Value = serde_json::from_str(&snapshot.to_json()).expect("valid JSON");
        assert_eq!(value["protocol"], PROTOCOL);
        assert_eq!(value["asset"]["sha256"], "dae19324");
        assert_eq!(value["asset"]["present"], true);
        assert_eq!(value["asset"]["pending"], false);
        assert_eq!(value["asset"]["failed"], false);
        assert_eq!(value["asset"]["meshes"], 3);
        assert_eq!(value["clip"]["count"], 2);
        assert_eq!(value["joint_count"], 6);
        let forearm = value["joints"]
            .as_array()
            .expect("joint array")
            .iter()
            .find(|joint| joint["name"] == "mixamorig:LeftForeArm")
            .expect("forearm is reported");
        let height = forearm["position"][1].as_f64().expect("number");
        assert!((height - 1.1).abs() < 1e-6, "{height}");
        // Material evidence is what a "the underwear looks different" question
        // is answered with, so its shape is pinned: name, alpha, blend, colour
        // and the store index that keeps same-named materials apart.
        assert_eq!(value["material_count"], 2);
        let bra = value["materials"]
            .as_array()
            .expect("material array")
            .iter()
            .find(|entry| entry["material"] == "bra")
            .expect("bra is reported");
        assert_eq!(bra["mesh"], "3BA");
        assert_eq!(bra["asset"], 1);
        assert_eq!(bra["blend"], "Blend");
        assert_eq!(bra["unlit"], false);
        let alpha = bra["alpha"].as_f64().expect("alpha");
        assert!((alpha - 0.65).abs() < 1e-6, "{alpha}");
    }

    #[test]
    fn help_lists_every_command() {
        let value: Value = serde_json::from_str(&help_json()).expect("valid JSON");
        for command in [
            "state", "help", "joint", "view", "target", "clip", "playback", "skeleton", "capture",
            "quit",
        ] {
            assert!(
                value["commands"].get(command).is_some(),
                "missing {command}"
            );
        }
    }

    #[test]
    fn camera_numbers_must_survive_the_f32_narrowing() {
        // 1e100 is a finite f64 and an infinite f32: it must be refused at
        // parse time, before any camera state changes.
        for request in [
            r#"{"cmd":"view","yaw":1e100}"#,
            r#"{"cmd":"view","distance":1e100}"#,
            r#"{"cmd":"view","pitch":1e100}"#,
            r#"{"cmd":"view","yaw":1e40}"#,
            r#"{"cmd":"target","point":[1e100,0,0]}"#,
            r#"{"cmd":"target","point":[0,0,-1e100]}"#,
        ] {
            let error = parse_request(request).expect_err(request);
            assert!(error.contains("f32"), "{request} -> {error}");
        }
        // A large but representable value still parses.
        let parsed = parse_request(r#"{"cmd":"view","yaw":1234.5}"#).expect("representable");
        assert_eq!(
            parsed,
            Command::View {
                name: None,
                yaw: Some(1234.5),
                pitch: None,
                look_pitch: None,
                distance: None,
                target: None
            }
        );
        // Non-numeric and non-finite JSON are named separately.
        assert!(parse_request(r#"{"cmd":"view","yaw":"0.5"}"#)
            .expect_err("string")
            .contains("must be a number"));
        assert!(parse_request(r#"{"cmd":"target","point":[null,0,0]}"#)
            .expect_err("null")
            .contains("must be numbers"));
    }

    /// `expect_err` needs `Debug` on the success type, and a bound listener is
    /// not `Debug`; this keeps the failing case explicit instead.
    fn refused(result: std::io::Result<ControlServer>, message: &str) -> std::io::Error {
        match result {
            Ok(_) => panic!("{message}"),
            Err(error) => error,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viewer-control-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn existing_directory_mode_and_files_are_preserved() {
        let dir = temp_dir("preserve");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("mode");
        let path = dir.join("viewer.sock");

        // A regular file at the socket path is never replaced.
        std::fs::write(&path, b"keep me").expect("write file");
        let error = refused(
            start_with_idle(&path, Duration::from_millis(300)),
            "a regular file is refused",
        );
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}");
        assert_eq!(std::fs::read(&path).expect("file survives"), b"keep me");

        // Neither is a symlink, whose target must stay untouched.
        let target = dir.join("real-file.txt");
        std::fs::write(&target, b"target").expect("write target");
        std::fs::remove_file(&path).expect("clear path");
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        let error = refused(
            start_with_idle(&path, Duration::from_millis(300)),
            "a symlink is refused",
        );
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error}");
        assert_eq!(std::fs::read(&target).expect("target survives"), b"target");
        assert!(
            std::fs::symlink_metadata(&path)
                .expect("symlink survives")
                .file_type()
                .is_symlink(),
            "the symlink is still a symlink"
        );

        // The existing directory keeps its mode; only a file this call creates
        // is owner-only.
        std::fs::remove_file(&path).expect("clear path");
        let server = start_with_idle(&path, Duration::from_millis(300)).expect("bind");
        let dir_mode = std::fs::metadata(&dir).expect("dir").permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o755, "existing directory mode preserved");
        let socket_mode = std::fs::metadata(&path)
            .expect("socket")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(socket_mode, 0o600, "socket owner-only");
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_directory_is_owner_only() {
        let dir = temp_dir("fresh");
        std::fs::remove_dir_all(&dir).expect("start empty");
        let path = dir.join("nested/viewer.sock");
        let server = start_with_idle(&path, Duration::from_millis(300)).expect("bind");
        let mode = std::fs::metadata(dir.join("nested"))
            .expect("created dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "created directory is owner-only");
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_socket_is_refused_and_a_stale_one_reclaimed() {
        use std::os::unix::net::UnixStream;

        let dir = temp_dir("lifecycle");
        let path = dir.join("viewer.sock");
        let server = start_with_idle(&path, Duration::from_millis(300)).expect("first bind");
        let error = refused(
            start_with_idle(&path, Duration::from_millis(300)),
            "a live socket is refused",
        );
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error}");
        assert!(
            error.to_string().contains("another viewer is listening"),
            "{error}"
        );
        // The first socket still works after the refused bind.
        let mut stream = UnixStream::connect(&path).expect("first socket alive");
        stream.write_all(b"{oops\n").expect("write");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("read");
        assert!(line.contains("malformed JSON:"), "{line}");
        drop(server);

        // A listener dropped without cleanup leaves an inode nobody serves:
        // the only case where replacing it loses nothing.
        std::fs::remove_file(&path).expect("clear");
        {
            let listener = std::os::unix::net::UnixListener::bind(&path).expect("bare bind");
            assert!(listener.local_addr().is_ok());
        }
        assert!(path.exists(), "stale inode remains");
        let recovered = start_with_idle(&path, Duration::from_millis(300)).expect("reclaim stale");
        assert_eq!(recovered.path, path);
        drop(recovered);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_idle_connection_releases_the_thread() {
        use std::os::unix::net::UnixStream;

        let dir = temp_dir("idle");
        let path = dir.join("viewer.sock");
        let server = start_with_idle(&path, Duration::from_millis(300)).expect("bind");
        // Nobody sends on this one; it owns the accept thread until the idle
        // timeout drops it.
        let idle = UnixStream::connect(&path).expect("idle connect");
        let started = std::time::Instant::now();
        let mut later = UnixStream::connect(&path).expect("second connect");
        later
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        later.write_all(b"{oops\n").expect("write");
        let mut line = String::new();
        BufReader::new(later).read_line(&mut line).expect("read");
        assert!(line.contains("malformed JSON:"), "{line}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "later connection served after the idle one timed out"
        );
        drop(idle);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn socket_answers_a_malformed_line_without_an_app() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixStream;

        let dir =
            std::env::temp_dir().join(format!("viewer-control-socket-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("viewer.sock");
        let server = start(&path).expect("bind socket");
        let mode = std::fs::metadata(&path)
            .expect("socket exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "socket is owner-only");
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream.write_all(b"{oops\n").expect("write");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("read");
        let value: Value = serde_json::from_str(line.trim()).expect("valid JSON");
        assert_eq!(value["ok"], false);
        assert!(
            value["error"]
                .as_str()
                .unwrap()
                .starts_with("malformed JSON:"),
            "{line}"
        );
        // A second connection on the same server still works after a reject.
        let mut stream = UnixStream::connect(&path).expect("connect again");
        stream.write_all(b"not json at all\n").expect("write");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("read");
        assert!(
            line.contains("malformed JSON:"),
            "second connection answered: {line}"
        );
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
