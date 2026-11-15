//! Rest-frame rewrite for an existing skinned actor GLB. @req DECL0013
//!
//! `actor fit` derives new rest frames for the extremity chains; this writer
//! puts them into the file without re-encoding any geometry. Only two things
//! change: a joint node's translation/rotation/scale, and the floats behind the
//! skin's inverse bind matrices. Every accessor offset, triangle, material,
//! influence row and animation sample keeps its value, so the rest pose is
//! reproduced exactly by construction (`world * inverse_bind` is unchanged).
//!
//! The framing is written here rather than through [`crate::pack::PackWriter`]
//! because the packer appends payload; this module overwrites in place, and it
//! must refuse an accessor layout it cannot address instead of re-encoding one.

use std::borrow::Cow;

use serde_json::Value;

/// Why a frame rewrite could not be written.
#[derive(Debug)]
pub enum FrameError {
    /// The GLB framing did not parse, or could not be framed again.
    Decode(gltf::Error),
    /// The document does not carry the layout the caller addressed.
    Invalid {
        /// What was being written.
        context: String,
        /// Why it cannot be written.
        reason: String,
    },
}

impl FrameError {
    fn invalid(context: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            context: context.into(),
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => write!(formatter, "invalid GLB: {error}"),
            Self::Invalid { context, reason } => write!(formatter, "{context}: {reason}"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Invalid { .. } => None,
        }
    }
}

/// One joint node's new rest transform, in the actor's own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointFrame {
    pub node: usize,
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: f64,
}

/// One joint's new inverse bind matrix, column-major like glTF's accessor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointBind {
    pub node: usize,
    pub matrix: [[f64; 4]; 4],
}

/// Rewrite joint rest transforms and inverse bind matrices in place.
///
/// # Errors
/// A GLB that does not parse, a joint node that is not addressed by index, a
/// node that carries a static `matrix` instead of TRS, a skin whose
/// `inverseBindMatrices` accessor is not tightly packed `f32 MAT4`, or a bind
/// for a node the skin does not name.
pub fn rewrite_frames(
    glb: &[u8],
    frames: &[JointFrame],
    binds: &[JointBind],
) -> Result<Vec<u8>, FrameError> {
    let framing = gltf::binary::Glb::from_slice(glb).map_err(FrameError::Decode)?;
    let mut json: Value = serde_json::from_slice(&framing.json)
        .map_err(|error| FrameError::invalid("GLB JSON", error.to_string()))?;
    let mut bin = framing
        .bin
        .ok_or_else(|| FrameError::invalid("frame rewrite", "missing BIN chunk"))?
        .into_owned();

    for frame in frames {
        let node = json
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .and_then(|nodes| nodes.get_mut(frame.node))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                FrameError::invalid(
                    format!("node {}", frame.node),
                    "missing node object to reframe",
                )
            })?;
        if node.contains_key("matrix") {
            return Err(FrameError::invalid(
                format!("node {}", frame.node),
                "node carries a static matrix; a TRS rewrite cannot preserve it",
            ));
        }
        node.insert(
            "translation".to_owned(),
            serde_json::json!(frame.translation),
        );
        node.insert("rotation".to_owned(), serde_json::json!(frame.rotation));
        node.insert(
            "scale".to_owned(),
            serde_json::json!([frame.scale, frame.scale, frame.scale]),
        );
    }

    for bind in binds {
        write_inverse_bind(&json, &mut bin, bind)?;
    }

    let buffer = json
        .get_mut("buffers")
        .and_then(Value::as_array_mut)
        .and_then(|buffers| buffers.first_mut())
        .and_then(Value::as_object_mut)
        .ok_or_else(|| FrameError::invalid("frame rewrite", "missing buffer"))?;
    buffer.insert("byteLength".to_owned(), serde_json::json!(bin.len()));
    let json = serde_json::to_vec(&json)
        .map_err(|error| FrameError::invalid("GLB JSON", error.to_string()))?;
    gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(json),
        bin: Some(Cow::Owned(bin)),
    }
    .to_vec()
    .map_err(FrameError::Decode)
}

/// Overwrite one skin's inverse bind matrix for a joint node.
fn write_inverse_bind(
    json: &serde_json::Value,
    bin: &mut [u8],
    bind: &JointBind,
) -> Result<(), FrameError> {
    let skins = json
        .get("skins")
        .and_then(Value::as_array)
        .ok_or_else(|| FrameError::invalid("frame rewrite", "the document has no skins"))?;
    for (index, skin) in skins.iter().enumerate() {
        let joints = skin
            .get("joints")
            .and_then(Value::as_array)
            .ok_or_else(|| FrameError::invalid(format!("skin {index}"), "missing joints"))?;
        let Some(position) = joints
            .iter()
            .position(|joint| joint.as_u64() == Some(bind.node as u64))
        else {
            continue;
        };
        let accessor_index = skin
            .get("inverseBindMatrices")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                FrameError::invalid(
                    format!("skin {index}"),
                    "carries no inverseBindMatrices accessor",
                )
            })? as usize;
        let accessor = json
            .get("accessors")
            .and_then(Value::as_array)
            .and_then(|accessors| accessors.get(accessor_index))
            .ok_or_else(|| {
                FrameError::invalid(
                    format!("accessor {accessor_index}"),
                    "missing inverse bind accessor",
                )
            })?;
        if accessor.get("type").and_then(Value::as_str) != Some("MAT4")
            || accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        {
            return Err(FrameError::invalid(
                format!("accessor {accessor_index}"),
                "inverse bind matrices are not f32 MAT4",
            ));
        }
        let view_index = accessor
            .get("bufferView")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                FrameError::invalid(
                    format!("accessor {accessor_index}"),
                    "inverse bind accessor has no bufferView",
                )
            })? as usize;
        let view = json
            .get("bufferViews")
            .and_then(Value::as_array)
            .and_then(|views| views.get(view_index))
            .ok_or_else(|| {
                FrameError::invalid(format!("bufferView {view_index}"), "missing buffer view")
            })?;
        if view.get("byteStride").is_some() {
            return Err(FrameError::invalid(
                format!("bufferView {view_index}"),
                "interleaved inverse bind matrices are not addressed by this rewrite",
            ));
        }
        let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let accessor_offset = accessor
            .get("byteOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let start = view_offset + accessor_offset + position * 64;
        let end = start + 64;
        if end > bin.len() {
            return Err(FrameError::invalid(
                format!("accessor {accessor_index}"),
                format!(
                    "matrix {position} ends at {end}, past the {} byte buffer",
                    bin.len()
                ),
            ));
        }
        // glTF stores matrices column-major, the same layout the caller passes.
        let mut lane = 0usize;
        for column in bind.matrix {
            for value in column {
                let bytes = (value as f32).to_le_bytes();
                bin[start + lane * 4..start + lane * 4 + 4].copy_from_slice(&bytes);
                lane += 1;
            }
        }
        return Ok(());
    }
    Err(FrameError::invalid(
        format!("node {}", bind.node),
        "no skin names this joint, so it has no inverse bind matrix",
    ))
}
