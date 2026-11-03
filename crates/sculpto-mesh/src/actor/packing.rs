//! Append baked geometry without re-encoding the actor's original payload. @req DECL0013

mod validate;

use std::borrow::Cow;
use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Actor, ActorError, decode, geometry, invalid, unsupported};

/// Explicit identity and sample schedule; no mesh correspondence is inferred. @req DECL0013
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ClothPacking<'input> {
    pub node: &'input str,
    pub clip: &'input str,
    pub time_seconds: &'input [f64],
}

impl<'input> ClothPacking<'input> {
    pub fn new(node: &'input str, clip: &'input str, time_seconds: &'input [f64]) -> Self {
        Self {
            node,
            clip,
            time_seconds,
        }
    }
}

/// Packed bytes and source identity for adapters' reports. @req DECL0013
#[derive(Debug)]
#[non_exhaustive]
pub struct PackedCloth {
    pub glb: Vec<u8>,
    pub node: usize,
    pub mesh: usize,
    pub clip: usize,
    pub primitives: usize,
    pub time_seconds: Vec<f32>,
}

struct Source {
    actor: Actor,
    json: Value,
}

impl Source {
    fn read(bytes: &[u8]) -> Result<Self, ActorError> {
        let actor = Actor::from_glb(bytes)?;
        let framing = gltf::binary::Glb::from_slice(bytes).map_err(ActorError::Decode)?;
        let json = serde_json::from_slice(&framing.json)
            .map_err(|error| invalid("GLB JSON", error.to_string()))?;
        Ok(Self { actor, json })
    }

    fn accessor(&self, index: usize) -> Result<gltf::Accessor<'_>, ActorError> {
        let accessor = self
            .actor
            .document
            .accessors()
            .nth(index)
            .ok_or_else(|| invalid(format!("accessor {index}"), "missing accessor"))?;
        decode::accessor(
            &accessor,
            accessor.dimensions(),
            &[accessor.data_type()],
            "cloth packing",
        )?;
        match accessor.dimensions() {
            gltf::accessor::Dimensions::Scalar
            | gltf::accessor::Dimensions::Vec2
            | gltf::accessor::Dimensions::Vec3
            | gltf::accessor::Dimensions::Vec4 => {}
            _ => return Err(unsupported("selected accessor", "matrix attribute layout")),
        }
        reject_extensions(&self.json["accessors"][index], "selected accessor")?;
        let view = accessor
            .view()
            .ok_or_else(|| unsupported("cloth packing", "accessor without view"))?;
        match view.offset().is_multiple_of(accessor.data_type().size()) {
            true => {}
            false => return Err(invalid("selected accessor", "misaligned buffer view")),
        }
        reject_extensions(
            &self.json["bufferViews"][view.index()],
            "selected buffer view",
        )?;
        Ok(accessor)
    }

    fn elements(&self, accessor: &gltf::Accessor<'_>) -> Result<Vec<u8>, ActorError> {
        let view = accessor
            .view()
            .ok_or_else(|| unsupported("cloth packing", "accessor without view"))?;
        let width = accessor.size();
        let stride = view.stride().unwrap_or(width);
        let start = view
            .offset()
            .checked_add(accessor.offset())
            .ok_or_else(|| invalid("cloth accessor", "offset overflow"))?;
        let capacity = accessor
            .count()
            .checked_mul(width)
            .ok_or_else(|| invalid("cloth accessor", "byte count overflow"))?;
        let mut bytes = Vec::with_capacity(capacity);
        (0..accessor.count()).try_for_each(|element| {
            let offset = element
                .checked_mul(stride)
                .and_then(|offset| start.checked_add(offset))
                .ok_or_else(|| invalid("cloth accessor", "element offset overflow"))?;
            let end = offset
                .checked_add(width)
                .ok_or_else(|| invalid("cloth accessor", "element end overflow"))?;
            let value = self
                .actor
                .blob
                .get(offset..end)
                .ok_or_else(|| invalid("cloth accessor", "element exceeds BIN"))?;
            bytes.extend_from_slice(value);
            Ok::<_, ActorError>(())
        })?;
        Ok(bytes)
    }
}

