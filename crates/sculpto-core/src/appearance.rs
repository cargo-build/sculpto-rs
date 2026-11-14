//! Human face and hairstyle catalogue records over indexed boethos assets.
//!
//! CAT-01 step 1: the record a replaceable head needs, and the smallest
//! reader over the boethos asset store. Faces come from baked FaceGeom NIFs
//! (headless mesh swap) and RaceMenu `.jslot` presets (game-side FaceGen
//! only); hairstyles come from `hair`-classified meshes. There is no second
//! index: every entry keeps the boethos identity, so provenance survives.
//!
//! Honest metadata limits inherited from the catalogue: `sex` and `family`
//! are inferred by boethos from pack names (`Bijin` female, `Pride`/`PAN`
//! male), and baked FaceGeom names carry no race, so most faces stay
//! [`FaceRace::Unverified`] instead of a guessed human label. A race token in
//! a name (`darkElf`, `nord`, `orc`, …) is recorded as evidence.

use std::fmt;
use std::path::{Path, PathBuf};

use boethos::Store;
use boethos_catalogue::{Asset, Assets, Category, Sex};

/// Body families the reader can pair with faces, with their target sex.
///
/// Simple invariant, not configuration: the CBBE line (`cbbe`, `3ba`,
/// `3bbb`) is the accepted female body (DECL0022) and `himbo` is the male
/// line. Every other token is a named refusal.
pub const SUPPORTED_BODY_FAMILIES: &[(&str, Sex)] = &[
    ("cbbe", Sex::Female),
    ("3ba", Sex::Female),
    ("3bbb", Sex::Female),
    ("himbo", Sex::Male),
];

/// Head rig a catalogue entry attaches to.
///
/// Every catalogued face is a Skyrim `FaceGenData/FaceGeom` export or a
/// Skyrim RaceMenu preset, so the vanilla humanoid head bone set is the only
/// compatibility marker the record can honestly claim today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadRig {
    /// Vanilla Skyrim humanoid head bone set.
    SkyrimHumanHead,
}

impl HeadRig {
    /// Stable text token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SkyrimHumanHead => "skyrim-human-head",
        }
    }
}

/// How a face entry reaches a replaceable head slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceApplication {
    /// Baked FaceGeom NIF mesh; the engine can swap it headlessly.
    BakedFaceGeom,
    /// RaceMenu `.jslot` preset; needs game-side FaceGen baking first.
    RaceMenuSlot,
}

impl FaceApplication {
    /// Stable text token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BakedFaceGeom => "baked-facegeom",
            Self::RaceMenuSlot => "racemenu-jslot",
        }
    }

    /// Whether the entry can be applied without a game-side render.
    pub fn headless(self) -> bool {
        matches!(self, Self::BakedFaceGeom)
    }
}

/// Race evidence parsed from record names and member paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaceRace {
    /// A human race token was observed (`nord`, `imperial`, …).
    Human(String),
    /// A non-human token was observed; CAT-01 excludes these faces.
    Forbidden(String),
    /// No race evidence in the catalogue record.
    Unverified,
}

impl FaceRace {
    /// Stable text token for reports.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Human(token) => token.as_str(),
            Self::Forbidden(token) => token.as_str(),
            Self::Unverified => "unverified",
        }
    }

    /// Whether CAT-01 may keep the entry.
    pub fn eligible(&self) -> bool {
        !matches!(self, Self::Forbidden(_))
    }
}

/// Tokens that mark a face human when found in a name or member path.
const HUMAN_RACE_TOKENS: &[&str] = &["nord", "imperial", "breton", "redguard"];

/// Tokens that mark a face non-human (excluded by CAT-01).
const FORBIDDEN_RACE_TOKENS: &[&str] = &[
    "elf", "darkelf", "highelf", "woodelf", "dunmer", "altmer", "bosmer", "orc", "orsimer",
    "khajiit", "argonian",
];

/// One replaceable head from the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadEntry {
    /// Boethos identity (sha256 hex).
    pub id: String,
    /// Lookup key: `esp/formid` for FaceGeom, file stem for `.jslot`.
    pub name: String,
    /// NPC identity parsed from the member path, when the pack names one.
    pub npc: Option<String>,
    /// Target sex as inferred by the catalogue.
    pub sex: Sex,
    /// Face family token such as `bijin` or `pride`.
    pub family: Option<String>,
    /// Race evidence; most baked faces stay unverified.
    pub race: FaceRace,
    /// Owning archive path, when the face is an archive member.
    pub archive: Option<String>,
    /// Host path, or `archive::entry` for a member.
    pub path: String,
    /// Member path inside the archive.
    pub entry: Option<String>,
    /// Attachment compatibility marker.
    pub rig: HeadRig,
    /// How the face becomes a head.
    pub application: FaceApplication,
}

