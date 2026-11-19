//! Indexed asset record.

use std::fmt;

use boethos::{Kind, Put, Ref, Store, StoreError, StoreResult};
use serde::{Deserialize, Serialize};

use crate::face_labels::retain_face_labels;

const FAVORITE: &str = "favorite";
const PREVIEW: &str = "preview";

/// What an indexed file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Body mesh or body source.
    Body,
    /// A shape inside a body NIF (body, feet, hands, head, underwear).
    #[serde(rename = "body-part")]
    BodyPart,
    /// Garment or armour.
    Outfit,
    /// Hairstyle.
    Hair,
    /// Face or head mesh.
    Face,
    /// Motion clip or behavior.
    Animation,
    /// Skyrim Havok clip or behavior graph (`.hkx`).
    #[serde(rename = "skyrim-animation")]
    SkyrimAnimation,
    /// One animation embedded in an actor `glb`/`gltf`.
    Clip,
    /// BodySlide preset XML.
    Preset,
    /// RaceMenu `.jslot` face preset.
    FacePreset,
    /// BodySlide project (`.osp`, `.osd`, `.bsd`).
    Project,
    /// Texture map.
    Texture,
    /// Generic mesh export.
    Mesh,
    /// Review or reference image.
    Image,
    /// Supplied archive.
    Archive,
    /// Source not classified by path or extension.
    Other,
}

impl Category {
    /// Stable text token used by filters and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::BodyPart => "body-part",
            Self::Outfit => "outfit",
            Self::Hair => "hair",
            Self::Face => "face",
            Self::Animation => "animation",
            Self::SkyrimAnimation => "skyrim-animation",
            Self::Clip => "clip",
            Self::Preset => "preset",
            Self::FacePreset => "face_preset",
            Self::Project => "project",
            Self::Texture => "texture",
            Self::Mesh => "mesh",
            Self::Image => "image",
            Self::Archive => "archive",
            Self::Other => "other",
        }
    }

    /// Parse a filter token.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "body" => Self::Body,
            "body-part" => Self::BodyPart,
            "outfit" => Self::Outfit,
            "hair" => Self::Hair,
            "face" => Self::Face,
            "animation" => Self::Animation,
            "skyrim-animation" => Self::SkyrimAnimation,
            "clip" => Self::Clip,
            "preset" => Self::Preset,
            "face_preset" => Self::FacePreset,
            "project" => Self::Project,
            "texture" => Self::Texture,
            "mesh" => Self::Mesh,
            "image" => Self::Image,
            "archive" => Self::Archive,
            "other" => Self::Other,
            _ => return None,
        })
    }
}

impl fmt::Display for Category {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Target sex when the source path or manifest declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sex {
    /// Female source.
    Female,
    /// Male source.
    Male,
    /// Not declared.
    Unknown,
}

impl Status {
    /// Stable text token used by reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Prepared => "prepared",
            Self::Accepted => "accepted",
            Self::Unknown => "unknown",
        }
    }
}

impl Sex {
    /// Stable text token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Female => "female",
            Self::Male => "male",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a filter token.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "female" => Self::Female,
            "male" => Self::Male,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

/// How far the file has travelled through the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Supplied or downloaded source.
    Raw,
    /// Derived output ready for use.
    Prepared,
    /// Accepted reference.
    Accepted,
    /// Not established.
    Unknown,
}

/// Measured animation data for a `clip` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipDetails {
    /// Number of animation channels.
    pub channels: u64,
    /// Largest sampler input key count.
    pub keyframes: u64,
    /// Longest sampler input time in seconds.
    pub duration_seconds: f64,
    /// Every targeted node, in node order.
    pub bones: Vec<String>,
    /// Number of distinct skin joints in the actor.
    pub palette_joints: u64,
    /// Fraction of skin joints the channels target; `None` without a skin.
    pub palette_coverage: Option<f64>,
}

/// One indexed file, archive or archive member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// Content-derived identity (sha256 hex).
    pub id: String,
    /// Domain category.
    pub category: Category,
    /// Human-readable name.
    pub name: String,
    /// Host path, or `archive::entry` for a member.
    pub path: String,
    /// Indexed root or pack label.
    pub source: String,
    /// Owning archive, when this is a member.
    pub archive: Option<String>,
    /// Member path inside the archive.
    pub entry: Option<String>,
    /// Lowercase extension without the dot.
    pub format: String,
    /// Shape name inside a NIF, when this row is a body part.
    #[serde(default)]
    pub shape: Option<String>,
    /// Shape vertex count, when measured.
    #[serde(default)]
    pub vertices: Option<u64>,
    /// Shape triangle count, when measured.
    #[serde(default)]
    pub triangles: Option<u64>,
    /// Embedded animation details, when this row is a `clip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipDetails>,
    /// Content hash when the file was read; `None` for archive members.
    pub sha256: Option<String>,
    /// Digest of the source model a derived part came from.
    #[serde(default)]
    pub source_sha256: Option<String>,
    /// Size in bytes when known.
    pub bytes: Option<u64>,
    /// Declared target sex.
    pub sex: Option<Sex>,
    /// Family token such as `bijin`, `himbo`, `3ba`, `pride`.
    pub family: Option<String>,
    /// Search tags such as `facegeom`, `crc:...`.
    pub tags: Vec<String>,
    /// Pipeline status.
    pub status: Status,
}

