//! Embedded animation clips inside a `glb`/`gltf` document.
//!
//! A clip is a property of the actor file, not a file on disk. `index` writes
//! one `clip` row per `animations[]` entry so the catalogue can answer which
//! clips an actor carries; `read_clips` is the same measurement without a
//! database, used by `boethos clips --glb PATH`.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Asset, Category, ClipDetails, Status};

const GLB_MAGIC: u32 = 0x4654_6C67;
const GLB_JSON_CHUNK: u32 = 0x4E4F_534A;

/// One measured `animations[]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipInfo {
    /// Clip name, or `clip<index>` when the document is unnamed.
    pub name: String,
    /// Number of animation channels.
    pub channels: u64,
    /// Largest sampler input key count.
    pub keyframes: u64,
    /// Longest sampler input time in seconds; `0.0` when unmeasured.
    pub duration_seconds: f64,
    /// Every targeted node, in node order; names fall back to `node<index>`.
    pub bones: Vec<String>,
    /// Number of distinct skin joints in the document.
    pub palette_joints: u64,
    /// Fraction of skin joints the channels target; `None` without a skin.
    pub palette_coverage: Option<f64>,
}

/// Clips measured in one document plus the palette they share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipDocument {
    /// Distinct skin joints in the document.
    pub palette_joints: u64,
    /// `animations[]` entries in document order.
    pub clips: Vec<ClipInfo>,
}

/// Read the clips embedded in a `.glb` or `.gltf` file.
pub fn read_clips(path: &Path) -> Result<ClipDocument, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    let document = match extension.as_str() {
        "glb" => parse_glb_json(&bytes).map_err(|error| format!("{}: {error}", path.display()))?,
        "gltf" => serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: invalid glTF JSON: {error}", path.display()))?,
        other => {
            return Err(format!(
                "{}: expected a glb or gltf document, got {other:?}",
                path.display()
            ));
        }
    };
    measure_document(&document).map_err(|error| format!("{}: {error}", path.display()))
}

/// Build one clip row from the actor it belongs to.
///
/// Identity includes the clip index, so two clips with the same name in one
/// document stay distinct rows. `source_sha256` keeps the actor digest.
pub fn clip_asset(
    source: &str,
    actor_path: &str,
    actor_sha256: Option<&str>,
    index: usize,
    clip: &ClipInfo,
    status: Status,
) -> Asset {
    Asset {
        id: crate::identity(&["clip", source, actor_path, &index.to_string(), &clip.name]),
        category: Category::Clip,
        name: clip.name.clone(),
        path: format!("{actor_path}::#{}", clip.name),
        source: source.to_string(),
        archive: None,
        entry: None,
        format: "clip".to_string(),
        shape: None,
        vertices: None,
        triangles: None,
        clip: Some(ClipDetails {
            channels: clip.channels,
            keyframes: clip.keyframes,
            duration_seconds: clip.duration_seconds,
            bones: clip.bones.clone(),
            palette_joints: clip.palette_joints,
            palette_coverage: clip.palette_coverage,
        }),
        sha256: None,
        source_sha256: actor_sha256.map(str::to_string),
        bytes: None,
        sex: None,
        family: None,
        tags: Vec::new(),
        status,
    }
}

/// Extract the JSON chunk from a binary glTF container.
fn parse_glb_json(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() < 20 {
        return Err("glb is shorter than its header and JSON chunk".to_string());
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().expect("four bytes"));
    if magic != GLB_MAGIC {
        return Err("glb magic is not `glTF`".to_string());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes"));
    if version != 2 {
        return Err(format!("unsupported glb version {version}"));
    }
    let declared = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes")) as usize;
    if declared < 20 || declared > bytes.len() {
        return Err(format!(
            "glb length {declared} does not fit the {} bytes read",
            bytes.len()
        ));
    }
    let mut offset = 12usize;
    while offset + 8 <= declared {
        let chunk_len =
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes")) as usize;
        let chunk_type = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .expect("four bytes"),
        );
        let start = offset + 8;
        let end = start
            .checked_add(chunk_len)
            .filter(|end| *end <= declared)
            .ok_or_else(|| format!("glb chunk at {offset} overruns the document"))?;
        if chunk_type == GLB_JSON_CHUNK {
            return serde_json::from_slice(&bytes[start..end])
                .map_err(|error| format!("invalid glTF JSON: {error}"));
        }
        offset = end;
    }
    Err("glb has no JSON chunk".to_string())
}

fn measure_document(document: &Value) -> Result<ClipDocument, String> {
    let object = document
        .as_object()
        .ok_or_else(|| "glTF document is not a JSON object".to_string())?;
    let nodes = object.get("nodes").and_then(Value::as_array);
    let accessors = object.get("accessors").and_then(Value::as_array);
    let joints = palette_joints(object);
    let mut clips = Vec::new();
    let animations = object
        .get("animations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for (index, animation) in animations.iter().enumerate() {
        let animation = animation
            .as_object()
            .ok_or_else(|| format!("animations[{index}] is not an object"))?;
        let name = animation
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("clip{index}"));
        let channels = animation
            .get("channels")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut driven: Vec<usize> = Vec::new();
        for channel in channels {
            if let Some(node) = channel
                .get("target")
                .and_then(|target| target.get("node"))
                .and_then(Value::as_u64)
            {
                driven.push(node as usize);
            }
        }
        driven.sort_unstable();
        driven.dedup();
        let bones = driven
            .iter()
            .map(|node| node_name(nodes, *node))
            .collect::<Vec<_>>();
        let covered = driven
            .iter()
            .filter(|node| joints.iter().any(|joint| joint == *node))
            .count();
        let palette_coverage = (!joints.is_empty()).then(|| covered as f64 / joints.len() as f64);
        let (keyframes, duration_seconds) = sampler_span(animation, accessors);
        clips.push(ClipInfo {
            name,
            channels: channels.len() as u64,
            keyframes,
            duration_seconds,
            bones,
            palette_joints: joints.len() as u64,
            palette_coverage,
        });
    }
    Ok(ClipDocument {
        palette_joints: joints.len() as u64,
        clips,
    })
}

