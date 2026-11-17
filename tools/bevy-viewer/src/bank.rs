//! Explicit rest/clip slots for comparing poses without rebaking them in the viewer.
use std::path::{Path, PathBuf};

use bevy::prelude::Resource;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    slots: Vec<SlotSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotSpec {
    name: String,
    asset: PathBuf,
    clip: Option<String>,
}

pub struct Slot {
    pub name: String,
    pub asset: PathBuf,
    pub clip: Option<usize>,
    pub clip_name: Option<String>,
}

#[derive(Resource)]
pub struct Bank {
    pub slots: Vec<Slot>,
    pub current: usize,
    pub requested: Option<usize>,
    pub error: Option<String>,
}

impl Bank {
    /// Validate every entry before replacing a live actor; missing clips never fall back.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if manifest.version != 1 || manifest.slots.is_empty() || manifest.slots.len() > 10 {
            return Err("bank requires version1 and between1 and10 slots".into());
        }
        let parent = path.parent().ok_or("bank path has no parent")?;
        let slots = manifest
            .slots
            .into_iter()
            .map(|spec| {
                if spec.name.trim().is_empty() {
                    return Err("bank slot name must not be empty".into());
                }
                let asset = parent
                    .join(&spec.asset)
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", spec.asset.display()))?;
                if asset.extension().is_none_or(|extension| extension != "glb") {
                    return Err(format!("{}: bank assets must be GLB", asset.display()));
                }
                let bytes = std::fs::read(&asset).map_err(|error| error.to_string())?;
                let names = crate::glb_animation_names_bytes(&bytes)
                    .ok_or_else(|| format!("{}: invalid GLB metadata", asset.display()))?;
                let clip = resolve_clip(&names, spec.clip.as_deref())?;
                Ok(Slot {
                    name: spec.name,
                    asset,
                    clip,
                    clip_name: spec.clip,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            slots,
            current: 0,
            requested: None,
            error: None,
        })
    }

    pub fn state(&self) -> Value {
        json!({
            "current": self.current,
            "requested": self.requested,
            "error": self.error,
            "slots": self.slots.iter().enumerate().map(|(index, slot)| json!({
                "index": index, "name": slot.name, "asset": slot.asset,
                "clip": slot.clip,
            })).collect::<Vec<_>>(),
        })
    }
}

fn resolve_clip(names: &[String], requested: Option<&str>) -> Result<Option<usize>, String> {
    let Some(name) = requested else {
        return Ok(None);
    };
    let matches: Vec<usize> = names
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (candidate == name).then_some(index))
        .collect();
    match matches.as_slice() {
        [index] => Ok(Some(*index)),
        [] => Err(format!(
            "bank clip {name:?} is absent; available: {names:?}"
        )),
        _ => Err(format!("bank clip {name:?} is ambiguous")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_and_named_clips_are_distinct_and_missing_names_do_not_fall_back() {
        let names = vec!["Rest".into(), "Walk".into(), "Run".into()];
        assert_eq!(resolve_clip(&names, None).unwrap(), None);
        assert_eq!(resolve_clip(&names, Some("Run")).unwrap(), Some(2));
        assert!(resolve_clip(&names, Some("Wlak")).is_err());
        assert!(resolve_clip(&["Walk".into(), "Walk".into()], Some("Walk")).is_err());
    }
}
