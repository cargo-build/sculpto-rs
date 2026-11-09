//! Cloth packing: append baked geometry to an existing GLB without
//! re-encoding its untouched payload.
//!
//! [`PackWriter`] is the only writer of the packer's glTF JSON. Admission —
//! which node, clip, primitive and accessor the pack may touch — stays with
//! the caller (`sculpto-mesh`'s actor checks), so every method here assumes the
//! accessors it receives were validated and only reports container-level
//! failures. The mutations it owns: `bufferViews`/`accessors` appends, the
//! primitive attribute and morph-target replacement, the zero default weights,
//! the mesh target names, the weights animation channel, the buffer length and
//! the GLB framing.
//!
//! Untouched JSON keeps its exact number literals: the base is parsed with
//! `serde_json`'s `arbitrary_precision`, so a decimal the packer does not touch
//! is re-serialized with the digits it arrived with.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use serde_json::{Map, Value, json};

/// Why the packer could not append to the base GLB.
#[derive(Debug)]
pub enum PackError {
    /// The GLB framing did not parse.
    Decode(gltf::Error),
    /// The document cannot represent the requested write.
    Invalid {
        /// What was being written.
        context: String,
        /// Why it cannot be written.
        reason: String,
    },
    /// A layout the packer does not support.
    Unsupported {
        /// What was being written.
        context: String,
        /// The layout that has no representation.
        feature: String,
    },
}

impl PackError {
    fn invalid(context: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            context: context.into(),
            reason: reason.into(),
        }
    }

    fn unsupported(context: impl Into<String>, feature: impl Into<String>) -> Self {
        Self::Unsupported {
            context: context.into(),
            feature: feature.into(),
        }
    }
}

impl fmt::Display for PackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(formatter, "invalid GLB: {error}"),
            Self::Invalid { context, reason } => write!(formatter, "{context}: {reason}"),
            Self::Unsupported { context, feature } => {
                write!(formatter, "{context}: unsupported {feature}")
            }
        }
    }
}

impl std::error::Error for PackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

/// One accessor to copy: the source JSON to clone and the view bytes behind it.
///
/// The caller has already read and validated the accessor; this only carries
/// the pieces the append needs.
pub struct AccessorCopy<'input> {
    /// Index in the source document; repeats share one appended accessor.
    pub accessor: usize,
    /// Index in the source document; repeats share one appended buffer view.
    pub view: usize,
    /// Source accessor JSON, copied except for `bufferView`.
    pub description: &'input Value,
    /// Source buffer view JSON, copied except for the byte range.
    pub view_description: &'input Value,
    /// The view's BIN bytes.
    pub bytes: &'input [u8],
}

/// Appends to one embedded GLB: the base JSON and BIN plus its copy caches.
pub struct PackWriter {
    json: Value,
    bin: Vec<u8>,
    views: HashMap<usize, usize>,
    accessors: HashMap<usize, usize>,
}

impl PackWriter {
    /// Parse one embedded GLB and keep its JSON and BIN for appends.
    pub fn from_glb(glb: &[u8]) -> Result<Self, PackError> {
        let framing = gltf::binary::Glb::from_slice(glb).map_err(PackError::Decode)?;
        let json = serde_json::from_slice(&framing.json)
            .map_err(|error| PackError::invalid("GLB JSON", error.to_string()))?;
        let bin = framing
            .bin
            .ok_or_else(|| PackError::invalid("packed GLB", "missing BIN chunk"))?
            .into_owned();
        Ok(Self {
            json,
            bin,
            views: HashMap::new(),
            accessors: HashMap::new(),
        })
    }

    /// Copy one validated accessor and its buffer view, returning the new
    /// accessor index; a repeated source index reuses the first copy.
    pub fn copy_accessor(&mut self, copy: AccessorCopy<'_>) -> Result<usize, PackError> {
        if let Some(&copied) = self.accessors.get(&copy.accessor) {
            return Ok(copied);
        }
        let copied_view = match self.views.get(&copy.view) {
            Some(&copied) => copied,
            None => {
                let copied = self.view(copy.bytes, copy.view_description)?;
                self.views.insert(copy.view, copied);
                copied
            }
        };
        let mut description = copy.description.clone();
        let object = description
            .as_object_mut()
            .ok_or_else(|| PackError::invalid("target accessor", "expected accessor object"))?;
        object.insert("bufferView".into(), json!(copied_view));
        let copied = self.append("accessors", description)?;
        self.accessors.insert(copy.accessor, copied);
        Ok(copied)
    }

