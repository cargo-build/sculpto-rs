//! Resolve a project + preset into per-vertex morph offsets, then apply them.
//!
//! Resolution is the half of a bake that touches no geometry: it selects the
//! slider set the preset names, computes one percentage per slider and joins
//! each `Data` reference to offsets inside an `.osd` named block or a bare
//! `.bsd` record set. Everything it cannot resolve is reported by name instead
//! of skipped, because a silently missing record is a wrong body.
//!
//! Application takes positions, not a mesh: the NIF adapter supplies vertices
//! later, and this crate deliberately does not depend on a format reader.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sculpto_core::shape_data::MorphData;
use sculpto_core::{Preset, Project, SliderDataRef, SliderSet, SliderSize, VertexOffset};

/// One resolved block and the shape it deforms.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedShape {
    /// Target shape name as authored in the project.
    pub target: String,
    /// Block name inside the container. A bare `.bsd` stores no name, so this is
    /// the name the project reference supplies.
    pub block: String,
    /// Sparse offsets in document order.
    pub offsets: Vec<VertexOffset>,
}

/// A slider that can be applied to positions.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedSlider {
    /// Slider name.
    pub name: String,
    /// Authored percentage after endpoint selection.
    pub percent: f32,
    /// Reverse the morph direction.
    pub invert: bool,
    /// Resolved blocks in document order.
    pub shapes: Vec<PlannedShape>,
}

impl PlannedSlider {
    /// Apply this slider to a copy of the target's positions.
    ///
    /// # Errors
    /// Refuses an offset outside the provided vertex array.
    pub fn apply(&self, target: &str, positions: &[[f32; 3]]) -> Result<Vec<[f32; 3]>, BakeError> {
        let mut out = positions.to_vec();
        self.apply_to(target, &mut out)?;
        Ok(out)
    }

    fn apply_to(&self, target: &str, positions: &mut [[f32; 3]]) -> Result<(), BakeError> {
        let scale = self.percent / 100.0 * if self.invert { -1.0 } else { 1.0 };
        let vertices = positions.len();
        for shape in self.shapes.iter().filter(|shape| shape.target == target) {
            for offset in &shape.offsets {
                let slot = positions
                    .get_mut(usize::from(offset.index))
                    .ok_or_else(|| BakeError::IndexOutOfBounds {
                        slider: self.name.clone(),
                        index: offset.index,
                        vertices,
                    })?;
                for (component, delta) in slot.iter_mut().zip(offset.delta) {
                    *component += scale * delta;
                }
            }
        }
        Ok(())
    }
}

/// Why a `Data` reference could not be joined to an offset block.
#[derive(Debug, Clone, PartialEq)]
pub enum UnresolvedReason {
    /// The project records no container for this reference.
    Unrecorded,
    /// The container file does not exist at the resolved path.
    MissingFile(PathBuf),
    /// The container parsed but has no block with this name.
    MissingBlock(String),
}

/// A reference the plan reports instead of applying.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedRef {
    /// Slider that owns the reference.
    pub slider: String,
    /// Target shape the reference names.
    pub target: String,
    /// Why it stayed unresolved.
    pub reason: UnresolvedReason,
}

/// Why a slider is not applied to positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredReason {
    /// UV slider: offsets belong to UV coordinates.
    Uv,
    /// Zap slider: it removes topology instead of moving vertices.
    Zap,
    /// Clamp/default semantics are not verified against a native build yet.
    ClampSemantics,
}

/// A slider the plan deliberately does not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredSlider {
    /// Slider name.
    pub name: String,
    /// Why it is deferred.
    pub reason: DeferredReason,
}

/// A resolved bake: what to add to which vertex, and what could not be resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct BakePlan {
    /// Slider set the preset resolved to.
    pub slider_set: String,
    /// Preset name.
    pub preset: String,
    /// Endpoint the percentages were taken from.
    pub size: SliderSize,
    /// Sliders that apply to positions.
    pub sliders: Vec<PlannedSlider>,
    /// References that could not be resolved.
    pub unresolved: Vec<UnresolvedRef>,
    /// Sliders deferred to a later contract.
    pub deferred: Vec<DeferredSlider>,
}

/// A bake that cannot be assembled or applied.
#[derive(Debug)]
pub enum BakeError {
    /// The requested slider set is not in the project.
    SetNotResolved {
        /// Set name the caller asked for.
        name: String,
    },
    /// A container existed but failed to parse.
    Container {
        /// Container path.
        path: PathBuf,
        /// Reader failure.
        source: sculpto_core::Error,
    },
    /// A record addressed a vertex the supplied positions do not have.
    IndexOutOfBounds {
        /// Slider whose record failed.
        slider: String,
        /// Vertex index from the record.
        index: u16,
        /// Number of positions supplied.
        vertices: usize,
    },
}

impl fmt::Display for BakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetNotResolved { name } => {
                write!(f, "the project contains no slider set named `{name}`")
            }
            Self::Container { path, source } => write!(f, "{}: {source}", path.display()),
            Self::IndexOutOfBounds {
                slider,
                index,
                vertices,
            } => write!(
                f,
                "slider `{slider}` addresses vertex {index}, but only {vertices} positions were supplied"
            ),
        }
    }
}