/// One hairstyle from the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HairstyleEntry {
    /// Boethos identity (sha256 hex).
    pub id: String,
    /// Record name.
    pub name: String,
    /// Target sex as inferred by the catalogue (`None` = unknown).
    pub sex: Option<Sex>,
    /// Host path, or `archive::entry` for a member.
    pub path: String,
    /// Attachment compatibility marker; static attachment, no physics.
    pub rig: HeadRig,
}

/// Faces and hairstyles indexed by boethos.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppearanceCatalogue {
    /// Every face: baked FaceGeom NIFs and RaceMenu `.jslot` presets.
    pub faces: Vec<HeadEntry>,
    /// `hair`-classified meshes; empty until hair members are indexed.
    pub hairstyles: Vec<HairstyleEntry>,
}

impl AppearanceCatalogue {
    /// Project indexed assets onto catalogue entries.
    ///
    /// Only `face` and `face_preset` categories become faces and only
    /// `hair` becomes a hairstyle; everything else is not catalogue
    /// business for this record.
    pub fn from_assets(assets: &[Asset]) -> Self {
        let mut faces = Vec::new();
        let mut hairstyles = Vec::new();
        for asset in assets {
            match asset.category {
                Category::Face | Category::FacePreset => faces.push(head_entry(asset)),
                Category::Hair => hairstyles.push(HairstyleEntry {
                    id: asset.id.clone(),
                    name: asset.name.clone(),
                    sex: asset.sex,
                    path: asset.path.clone(),
                    rig: HeadRig::SkyrimHumanHead,
                }),
                _ => {}
            }
        }
        faces.sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
        hairstyles.sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
        Self { faces, hairstyles }
    }

    /// Faces compatible with a body family: matching sex, race not excluded.
    ///
    /// # Errors
    /// Refuses an unsupported body family token by name.
    pub fn compatible_faces(&self, body_family: &str) -> Result<Vec<&HeadEntry>, AppearanceError> {
        let sex = body_family_sex(body_family)?;
        Ok(self
            .faces
            .iter()
            .filter(|face| face.sex == sex && face.race.eligible())
            .collect())
    }

    /// Hairstyles compatible with a body family (matching sex, or unknown).
    ///
    /// # Errors
    /// Refuses an unsupported body family token by name.
    pub fn compatible_hairstyles(
        &self,
        body_family: &str,
    ) -> Result<Vec<&HairstyleEntry>, AppearanceError> {
        let sex = body_family_sex(body_family)?;
        Ok(self
            .hairstyles
            .iter()
            .filter(|style| style.sex.is_none_or(|style_sex| style_sex == sex))
            .collect())
    }

    /// One face by exact catalogue name or boethos id.
    ///
    /// # Errors
    /// Refuses unknown names, and names that match more than one record
    /// (the same `esp/formid` can ship in several packs).
    pub fn face(&self, name: &str) -> Result<&HeadEntry, AppearanceError> {
        let matches: Vec<&HeadEntry> = self
            .faces
            .iter()
            .filter(|face| face.name == name || face.id == name)
            .collect();
        match matches.as_slice() {
            [] => Err(AppearanceError::UnknownFace {
                requested: name.to_string(),
            }),
            [only] => Ok(only),
            many => Err(AppearanceError::AmbiguousFace {
                requested: name.to_string(),
                matches: many.iter().map(|face| face.id.clone()).collect(),
            }),
        }
    }
}

/// Read the catalogue from a boethos database, read-only.
///
/// # Errors
/// Refuses a database that cannot be opened or iterated.
pub fn load_appearance_catalogue(db: &Path) -> Result<AppearanceCatalogue, AppearanceError> {
    let store = Store::open_read_only(db).map_err(|error| AppearanceError::Store {
        path: db.to_path_buf(),
        detail: error.to_string(),
    })?;
    let rows = store
        .iter_kind::<Assets>()
        .map_err(|error| AppearanceError::Store {
            path: db.to_path_buf(),
            detail: error.to_string(),
        })?;
    let assets: Vec<Asset> = rows.into_iter().map(|(_, asset)| asset).collect();
    Ok(AppearanceCatalogue::from_assets(&assets))
}