    /// Point one primitive attribute at a copied accessor.
    pub fn set_attribute(
        &mut self,
        mesh: usize,
        primitive: usize,
        name: &str,
        accessor: usize,
    ) -> Result<(), PackError> {
        let object = object_of(self.primitive(mesh, primitive)?, "selected primitive")?;
        let attributes = match object.get_mut("attributes") {
            Some(attributes) => attributes,
            None => {
                object.insert("attributes".into(), json!({}));
                object
                    .get_mut("attributes")
                    .ok_or_else(|| PackError::invalid("selected primitive", "missing attributes"))?
            }
        };
        let attributes = attributes.as_object_mut().ok_or_else(|| {
            PackError::invalid("selected primitive", "attributes is not an object")
        })?;
        attributes.insert(name.to_owned(), json!(accessor));
        Ok(())
    }

    /// Replace one primitive's morph targets with copies of the target set.
    pub fn set_targets(
        &mut self,
        mesh: usize,
        primitive: usize,
        targets: &[Vec<(String, usize)>],
    ) -> Result<(), PackError> {
        let rows: Vec<Value> = targets
            .iter()
            .map(|attributes| {
                Value::Object(
                    attributes
                        .iter()
                        .map(|(name, accessor)| (name.clone(), json!(accessor)))
                        .collect(),
                )
            })
            .collect();
        let object = object_of(self.primitive(mesh, primitive)?, "selected primitive")?;
        object.insert("targets".into(), Value::Array(rows));
        Ok(())
    }

    /// Zero the mesh default weights, one per morph target.
    pub fn set_mesh_weights(&mut self, mesh: usize, count: usize) -> Result<(), PackError> {
        let object = object_of(self.mesh(mesh)?, "selected mesh")?;
        object.insert("weights".into(), json!(vec![0.0_f64; count]));
        Ok(())
    }

    /// Zero the node default weights when the node declares any.
    pub fn set_node_weights(&mut self, node: usize, count: usize) -> Result<(), PackError> {
        let node = self
            .json
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .and_then(|nodes| nodes.get_mut(node));
        if let Some(weights) = node.and_then(|node| node.get_mut("weights")) {
            *weights = json!(vec![0.0_f64; count]);
        }
        Ok(())
    }

