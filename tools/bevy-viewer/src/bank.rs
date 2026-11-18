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
    inspection: Option<SlotSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Slot(usize),
    Inspection,
}
impl Selection {
    pub fn value(self) -> Value {
        match self {
            Self::Slot(i) => json!(i),
            Self::Inspection => json!("inspection"),
        }
    }
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
    pub inspection: Option<Slot>,
    pub current: Selection,
    pub requested: Option<Selection>,
    pub error: Option<String>,
}

impl Bank {
    /// Validate every entry before replacing a live actor; missing clips never fall back.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if manifest.version != 1 || manifest.slots.is_empty() {
            return Err("bank requires version 1 and at least one slot".into());
        }
        let parent = path.parent().ok_or("bank path has no parent")?;
        let mut cache = std::collections::HashMap::new();
        let mut load_slot = |spec: SlotSpec| -> Result<Slot, String> {
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
            let names = match cache.entry(asset.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let bytes = std::fs::read(&asset).map_err(|error| error.to_string())?;
                    e.insert(
                        crate::glb_animation_names_bytes(&bytes)
                            .ok_or_else(|| format!("{}: invalid GLB metadata", asset.display()))?,
                    )
                }
            };
            let clip = resolve_clip(names, spec.clip.as_deref())?;
            Ok(Slot {
                name: spec.name,
                asset,
                clip,
                clip_name: spec.clip,
            })
        };
        if manifest
            .inspection
            .as_ref()
            .is_some_and(|s| s.clip.is_some())
        {
            return Err("Ctrl+0 inspection must be a rest asset".into());
        }
        let inspection = manifest.inspection.map(&mut load_slot).transpose()?;
        let slots = manifest
            .slots
            .into_iter()
            .map(load_slot)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            slots,
            inspection,
            current: Selection::Slot(0),
            requested: None,
            error: None,
        })
    }

    pub fn slot(&self, i: Selection) -> Option<&Slot> {
        match i {
            Selection::Slot(n) => self.slots.get(n),
            Selection::Inspection => self.inspection.as_ref(),
        }
    }
    pub fn caption(&self, i: Selection) -> String {
        let slot = self.slot(i).expect("only admitted slots receive captions");
        match i {
            Selection::Slot(n) => format!("{n} · {}", slot.name),
            Selection::Inspection => format!("Ctrl+0 · {}", slot.name),
        }
    }

    pub fn state(&self) -> Value {
        json!({
            "current": self.current.value(),
            "requested": self.requested.map(Selection::value),
            "error": self.error,
            "inspection":self.inspection.as_ref().map(|s|json!({"key":"Ctrl+0","name":s.name,"asset":s.asset})),
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

    #[test]
    fn thirty_three_slots_coexist_with_a_separate_inspection() {
        let dir = std::env::temp_dir().join(format!("sculpto-large-bank-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bank.json");
        let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic.glb");
        let slots = (0..33)
            .map(|i| json!({"name": format!("Pose {i}"), "asset": asset, "clip":null}))
            .collect::<Vec<_>>();
        let inspection = json!({"name":"Cut", "asset":asset, "clip":null});
        std::fs::write(
            &path,
            json!({"version":1,"slots":slots,"inspection":inspection}).to_string(),
        )
        .unwrap();
        let loaded = Bank::load(&path);
        std::fs::remove_dir_all(dir).unwrap();
        let bank = loaded.unwrap();
        assert_eq!(bank.slots.len(), 33);
        for i in [0, 10, 12, 32] {
            assert_eq!(
                bank.slot(Selection::Slot(i)).unwrap().name,
                format!("Pose {i}")
            );
        }
        assert_eq!(bank.slot(Selection::Inspection).unwrap().name, "Cut");
        assert!(bank.slot(Selection::Slot(33)).is_none());
    }
}