/// Validate correspondence before replacing one cloth bank and clip channel. @req DECL0013
pub fn pack_cloth(
    base: &[u8],
    targets: &[u8],
    request: ClothPacking<'_>,
) -> Result<PackedCloth, ActorError> {
    let base = Source::read(base)?;
    validate::images(&base)?;
    let targets = Source::read(targets)?;
    let times = validate::schedule(request.time_seconds)?;
    let selected = geometry::select(&base.actor.document, &[request.node])?;
    let node = selected[0];
    let mesh = validate::frames(&base, &targets, node)?;
    let (clip, channel) = validate::animation(&base, node, request.clip, &times)?;
    validate::primitives(&base, &targets, mesh, times.len())?;
    let primitive_count = base.json["meshes"][mesh]["primitives"]
        .as_array()
        .ok_or_else(|| invalid("selected mesh", "missing primitives"))?
        .len();
    let mut writer = Writer {
        json: base.json,
        bin: base.actor.blob,
        views: HashMap::new(),
        accessors: HashMap::new(),
    };
    (0..primitive_count).try_for_each(|primitive| {
        let source = &targets.json["meshes"][0]["primitives"][primitive];
        ["POSITION", "NORMAL", "TANGENT"]
            .into_iter()
            .try_for_each(|name| {
                if let Some(index) = source["attributes"].get(name) {
                    let copied =
                        writer.copy_accessor(&targets, index_of(index, "geometry accessor")?)?;
                    writer.json["meshes"][mesh]["primitives"][primitive]["attributes"][name] =
                        json!(copied);
                }
                Ok::<_, ActorError>(())
            })?;
        let morphs = source["targets"]
            .as_array()
            .ok_or_else(|| invalid("target primitive", "missing morph targets"))?
            .iter()
            .map(|target| {
                target
                    .as_object()
                    .ok_or_else(|| invalid("morph target", "expected attribute map"))?
                    .iter()
                    .map(|(name, index)| {
                        Ok((
                            name.clone(),
                            json!(
                                writer
                                    .copy_accessor(&targets, index_of(index, "morph accessor")?)?
                            ),
                        ))
                    })
                    .collect::<Result<serde_json::Map<_, _>, ActorError>>()
                    .map(Value::Object)
            })
            .collect::<Result<Vec<_>, _>>()?;
        writer.json["meshes"][mesh]["primitives"][primitive]["targets"] = json!(morphs);
        Ok::<_, ActorError>(())
    })?;
    writer.json["meshes"][mesh]["weights"] = json!(vec![0.0; times.len()]);
    if writer.json["nodes"][node].get("weights").is_some() {
        writer.json["nodes"][node]["weights"] = json!(vec![0.0; times.len()]);
    }
    target_names(
        &mut writer.json["meshes"][mesh],
        &targets.json["meshes"][0],
        times.len(),
    )?;
    writer.weights(clip, channel, node, &times)?;
    Ok(PackedCloth {
        glb: writer.finish()?,
        node,
        mesh,
        clip,
        primitives: primitive_count,
        time_seconds: times,
    })
}

fn reject_extensions(value: &Value, context: &str) -> Result<(), ActorError> {
    match value.get("extensions") {
        None => Ok(()),
        Some(Value::Object(extensions)) if extensions.is_empty() => Ok(()),
        Some(_) => Err(unsupported(context, "selected extension layout")),
    }
}

fn index_of(value: &Value, context: &str) -> Result<usize, ActorError> {
    value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| invalid(context, "expected accessor index"))
}

fn target_names(mesh: &mut Value, source: &Value, count: usize) -> Result<(), ActorError> {
    let names = source
        .get("extras")
        .and_then(|extras| extras.get("targetNames"));
    match names {
        Some(Value::Array(names)) if names.len() == count && names.iter().all(Value::is_string) => {
            match mesh.get("extras") {
                Some(Value::Object(_)) => {}
                Some(_) => {
                    return Err(unsupported(
                        "selected mesh extras",
                        "non-object extras with target names",
                    ));
                }
                None => mesh["extras"] = json!({}),
            }
            mesh["extras"]["targetNames"] = json!(names);
        }
        Some(_) => {
            return Err(invalid(
                "target mesh targetNames",
                "expected one string per morph target",
            ));
        }
        None => {
            if let Some(extras) = mesh.get_mut("extras").and_then(Value::as_object_mut) {
                extras.remove("targetNames");
            }
        }
    }
    Ok(())
}

struct Writer {
    json: Value,
    bin: Vec<u8>,
    views: HashMap<usize, usize>,
    accessors: HashMap<usize, usize>,
}