    /// Write the target names, or remove them when the target mesh has none.
    pub fn set_target_names(
        &mut self,
        mesh: usize,
        names: Option<&[String]>,
    ) -> Result<(), PackError> {
        let mesh = self.mesh(mesh)?;
        match names {
            Some(names) => {
                if mesh.get("extras").is_none() {
                    mesh["extras"] = json!({});
                }
                let extras = mesh
                    .get_mut("extras")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| {
                        PackError::unsupported(
                            "selected mesh extras",
                            "non-object extras with target names",
                        )
                    })?;
                extras.insert("targetNames".into(), json!(names));
            }
            None => {
                if let Some(extras) = mesh.get_mut("extras").and_then(Value::as_object_mut) {
                    extras.remove("targetNames");
                }
            }
        }
        Ok(())
    }

    /// Replace or append the clip channel that drives the morph weights.
    pub fn set_weights_channel(
        &mut self,
        clip: usize,
        channel: Option<usize>,
        node: usize,
        times: &[f32],
    ) -> Result<(), PackError> {
        let input = self.floats(times, true)?;
        let weights: Vec<_> = (0..times.len())
            .flat_map(|sample| (0..times.len()).map(move |target| f32::from(sample == target)))
            .collect();
        let output = self.floats(&weights, false)?;
        let animation = self
            .json
            .get_mut("animations")
            .and_then(Value::as_array_mut)
            .and_then(|animations| animations.get_mut(clip))
            .ok_or_else(|| PackError::invalid("cloth clip", "missing clip"))?;
        let animation = animation
            .as_object_mut()
            .ok_or_else(|| PackError::invalid("cloth clip", "expected animation object"))?;
        match channel {
            Some(channel) => {
                let sampler = animation
                    .get("channels")
                    .and_then(Value::as_array)
                    .and_then(|channels| channels.get(channel))
                    .and_then(|channel| channel.get("sampler"))
                    .and_then(Value::as_u64)
                    .and_then(|sampler| usize::try_from(sampler).ok())
                    .ok_or_else(|| {
                        PackError::invalid("cloth channel sampler", "expected accessor index")
                    })?;
                let sampler = animation
                    .get_mut("samplers")
                    .and_then(Value::as_array_mut)
                    .and_then(|samplers| samplers.get_mut(sampler))
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| {
                        PackError::invalid("cloth sampler", "expected sampler object")
                    })?;
                sampler.insert("input".into(), json!(input));
                sampler.insert("output".into(), json!(output));
                sampler.insert("interpolation".into(), json!("LINEAR"));
            }
            None => {
                let samplers = animation
                    .get_mut("samplers")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| PackError::invalid("clip", "missing samplers"))?;
                let sampler = samplers.len();
                samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
                animation
                    .get_mut("channels")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| PackError::invalid("clip", "missing channels"))?
                    .push(json!({"sampler": sampler, "target": {"node": node, "path": "weights"}}));
            }
        }
        Ok(())
    }

    /// Write the buffer length, frame the JSON and BIN chunks and return the GLB.
    pub fn finish(mut self) -> Result<Vec<u8>, PackError> {
        let buffer = self
            .json
            .get_mut("buffers")
            .and_then(Value::as_array_mut)
            .and_then(|buffers| buffers.first_mut())
            .ok_or_else(|| PackError::invalid("packed GLB", "missing buffer"))?;
        buffer["byteLength"] = json!(self.bin.len());
        let json = serde_json::to_vec(&self.json)
            .map_err(|error| PackError::invalid("packed JSON", error.to_string()))?;
        let length = json
            .len()
            .checked_add(self.bin.len())
            .and_then(|length| length.checked_add(34))
            .ok_or_else(|| PackError::invalid("packed GLB", "length overflow"))?;
        u32::try_from(length)
            .map_err(|_| PackError::invalid("packed GLB", "exceeds GLB u32 size"))?;
        gltf::binary::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: Cow::Owned(json),
            bin: Some(Cow::Owned(self.bin)),
        }
        .to_vec()
        .map_err(|error| PackError::invalid("packed GLB", error.to_string()))
    }

    fn mesh(&mut self, index: usize) -> Result<&mut Value, PackError> {
        self.element("meshes", index)
    }

    fn primitive(&mut self, mesh: usize, index: usize) -> Result<&mut Value, PackError> {
        self.mesh(mesh)?
            .get_mut("primitives")
            .and_then(Value::as_array_mut)
            .and_then(|primitives| primitives.get_mut(index))
            .ok_or_else(|| {
                PackError::invalid(format!("packed primitive {index}"), "missing primitive")
            })
    }

    fn element(&mut self, array: &str, index: usize) -> Result<&mut Value, PackError> {
        self.json
            .get_mut(array)
            .and_then(Value::as_array_mut)
            .and_then(|values| values.get_mut(index))
            .ok_or_else(|| PackError::invalid(format!("packed {array}"), "missing entry"))
    }

    /// Append one four-aligned buffer view and return its index.
    fn view(&mut self, data: &[u8], description: &Value) -> Result<usize, PackError> {
        let padding = (4 - self.bin.len() % 4) % 4;
        let offset = self
            .bin
            .len()
            .checked_add(padding)
            .ok_or_else(|| PackError::invalid("packed BIN", "offset overflow"))?;
        let end = offset
            .checked_add(data.len())
            .ok_or_else(|| PackError::invalid("packed BIN", "length overflow"))?;
        u32::try_from(end).map_err(|_| PackError::invalid("packed BIN", "exceeds GLB u32 size"))?;
        self.bin.resize(offset, 0);
        self.bin.extend_from_slice(data);
        let mut description = description.clone();
        let object = description
            .as_object_mut()
            .ok_or_else(|| PackError::invalid("target view", "expected buffer view object"))?;
        object.insert("buffer".into(), json!(0));
        object.insert("byteOffset".into(), json!(offset));
        object.insert("byteLength".into(), json!(data.len()));
        self.append("bufferViews", description)
    }

    /// Append one accessor whose values were just written as floats.
    fn floats(&mut self, values: &[f32], limits: bool) -> Result<usize, PackError> {
        let bytes: Vec<_> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let view = self.view(&bytes, &json!({}))?;
        let mut accessor = json!({"bufferView": view, "componentType": 5126, "type": "SCALAR", "count": values.len()});
        if limits {
            accessor["min"] = json!([values.first()]);
            accessor["max"] = json!([values.last()]);
        }
        self.append("accessors", accessor)
    }

    fn append(&mut self, array: &str, value: Value) -> Result<usize, PackError> {
        let root = self
            .json
            .as_object_mut()
            .ok_or_else(|| PackError::invalid("packed JSON", "expected object"))?;
        let slot = root.entry(array.to_owned()).or_insert_with(|| json!([]));
        let values = slot
            .as_array_mut()
            .ok_or_else(|| PackError::invalid(array, "expected array"))?;
        let index = values.len();
        values.push(value);
        Ok(index)
    }
}

/// The mutable object view of one document value.
fn object_of<'value>(
    value: &'value mut Value,
    context: &str,
) -> Result<&'value mut Map<String, Value>, PackError> {
    value
        .as_object_mut()
        .ok_or_else(|| PackError::invalid(context, "expected object"))
}
