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
//! Zap sliders are the one slider class the plan resolves beyond offsets: an
//! active zap's block is read as deletion indices, ready for the build step
//! that removes the tagged geometry.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sculpto_core::shape_data::MorphData;
use sculpto_core::{
    Preset, Project, SliderDataRef, SliderDef, SliderSet, SliderSize, VertexOffset,
};

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
    /// Complement the value instead of reversing the diff: native replaces the
    /// normalized value with `1 - value`, so 0 percent is full motion and 100
    /// percent is none.
    pub invert: bool,
    /// Resolved blocks in document order.
    pub shapes: Vec<PlannedShape>,
}

impl PlannedSlider {
    /// The normalized value native BodySlide feeds to its diff application.
    ///
    /// Authored percentages are 0..100; native stores the same value divided by
    /// 100 and, for an inverted slider, applies the complement before the diff,
    /// so this is the multiplier, not a negated percentage.
    pub fn value(&self) -> f32 {
        native_value(self.percent, self.invert)
    }

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
        let scale = self.value();
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

/// A zap slider resolved to the vertices it deletes for one target. @req BLD-01
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedZap {
    /// Slider name.
    pub slider: String,
    /// Target shape the deletions apply to.
    pub target: String,
    /// Block name the indices were collected from.
    pub block: String,
    /// Vertices the zap deletes: every index with a non-zero delta on any
    /// axis, ascending without duplicates.
    pub indices: Vec<u16>,
}

/// The authored non-destructive nipple-coverage morph. @req BODY-01
///
/// CBBE and 3BA ship this body-side position slider as the morph twin of the
/// NeverNude `NippleZap`: it flattens the nipple cap into the breast without
/// deleting any vertex, so the nude state stays one slider value away instead
/// of a destroyed patch. The name is the product-wide agent contract: authored
/// CBBE/3BA data already carries it, and any family without it (Bijin, Mike)
/// authors its coverage block under the same slider name.
pub const COVERAGE_SLIDER: &str = "NipBGone";

/// Percent the clothed state applies the coverage morph at. @req BODY-01
pub const CLOTHED_COVERAGE_PERCENT: f32 = 100.0;

/// Whether a body build hides the chest anatomy. @req BODY-01
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Hide the anatomy through the set's coverage morph. An explicit caller
    /// override for [`COVERAGE_SLIDER`] wins with its own value, so partial
    /// coverage stays expressible through the plain slider surface.
    Clothed,
    /// Show the anatomy: keep the authored preset values (Isla keeps her 32),
    /// with no implicit coverage override. Explicit slider overrides still
    /// apply.
    Nude,
}

impl Coverage {
    /// Parse the argv/wire token.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "clothed" => Some(Self::Clothed),
            "nude" => Some(Self::Nude),
            _ => None,
        }
    }

    /// The argv/wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clothed => "clothed",
            Self::Nude => "nude",
        }
    }
}

/// Coverage state a bake ended in, for reporting. @req BODY-01
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageState {
    /// The coverage morph was applied (implicit default or explicit request).
    Clothed,
    /// Explicit nude: preset values, no implicit override.
    Nude,
    /// The set carries no coverage morph and none was required; the bake is
    /// neither clothed nor an explicit nude.
    Unauthored,
}

impl CoverageState {
    /// The report token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clothed => "clothed",
            Self::Nude => "nude",
            Self::Unauthored => "unauthored",
        }
    }
}

