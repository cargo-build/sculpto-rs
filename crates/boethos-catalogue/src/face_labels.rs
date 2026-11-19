//! Explicit classification of indexed face assets.

use std::collections::HashSet;

use boethos::{Kind, Put, Store, StoreError, StoreResult};
use serde::Deserialize;

use crate::model::{Asset, Assets, Category, Sex};

/// Hair color group used for catalogue filters.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HairColor {
    /// Black and brown shades.
    Dark,
    /// Blond, grey and white shades.
    Light,
    /// Red or a vivid hue.
    Colored,
    /// Bald character.
    None,
    /// A color the source does not resolve.
    Unknown,
}

impl HairColor {
    const fn token(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Colored => "colored",
            Self::None => "none",
            Self::Unknown => "unknown",
        }
    }
}

/// Labels for one existing face or RaceMenu preset record.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceClassification {
    /// Full catalogue ID.
    pub id: String,
    /// Declared sex, also used by the typed catalogue filter.
    pub sex: Sex,
    /// English race token.
    pub race: String,
    /// Color group rather than an individual shade.
    pub hair_color: HairColor,
    /// English style token.
    pub hairstyle: String,
    /// Provenance token for the authored hair.
    pub hair_source: String,
    /// Individual hairdo identifier when resolved.
    pub hairdo: Option<String>,
}

/// Validate all targets before applying any labels. Each row is retryable.
pub fn classify_faces(store: &Store, labels: &[FaceClassification]) -> StoreResult<usize> {
    let mut ids = HashSet::with_capacity(labels.len());
    for row in labels {
        if row.id.len() != 64
            || !row
                .id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(invalid_label(format!("{} must be a full asset ID", row.id)));
        }
        if !ids.insert(row.id.as_str()) {
            return Err(invalid_label(format!("duplicate face ID {}", row.id)));
        }
        for (field, token) in [
            ("race", Some(row.race.as_str())),
            ("hairstyle", Some(row.hairstyle.as_str())),
            ("hair_source", Some(row.hair_source.as_str())),
            ("hairdo", row.hairdo.as_deref()),
        ] {
            if let Some(token) = token
                && !token.split('-').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                })
            {
                return Err(invalid_label(format!(
                    "{} has an invalid {field} token {token:?}",
                    row.id
                )));
            }
        }
        match store.get::<Assets>(&row.id)? {
            Some(asset) if is_face(asset.category) => {}
            Some(asset) => {
                return Err(invalid_label(format!(
                    "{} has category {}, expected face or face_preset",
                    row.id, asset.category
                )));
            }
            None => return Err(invalid_label(format!("asset {} is missing", row.id))),
        }
    }
    labels.iter().try_fold(0, |updated, row| {
        let put = store.update::<Assets>(&row.id, |previous| {
            let mut asset = previous.filter(|asset| is_face(asset.category))?;
            asset.tags.retain(|tag| !is_face_label(tag));
            asset.sex = Some(row.sex);
            asset.tags.extend([
                format!("race:{}", row.race),
                format!("sex:{}", row.sex.as_str()),
                format!("hair-color:{}", row.hair_color.token()),
                format!("hairstyle:{}", row.hairstyle),
                format!("hair-source:{}", row.hair_source),
            ]);
            asset
                .tags
                .extend(row.hairdo.iter().map(|name| format!("hairdo:{name}")));
            Some(asset)
        })?;
        match put {
            Some(Put::Exists) => Ok(updated),
            Some(Put::Replaced) => Ok(updated + 1),
            _ => Err(invalid_label(format!(
                "face {} disappeared during import",
                row.id
            ))),
        }
    })
}

pub(crate) fn retain_face_labels(previous: &Asset, refreshed: &mut Asset) {
    if is_face(previous.category)
        && is_face(refreshed.category)
        && previous.tags.iter().any(|tag| tag.starts_with("race:"))
    {
        refreshed.sex = previous.sex;
        refreshed.tags.retain(|tag| !is_face_label(tag));
        refreshed.tags.extend(
            previous
                .tags
                .iter()
                .filter(|tag| is_face_label(tag))
                .cloned(),
        );
    }
}

fn is_face(category: Category) -> bool {
    matches!(category, Category::Face | Category::FacePreset)
}

fn is_face_label(tag: &str) -> bool {
    matches!(
        tag.split_once(':').map(|(key, _)| key),
        Some("race" | "sex" | "hair-color" | "hairstyle" | "hair-source" | "hairdo")
    )
}

fn invalid_label(detail: String) -> StoreError {
    StoreError::Validation {
        kind: Assets::NAME,
        detail,
    }
}