impl Writer {
    fn append(&mut self, array: &str, value: Value) -> Result<usize, ActorError> {
        if self.json.get(array).is_none() {
            self.json[array] = json!([]);
        }
        let values = self.json[array]
            .as_array_mut()
            .ok_or_else(|| invalid(array, "expected array"))?;
        let index = values.len();
        values.push(value);
        Ok(index)
    }

    fn view(&mut self, data: &[u8], mut description: Value) -> Result<usize, ActorError> {
        let padding = (4 - self.bin.len() % 4) % 4;
        let offset = self
            .bin
            .len()
            .checked_add(padding)
            .ok_or_else(|| invalid("packed BIN", "offset overflow"))?;
        let end = offset
            .checked_add(data.len())
            .ok_or_else(|| invalid("packed BIN", "length overflow"))?;
        u32::try_from(end).map_err(|_| invalid("packed BIN", "exceeds GLB u32 size"))?;
        self.bin.resize(offset, 0);
        self.bin.extend_from_slice(data);
        description["buffer"] = json!(0);
        description["byteOffset"] = json!(offset);
        description["byteLength"] = json!(data.len());
        self.append("bufferViews", description)
    }

    fn copy_accessor(&mut self, source: &Source, index: usize) -> Result<usize, ActorError> {
        if let Some(&copied) = self.accessors.get(&index) {
            return Ok(copied);
        }
        let accessor = source.accessor(index)?;
        let view = accessor
            .view()
            .ok_or_else(|| unsupported("target accessor", "missing view"))?;
        let copied_view = match self.views.get(&view.index()) {
            Some(&copied) => copied,
            None => {
                let end = view
                    .offset()
                    .checked_add(view.length())
                    .ok_or_else(|| invalid("target view", "range overflow"))?;
                let bytes = source
                    .actor
                    .blob
                    .get(view.offset()..end)
                    .ok_or_else(|| invalid("target view", "range exceeds BIN"))?;
                let copied = self.view(bytes, source.json["bufferViews"][view.index()].clone())?;
                self.views.insert(view.index(), copied);
                copied
            }
        };
        let mut description = source.json["accessors"][index].clone();
        description["bufferView"] = json!(copied_view);
        let copied = self.append("accessors", description)?;
        self.accessors.insert(index, copied);
        Ok(copied)
    }

    fn floats(&mut self, values: &[f32], limits: bool) -> Result<usize, ActorError> {
        let bytes: Vec<_> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let view = self.view(&bytes, json!({}))?;
        let mut accessor = json!({"bufferView": view, "componentType": 5126, "type": "SCALAR", "count": values.len()});
        if limits {
            accessor["min"] = json!([values.first()]);
            accessor["max"] = json!([values.last()]);
        }
        self.append("accessors", accessor)
    }

    fn weights(
        &mut self,
        clip: usize,
        channel: Option<usize>,
        node: usize,
        times: &[f32],
    ) -> Result<(), ActorError> {
        let input = self.floats(times, true)?;
        let weights: Vec<_> = (0..times.len())
            .flat_map(|sample| (0..times.len()).map(move |target| f32::from(sample == target)))
            .collect();
        let output = self.floats(&weights, false)?;
        let animation = &mut self.json["animations"][clip];
        match channel {
            Some(channel) => {
                let sampler = index_of(
                    &animation["channels"][channel]["sampler"],
                    "cloth channel sampler",
                )?;
                animation["samplers"][sampler]["input"] = json!(input);
                animation["samplers"][sampler]["output"] = json!(output);
                animation["samplers"][sampler]["interpolation"] = json!("LINEAR");
            }
            None => {
                let samplers = animation["samplers"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("clip", "missing samplers"))?;
                let sampler = samplers.len();
                samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
                animation["channels"]
                    .as_array_mut()
                    .ok_or_else(|| invalid("clip", "missing channels"))?
                    .push(json!({"sampler": sampler, "target": {"node": node, "path": "weights"}}));
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>, ActorError> {
        self.json["buffers"][0]["byteLength"] = json!(self.bin.len());
        let json = serde_json::to_vec(&self.json)
            .map_err(|error| invalid("packed JSON", error.to_string()))?;
        let length = json
            .len()
            .checked_add(self.bin.len())
            .and_then(|length| length.checked_add(34))
            .ok_or_else(|| invalid("packed GLB", "length overflow"))?;
        u32::try_from(length).map_err(|_| invalid("packed GLB", "exceeds GLB u32 size"))?;
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
        .map_err(|error| invalid("packed GLB", error.to_string()))
    }
}
