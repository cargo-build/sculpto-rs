//! Split a generated multi-figure GLB into one GLB per figure.
//!
//! The engine owns the geometry rule (`sculpto_mesh::spatial_split`); this
//! tool owns the file hand-off: read the source, write each part, and emit the
//! JSONL manifest that `boethos import-raw-parts` consumes. Embedded textures
//! are not carried; the catalogue needs geometry and provenance, and the
//! source GLB remains the visual reference.

use std::fs;
use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sha2::{Digest, Sha256};

/// One written part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartReport {
    /// Stable figure name (`<stem>-figure-01`).
    pub name: String,
    /// Part token (`figure-01`).
    pub part: String,
    /// Written GLB path.
    pub path: PathBuf,
    /// Written GLB digest.
    pub sha256: String,
    /// Written GLB size.
    pub bytes: u64,
    /// Vertex count.
    pub vertices: u64,
    /// Triangle count.
    pub triangles: u64,
}

/// Result of splitting one source model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitReport {
    /// Source GLB path.
    pub source: PathBuf,
    /// Source GLB digest.
    pub source_sha256: String,
    /// One report per written part, in ascending X order.
    pub parts: Vec<PartReport>,
}

/// Split `input` into one GLB per empty X band under `out_dir`.
///
/// The source is read with the engine's generated-export reader; a mesh
/// without a qualifying gap is refused rather than copied as a false part.
pub fn split_glb(input: &Path, out_dir: &Path) -> Result<SplitReport, String> {
    let raw = sculpto_mesh::intake_gltf::read_raw(input)
        .map_err(|error| format!("{}: {error}", input.display()))?;
    let parts = sculpto_mesh::spatial_split::split_x(
        &raw.positions,
        &raw.normals,
        &raw.uvs,
        &raw.triangles,
    )
    .map_err(|error| format!("{}: {error}", input.display()))?;
    fs::create_dir_all(out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let stem = stem(input);
    let source_sha256 =
        digest(&fs::read(input).map_err(|error| format!("{}: {error}", input.display()))?);
    let mut reports = Vec::with_capacity(parts.len());
    for (index, part) in parts.into_iter().enumerate() {
        let part_token = format!("figure-{:02}", index + 1);
        let name = format!("{stem}-{part_token}");
        let vertices = part.positions.len() as u64;
        let triangles = part.triangles.len() as u64;
        let asset = MeshAsset {
            name: name.clone(),
            positions: part.positions,
            normals: part.normals,
            uvs: part.uvs,
            triangles: part.triangles,
            triangle_partitions: vec![PartitionId(0); triangles as usize],
            materials: vec![raw.material.clone()],
            ..MeshAsset::default()
        };
        let bytes = sculpto_gltf::write_glb(&asset).map_err(|error| format!("{name}: {error}"))?;
        let path = out_dir.join(format!("{name}.glb"));
        fs::write(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        reports.push(PartReport {
            name,
            part: part_token,
            path,
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
            vertices,
            triangles,
        });
    }
    Ok(SplitReport {
        source: input.to_path_buf(),
        source_sha256,
        parts: reports,
    })
}

/// Write the manifest rows consumed by `boethos import-raw-parts`.
pub fn write_manifest(
    report: &SplitReport,
    manifest: &Path,
    source_label: &str,
) -> Result<(), String> {
    let mut text = String::new();
    for part in &report.parts {
        let row = serde_json::json!({
            "name": part.name,
            "part": part.part,
            "category": "mesh",
            "source": source_label,
            "path": part.path,
            "source_path": report.source,
            "source_sha256": report.source_sha256,
            "sha256": part.sha256,
            "bytes": part.bytes,
            "vertices": part.vertices,
            "triangles": part.triangles,
            "tags": ["raw:part", format!("figure:{}", part.part.trim_start_matches("figure-"))],
        });
        text.push_str(&serde_json::to_string(&row).map_err(|error| error.to_string())?);
        text.push('\n');
    }
    fs::write(manifest, text).map_err(|error| format!("{}: {error}", manifest.display()))
}

fn stem(input: &Path) -> String {
    let raw = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("tripo");
    let mut stem = String::new();
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            stem.push(character.to_ascii_lowercase());
        } else if !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem = stem.trim_matches('-');
    if stem.is_empty() {
        "tripo".to_string()
    } else {
        stem.to_string()
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