impl std::error::Error for BakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Container { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl BakePlan {
    /// Resolve against a BodySlide home on disk.
    ///
    /// `set_name` is the slider set to bake. A preset's own `set` attribute
    /// names the project (the `.osp` stem), not a set, so selecting the set is
    /// the caller's decision.
    pub fn resolve(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
        home: &Path,
    ) -> Result<Self, BakeError> {
        Self::resolve_overrides(project, preset, set_name, size, home, &BTreeMap::new())
    }

    /// Resolve with per-slider percent overrides: a value for a slider name
    /// replaces the preset value for the requested endpoint.
    pub fn resolve_overrides(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
        home: &Path,
        overrides: &BTreeMap<String, f32>,
    ) -> Result<Self, BakeError> {
        Self::resolve_with_overrides(project, preset, set_name, size, home, overrides, |path| {
            MorphData::from_file(path).map_err(|source| BakeError::Container {
                path: path.to_path_buf(),
                source,
            })
        })
    }

    /// Resolve with a caller-supplied morph data loader.
    ///
    /// The loader returns anything that converts into [`MorphData`]: a bare
    /// `.bsd` record set or a named `.osd` container, so both real formats reach
    /// the plan through one codec.
    pub fn resolve_with<F, T>(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
        home: &Path,
        load: F,
    ) -> Result<Self, BakeError>
    where
        F: FnMut(&Path) -> Result<T, BakeError>,
        T: Into<MorphData>,
    {
        Self::resolve_with_overrides(
            project,
            preset,
            set_name,
            size,
            home,
            &BTreeMap::new(),
            load,
        )
    }

    /// Resolve with both overrides and a caller-supplied morph data loader.
    ///
    /// Containers live under `home/ShapeData`: local records use the set folder;
    /// external records select the first existing file in their target's folders.
    pub fn resolve_with_overrides<F, T>(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
        home: &Path,
        overrides: &BTreeMap<String, f32>,
        mut load: F,
    ) -> Result<Self, BakeError>
    where
        F: FnMut(&Path) -> Result<T, BakeError>,
        T: Into<MorphData>,
    {
        let set = project
            .sets
            .iter()
            .find(|set| set.name == set_name)
            .ok_or_else(|| BakeError::SetNotResolved {
                name: set_name.to_string(),
            })?;

        let preset_values: BTreeMap<(&str, SliderSize), f32> = preset
            .values
            .iter()
            .map(|value| ((value.slider.as_str(), value.size), value.percent))
            .collect();

        let mut cache: BTreeMap<PathBuf, MorphData> = BTreeMap::new();
        let mut sliders = Vec::new();
        let mut unresolved = Vec::new();
        let mut deferred = Vec::new();

        for slider in &set.sliders {
            let percent = overrides
                .get(slider.name.as_str())
                .copied()
                .or_else(|| preset_values.get(&(slider.name.as_str(), size)).copied())
                .unwrap_or(match size {
                    SliderSize::Small => slider.small,
                    SliderSize::Big => slider.big,
                });
            if percent == 0.0 {
                continue;
            }
            if slider.uv {
                deferred.push(DeferredSlider {
                    name: slider.name.clone(),
                    reason: DeferredReason::Uv,
                });
                continue;
            }
            if slider.zap {
                deferred.push(DeferredSlider {
                    name: slider.name.clone(),
                    reason: DeferredReason::Zap,
                });
                continue;
            }
            if slider.clamp || slider.default.is_some() {
                deferred.push(DeferredSlider {
                    name: slider.name.clone(),
                    reason: DeferredReason::ClampSemantics,
                });
                continue;
            }

            let mut shapes = Vec::new();
            for data in &slider.data {
                let Some(file) = data.file.as_deref() else {
                    unresolved.push(UnresolvedRef {
                        slider: slider.name.clone(),
                        target: data.target.clone(),
                        reason: UnresolvedReason::Unrecorded,
                    });
                    continue;
                };
                let path = container_path(home, set, data, file);
                if !cache.contains_key(&path) {
                    if !path.is_file() {
                        unresolved.push(UnresolvedRef {
                            slider: slider.name.clone(),
                            target: data.target.clone(),
                            reason: UnresolvedReason::MissingFile(path),
                        });
                        continue;
                    }
                    cache.insert(path.clone(), load(&path)?.into());
                }
                let data_file = &cache[&path];
                let block_name = data
                    .record
                    .clone()
                    .unwrap_or_else(|| format!("{}{}", data.target, slider.name));
                match data_file.offsets(&block_name) {
                    Some(offsets) => shapes.push(PlannedShape {
                        target: data.target.clone(),
                        block: block_name,
                        offsets: offsets.to_vec(),
                    }),
                    None => unresolved.push(UnresolvedRef {
                        slider: slider.name.clone(),
                        target: data.target.clone(),
                        reason: UnresolvedReason::MissingBlock(block_name),
                    }),
                }
            }
            if !shapes.is_empty() {
                sliders.push(PlannedSlider {
                    name: slider.name.clone(),
                    percent,
                    invert: slider.invert,
                    shapes,
                });
            }
        }

        Ok(Self {
            slider_set: set.name.clone(),
            preset: preset.name.clone(),
            size,
            sliders,
            unresolved,
            deferred,
        })
    }