/// Resolve the coverage request against the set and merge the resulting
/// override. @req BODY-01
///
/// The implicit default is clothed wherever the set carries the coverage
/// morph: the standard bodies ship covered and nude is the explicit toggle.
/// A set without the morph has no coverage to apply, so the default bakes it
/// as authored ([`CoverageState::Unauthored`]) — batch parity over outfit and
/// male sets stays possible. An explicit [`Coverage::Clothed`] is a required
/// value, not a wish: a set that does not author the morph is refused instead
/// of silently baking a visible body.
///
/// Returns the state to report and, when coverage applies, the effective
/// percent for [`COVERAGE_SLIDER`] (a caller override value when one was
/// supplied, otherwise [`CLOTHED_COVERAGE_PERCENT`], which is also inserted
/// into `overrides`).
///
/// # Errors
/// Refuses an explicit clothed request when the set lacks the coverage morph.
pub fn apply_coverage(
    set: &SliderSet,
    requested: Option<Coverage>,
    overrides: &mut BTreeMap<String, f32>,
) -> Result<(CoverageState, Option<f32>), BakeError> {
    let authored = set
        .sliders
        .iter()
        .any(|slider| slider.name == COVERAGE_SLIDER);
    match requested {
        Some(Coverage::Nude) => Ok((CoverageState::Nude, None)),
        explicit_clothed => {
            if !authored {
                return match explicit_clothed {
                    Some(_) => Err(BakeError::CoverageSliderAbsent {
                        set: set.name.clone(),
                        slider: COVERAGE_SLIDER.to_string(),
                    }),
                    None => Ok((CoverageState::Unauthored, None)),
                };
            }
            let percent = match overrides.get(COVERAGE_SLIDER) {
                Some(explicit) => *explicit,
                None => {
                    overrides.insert(COVERAGE_SLIDER.to_string(), CLOTHED_COVERAGE_PERCENT);
                    CLOTHED_COVERAGE_PERCENT
                }
            };
            Ok((CoverageState::Clothed, Some(percent)))
        }
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
///
/// Zap sliders are never listed here: the plan resolves them into
/// [`BakePlan::zaps`] deletion indices for the build step, so a zap is not
/// deferred, it is planned for deletion. @req BLD-01
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredReason {
    /// UV slider: offsets belong to UV coordinates.
    Uv,
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
    /// Zap sliders resolved to deletion indices, in document order. They never
    /// enter `sliders`: native collects indices at build time and deletes the
    /// geometry instead of deforming it.
    pub zaps: Vec<PlannedZap>,
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
    /// An explicitly clothed bake found no coverage morph in the set: the
    /// required coverage value cannot be honoured, and silently baking a
    /// visible body would hide the miss. @req BODY-01
    CoverageSliderAbsent {
        /// Set that was required to carry the coverage morph.
        set: String,
        /// Coverage slider name that was required.
        slider: String,
    },
    /// A clothed bake resolved, but the coverage morph never reached the plan
    /// (unresolved morph data or a deferred slider class), so the output would
    /// be a visible body reported as covered. @req BODY-01
    CoverageUnapplied {
        /// Set whose coverage morph did not apply.
        set: String,
        /// Coverage slider that did not reach the plan.
        slider: String,
        /// Where the slider ended up instead.
        detail: String,
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
            Self::CoverageSliderAbsent { set, slider } => write!(
                f,
                "slider set `{set}` carries no coverage slider `{slider}`; a clothed bake \
                 requires the morph, so bake `coverage=nude` or a set that authors it"
            ),
            Self::CoverageUnapplied {
                set,
                slider,
                detail,
            } => write!(
                f,
                "coverage slider `{slider}` of set `{set}` did not reach the plan ({detail}); \
                 a clothed bake cannot silently output a visible body"
            ),
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
        let mut zaps = Vec::new();
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
            // Native applies a slider whenever the post-inversion value is
            // non-zero, so inversion makes 0 percent active and 100 percent inert.
            if native_value(percent, slider.invert) == 0.0 {
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
                // Zap sliders are not deferred: they resolve into `zaps` for
                // the build's deletion step. Native collects zap indices only
                // when the post-inversion value is positive (`vbig > 0`,
                // BodySlideApp.cpp:5215), so a zero or negative zap deletes
                // nothing. UV zaps never reach here: native reads them through
                // the UV path (`bZap && !bUV`, BodySlideApp.cpp:5206).
                if native_value(percent, slider.invert) > 0.0 {
                    for data in &slider.data {
                        match resolve_ref(home, set, slider, data, &mut cache, &mut load)? {
                            Ok(shape) => zaps.push(PlannedZap {
                                slider: slider.name.clone(),
                                target: shape.target,
                                block: shape.block,
                                indices: zap_block_indices(&shape.offsets),
                            }),
                            Err(reason) => unresolved.push(UnresolvedRef {
                                slider: slider.name.clone(),
                                target: data.target.clone(),
                                reason,
                            }),
                        }
                    }
                }
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
                match resolve_ref(home, set, slider, data, &mut cache, &mut load)? {
                    Ok(shape) => shapes.push(shape),
                    Err(reason) => unresolved.push(UnresolvedRef {
                        slider: slider.name.clone(),
                        target: data.target.clone(),
                        reason,
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
            zaps,
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

    /// Deletion indices per target shape: every active zap's indices unioned,
    /// ascending without duplicates. @req BLD-01
    ///
    /// Native accumulates one sorted, deduplicated list per shape across all
    /// active zap sliders (`zapIdxAll`, BodySlideApp.cpp:5215-5217), so a
    /// vertex named by two zaps is deleted once. Targets appear once, ordered
    /// by name; the reference map is unordered and read by shape name.
    pub fn zap_indices(&self) -> Vec<(String, Vec<u16>)> {
        let mut per_target: BTreeMap<&str, Vec<u16>> = BTreeMap::new();
        for zap in &self.zaps {
            per_target
                .entry(zap.target.as_str())
                .or_default()
                .extend_from_slice(&zap.indices);
        }
        per_target
            .into_iter()
            .map(|(target, mut indices)| {
                indices.sort_unstable();
                indices.dedup();
                (target.to_string(), indices)
            })
            .collect()
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

/// Normalized value native BodySlide applies for one slider. @req DECL0007
///
/// Native keeps slider values in `0..=1` and replaces an inverted one with
/// `1.0f - value` right before `ApplyDiff`, which multiplies the authored diff
/// by that value. Negating the percentage instead is a different curve and a
/// different zero point.
fn native_value(percent: f32, invert: bool) -> f32 {
    let authored = percent / 100.0;
    match invert {
        true => 1.0 - authored,
        false => authored,
    }
}

/// Resolve one `Data` reference to its offset block.
///
/// The outer `Err` aborts the plan: an unreadable container fails the bake.
/// The inner `Err` is reported by name, because a silently missing record is
/// a wrong body.
fn resolve_ref<F, T>(
    home: &Path,
    set: &SliderSet,
    slider: &SliderDef,
    data: &SliderDataRef,
    cache: &mut BTreeMap<PathBuf, MorphData>,
    load: &mut F,
) -> Result<Result<PlannedShape, UnresolvedReason>, BakeError>
where
    F: FnMut(&Path) -> Result<T, BakeError>,
    T: Into<MorphData>,
{
    let Some(file) = data.file.as_deref() else {
        return Ok(Err(UnresolvedReason::Unrecorded));
    };
    let path = container_path(home, set, data, file);
    if !cache.contains_key(&path) {
        if !path.is_file() {
            return Ok(Err(UnresolvedReason::MissingFile(path)));
        }
        let loaded: MorphData = load(&path)?.into();
        cache.insert(path.clone(), loaded);
    }
    let block_name = data
        .record
        .clone()
        .unwrap_or_else(|| format!("{}{}", data.target, slider.name));
    match cache[&path].offsets(&block_name) {
        Some(offsets) => Ok(Ok(PlannedShape {
            target: data.target.clone(),
            block: block_name,
            offsets: offsets.to_vec(),
        })),
        None => Ok(Err(UnresolvedReason::MissingBlock(block_name))),
    }
}

/// Vertices one zap block deletes. @req BLD-01
///
/// Mirrors `GetDiffIndices` with threshold 0 (DiffData.cpp:556-580): an index
/// is kept when any delta component's magnitude is strictly positive, then the
/// output is sorted ascending and deduplicated.
fn zap_block_indices(offsets: &[VertexOffset]) -> Vec<u16> {
    let mut indices: Vec<u16> = offsets
        .iter()
        .filter(|offset| offset.delta.iter().any(|d| d.abs() > 0.0))
        .map(|offset| offset.index)
        .collect();
    indices.sort_unstable();
    indices.dedup();
    indices
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

    /// A CBBE/3BA-style set: one ordinary slider plus the authored coverage
    /// morph the clothed default looks for.
    const COVERAGE_PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Covered Body">
    <DataFolder>Covered</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Body">Body</Shape>
    <Slider name="Lean" small="0" big="0">
      <Data target="Body" local="true">body.bsd</Data>
    </Slider>
    <Slider name="NipBGone" small="0" big="0">
      <Data target="Body" local="true">cover.bsd</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

    fn set_of(document: &str) -> SliderSet {
        let Document::Project(mut project) = Document::parse_str(document, "inline.osp").unwrap()
        else {
            panic!("expected a project");
        };
        project.sets.pop().expect("one set")
    }

    #[test]
    fn default_coverage_applies_the_authored_morph_at_full_value() {
        let set = set_of(COVERAGE_PROJECT);
        let mut overrides = BTreeMap::new();
        let (state, percent) = apply_coverage(&set, None, &mut overrides).expect("default");
        assert_eq!(state, CoverageState::Clothed);
        assert_eq!(percent, Some(CLOTHED_COVERAGE_PERCENT));
        assert_eq!(
            overrides.get(COVERAGE_SLIDER),
            Some(&CLOTHED_COVERAGE_PERCENT),
            "the default override is merged for the resolver"
        );
    }

    #[test]
    fn explicit_nude_keeps_the_authored_values() {
        let set = set_of(COVERAGE_PROJECT);
        let mut overrides = BTreeMap::new();
        let (state, percent) = apply_coverage(&set, Some(Coverage::Nude), &mut overrides)
            .expect("explicit nude is always satisfiable");
        assert_eq!(state, CoverageState::Nude);
        assert_eq!(percent, None);
        assert!(
            overrides.is_empty(),
            "nude must not touch any slider, Isla keeps her authored 32"
        );
    }

    #[test]
    fn explicit_slider_value_wins_over_the_clothed_default() {
        let set = set_of(COVERAGE_PROJECT);
        let mut overrides = BTreeMap::from([(COVERAGE_SLIDER.to_string(), 37.0)]);
        let (state, percent) =
            apply_coverage(&set, Some(Coverage::Clothed), &mut overrides).expect("merges");
        assert_eq!(state, CoverageState::Clothed);
        assert_eq!(percent, Some(37.0));
        assert_eq!(overrides.get(COVERAGE_SLIDER), Some(&37.0));
    }

    #[test]
    fn explicit_clothed_without_the_morph_is_a_typed_refusal() {
        let set = set_of(PROJECT);
        let mut overrides = BTreeMap::new();
        let error = apply_coverage(&set, Some(Coverage::Clothed), &mut overrides)
            .expect_err("a set without the morph cannot honour clothed");
        assert!(matches!(
            error,
            BakeError::CoverageSliderAbsent { ref set, ref slider }
                if set == "Body" && slider == COVERAGE_SLIDER
        ));
        assert!(
            overrides.is_empty(),
            "a refused request must not leave a half-merged override"
        );
    }

    #[test]
    fn default_without_the_morph_bakes_as_authored() {
        let set = set_of(PROJECT);
        let mut overrides = BTreeMap::new();
        let (state, percent) = apply_coverage(&set, None, &mut overrides).expect("default");
        assert_eq!(state, CoverageState::Unauthored);
        assert_eq!(percent, None);
        assert!(overrides.is_empty());
    }
}
