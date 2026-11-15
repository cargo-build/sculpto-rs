//! Body-part manifest rows for shapes inside body NIFs.

use std::path::Path;

use serde::Deserialize;

use crate::model::{Asset, Category, Sex, Status};

/// One measured shape ready to enter the catalogue.
#[derive(Debug, Clone, Deserialize)]
pub struct BodyPartInput {
    /// Human-readable part name (`Feet`, `Hands`, `BaseShape`, …).
    pub name: String,
    /// Shape name inside the NIF.
    pub shape: String,
    /// Target sex when known.
    #[serde(default)]
    pub sex: Option<Sex>,
    /// Body family (`bijin`, `cbbe`, `3ba`, `cbbe-3ba`, `himbo`).
    #[serde(default)]
    pub family: Option<String>,
    /// Index label or provenance label.
    pub source: String,
    /// Host path for a loose source.
    #[serde(default)]
    pub path: Option<String>,
    /// Archive containing the member, when any.
    #[serde(default)]
    pub archive: Option<String>,
    /// Member path inside the archive.
    #[serde(default)]
    pub entry: Option<String>,
    /// NIF or member digest.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Source size when known.
    #[serde(default)]
    pub bytes: Option<u64>,
    /// Extra tags, merged with `body-part` and the part tag.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Measured vertices.
    pub vertices: u64,
    /// Measured triangles.
    pub triangles: u64,
}

/// Build one `body-part` asset from measured manifest data.
///
/// The identity is derived from source, source path/entry and shape name, so a
/// re-import updates the same row and two shapes in one NIF stay distinct.
pub fn body_part_asset(input: &BodyPartInput) -> Result<Asset, String> {
    let (path, archive, entry) = match (&input.path, &input.archive, &input.entry) {
        (Some(path), None, None) => (path.clone(), None, None),
        (None, Some(archive), Some(entry)) => (
            format!("{archive}::{entry}"),
            Some(archive.clone()),
            Some(entry.clone()),
        ),
        _ => {
            return Err(format!(
                "{}: body part needs exactly one of path or archive+entry",
                input.name
            ));
        }
    };
    let format = Path::new(&path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut tags = input.tags.clone();
    tags.push("body-part".to_string());
    if let Some(part) = part_tag(&input.name, &input.shape) {
        tags.push(part.to_string());
    }
    if let Some(origin) = crate::origin_tag(&input.source, "") {
        tags.push(origin.to_string());
    }
    tags.sort();
    tags.dedup();
    Ok(Asset {
        id: crate::identity(&["body-part", &input.source, &path, &input.shape]),
        category: Category::BodyPart,
        name: input.name.clone(),
        path: path.clone(),
        source: input.source.clone(),
        archive,
        entry,
        format,
        shape: Some(input.shape.clone()),
        vertices: Some(input.vertices),
        triangles: Some(input.triangles),
        clip: None,
        sha256: input.sha256.clone(),
        source_sha256: None,
        bytes: input.bytes,
        sex: input.sex,
        family: input.family.clone(),
        tags,
        status: Status::Raw,
    })
}

fn part_tag(name: &str, shape: &str) -> Option<&'static str> {
    let text = format!("{name} {shape}").to_lowercase();
    if text.contains("feet") || text.contains("foot") {
        Some("part:feet")
    } else if text.contains("hands") || text.contains("hand") {
        Some("part:hands")
    } else if text.contains("head") {
        Some("part:head")
    } else if ["boxer", "underwear", "panty", "bra"]
        .iter()
        .any(|token| text.contains(token))
    {
        Some("part:underwear")
    } else if text.contains("body")
        || text.contains("baseshape")
        || text.contains("3ba")
        || text.contains("cbbe")
    {
        Some("part:body")
    } else {
        None
    }
}
