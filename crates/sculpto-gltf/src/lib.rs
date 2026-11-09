//! glTF adapter: write a Sculpto [`MeshAsset`](sculpto_core::mesh::MeshAsset)
//! as a single-buffer GLB.
//!
//! Geometry and skin only for now: materials, morph targets and the physics
//! sidecar are later slices (E03, E04, E06) and are not emitted, so an asset
//! carrying them exports without them.
//!
//! Layout contract of [`write_glb`]:
//! - One mesh with one primitive per triangle partition, primitives in
//!   ascending partition-id order. The model is one flat vertex arena, so all
//!   primitives share the same vertex accessors and each keeps only its own
//!   index accessor; the material slice maps slots per primitive.
//! - POSITION (with `min`/`max`), plus NORMAL, TEXCOORD_0 and TANGENT when the
//!   model carries them. Indices are `u32`. Node and mesh names come from
//!   `MeshAsset::name` and joint nodes from the bone names.
//! - One skin on the mesh node: joints in model bone order (one identity node
//!   per bone; the skin's `skeleton` root is the first joint), inverse bind
//!   matrices from [`sculpto_core::mesh::Skin::bind_inverse`]. The model
//!   stores bind matrices column-major, which is already the glTF `MAT4`
//!   layout, so they are flattened without transposing; an empty array means
//!   the identity per bone.
//! - `JOINTS_0` is `u16` and `WEIGHTS_0` is `f32`: `f32` keeps the model's
//!   weights bit-exact instead of quantizing them into normalized `u16`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use sculpto_core::mesh::{MeshAsset, MeshError, PartitionId};
use serde_json::{Map, Value, json};

/// `f32` accessor component type.
const FLOAT: u32 = 5126;
/// `u16` accessor component type.
const UNSIGNED_SHORT: u32 = 5123;
/// `u32` accessor component type.
const UNSIGNED_INT: u32 = 5125;
/// `TRIANGLES` primitive mode.
const TRIANGLES: u32 = 4;
/// Column-major identity, the bind fallback for a skin without matrices.
const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Why a model cannot be exported as GLB.
#[derive(Debug, Clone, PartialEq)]
pub enum GltfError {
    /// `MeshAsset::validate()` refused the model; nothing was written.
    InvalidMesh(MeshError),
    /// The model has no triangles, so there is no primitive to write.
    NoTriangles,
    /// An export size exceeds the glTF `u32` field it must fit in.
    TooLarge {
        /// The field that overflowed.
        field: &'static str,
        /// The size that does not fit.
        size: usize,
    },
    /// The JSON chunk failed to serialize.
    Serialize(String),
    /// The GLB container failed to frame the chunks.
    Encode(String),
}

impl fmt::Display for GltfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMesh(error) => write!(f, "mesh refused: {error:?}"),
            Self::NoTriangles => write!(f, "mesh has no triangles to export"),
            Self::TooLarge { field, size } => {
                write!(f, "{field} ({size}) exceeds the glTF u32 limit")
            }
            Self::Serialize(detail) => write!(f, "GLB JSON failed: {detail}"),
            Self::Encode(detail) => write!(f, "GLB framing failed: {detail}"),
        }
    }
}

impl std::error::Error for GltfError {}