/// Sex a body family pairs with.
///
/// # Errors
/// Refuses an unknown token, naming the supported families.
fn body_family_sex(body_family: &str) -> Result<Sex, AppearanceError> {
    let token = body_family.trim().to_lowercase();
    SUPPORTED_BODY_FAMILIES
        .iter()
        .find(|(family, _)| *family == token)
        .map(|(_, sex)| *sex)
        .ok_or_else(|| AppearanceError::UnknownBodyFamily {
            requested: body_family.to_string(),
            supported: SUPPORTED_BODY_FAMILIES
                .iter()
                .map(|(family, _)| *family)
                .collect(),
        })
}

fn head_entry(asset: &Asset) -> HeadEntry {
    let application = match asset.category {
        Category::FacePreset => FaceApplication::RaceMenuSlot,
        _ => FaceApplication::BakedFaceGeom,
    };
    HeadEntry {
        id: asset.id.clone(),
        name: face_name(asset),
        npc: npc_name(asset),
        sex: asset.sex.unwrap_or(Sex::Unknown),
        family: asset.family.clone(),
        race: face_race(asset),
        archive: asset.archive.clone(),
        path: asset.path.clone(),
        entry: asset.entry.clone(),
        rig: HeadRig::SkyrimHumanHead,
        application,
    }
}

/// Lookup key: lowercase `esp/formid` for FaceGeom, `.jslot` file stem for
/// presets (record names are relative paths, so the basename is the key).
fn face_name(asset: &Asset) -> String {
    match asset.category {
        Category::FacePreset => {
            let base = asset
                .name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(asset.name.as_str());
            base.strip_suffix(".jslot").unwrap_or(base).to_string()
        }
        _ => asset.name.to_lowercase(),
    }
}

/// Deepest member path segment before `meshes/` or `data/`, when one exists.
///
/// `…/01 Characters/Ingjard/meshes/…` names Ingjard; packs that only carry a
/// grouping directory (`PA44_POS_Solitude_NPC`) yield that directory, so this
/// is auxiliary identity, never the lookup key.
fn npc_name(asset: &Asset) -> Option<String> {
    let entry = asset.entry.as_deref()?;
    let segments: Vec<&str> = entry.split(['/', '\\']).collect();
    let boundary = segments.iter().position(|part| {
        part.eq_ignore_ascii_case("meshes") || part.eq_ignore_ascii_case("data")
    })?;
    if boundary == 0 {
        return None;
    }
    segments
        .get(boundary.wrapping_sub(1))
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
}

/// Race evidence from name and member path tokens.
fn face_race(asset: &Asset) -> FaceRace {
    let mut tokens: Vec<String> = asset
        .name
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(|token| token.to_lowercase())
        .filter(|token| !token.is_empty())
        .collect();
    if let Some(entry) = asset.entry.as_deref() {
        tokens.extend(
            entry
                .split(|character: char| !character.is_ascii_alphanumeric())
                .map(|token| token.to_lowercase()),
        );
    }
    for token in &tokens {
        if FORBIDDEN_RACE_TOKENS.contains(&token.as_str()) {
            return FaceRace::Forbidden(token.clone());
        }
    }
    for token in &tokens {
        if HUMAN_RACE_TOKENS.contains(&token.as_str()) {
            return FaceRace::Human(token.clone());
        }
    }
    FaceRace::Unverified
}

/// Catalogue reader failures, all named refusals.
#[derive(Debug)]
pub enum AppearanceError {
    /// Body family token is not supported.
    UnknownBodyFamily {
        /// The refused token.
        requested: String,
        /// Supported tokens, for the refusal message.
        supported: Vec<&'static str>,
    },
    /// No face carries that name or id.
    UnknownFace {
        /// The refused name.
        requested: String,
    },
    /// More than one face carries that name.
    AmbiguousFace {
        /// The ambiguous name.
        requested: String,
        /// Ids of every match.
        matches: Vec<String>,
    },
    /// The boethos database could not be read.
    Store {
        /// Database path that failed.
        path: PathBuf,
        /// Store failure detail.
        detail: String,
    },
}

impl fmt::Display for AppearanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBodyFamily {
                requested,
                supported,
            } => write!(
                formatter,
                "unknown body family `{requested}`; supported: {}",
                supported.join(", ")
            ),
            Self::UnknownFace { requested } => {
                write!(formatter, "no catalogue face named `{requested}`")
            }
            Self::AmbiguousFace { requested, matches } => write!(
                formatter,
                "face name `{requested}` matches {} records: {}",
                matches.len(),
                matches.join(", ")
            ),
            Self::Store { path, detail } => {
                write!(formatter, "boethos database {}: {detail}", path.display())
            }
        }
    }
}

impl std::error::Error for AppearanceError {}
