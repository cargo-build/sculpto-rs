//! Append baked geometry without re-encoding the actor's original payload. @req DECL0013
//!
//! Admission stays here: the actor document decides which node, clip,
//! primitive and accessor the pack may touch. Every glTF write goes through
//! [`sculpto_gltf::pack`], so no JSON surgery lives in this crate.

mod validate;

use serde_json::Value;

use sculpto_gltf::pack::{AccessorCopy, PackError, PackWriter};

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

/// Translate one adapter failure into the actor error the packer reports.
fn packed(error: PackError) -> ActorError {
    match error {
        PackError::Decode(error) => ActorError::Decode(error),
        PackError::Invalid { context, reason } => ActorError::Invalid { context, reason },
        PackError::Unsupported { context, feature } => ActorError::Unsupported { context, feature },
    }
}

/// Validate correspondence before replacing one cloth bank and clip channel. @req DECL0013
pub fn pack_cloth(
    base: &[u8],
    targets: &[u8],
    request: ClothPacking<'_>,
) -> Result<PackedCloth, ActorError> {
    let (base_bytes, targets_bytes) = (base, targets);
    let base = Source::read(base_bytes)?;
    validate::images(&base)?;
    let targets = Source::read(targets_bytes)?;
    let times = validate::schedule(request.time_seconds)?;
    let selected = geometry::select(&base.actor.document, &[request.node])?;
    let node = selected[0];
    let mesh = validate::frames(&base, &targets, node)?;
    let (clip, channel) = validate::animation(&base, node, request.clip, &times)?;
    let primitive_count = validate::primitives(&base, &targets, mesh, times.len())?;
    let names = target_names(&targets, times.len())?;
    let mut writer = PackWriter::from_glb(base_bytes).map_err(packed)?;
    (0..primitive_count).try_for_each(|primitive| {
        let source = &targets.json["meshes"][0]["primitives"][primitive];
        ["POSITION", "NORMAL", "TANGENT"]
            .into_iter()
            .try_for_each(|name| {
                if let Some(index) = source["attributes"].get(name) {
                    let copied = copy_accessor(
                        &mut writer,
                        &targets,
                        index_of(index, "geometry accessor")?,
                    )?;
                    writer
                        .set_attribute(mesh, primitive, name, copied)
                        .map_err(packed)?;
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
                            copy_accessor(
                                &mut writer,
                                &targets,
                                index_of(index, "morph accessor")?,
                            )?,
                        ))
                    })
                    .collect::<Result<Vec<_>, ActorError>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        writer
            .set_targets(mesh, primitive, &morphs)
            .map_err(packed)?;
        Ok::<_, ActorError>(())
    })?;
    writer.set_mesh_weights(mesh, times.len()).map_err(packed)?;
    writer.set_node_weights(node, times.len()).map_err(packed)?;
    writer
        .set_target_names(mesh, names.as_deref())
        .map_err(packed)?;
    writer
        .set_weights_channel(clip, channel, node, &times)
        .map_err(packed)?;
    Ok(PackedCloth {
        glb: writer.finish().map_err(packed)?,
        node,
        mesh,
        clip,
        primitives: primitive_count,
        time_seconds: times,
    })
}

/// Copy one already-validated target accessor through the adapter.
fn copy_accessor(
    writer: &mut PackWriter,
    targets: &Source,
    index: usize,
) -> Result<usize, ActorError> {
    let accessor = targets.accessor(index)?;
    let view = accessor
        .view()
        .ok_or_else(|| unsupported("target accessor", "missing view"))?;
    let end = view
        .offset()
        .checked_add(view.length())
        .ok_or_else(|| invalid("target view", "range overflow"))?;
    let bytes = targets
        .actor
        .blob
        .get(view.offset()..end)
        .ok_or_else(|| invalid("target view", "range exceeds BIN"))?;
    let copy = AccessorCopy {
        accessor: index,
        view: view.index(),
        description: &targets.json["accessors"][index],
        view_description: &targets.json["bufferViews"][view.index()],
        bytes,
    };
    writer.copy_accessor(copy).map_err(packed)
}

/// Morph target names the target mesh declares, one per scheduled sample.
fn target_names(targets: &Source, count: usize) -> Result<Option<Vec<String>>, ActorError> {
    let Some(names) = targets.json["meshes"][0]
        .get("extras")
        .and_then(|extras| extras.get("targetNames"))
    else {
        return Ok(None);
    };
    let names = names
        .as_array()
        .filter(|names| names.len() == count)
        .and_then(|names| {
            names
                .iter()
                .map(|name| name.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| {
            invalid(
                "target mesh targetNames",
                "expected one string per morph target",
            )
        })?;
    Ok(Some(names))
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