    /// Effective percentages for every slider in the set, without touching
    /// containers: the preset value for the endpoint, otherwise the slider's
    /// `small`/`big` default. Used by the slider UI and by diagnostics.
    pub fn percents(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
    ) -> Result<Vec<(String, f32)>, BakeError> {
        let set = project
            .sets
            .iter()
            .find(|set| set.name == set_name)
            .ok_or_else(|| BakeError::SetNotResolved {
                name: set_name.to_string(),
            })?;
        Ok(set
            .sliders
            .iter()
            .map(|slider| {
                let percent = preset
                    .values
                    .iter()
                    .find(|value| value.slider == slider.name && value.size == size)
                    .map(|value| value.percent)
                    .unwrap_or(match size {
                        SliderSize::Small => slider.small,
                        SliderSize::Big => slider.big,
                    });
                (slider.name.clone(), percent)
            })
            .collect())
    }

    /// Number of resolved offset blocks.
    pub fn block_count(&self) -> usize {
        self.sliders.iter().map(|slider| slider.shapes.len()).sum()
    }

    /// Apply every planned offset for `target` to a copy of `positions`.
    pub fn apply(&self, target: &str, positions: &[[f32; 3]]) -> Result<Vec<[f32; 3]>, BakeError> {
        let mut out = positions.to_vec();
        for slider in &self.sliders {
            slider.apply_to(target, &mut out)?;
        }
        Ok(out)
    }
}

/// Keep file selection independent of block lookup, matching native builds. @req BLD-05
fn container_path(home: &Path, set: &SliderSet, data: &SliderDataRef, file: &str) -> PathBuf {
    let root = home.join("ShapeData");
    let file = file.replace('\\', "/");
    let path = |folder: &str| root.join(folder.replace('\\', "/")).join(&file);
    let folders = match data.local {
        true => None,
        false => set
            .shapes
            .iter()
            .find(|shape| shape.target == data.target)
            .and_then(|shape| shape.data_folder.as_deref()),
    };
    let mut candidates = folders
        .into_iter()
        .flat_map(|list| list.split_terminator(';'))
        .map(path);
    let first = match candidates.next() {
        Some(first) => first,
        None => return path(&set.data_folder),
    };
    match first.is_file() {
        true => first,
        false => candidates
            .find(|candidate| candidate.is_file())
            .unwrap_or(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpto_core::{Document, shape_data};

    const PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body">
    <DataFolder>HIMBO</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Body">Body</Shape>
    <Slider name="Lean" small="0" big="100">
      <Data target="Body" local="true">body.bsd</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

    /// The production loader, not a stub: a bare `.bsd` is resolved by name from
    /// the project reference, because the file stores none.
    #[test]
    fn bare_bsd_resolves_through_the_production_loader() {
        let root = std::env::temp_dir().join(format!("sculpto-bake-bsd-{}", std::process::id()));
        let folder = root.join("ShapeData/HIMBO");
        std::fs::create_dir_all(&folder).expect("temp dir");
        let records = [
            VertexOffset {
                index: 1,
                delta: [0.5, 0.0, 0.0],
            },
            VertexOffset {
                index: 2,
                delta: [0.0, -1.0, 0.0],
            },
        ];
        std::fs::write(
            folder.join("body.bsd"),
            shape_data::to_bsd_bytes(&records).expect("writes"),
        )
        .expect("fixture file");

        let Document::Project(project) =
            Document::parse_str(PROJECT, "inline.osp").expect("parses")
        else {
            panic!("expected a project");
        };
        let Document::Presets(presets) = Document::parse_str(
            r#"<SliderPresets><Preset name="Chonky" set="Body"><SetSlider name="Lean" size="big" value="50"/></Preset></SliderPresets>"#,
            "inline.xml",
        )
        .expect("parses")
        else {
            panic!("expected presets");
        };

        let plan = BakePlan::resolve(&project, &presets[0], "Body", SliderSize::Big, &root)
            .expect("bare .bsd resolves");
        assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);
        assert_eq!(plan.sliders[0].shapes[0].block, "BodyLean");
        assert_eq!(plan.sliders[0].shapes[0].offsets.len(), 2);

        let out = plan
            .apply("Body", &[[0.0; 3], [1.0; 3], [2.0; 3]])
            .expect("applies");
        assert_eq!(out[1], [1.25, 1.0, 1.0]);
        assert_eq!(out[2], [2.0, 1.5, 2.0]);
        assert_eq!(out[0], [0.0; 3]);
        std::fs::remove_dir_all(&root).expect("remove fixture directory");
    }
}