/// `asset` kind registration.
pub struct Assets;

impl Kind for Assets {
    const NAME: &'static str = "asset";
    const VERSION: u32 = 1;
    type Record = Asset;

    fn validate(record: &Self::Record) -> Result<(), String> {
        if record.id.trim().is_empty() {
            return Err("id must not be empty".to_string());
        }
        if record.name.trim().is_empty() {
            return Err("name must not be empty".to_string());
        }
        if record.path.trim().is_empty() {
            return Err("path must not be empty".to_string());
        }
        Ok(())
    }
}

/// Change to a user's favorite tag.
#[derive(Debug, Clone, Copy)]
pub enum FavoriteChange {
    /// Add the tag if absent.
    Add,
    /// Remove the tag if present.
    Remove,
}

/// Set or clear the tag. Returns false when the asset does not exist.
pub fn change_favorite(store: &Store, id: &str, change: FavoriteChange) -> StoreResult<bool> {
    let outcome = store.update::<Assets>(id, |asset| {
        asset.map(|mut asset| {
            match change {
                FavoriteChange::Add => mark_favorite(&mut asset),
                FavoriteChange::Remove => asset.tags.retain(|tag| tag != FAVORITE),
            }
            asset
        })
    })?;
    Ok(outcome.is_some())
}

/// Refresh an indexed asset while retaining operator labels.
pub fn refresh_asset(store: &Store, asset: &Asset) -> StoreResult<Put> {
    let outcome = store.update::<Assets>(&asset.id, |previous| {
        if previous.as_ref() == Some(asset) {
            return None;
        }
        let mut refreshed = asset.clone();
        if let Some(previous) = previous.as_ref() {
            if is_favorite(previous) {
                mark_favorite(&mut refreshed);
            }
            retain_face_labels(previous, &mut refreshed);
        }
        (previous.as_ref() != Some(&refreshed)).then_some(refreshed)
    })?;
    Ok(outcome.unwrap_or(Put::Exists))
}

fn is_favorite(asset: &Asset) -> bool {
    asset.tags.iter().any(|tag| tag == FAVORITE)
}

fn mark_favorite(asset: &mut Asset) {
    if !is_favorite(asset) {
        asset.tags.push(FAVORITE.to_string());
    }
}

/// Link an indexed image to a preset or outfit. Repeated attachments are idempotent.
pub fn attach_preview(store: &Store, subject: &Asset, image: &Asset) -> StoreResult<bool> {
    require_preview_subject(subject)?;
    require_category(image, Category::Image)?;
    store.link(asset_ref(subject), PREVIEW, asset_ref(image))
}

/// Resolve a preset or outfit's preview links to currently indexed images.
pub fn asset_previews(store: &Store, subject: &Asset) -> StoreResult<Vec<Asset>> {
    require_preview_subject(subject)?;
    store
        .links_from(asset_ref(subject))?
        .into_iter()
        .filter(|link| link.relation == PREVIEW && link.to_kind == Assets::NAME)
        .filter_map(|link| match store.get::<Assets>(&link.to_id) {
            Ok(Some(image)) if image.category == Category::Image => Some(Ok(image)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn asset_ref(asset: &Asset) -> Ref<'_> {
    Ref {
        kind: Assets::NAME,
        id: &asset.id,
    }
}

fn require_preview_subject(asset: &Asset) -> StoreResult<()> {
    if matches!(asset.category, Category::Preset | Category::Outfit) {
        return Ok(());
    }
    Err(StoreError::Validation {
        kind: Assets::NAME,
        detail: format!(
            "{} has category {}, expected preset or outfit",
            asset.id, asset.category
        ),
    })
}

fn require_category(asset: &Asset, expected: Category) -> StoreResult<()> {
    if asset.category == expected {
        return Ok(());
    }
    Err(StoreError::Validation {
        kind: Assets::NAME,
        detail: format!(
            "{} has category {}, expected {expected}",
            asset.id, asset.category
        ),
    })
}