fn palette_joints(document: &serde_json::Map<String, Value>) -> Vec<usize> {
    let mut joints: Vec<usize> = document
        .get("skins")
        .and_then(Value::as_array)
        .map(|skins| {
            skins
                .iter()
                .flat_map(|skin| {
                    skin.get("joints")
                        .and_then(Value::as_array)
                        .map(|joints| {
                            joints
                                .iter()
                                .filter_map(Value::as_u64)
                                .map(|joint| joint as usize)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    joints.sort_unstable();
    joints.dedup();
    joints
}

fn node_name(nodes: Option<&Vec<Value>>, index: usize) -> String {
    nodes
        .and_then(|nodes| nodes.get(index))
        .and_then(|node| node.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("node{index}"))
}

fn sampler_span(
    animation: &serde_json::Map<String, Value>,
    accessors: Option<&Vec<Value>>,
) -> (u64, f64) {
    let mut keyframes = 0u64;
    let mut duration = 0.0f64;
    for sampler in animation
        .get("samplers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(input) = sampler.get("input").and_then(Value::as_u64) else {
            continue;
        };
        let Some(accessor) = accessors.and_then(|accessors| accessors.get(input as usize)) else {
            continue;
        };
        keyframes = keyframes.max(accessor.get("count").and_then(Value::as_u64).unwrap_or(0));
        if let Some(max) = accessor
            .get("max")
            .and_then(Value::as_array)
            .and_then(|max| max.first())
            .and_then(Value::as_f64)
            && max.is_finite()
        {
            duration = duration.max(max);
        }
    }
    (keyframes, duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glb(json: &str) -> Vec<u8> {
        let mut chunk = json.as_bytes().to_vec();
        while !chunk.len().is_multiple_of(4) {
            chunk.push(b' ');
        }
        let length = 12 + 8 + chunk.len();
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&(length as u32).to_le_bytes());
        bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&GLB_JSON_CHUNK.to_le_bytes());
        bytes.extend_from_slice(&chunk);
        bytes
    }

    const ACTOR: &str = r#"{
        "nodes": [{"name": "Root"}, {"name": "Hip"}, {"name": "Spine"}],
        "skins": [{"joints": [1, 2]}],
        "accessors": [
            {"count": 3, "max": [1.5]},
            {"count": 3, "max": [1.5]},
            {"count": 2, "max": [0.5]}
        ],
        "animations": [
            {
                "name": "Walk",
                "channels": [
                    {"sampler": 0, "target": {"node": 1, "path": "rotation"}},
                    {"sampler": 1, "target": {"node": 2, "path": "translation"}}
                ],
                "samplers": [{"input": 0, "output": 0}, {"input": 1, "output": 2}]
            },
            {
                "name": "RootOnly",
                "channels": [{"sampler": 0, "target": {"node": 0, "path": "translation"}}],
                "samplers": [{"input": 2, "output": 0}]
            }
        ]
    }"#;

    #[test]
    fn measures_channels_keys_duration_bones_and_coverage() {
        let document =
            measure_document(&serde_json::from_str(ACTOR).expect("json")).expect("measure");
        assert_eq!(document.palette_joints, 2);
        assert_eq!(document.clips.len(), 2);
        let walk = &document.clips[0];
        assert_eq!(walk.name, "Walk");
        assert_eq!(walk.channels, 2);
        assert_eq!(walk.keyframes, 3);
        assert_eq!(walk.duration_seconds, 1.5);
        assert_eq!(walk.bones, ["Hip", "Spine"]);
        assert_eq!(walk.palette_coverage, Some(1.0));
        let root = &document.clips[1];
        assert_eq!(root.channels, 1);
        assert_eq!(root.keyframes, 2);
        assert_eq!(root.duration_seconds, 0.5);
        assert_eq!(root.bones, ["Root"]);
        assert_eq!(root.palette_coverage, Some(0.0));
    }

    #[test]
    fn reads_a_glb_container_and_refuses_a_broken_one() {
        let dir = std::env::temp_dir().join(format!(
            "boethos-clip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("actor.glb");
        std::fs::write(&path, glb(ACTOR)).expect("write");
        let document = read_clips(&path).expect("clips");
        assert_eq!(document.clips.len(), 2);
        std::fs::write(&path, b"not a glb").expect("write");
        assert!(read_clips(&path).is_err());
        std::fs::write(&path, glb(ACTOR)).expect("restore");
        let mut truncated = std::fs::read(&path).expect("read");
        truncated.truncate(truncated.len() - 4);
        std::fs::write(&path, truncated).expect("write");
        assert!(read_clips(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_document_without_animations_has_no_clips() {
        let document =
            measure_document(&serde_json::json!({"nodes": [{"name": "Root"}]})).expect("measure");
        assert!(document.clips.is_empty());
        assert_eq!(document.palette_joints, 0);
    }
}
