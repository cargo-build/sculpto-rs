//! Indexed asset record.

use std::fmt;

use boethos::Kind;
use serde::{Deserialize, Serialize};

/// What an indexed file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Body mesh or body source.
    Body,
    /// Garment or armour.
    Outfit,
    /// Hairstyle.
    Hair,
    /// Face or head mesh.
    Face,
    /// Motion clip or behavior.
    Animation,
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
            Self::Outfit => "outfit",
            Self::Hair => "hair",
            Self::Face => "face",
            Self::Animation => "animation",
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
            "outfit" => Self::Outfit,
            "hair" => Self::Hair,
            "face" => Self::Face,
            "animation" => Self::Animation,
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
    /// Content hash when the file was read; `None` for archive members.
    pub sha256: Option<String>,
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
