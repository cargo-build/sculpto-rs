//! Raw-model part rows written by a splitter manifest.

use std::path::Path;

use serde::Deserialize;

use crate::model::{Asset, Category, Sex, Status};

/// One split part ready to enter the catalogue.
#[derive(Debug, Clone, Deserialize)]
pub struct RawPartInput {
    /// Human-readable part name.
    pub name: String,
    /// Stable part token (`figure-01`).
    pub part: String,
    /// `mesh` or `outfit`; defaults to `mesh`.
    #[serde(default)]
    pub category: Option<String>,
    /// Target sex when known.
    #[serde(default)]
    pub sex: Option<Sex>,
    /// Body family when known.
    #[serde(default)]
    pub family: Option<String>,
    /// Index label or provenance label.
    pub source: String,
    /// Written part file.
    pub path: String,
    /// Original merged model.
    #[serde(default)]
    pub source_path: Option<String>,
    /// Digest of the original merged model.
    #[serde(default)]
    pub source_sha256: Option<String>,
    /// Digest of the part file.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Part file size.
    #[serde(default)]
    pub bytes: Option<u64>,
    /// Extra tags, merged with `raw:part` and `origin:*`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Measured vertices.
    pub vertices: u64,
    /// Measured triangles.
    pub triangles: u64,
}

/// Build one raw-part asset from splitter output.
///
/// Identity is `sha256("raw-part", source, path, part)`, so a re-split of the
/// same model updates the same part rows and the source model stays separate.
pub fn raw_part_asset(input: &RawPartInput) -> Result<Asset, String> {
    if input.path.trim().is_empty() {
        return Err(format!("{}: raw part needs a path", input.name));
    }
    let category = match input.category.as_deref().unwrap_or("mesh") {
        "mesh" => Category::Mesh,
        "outfit" => Category::Outfit,
        other => {
            return Err(format!(
                "{}: raw part category must be mesh or outfit, got {other}",
                input.name
            ));
        }
    };
    let path = Path::new(&input.path);
    let format = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut tags = input.tags.clone();
    tags.push("raw:part".to_string());
    if let Some(origin) = crate::origin_tag(&input.source, &input.path) {
        tags.push(origin.to_string());
    }
    tags.sort();
    tags.dedup();
    Ok(Asset {
        id: crate::identity(&["raw-part", &input.source, &input.path, &input.part]),
        category,
        name: input.name.clone(),
        path: input.path.clone(),
        source: input.source.clone(),
        archive: None,
        entry: None,
        format,
        shape: Some(input.part.clone()),
        vertices: Some(input.vertices),
        triangles: Some(input.triangles),
        clip: None,
        sha256: input.sha256.clone(),
        source_sha256: input.source_sha256.clone(),
        bytes: input.bytes,
        sex: input.sex,
        family: input.family.clone(),
        tags,
        status: Status::Raw,
    })
}
