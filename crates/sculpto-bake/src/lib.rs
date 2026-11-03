//! Resolve a project + preset into per-vertex morph offsets, then apply them.
//!
//! Resolution is the half of a bake that touches no geometry: it selects the
//! slider set the preset names, computes one percentage per slider and joins
//! each `Data` reference to a named block inside an `.osd`/`.bsd` container.
//! Everything it cannot resolve is reported by name instead of skipped, because
//! a silently missing record is a wrong body.
//!
//! Application takes positions, not a mesh: the NIF adapter supplies vertices
//! later, and this crate deliberately does not depend on a format reader.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sculpto_core::{MorphContainer, Preset, Project, SliderSize, VertexOffset};

/// One resolved block and the shape it deforms.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedShape {
    /// Target shape name as authored in the project.
    pub target: String,
    /// Block name inside the container.
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
            MorphContainer::parse_file(path).map_err(|source| BakeError::Container {
                path: path.to_path_buf(),
                source,
            })
        })
    }

    /// Resolve with a caller-supplied container loader.
    pub fn resolve_with<F>(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
        home: &Path,
        load: F,
    ) -> Result<Self, BakeError>
    where
        F: FnMut(&Path) -> Result<MorphContainer, BakeError>,
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

    /// Resolve with both overrides and a caller-supplied container loader.
    ///
    /// `home` is the BodySlide directory: local references resolve under
    /// `ShapeData/<data folder>/`, non-local ones under `Data/`.
    pub fn resolve_with_overrides<F>(
        project: &Project,
        preset: &Preset,
        set_name: &str,
        size: SliderSize,
        home: &Path,
        overrides: &BTreeMap<String, f32>,
        mut load: F,
    ) -> Result<Self, BakeError>
    where
        F: FnMut(&Path) -> Result<MorphContainer, BakeError>,
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

        let mut cache: BTreeMap<PathBuf, MorphContainer> = BTreeMap::new();
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
                let path = container_path(home, set, data.local, file);
                if !cache.contains_key(&path) {
                    if !path.is_file() {
                        unresolved.push(UnresolvedRef {
                            slider: slider.name.clone(),
                            target: data.target.clone(),
                            reason: UnresolvedReason::MissingFile(path),
                        });
                        continue;
                    }
                    cache.insert(path.clone(), load(&path)?);
                }
                let container = &cache[&path];
                let block_name = data
                    .record
                    .clone()
                    .unwrap_or_else(|| format!("{}{}", data.target, slider.name));
                match container
                    .blocks
                    .iter()
                    .find(|block| block.name == block_name)
                {
                    Some(block) => shapes.push(PlannedShape {
                        target: data.target.clone(),
                        block: block_name,
                        offsets: block.offsets.clone(),
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
            let scale = slider.percent / 100.0 * if slider.invert { -1.0 } else { 1.0 };
            for shape in &slider.shapes {
                if shape.target != target {
                    continue;
                }
                for offset in &shape.offsets {
                    let slot =
                        out.get_mut(offset.index as usize)
                            .ok_or(BakeError::IndexOutOfBounds {
                                slider: slider.name.clone(),
                                index: offset.index,
                                vertices: positions.len(),
                            })?;
                    for (component, delta) in slot.iter_mut().zip(offset.delta) {
                        *component += scale * delta;
                    }
                }
            }
        }
        Ok(out)
    }
}

fn container_path(home: &Path, set: &sculpto_core::SliderSet, local: bool, file: &str) -> PathBuf {
    // Authored folders use Windows separators (`HIMBO Body\Vanilla`); on Linux
    // they must become real path components.
    let folder = set.data_folder.replace('\\', "/");
    let file = file.replace('\\', "/");
    if local {
        home.join("ShapeData").join(folder).join(file)
    } else {
        home.join("Data").join(file)
    }
}