/// Write one model as a GLB with an embedded single buffer.
///
/// The model is validated first, so a refused asset returns a typed
/// [`GltfError`] and never bytes.
pub fn write_glb(asset: &MeshAsset) -> Result<Vec<u8>, GltfError> {
    asset.validate().map_err(GltfError::InvalidMesh)?;
    if asset.triangles.is_empty() {
        return Err(GltfError::NoTriangles);
    }
    // Accessor counts are glTF u32 fields; the writer refuses what they cannot hold.
    u32::try_from(asset.positions.len()).map_err(|_| GltfError::TooLarge {
        field: "vertex count",
        size: asset.positions.len(),
    })?;

    let mut writer = Writer::default();
    let mut attributes = Map::new();

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in &asset.positions {
        for (axis, value) in position.iter().enumerate() {
            min[axis] = min[axis].min(*value);
            max[axis] = max[axis].max(*value);
        }
    }
    let position_view = writer.view(&bytes_f32(
        asset.positions.iter().flat_map(|row| row.iter().copied()),
    ))?;
    writer.accessors.push(json!({
        "bufferView": position_view,
        "componentType": FLOAT,
        "type": "VEC3",
        "count": asset.positions.len(),
        "min": min,
        "max": max,
    }));
    attributes.insert("POSITION".into(), json!(writer.accessors.len() - 1));

    if !asset.normals.is_empty() {
        let view = writer.view(&bytes_f32(
            asset.normals.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "NORMAL".into(),
            json!(writer.accessor(view, FLOAT, "VEC3", asset.normals.len())),
        );
    }
    if !asset.uvs.is_empty() {
        let view = writer.view(&bytes_f32(
            asset.uvs.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "TEXCOORD_0".into(),
            json!(writer.accessor(view, FLOAT, "VEC2", asset.uvs.len())),
        );
    }
    if !asset.tangents.is_empty() {
        let view = writer.view(&bytes_f32(
            asset.tangents.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "TANGENT".into(),
            json!(writer.accessor(view, FLOAT, "VEC4", asset.tangents.len())),
        );
    }

    let mut skin_json = None;
    if let Some(skin) = &asset.skin {
        let joints_view = writer.view(&bytes_u16(
            skin.indices.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "JOINTS_0".into(),
            json!(writer.accessor(joints_view, UNSIGNED_SHORT, "VEC4", skin.indices.len())),
        );
        let weights_view = writer.view(&bytes_f32(
            skin.weights.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "WEIGHTS_0".into(),
            json!(writer.accessor(weights_view, FLOAT, "VEC4", skin.weights.len())),
        );

        let matrices =
            (0..skin.bones.len()).map(|bone| skin.bind_inverse.get(bone).unwrap_or(&IDENTITY));
        let ibm_view =
            writer.view(&bytes_f32(matrices.flat_map(|matrix| {
                matrix.iter().flat_map(|column| column.iter().copied())
            })))?;
        let ibm = writer.accessor(ibm_view, FLOAT, "MAT4", skin.bones.len());
        let joint_nodes: Vec<usize> = (1..=skin.bones.len()).collect();
        skin_json = Some(json!({
            "inverseBindMatrices": ibm,
            "joints": joint_nodes,
            "skeleton": joint_nodes[0],
        }));
    }

    let mut by_partition: BTreeMap<PartitionId, Vec<usize>> = BTreeMap::new();
    for (index, partition) in asset.triangle_partitions.iter().enumerate() {
        by_partition.entry(*partition).or_default().push(index);
    }
    let primitives: Vec<Value> = by_partition
        .values()
        .map(|triangle_indices| {
            let indices: Vec<u32> = triangle_indices
                .iter()
                .flat_map(|&triangle| asset.triangles[triangle])
                .collect();
            let view = writer.view(&bytes_u32(indices.iter().copied()))?;
            Ok(json!({
                "attributes": attributes.clone(),
                "indices": writer.accessor(view, UNSIGNED_INT, "SCALAR", indices.len()),
                "mode": TRIANGLES,
            }))
        })
        .collect::<Result<Vec<_>, GltfError>>()?;

    let mut mesh = Map::new();
    if !asset.name.is_empty() {
        mesh.insert("name".into(), json!(asset.name));
    }
    mesh.insert("primitives".into(), json!(primitives));

    let mut mesh_node = Map::new();
    if !asset.name.is_empty() {
        mesh_node.insert("name".into(), json!(asset.name));
    }
    mesh_node.insert("mesh".into(), json!(0));
    if skin_json.is_some() {
        mesh_node.insert("skin".into(), json!(0));
    }
    let mut nodes = vec![Value::Object(mesh_node)];
    let mut scene_nodes = vec![0_usize];
    if let Some(skin) = &asset.skin {
        for bone in &skin.bones {
            let mut node = Map::new();
            if !bone.is_empty() {
                node.insert("name".into(), json!(bone));
            }
            scene_nodes.push(nodes.len());
            nodes.push(Value::Object(node));
        }
    }

    let mut root = Map::new();
    root.insert(
        "asset".into(),
        json!({"version": "2.0", "generator": "sculpto-gltf"}),
    );
    root.insert("scene".into(), json!(0));
    root.insert("scenes".into(), json!([{"nodes": scene_nodes}]));
    root.insert("nodes".into(), json!(nodes));
    root.insert("meshes".into(), json!([Value::Object(mesh)]));
    if let Some(skin) = skin_json {
        root.insert("skins".into(), json!([skin]));
    }
    root.insert("accessors".into(), json!(writer.accessors));
    root.insert("bufferViews".into(), json!(writer.views));
    root.insert("buffers".into(), json!([{"byteLength": writer.bin.len()}]));

    let json = serde_json::to_vec(&Value::Object(root))
        .map_err(|error| GltfError::Serialize(error.to_string()))?;
    let json_padded = json.len() + (4 - json.len() % 4) % 4;
    let bin_padded = writer.bin.len() + (4 - writer.bin.len() % 4) % 4;
    let total = 12 + 8 + json_padded + 8 + bin_padded;
    if u32::try_from(total).is_err() {
        return Err(GltfError::TooLarge {
            field: "GLB total length",
            size: total,
        });
    }
    gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(json),
        bin: Some(Cow::Owned(writer.bin)),
    }
    .to_vec()
    .map_err(|error| GltfError::Encode(error.to_string()))
}

/// The accumulating GLB: a growing binary buffer with its views and accessors.
#[derive(Default)]
struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Writer {
    /// Append one tightly packed view, four-aligned, returning its index.
    fn view(&mut self, data: &[u8]) -> Result<usize, GltfError> {
        let padding = (4 - self.bin.len() % 4) % 4;
        let offset = self.bin.len() + padding;
        let end = offset + data.len();
        u32::try_from(end).map_err(|_| GltfError::TooLarge {
            field: "binary buffer",
            size: end,
        })?;
        self.bin.resize(offset, 0);
        self.bin.extend_from_slice(data);
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len(),
        }));
        Ok(self.views.len() - 1)
    }

    /// Wrap one view in an accessor, returning its index.
    fn accessor(
        &mut self,
        view: usize,
        component_type: u32,
        gltf_type: &str,
        count: usize,
    ) -> usize {
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": component_type,
            "type": gltf_type,
            "count": count,
        }));
        self.accessors.len() - 1
    }
}

/// Little-endian bytes of a flat `f32` payload.
fn bytes_f32(values: impl Iterator<Item = f32>) -> Vec<u8> {
    values.flat_map(f32::to_le_bytes).collect()
}

/// Little-endian bytes of a flat `u16` payload.
fn bytes_u16(values: impl Iterator<Item = u16>) -> Vec<u8> {
    values.flat_map(u16::to_le_bytes).collect()
}

/// Little-endian bytes of a flat `u32` payload.
fn bytes_u32(values: impl Iterator<Item = u32>) -> Vec<u8> {
    values.flat_map(u32::to_le_bytes).collect()
}
