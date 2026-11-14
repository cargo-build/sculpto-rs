//! Declared garment layer stack ("burger") grading.
//!
//! The declared layer model replaces a name-prefix inner/outer split with a
//! ranked manifest: every piece carries a sparse order over the body plus the
//! exceptions that make contact legitimate — an accessory anchored to one or
//! more named layers, footwear declared `under-trousers` or `over-trousers`.
//! Contact is classified by type, never by one distance, and only two types
//! fail a declared stack: `penetration` and `boot_mode_violation`.
//!
//! Which layers touch is declared, not inferred: a manifest may list its
//! layer-to-layer contacts as `pairs`, and only those are graded. Without the
//! list every strictly lower order is a candidate, and a candidate whose pieces
//! have disjoint bounding boxes is refused — a depth between pieces that cannot
//! meet is a far-surface artifact of the one-sided measurement, not a defect.
//! Orders are therefore only compared between layers a pair actually relates;
//! unpaired pieces may share a rank, and footwear has no fixed slot.
//!
//! Grading asks one question per graded pair: the outer piece's occlusion-aware
//! visible depth against the inner surface, at the caller's tolerance. The
//! classification is [`crate::check::garment_depth_report`]'s, so the stack
//! cannot drift from `garment depth`, and a pair is only ever measured
//! outer-against-inner, which is why an inner piece standing outside its outer
//! layer can never become a defect here. The declared body stays the innermost
//! surface: every layer is graded against it whether or not `pairs` is present.
//! @req DECL0015

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::check;
use crate::wrap::{FitError, Mesh};

/// One OBJ as owned display-frame positions plus triangles.
pub type Geometry = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Order of the body: every declared layer is graded against it.
pub const BODY_ORDER: u32 = 0;

/// How footwear relates to the trousers layer it pairs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootMode {
    /// The trousers cover the boot shaft, so the trousers are the outer piece
    /// and the boots are graded as its inner surface.
    UnderTrousers,
    /// The boots cover the trousers, so the boots are the outer piece and the
    /// trousers are the inner surface.
    OverTrousers,
}

impl BootMode {
    /// Wire name, the one the manifest declares and the report prints.
    pub fn id(self) -> &'static str {
        match self {
            Self::UnderTrousers => "under-trousers",
            Self::OverTrousers => "over-trousers",
        }
    }
}

/// One declared layer of the stack.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// Stable name anchors and `pair` references use, and the report prints.
    pub id: String,
    /// Rank over the body: any integer above `0`, compared only between layers
    /// a pair relates. Sparse ranks are the norm and footwear needs no slot.
    pub order: u32,
    /// Garment OBJ, resolved against the manifest's directory.
    pub obj: PathBuf,
    /// Lower layer(s) this accessory is declared to touch: one id or a list of
    /// ids in the manifest. Every anchored contact is reported as
    /// `accessory_contact` and never fails the stack.
    #[serde(default, deserialize_with = "one_or_many")]
    pub anchor: Option<Vec<String>>,
    /// Footwear placement; requires [`Self::pair`] naming the trousers layer.
    #[serde(default)]
    pub mode: Option<BootMode>,
    /// Trousers layer [`Self::mode`] is declared against.
    #[serde(default)]
    pub pair: Option<String>,
    /// Destination of this layer's fitted OBJ in a stack fit; resolved against
    /// the manifest's directory, and its parent directory is created when
    /// missing. Grading ignores it.
    #[serde(default)]
    pub out: Option<PathBuf>,
}

/// A declared stack: the body surface plus its ranked layers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerStack {
    /// Body/collision OBJ every layer is graded against; `--body` can supply it
    /// instead, and the two must agree when both are present.
    #[serde(default)]
    pub body: Option<PathBuf>,
    /// Layer-to-layer contacts as `[outer_id, inner_id]`; when present, only
    /// these are graded between layers. Absent means every strictly lower
    /// order, refused when the two pieces' bounding boxes are disjoint.
    #[serde(default)]
    pub pairs: Option<Vec<[String; 2]>>,
    /// Declared layers; empty is a refusal, not an empty pass.
    pub layers: Vec<Layer>,
}

/// `anchor` in the manifest: one id or a list of ids. Both spell one anchor
/// set, so an existing single-string manifest keeps parsing unchanged.
fn one_or_many<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Ids;

    impl<'de> serde::de::Visitor<'de> for Ids {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an anchor id or a list of anchor ids")
        }

        fn visit_str<E: serde::de::Error>(self, id: &str) -> Result<Self::Value, E> {
            Ok(vec![id.to_owned()])
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut ids = Vec::new();
            while let Some(id) = seq.next_element::<String>()? {
                ids.push(id);
            }
            Ok(ids)
        }
    }

    Ok(Some(deserializer.deserialize_any(Ids)?))
}

/// Why a declaration or a grading run cannot produce a stack report.
#[derive(Debug, Clone, PartialEq)]
pub enum StackError {
    /// The manifest does not declare a gradable stack.
    Declaration {
        /// What is wrong, naming the layer, order or reference at fault.
        message: String,
    },
    /// A declared OBJ could not be supplied by the loader.
    Load {
        /// Named path.
        path: PathBuf,
        /// Loader message.
        message: String,
    },
    /// The tolerance cannot state a depth bound.
    Tolerance {
        /// Rejected value.
        tolerance: f32,
    },
    /// Two order-implied layers cannot touch: their bounding boxes are
    /// disjoint, so a depth between them is a far-surface artifact.
    Disjoint {
        /// Implied outer layer.
        outer: String,
        /// Implied inner layer.
        inner: String,
    },
    /// The geometry of one pair could not be graded.
    Grade {
        /// Declared outer layer.
        outer: String,
        /// Declared inner surface (`body` for the body).
        inner: String,
        /// The shared fit-input refusal.
        error: FitError,
    },
}

impl fmt::Display for StackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declaration { message } => write!(formatter, "{message}"),
            Self::Load { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::Tolerance { tolerance } => write!(
                formatter,
                "`--tolerance` must be a finite non-negative number, got `{tolerance}`"
            ),
            Self::Disjoint { outer, inner } => write!(
                formatter,
                "implied pair `{outer}` vs `{inner}`: their bounding boxes do not overlap, so a depth between them is a far-surface artifact, not a penetration; declare `pairs` for the contacts that exist"
            ),
            Self::Grade {
                outer,
                inner,
                error,
            } => {
                write!(
                    formatter,
                    "cannot grade `{outer}` against `{inner}`: {error}"
                )
            }
        }
    }
}

impl std::error::Error for StackError {}

/// Contact classification of one declared pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactClass {
    /// The outer piece's graded surface is inside the inner surface beyond the
    /// tolerance: a poke-through.
    Penetration,
    /// No graded contact, or contact only where the outer piece's own surface
    /// covers it (DECL0015 hidden region).
    LayeringOk,
    /// An accessory touching the layer it is anchored to: allowed, reported.
    AccessoryContact,
    /// Footwear against its trousers layer outside the declared boot mode.
    BootModeViolation,
}

impl ContactClass {
    /// Wire name, the one the report prints.
    pub fn id(self) -> &'static str {
        match self {
            Self::Penetration => "penetration",
            Self::LayeringOk => "layering_ok",
            Self::AccessoryContact => "accessory_contact",
            Self::BootModeViolation => "boot_mode_violation",
        }
    }

    /// `true` for the two classes that fail a stack.
    pub fn fails(self) -> bool {
        matches!(self, Self::Penetration | Self::BootModeViolation)
    }

    /// Aggregation rank: a layer reports its worst pair class.
    fn severity(self) -> u8 {
        match self {
            Self::LayeringOk => 0,
            Self::AccessoryContact => 1,
            Self::BootModeViolation => 2,
            Self::Penetration => 3,
        }
    }
}

/// What made one declared pair contact-legitimate, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    /// A layer against the body.
    Body,
    /// Two declared layers with no declared exception.
    Layer,
    /// An accessory against the layer it is anchored to.
    Accessory,
    /// Footwear against its declared trousers layer.
    Boots,
}

impl PairKind {
    /// Wire name, the one the report prints.
    pub fn id(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Layer => "layer",
            Self::Accessory => "accessory",
            Self::Boots => "boots",
        }
    }
}

/// One declared pair and its measured contact classification.
#[derive(Debug, Clone, PartialEq)]
pub struct PairGrade {
    /// Declared outer layer.
    pub outer: String,
    /// Declared inner surface: a layer id, or `body`.
    pub inner: String,
    /// Declared relation between the two.
    pub kind: PairKind,
    /// Contact classification at the caller's tolerance.
    pub classification: ContactClass,
    /// Vertices of the graded piece (`garment_depth_report`'s query side).
    pub vertices: usize,
    /// Vertices behind the inner surface, before the facing/occlusion split.
    pub inside: usize,
    /// Inside vertices whose own normal faces into the inner surface: the
    /// authored inner side of a shell, not a poke-through.
    pub inner_facing: usize,
    /// Inside vertices the graded piece's own surface covers (DECL0015).
    pub hidden: usize,
    /// Graded (occlusion-aware visible) vertices.
    pub visible: usize,
    /// Deepest graded depth, in the input frame's units.
    pub visible_max: f32,
    /// Graded vertices above the tolerance, including those past `worst`.
    pub violations: usize,
    /// Deepest violating vertices as `(index into the graded piece, depth)`,
    /// at most [`check::WORST_VERTICES`].
    pub worst: Vec<(usize, f32)>,
    /// Accepted visible depth this pair was graded against.
    pub tolerance: f32,
}

/// One declared layer's aggregate over the pairs it declares.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerGrade {
    /// Declared id.
    pub id: String,
    /// Declared order.
    pub order: u32,
    /// Worst class among the pairs this layer declares as the outer piece.
    pub classification: ContactClass,
    /// Pairs this layer declares.
    pub pairs: usize,
    /// Pairs of those that fail.
    pub failed: usize,
    /// Deepest graded depth over those pairs.
    pub visible_max: f32,
}

/// Whole-stack grading result.
#[derive(Debug, Clone, PartialEq)]
pub struct StackReport {
    /// Accepted visible depth every pair was graded against.
    pub tolerance: f32,
    /// Per-layer rows, ascending order.
    pub layers: Vec<LayerGrade>,
    /// Per-pair rows, outer order ascending then inner order ascending.
    pub pairs: Vec<PairGrade>,
    /// `true` only when no pair is `penetration` and none is
    /// `boot_mode_violation`.
    pub pass: bool,
    /// Failing `penetration` pairs.
    pub penetrations: usize,
    /// Failing `boot_mode_violation` pairs.
    pub boot_violations: usize,
}

impl LayerStack {
    /// Read a manifest and resolve its declared paths against `manifest`'s
    /// directory, so a stack file is portable beside its meshes.
    ///
    /// # Errors
    /// [`StackError::Declaration`] for invalid JSON or a declaration
    /// [`Self::validate`] refuses.
    pub fn parse_json(text: &str, manifest: &Path) -> Result<Self, StackError> {
        let mut stack: Self =
            serde_json::from_str(text).map_err(|error| StackError::Declaration {
                message: format!("{}: invalid manifest: {error}", manifest.display()),
            })?;
        let directory = manifest.parent().unwrap_or_else(|| Path::new("."));
        let resolve = |path: &mut PathBuf| {
            if path.is_relative() {
                *path = directory.join(&*path);
            }
        };
        if let Some(body) = stack.body.as_mut() {
            resolve(body);
        }
        for layer in &mut stack.layers {
            resolve(&mut layer.obj);
            if let Some(out) = layer.out.as_mut() {
                resolve(out);
            }
        }
        stack.validate()?;
        Ok(stack)
    }

    /// Refuse a declaration that cannot be graded.
    ///
    /// Every layer's rank must be above the body's. Ranks are sparse and need
    /// not be unique: they are compared only where a pair relates two layers,
    /// so `reference` and every declared `pairs` entry must name a strictly
    /// lower rank. That also makes a reference cycle impossible — a self
    /// reference is the only one a manifest can express — and it is refused.
    ///
    /// # Errors
    /// [`StackError::Declaration`] naming the first defect found.
    pub fn validate(&self) -> Result<(), StackError> {
        if self.layers.is_empty() {
            return Err(declaration("the stack declares no layers"));
        }
        for (index, layer) in self.layers.iter().enumerate() {
            if layer.id.trim().is_empty() {
                return Err(declaration(format!("layer {index} has an empty id")));
            }
            if layer.order == BODY_ORDER {
                return Err(declaration(format!(
                    "layer `{}` declares order {BODY_ORDER}, which is the body",
                    layer.id
                )));
            }
            if let Some(earlier) = self.layers[..index]
                .iter()
                .find(|other| other.id == layer.id)
            {
                return Err(declaration(format!("duplicate layer id `{}`", earlier.id)));
            }
        }
        for layer in &self.layers {
            if layer.anchor.is_some() && layer.mode.is_some() {
                return Err(declaration(format!(
                    "layer `{}` declares both an anchor and a boot mode",
                    layer.id
                )));
            }
            let anchors = layer.anchor.as_deref().unwrap_or_default();
            if layer.anchor.is_some() && anchors.is_empty() {
                return Err(declaration(format!(
                    "layer `{}` declares an empty `anchor`",
                    layer.id
                )));
            }
            for (position, anchor) in anchors.iter().enumerate() {
                if anchors[..position].contains(anchor) {
                    return Err(declaration(format!(
                        "layer `{}` names anchor `{anchor}` twice",
                        layer.id
                    )));
                }
                let target = self.reference(layer, anchor, "anchor")?;
                if target == layer.id {
                    return Err(declaration(format!(
                        "layer `{}` anchors to itself",
                        layer.id
                    )));
                }
            }
            match (layer.mode, layer.pair.as_deref()) {
                (Some(_), None) => {
                    return Err(declaration(format!(
                        "layer `{}` declares a boot mode without `pair`",
                        layer.id
                    )));
                }
                (None, Some(pair)) => {
                    return Err(declaration(format!(
                        "layer `{pair}` is declared by `{}` without a boot mode",
                        layer.id
                    )));
                }
                (Some(_), Some(pair)) => {
                    self.reference(layer, pair, "pair")?;
                }
                (None, None) => {}
            }
        }
        self.pair_indices()?;
        if let Some(pairs) = &self.pairs {
            for layer in &self.layers {
                let mut contacts: Vec<&str> = layer
                    .anchor
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(String::as_str)
                    .collect();
                if let Some(pair) = layer.pair.as_deref() {
                    contacts.push(pair);
                }
                for contact in contacts {
                    let listed = pairs
                        .iter()
                        .any(|[outer, inner]| outer == &layer.id && inner == contact);
                    if !listed {
                        return Err(declaration(format!(
                            "layer `{}` declares a contact with `{contact}`, but `pairs` does not list it",
                            layer.id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// The layer indices of the declared `pairs`, refused unless every entry
    /// names two distinct layers with the outer strictly above the inner and no
    /// contact is listed twice.
    fn pair_indices(&self) -> Result<Vec<(usize, usize)>, StackError> {
        let Some(pairs) = &self.pairs else {
            return Ok(Vec::new());
        };
        let mut indices = Vec::with_capacity(pairs.len());
        for [outer, inner] in pairs {
            let find = |id: &str| {
                self.layers
                    .iter()
                    .position(|layer| layer.id == id)
                    .ok_or_else(|| {
                        declaration(format!(
                            "declared pair [{outer}, {inner}] names unknown layer `{id}`"
                        ))
                    })
            };
            let (outer_index, inner_index) = (find(outer)?, find(inner)?);
            if outer_index == inner_index {
                return Err(declaration(format!(
                    "declared pair [{outer}, {inner}] relates `{outer}` to itself"
                )));
            }
            let (outer_order, inner_order) = (
                self.layers[outer_index].order,
                self.layers[inner_index].order,
            );
            if outer_order <= inner_order {
                return Err(declaration(format!(
                    "declared pair [{outer}, {inner}]: outer `{outer}` (order {outer_order}) is not above inner `{inner}` (order {inner_order})"
                )));
            }
            if indices.contains(&(outer_index, inner_index)) {
                return Err(declaration(format!(
                    "declared pair [{outer}, {inner}] is listed twice"
                )));
            }
            indices.push((outer_index, inner_index));
        }
        Ok(indices)
    }

    /// The layer `reference` names, refused unless it exists below `layer`.
    fn reference(&self, layer: &Layer, reference: &str, field: &str) -> Result<&str, StackError> {
        let Some(target) = self.layers.iter().find(|other| other.id == reference) else {
            return Err(declaration(format!(
                "layer `{}` names unknown {field} `{reference}`",
                layer.id
            )));
        };
        if target.id != layer.id && target.order >= layer.order {
            return Err(declaration(format!(
                "layer `{}` (order {}) names {field} `{reference}` at order {}, which is not below it",
                layer.id, layer.order, target.order
            )));
        }
        Ok(target.id.as_str())
    }
}

/// The body surface to grade against: `--body` wins and must agree with a
/// declared manifest body; a manifest body alone is accepted.
///
/// # Errors
/// [`StackError::Declaration`] when neither source names a body or the two
/// disagree.
pub fn resolve_body(stack: &LayerStack, selected: Option<&Path>) -> Result<PathBuf, StackError> {
    match (stack.body.as_deref(), selected) {
        (None, None) => Err(declaration(
            "the stack declares no body: pass `--body` or set the manifest's `body`",
        )),
        (Some(declared), None) => Ok(declared.to_path_buf()),
        (None, Some(selected)) => Ok(selected.to_path_buf()),
        (Some(declared), Some(selected)) if declared == selected => Ok(declared.to_path_buf()),
        (Some(declared), Some(selected)) => Err(declaration(format!(
            "the manifest declares body {} but `--body` names {}",
            declared.display(),
            selected.display()
        ))),
    }
}

/// Grade a declared stack.
///
/// `load` supplies one OBJ per distinct declared path, so the caller owns file
/// format and the crate owns the measurement. Every layer is graded against the
/// body, which is the stack's innermost surface. Against other layers, the
/// manifest's `pairs` select the contacts when declared; otherwise every
/// strictly lower order is graded, and one whose bounding boxes are disjoint is
/// refused instead of measured. An accessory's pairs with its anchors are
/// reported as [`ContactClass::AccessoryContact`] and a footwear pair is graded
/// in the direction its declared mode names.
///
/// # Errors
/// [`StackError`] for a refused declaration, a tolerance that cannot name a
/// bound, a failed load, an order-implied pair that cannot touch, or ungradable
/// geometry.
pub fn grade<F>(
    stack: &LayerStack,
    body: &Path,
    tolerance: f32,
    mut load: F,
) -> Result<StackReport, StackError>
where
    F: FnMut(&Path) -> Result<Geometry, String>,
{
    stack.validate()?;
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(StackError::Tolerance { tolerance });
    }
    let mut cache: HashMap<PathBuf, Geometry> = HashMap::new();
    for path in std::iter::once(body).chain(stack.layers.iter().map(|layer| layer.obj.as_path())) {
        if cache.contains_key(path) {
            continue;
        }
        let geometry = load(path).map_err(|message| StackError::Load {
            path: path.to_path_buf(),
            message,
        })?;
        cache.insert(path.to_path_buf(), geometry);
    }

    let mut order: Vec<usize> = (0..stack.layers.len()).collect();
    order.sort_by_key(|&index| (stack.layers[index].order, index));
    let declared = stack.pair_indices()?;
    if stack.pairs.is_none() {
        for &outer in &order {
            for &inner in order
                .iter()
                .take_while(|&&index| stack.layers[index].order < stack.layers[outer].order)
            {
                let (outer_layer, inner_layer) = (&stack.layers[outer], &stack.layers[inner]);
                if disjoint(&cache[&outer_layer.obj], &cache[&inner_layer.obj]) {
                    return Err(StackError::Disjoint {
                        outer: outer_layer.id.clone(),
                        inner: inner_layer.id.clone(),
                    });
                }
            }
        }
    }
    let body_mesh = mesh(&cache, body);
    let mut pairs = Vec::new();
    let mut layers = Vec::with_capacity(order.len());
    for &outer in &order {
        let layer = &stack.layers[outer];
        let layer_mesh = mesh(&cache, &layer.obj);
        let mut grade = LayerGrade {
            id: layer.id.clone(),
            order: layer.order,
            classification: ContactClass::LayeringOk,
            pairs: 0,
            failed: 0,
            visible_max: 0.0,
        };
        let row = measure(
            &layer.id,
            "body",
            PairKind::Body,
            layer_mesh,
            body_mesh,
            tolerance,
        )?;
        absorb(&mut grade, &row);
        pairs.push(row);

        let mut inners: Vec<usize> = if stack.pairs.is_some() {
            declared
                .iter()
                .filter(|&&(declared_outer, _)| declared_outer == outer)
                .map(|&(_, inner)| inner)
                .collect()
        } else {
            order
                .iter()
                .copied()
                .take_while(|&index| stack.layers[index].order < layer.order)
                .collect()
        };
        inners.sort_by_key(|&index| (stack.layers[index].order, index));
        for inner in inners {
            let target = &stack.layers[inner];
            let target_mesh = mesh(&cache, &target.obj);
            let anchored = layer
                .anchor
                .as_deref()
                .is_some_and(|ids| ids.iter().any(|id| id == &target.id));
            let footwear = layer.pair.as_deref() == Some(target.id.as_str());
            let kind = match (anchored, footwear) {
                (true, _) => PairKind::Accessory,
                (_, true) => PairKind::Boots,
                _ => PairKind::Layer,
            };
            let row = match kind {
                PairKind::Boots if layer.mode == Some(BootMode::UnderTrousers) => measure(
                    &layer.id,
                    &target.id,
                    kind,
                    target_mesh,
                    layer_mesh,
                    tolerance,
                )?,
                _ => measure(
                    &layer.id,
                    &target.id,
                    kind,
                    layer_mesh,
                    target_mesh,
                    tolerance,
                )?,
            };
            absorb(&mut grade, &row);
            pairs.push(row);
        }
        layers.push(grade);
    }

    let penetrations = pairs
        .iter()
        .filter(|pair| pair.classification == ContactClass::Penetration)
        .count();
    let boot_violations = pairs
        .iter()
        .filter(|pair| pair.classification == ContactClass::BootModeViolation)
        .count();
    Ok(StackReport {
        tolerance,
        layers,
        pairs,
        pass: penetrations == 0 && boot_violations == 0,
        penetrations,
        boot_violations,
    })
}

/// One pair's measurement: depth, split and the class its kind allows.
fn measure(
    outer: &str,
    inner: &str,
    kind: PairKind,
    graded: Mesh<'_>,
    surface: Mesh<'_>,
    tolerance: f32,
) -> Result<PairGrade, StackError> {
    let report =
        check::garment_depth_report(graded, surface).map_err(|error| StackError::Grade {
            outer: outer.to_string(),
            inner: inner.to_string(),
            error,
        })?;
    let verdict = report.visible_verdict(tolerance);
    let classification = classify(kind, verdict.passed);
    let mut worst = verdict.violations;
    let violations = worst.len();
    worst.truncate(check::WORST_VERTICES);
    Ok(PairGrade {
        outer: outer.to_string(),
        inner: inner.to_string(),
        kind,
        classification,
        vertices: graded.0.len(),
        inside: report.inside,
        inner_facing: report.inner.count,
        hidden: report.hidden.count,
        visible: report.visible_occluded.count,
        visible_max: report.visible_occluded.max,
        violations,
        worst,
        tolerance,
    })
}

/// The class a pair earns: allowed relations keep their own name whether or
/// not the depth test passes, because a declared exception is reported, not
/// graded away.
fn classify(kind: PairKind, passed: bool) -> ContactClass {
    match (kind, passed) {
        (PairKind::Accessory, _) => ContactClass::AccessoryContact,
        (PairKind::Boots, false) => ContactClass::BootModeViolation,
        (_, false) => ContactClass::Penetration,
        (_, true) => ContactClass::LayeringOk,
    }
}

/// Fold one pair row into its layer's aggregate.
fn absorb(grade: &mut LayerGrade, row: &PairGrade) {
    grade.pairs += 1;
    if row.classification.fails() {
        grade.failed += 1;
    }
    if row.classification.severity() > grade.classification.severity() {
        grade.classification = row.classification;
    }
    if row.visible_max > grade.visible_max {
        grade.visible_max = row.visible_max;
    }
}

/// Borrow one loaded geometry as a queryable mesh.
fn mesh<'a>(cache: &'a HashMap<PathBuf, Geometry>, path: &Path) -> Mesh<'a> {
    let (positions, triangles) = &cache[path];
    (positions, triangles)
}

/// Axis-aligned bounds of one geometry, or `None` when it has no vertices; the
/// grader reports an empty mesh as a geometry refusal, so the caller defers.
fn bounds(geometry: &Geometry) -> Option<([f32; 3], [f32; 3])> {
    let first = *geometry.0.first()?;
    let (mut min, mut max) = (first, first);
    for position in &geometry.0[1..] {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    Some((min, max))
}

/// Whether two geometries' axis-aligned boxes are disjoint on any axis. Pieces
/// whose boxes miss each other cannot meet, whatever the nearest-triangle side
/// says. An empty mesh is not judged here.
fn disjoint(outer: &Geometry, inner: &Geometry) -> bool {
    let (Some((outer_min, outer_max)), Some((inner_min, inner_max))) =
        (bounds(outer), bounds(inner))
    else {
        return false;
    };
    (0..3).any(|axis| outer_max[axis] < inner_min[axis] || inner_max[axis] < outer_min[axis])
}

/// Build a declaration refusal.
fn declaration(message: impl Into<String>) -> StackError {
    StackError::Declaration {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outward-wound cube of half-extent `half` around `center`.
    fn cube(center: [f32; 3], half: f32) -> Geometry {
        let [cx, cy, cz] = center;
        let positions = vec![
            [cx - half, cy - half, cz - half],
            [cx + half, cy - half, cz - half],
            [cx + half, cy + half, cz - half],
            [cx - half, cy + half, cz - half],
            [cx - half, cy - half, cz + half],
            [cx + half, cy - half, cz + half],
            [cx + half, cy + half, cz + half],
            [cx - half, cy + half, cz + half],
        ];
        let triangles = vec![
            [4u32, 5, 6],
            [4, 6, 7],
            [1, 0, 3],
            [1, 3, 2],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
            [3, 7, 6],
            [3, 6, 2],
            [0, 1, 5],
            [0, 5, 4],
        ];
        (positions, triangles)
    }

    /// One sheet facing `+x` at `x`, so its depth against a cube is readable.
    fn sheet(x: f32) -> Geometry {
        (
            vec![[x, 0.0, 0.0], [x, 0.1, 0.0], [x, 0.0, 0.1]],
            vec![[0u32, 1, 2]],
        )
    }

    fn files(pairs: &[(&str, Geometry)]) -> HashMap<String, Geometry> {
        pairs
            .iter()
            .map(|(name, geometry)| ((*name).to_string(), geometry.clone()))
            .collect()
    }

    fn loader(
        files: &HashMap<String, Geometry>,
    ) -> impl FnMut(&Path) -> Result<Geometry, String> + '_ {
        move |path: &Path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            files
                .get(&name)
                .cloned()
                .ok_or_else(|| format!("missing fixture `{name}`"))
        }
    }

    fn stack(layers: &str) -> LayerStack {
        let text = format!(r#"{{"body": "body.obj", "layers": [{layers}]}}"#);
        LayerStack::parse_json(&text, Path::new("/fixtures/stack.json")).unwrap()
    }

    fn stack_with_pairs(layers: &str, pairs: &str) -> LayerStack {
        let text = format!(r#"{{"body": "body.obj", "pairs": {pairs}, "layers": [{layers}]}}"#);
        LayerStack::parse_json(&text, Path::new("/fixtures/stack.json")).unwrap()
    }

    /// Grade a stack that must be refused, returning the refusal message.
    fn refusal(stack: &LayerStack, files: &HashMap<String, Geometry>, tolerance: f32) -> String {
        let body = resolve_body(stack, None).unwrap();
        grade(stack, &body, tolerance, loader(files))
            .expect_err("the stack was graded instead of refused")
            .to_string()
    }

    fn graded(
        stack: &LayerStack,
        files: &HashMap<String, Geometry>,
        tolerance: f32,
    ) -> StackReport {
        let body = resolve_body(stack, None).unwrap();
        grade(stack, &body, tolerance, loader(files)).unwrap()
    }

    fn row<'a>(report: &'a StackReport, outer: &str, inner: &str) -> &'a PairGrade {
        report
            .pairs
            .iter()
            .find(|pair| pair.outer == outer && pair.inner == inner)
            .unwrap_or_else(|| panic!("no `{outer}` vs `{inner}` pair"))
    }

    #[test]
    fn nested_shells_layer_cleanly() {
        let stack = stack(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "tunic", "order": 3, "obj": "tunic.obj"},
               {"id": "robe", "order": 4, "obj": "robe.obj"}"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("panty.obj", cube([0.0; 3], 0.5)),
            ("tunic.obj", cube([0.0; 3], 0.6)),
            ("robe.obj", cube([0.0; 3], 0.7)),
        ]);
        let report = graded(&stack, &files, 0.05);

        assert!(report.pass, "{:?}", report.pairs);
        assert_eq!(report.pairs.len(), 6, "every outer/inner pair once");
        for pair in &report.pairs {
            assert_eq!(pair.classification, ContactClass::LayeringOk, "{pair:?}");
            assert_eq!(pair.visible, 0, "{pair:?}");
        }
        // A pair is only measured outer-against-inner, so the outermost layer is
        // never the graded inner surface of anything.
        assert!(report.pairs.iter().all(|pair| pair.inner != "robe"));
        let declared: Vec<(&str, &str)> = report
            .pairs
            .iter()
            .map(|pair| (pair.outer.as_str(), pair.inner.as_str()))
            .collect();
        assert_eq!(
            declared,
            [
                ("panty", "body"),
                ("tunic", "body"),
                ("tunic", "panty"),
                ("robe", "body"),
                ("robe", "panty"),
                ("robe", "tunic"),
            ]
        );
        let rows: Vec<(&str, u32)> = report
            .layers
            .iter()
            .map(|layer| (layer.id.as_str(), layer.order))
            .collect();
        assert_eq!(rows, [("panty", 1), ("tunic", 3), ("robe", 4)]);
        assert_eq!(report.layers[2].pairs, 3, "body plus two inner layers");
    }

    #[test]
    fn forced_inside_inner_layer_is_penetration() {
        let stack = stack(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "tunic", "order": 3, "obj": "tunic.obj"}"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("panty.obj", cube([0.0; 3], 0.5)),
            // Outside the body by 0.02 and inside the panty by 0.08.
            ("tunic.obj", sheet(0.42)),
        ]);
        let report = graded(&stack, &files, 0.05);

        assert!(!report.pass);
        assert_eq!(report.penetrations, 1);
        assert_eq!(report.boot_violations, 0);
        assert_eq!(
            row(&report, "tunic", "body").classification,
            ContactClass::LayeringOk
        );
        let pair = row(&report, "tunic", "panty");
        assert_eq!(pair.classification, ContactClass::Penetration);
        assert_eq!(pair.visible, 3);
        assert_eq!(pair.violations, 3);
        assert_eq!(pair.worst.len(), 3);
        assert!(
            (pair.visible_max - 0.08).abs() < 1e-5,
            "{}",
            pair.visible_max
        );
        assert_eq!(report.layers[1].classification, ContactClass::Penetration);
        assert_eq!(report.layers[1].failed, 1);
    }

    #[test]
    fn accessory_contact_is_allowed_and_reported() {
        // The accessory declares the only layer contact it has; the panty is
        // graded against the body but never against a belt it cannot meet.
        let anchored = stack_with_pairs(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "tunic", "order": 3, "obj": "tunic.obj"},
               {"id": "belt", "order": 6, "obj": "belt.obj", "anchor": "tunic"}"#,
            r#"[["belt", "tunic"]]"#,
        );
        let loose = stack_with_pairs(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "tunic", "order": 3, "obj": "tunic.obj"},
               {"id": "belt", "order": 6, "obj": "belt.obj"}"#,
            r#"[["belt", "tunic"]]"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("panty.obj", cube([0.0; 3], 0.5)),
            ("tunic.obj", cube([0.0; 3], 0.6)),
            // Outside the panty and 0.06 inside the tunic.
            ("belt.obj", sheet(0.54)),
        ]);

        let allowed = graded(&anchored, &files, 0.05);
        assert!(allowed.pass, "{:?}", allowed.pairs);
        let pair = row(&allowed, "belt", "tunic");
        assert_eq!(pair.kind, PairKind::Accessory);
        assert_eq!(pair.classification, ContactClass::AccessoryContact);
        assert_eq!(pair.visible, 3);
        assert_eq!(pair.violations, 3, "reported, not hidden");
        assert!(
            (pair.visible_max - 0.06).abs() < 1e-5,
            "{}",
            pair.visible_max
        );
        assert!(
            !allowed
                .pairs
                .iter()
                .any(|pair| pair.outer == "belt" && pair.inner == "panty"),
            "the undeclared belt/panty contact is not graded: {:?}",
            allowed.pairs
        );
        assert_eq!(
            row(&allowed, "belt", "body").classification,
            ContactClass::LayeringOk
        );
        assert_eq!(
            allowed.layers[2].classification,
            ContactClass::AccessoryContact
        );

        // The same geometry without the declaration is a poke-through: the
        // anchor is what makes the contact legal, not the depth.
        let undeclared = graded(&loose, &files, 0.05);
        assert!(!undeclared.pass);
        assert_eq!(
            row(&undeclared, "belt", "tunic").classification,
            ContactClass::Penetration
        );
    }

    #[test]
    fn every_declared_anchor_is_an_accessory_contact() {
        // The brooch sits over both the robe and the cloak above it: one anchor
        // would grade the cloak row as a penetration.
        let anchored = stack_with_pairs(
            r#"{"id": "robe", "order": 4, "obj": "robe.obj"},
               {"id": "cloak", "order": 5, "obj": "cloak.obj"},
               {"id": "brooch", "order": 6, "obj": "brooch.obj",
                "anchor": ["robe", "cloak"]}"#,
            r#"[["brooch", "robe"], ["brooch", "cloak"]]"#,
        );
        // One anchor dropped while `pairs` still lists both contacts.
        let half = stack_with_pairs(
            r#"{"id": "robe", "order": 4, "obj": "robe.obj"},
               {"id": "cloak", "order": 5, "obj": "cloak.obj"},
               {"id": "brooch", "order": 6, "obj": "brooch.obj", "anchor": "robe"}"#,
            r#"[["brooch", "robe"], ["brooch", "cloak"]]"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("robe.obj", cube([0.0; 3], 0.6)),
            ("cloak.obj", cube([0.0; 3], 0.7)),
            // Outside the body, 0.14 inside the robe and 0.24 inside the cloak.
            ("brooch.obj", sheet(0.46)),
        ]);

        let allowed = graded(&anchored, &files, 0.05);
        assert!(allowed.pass, "{:?}", allowed.pairs);
        let robe = row(&allowed, "brooch", "robe");
        assert_eq!(robe.kind, PairKind::Accessory);
        assert_eq!(robe.classification, ContactClass::AccessoryContact);
        assert_eq!(robe.visible, 3);
        assert_eq!(robe.violations, 3, "reported, not hidden");
        assert!(
            (robe.visible_max - 0.14).abs() < 1e-5,
            "{}",
            robe.visible_max
        );
        let cloak = row(&allowed, "brooch", "cloak");
        assert_eq!(cloak.kind, PairKind::Accessory);
        assert_eq!(cloak.classification, ContactClass::AccessoryContact);
        assert_eq!(cloak.visible, 3);
        assert_eq!(cloak.violations, 3);
        assert!(
            (cloak.visible_max - 0.24).abs() < 1e-5,
            "{}",
            cloak.visible_max
        );
        assert_eq!(
            row(&allowed, "brooch", "body").classification,
            ContactClass::LayeringOk
        );
        assert_eq!(
            allowed.layers[2].classification,
            ContactClass::AccessoryContact
        );
        assert_eq!(allowed.layers[2].failed, 0);

        // Dropping the cloak anchor reverts only that row: the declaration,
        // not the depth, exempts, one anchor at a time.
        let half_anchored = graded(&half, &files, 0.05);
        assert!(!half_anchored.pass);
        assert_eq!(
            row(&half_anchored, "brooch", "robe").classification,
            ContactClass::AccessoryContact
        );
        let dropped = row(&half_anchored, "brooch", "cloak");
        assert_eq!(dropped.kind, PairKind::Layer);
        assert_eq!(dropped.classification, ContactClass::Penetration);
        assert!(
            (dropped.visible_max - 0.24).abs() < 1e-5,
            "{}",
            dropped.visible_max
        );
    }

    #[test]
    fn single_string_anchor_parses_and_grades_like_the_list() {
        let single = stack_with_pairs(
            r#"{"id": "robe", "order": 4, "obj": "robe.obj"},
               {"id": "brooch", "order": 6, "obj": "brooch.obj", "anchor": "robe"}"#,
            r#"[["brooch", "robe"]]"#,
        );
        let listed = stack_with_pairs(
            r#"{"id": "robe", "order": 4, "obj": "robe.obj"},
               {"id": "brooch", "order": 6, "obj": "brooch.obj", "anchor": ["robe"]}"#,
            r#"[["brooch", "robe"]]"#,
        );
        assert_eq!(single, listed, "both spellings parse to one declaration");
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("robe.obj", cube([0.0; 3], 0.6)),
            ("brooch.obj", sheet(0.46)),
        ]);
        assert_eq!(
            graded(&single, &files, 0.05),
            graded(&listed, &files, 0.05),
            "both spellings grade identically"
        );
    }

    #[test]
    fn explicit_pairs_override_the_order_rule() {
        // `cuff` sits ten units away from every other layer: the order rule
        // would compare it with all of them and report far-surface depths.
        let stack = stack_with_pairs(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "tunic", "order": 3, "obj": "tunic.obj"},
               {"id": "robe", "order": 4, "obj": "robe.obj"},
               {"id": "cuff", "order": 9, "obj": "cuff.obj"}"#,
            r#"[["tunic", "panty"], ["robe", "tunic"]]"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("panty.obj", cube([0.0; 3], 0.5)),
            ("tunic.obj", cube([0.0; 3], 0.6)),
            ("robe.obj", cube([0.0; 3], 0.7)),
            ("cuff.obj", cube([10.0, 0.0, 0.0], 0.3)),
        ]);
        let report = graded(&stack, &files, 0.05);

        assert!(report.pass, "{:?}", report.pairs);
        let declared: Vec<(&str, &str)> = report
            .pairs
            .iter()
            .map(|pair| (pair.outer.as_str(), pair.inner.as_str()))
            .collect();
        assert_eq!(
            declared,
            [
                ("panty", "body"),
                ("tunic", "body"),
                ("tunic", "panty"),
                ("robe", "body"),
                ("robe", "tunic"),
                ("cuff", "body"),
            ],
            "declared contacts plus the body, nothing implied by order"
        );
        // `robe` must not be graded against the panty, and `cuff` against
        // nothing above the body: far-surface rows are gone, not excused.
        assert!(
            !report
                .pairs
                .iter()
                .any(|pair| pair.outer == "cuff" && pair.inner != "body")
        );
        assert_eq!(report.layers[3].pairs, 1, "cuff: body only");
        // The body stays the innermost surface in declared mode.
        assert_eq!(
            row(&report, "cuff", "body").classification,
            ContactClass::LayeringOk
        );
    }

    #[test]
    fn nonoverlapping_implied_pair_is_refused() {
        let implied = stack(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "cuff", "order": 2, "obj": "cuff.obj"}"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("panty.obj", cube([0.0; 3], 0.5)),
            ("cuff.obj", cube([10.0, 0.0, 0.0], 0.3)),
        ]);
        let error = refusal(&implied, &files, 0.05);
        assert!(
            error.contains("`cuff` vs `panty`"),
            "the refused pair is named: {error}"
        );
        assert!(error.contains("bounding boxes do not overlap"), "{error}");

        // The same pieces are gradable when the manifest declares the contacts
        // that exist: the body, and nothing between them.
        let declared = stack_with_pairs(
            r#"{"id": "panty", "order": 1, "obj": "panty.obj"},
               {"id": "cuff", "order": 2, "obj": "cuff.obj"}"#,
            "[]",
        );
        let report = graded(&declared, &files, 0.05);
        assert!(report.pass, "{:?}", report.pairs);
        assert_eq!(report.pairs.len(), 2, "both layers against the body");
    }

    #[test]
    fn sparse_orders_and_shared_ranks_are_accepted() {
        // Footwear above the old slot, two pieces sharing a rank: both are
        // legal because no pair relates them to a layer at the same rank.
        let stack = stack_with_pairs(
            r#"{"id": "trousers", "order": 2, "obj": "trousers.obj"},
               {"id": "boots", "order": 12, "obj": "boots.obj", "mode": "over-trousers", "pair": "trousers"},
               {"id": "left_sleeve", "order": 3, "obj": "left.obj"},
               {"id": "right_sleeve", "order": 3, "obj": "right.obj"}"#,
            r#"[["boots", "trousers"]]"#,
        );
        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("trousers.obj", cube([0.0; 3], 0.5)),
            ("boots.obj", cube([0.0; 3], 0.6)),
            ("left.obj", sheet(-0.52)),
            ("right.obj", sheet(0.52)),
        ]);
        let report = graded(&stack, &files, 0.05);

        assert!(report.pass, "{:?}", report.pairs);
        let rows: Vec<(&str, u32)> = report
            .layers
            .iter()
            .map(|layer| (layer.id.as_str(), layer.order))
            .collect();
        assert_eq!(
            rows,
            [
                ("trousers", 2),
                ("left_sleeve", 3),
                ("right_sleeve", 3),
                ("boots", 12),
            ]
        );
        assert_eq!(
            row(&report, "boots", "trousers").classification,
            ContactClass::LayeringOk
        );
        assert_eq!(report.pairs.len(), 5, "four body rows plus one declared");
    }

    #[test]
    fn boot_mode_decides_the_graded_direction() {
        let over = stack(
            r#"{"id": "trousers", "order": 2, "obj": "trousers.obj"},
               {"id": "boots", "order": 7, "obj": "boots.obj", "mode": "over-trousers", "pair": "trousers"}"#,
        );
        let under = stack(
            r#"{"id": "trousers", "order": 2, "obj": "trousers.obj"},
               {"id": "boots", "order": 7, "obj": "boots.obj", "mode": "under-trousers", "pair": "trousers"}"#,
        );
        let body = cube([0.0; 3], 0.4);
        let wide = cube([0.0; 3], 0.55);
        let narrow = cube([0.0; 3], 0.5);

        // Boots under a wider trouser leg by 0.05: the boots are inside the
        // trousers, which `over-trousers` forbids and `under-trousers` requires.
        let boots_inside = files(&[
            ("body.obj", body.clone()),
            ("trousers.obj", wide.clone()),
            ("boots.obj", narrow.clone()),
        ]);
        let violation = graded(&over, &boots_inside, 0.03);
        assert!(!violation.pass);
        assert_eq!(violation.boot_violations, 1);
        assert_eq!(violation.penetrations, 0);
        let pair = row(&violation, "boots", "trousers");
        assert_eq!(pair.kind, PairKind::Boots);
        assert_eq!(pair.classification, ContactClass::BootModeViolation);
        assert!(pair.visible > 0, "{pair:?}");
        assert!(
            (pair.visible_max - 0.05).abs() < 1e-5,
            "{}",
            pair.visible_max
        );

        let declared = graded(&under, &boots_inside, 0.03);
        assert!(declared.pass, "{:?}", declared.pairs);
        assert_eq!(
            row(&declared, "boots", "trousers").classification,
            ContactClass::LayeringOk
        );
        assert_eq!(row(&declared, "boots", "trousers").visible, 0);

        // Trousers inside a wider boot shaft: the reverse case, same verdicts.
        let trousers_inside = files(&[
            ("body.obj", body),
            ("trousers.obj", narrow),
            ("boots.obj", wide),
        ]);
        let violation = graded(&under, &trousers_inside, 0.03);
        assert!(!violation.pass, "{:?}", violation.pairs);
        assert_eq!(
            row(&violation, "boots", "trousers").classification,
            ContactClass::BootModeViolation
        );
        let declared = graded(&over, &trousers_inside, 0.03);
        assert!(declared.pass, "{:?}", declared.pairs);
        assert_eq!(row(&declared, "boots", "trousers").visible, 0);
    }

    #[test]
    fn malformed_manifests_are_refused() {
        let cases = [
            (r#"{"layers": []}"#.to_string(), "declares no layers"),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"},
                              {"id": "a", "order": 2, "obj": "b.obj"}]}"#
                    .to_string(),
                "duplicate layer id `a`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj", "anchor": "ghost"}]}"#
                    .to_string(),
                "unknown anchor `ghost`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj", "anchor": "a"}]}"#
                    .to_string(),
                "anchors to itself",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"},
                              {"id": "b", "order": 2, "obj": "b.obj", "anchor": "c"}]}"#
                    .to_string(),
                "unknown anchor `c`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"},
                              {"id": "b", "order": 3, "obj": "b.obj", "mode": "over-trousers"}]}"#
                    .to_string(),
                "boot mode without `pair`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"},
                              {"id": "b", "order": 3, "obj": "b.obj", "pair": "a"}]}"#
                    .to_string(),
                "without a boot mode",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 0, "obj": "a.obj"}]}"#.to_string(),
                "which is the body",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj", "colour": "red"}]}"#
                    .to_string(),
                "unknown field `colour`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj", "anchor": "b"},
                              {"id": "b", "order": 2, "obj": "b.obj", "anchor": "a"}]}"#
                    .to_string(),
                "not below it",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj", "anchor": 5}]}"#.to_string(),
                "expected an anchor id or a list of anchor ids",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj", "anchor": []}]}"#
                    .to_string(),
                "declares an empty `anchor`",
            ),
            (
                r#"{"layers": [{"id": "b", "order": 2, "obj": "b.obj"},
                              {"id": "c", "order": 6, "obj": "c.obj", "anchor": ["b", "b"]}],
                   "pairs": [["c", "b"]]}"#
                    .to_string(),
                "names anchor `b` twice",
            ),
            (
                r#"{"layers": [{"id": "b", "order": 4, "obj": "b.obj"},
                              {"id": "c", "order": 5, "obj": "c.obj"},
                              {"id": "d", "order": 6, "obj": "d.obj",
                               "anchor": ["b", "c"]}],
                   "pairs": [["d", "b"]]}"#
                    .to_string(),
                "declares a contact with `c`, but `pairs` does not list it",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 2, "obj": "a.obj"}],
                    "pairs": [["ghost", "a"]]}"#
                    .to_string(),
                "names unknown layer `ghost`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 2, "obj": "a.obj"}],
                    "pairs": [["a", "ghost"]]}"#
                    .to_string(),
                "names unknown layer `ghost`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"},
                              {"id": "b", "order": 2, "obj": "b.obj"}],
                    "pairs": [["a", "b"]]}"#
                    .to_string(),
                "is not above inner `b`",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 2, "obj": "a.obj"}],
                    "pairs": [["a", "a"]]}"#
                    .to_string(),
                "relates `a` to itself",
            ),
            (
                r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"},
                              {"id": "b", "order": 2, "obj": "b.obj"}],
                    "pairs": [["b", "a"], ["b", "a"]]}"#
                    .to_string(),
                "is listed twice",
            ),
            (
                r#"{"layers": [{"id": "tunic", "order": 3, "obj": "tunic.obj"},
                              {"id": "belt", "order": 6, "obj": "belt.obj", "anchor": "tunic"}],
                    "pairs": []}"#
                    .to_string(),
                "but `pairs` does not list it",
            ),
            (
                r#"{"layers": [{"id": "trousers", "order": 2, "obj": "trousers.obj"},
                              {"id": "boots", "order": 9, "obj": "boots.obj",
                               "mode": "over-trousers", "pair": "trousers"}],
                    "pairs": []}"#
                    .to_string(),
                "but `pairs` does not list it",
            ),
        ];
        for (text, expected) in cases {
            let error = LayerStack::parse_json(&text, Path::new("/fixtures/stack.json"))
                .expect_err(&format!("accepted `{text}`"))
                .to_string();
            assert!(
                error.contains(expected),
                "`{text}` gave `{error}`, wanted `{expected}`"
            );
        }
    }

    #[test]
    fn body_and_tolerance_must_be_named() {
        let text = r#"{"layers": [{"id": "a", "order": 1, "obj": "a.obj"}]}"#;
        let stack = LayerStack::parse_json(text, Path::new("/fixtures/stack.json")).unwrap();
        let error = resolve_body(&stack, None).unwrap_err().to_string();
        assert!(error.contains("declares no body"), "{error}");
        assert_eq!(
            resolve_body(&stack, Some(Path::new("/fixtures/body.obj"))).unwrap(),
            PathBuf::from("/fixtures/body.obj")
        );

        let both = r#"{"body": "body.obj", "layers": [{"id": "a", "order": 1, "obj": "a.obj"}]}"#;
        let stack = LayerStack::parse_json(both, Path::new("/fixtures/stack.json")).unwrap();
        assert_eq!(
            resolve_body(&stack, Some(Path::new("/fixtures/body.obj"))).unwrap(),
            PathBuf::from("/fixtures/body.obj")
        );
        let error = resolve_body(&stack, Some(Path::new("/elsewhere/body.obj")))
            .unwrap_err()
            .to_string();
        assert!(error.contains("`--body` names"), "{error}");

        let files = files(&[
            ("body.obj", cube([0.0; 3], 0.4)),
            ("a.obj", cube([0.0; 3], 0.5)),
        ]);
        let error = grade(
            &stack,
            Path::new("/fixtures/body.obj"),
            -1.0,
            loader(&files),
        )
        .unwrap_err();
        assert!(matches!(error, StackError::Tolerance { .. }), "{error:?}");
        let error = grade(
            &stack,
            Path::new("/fixtures/missing.obj"),
            0.1,
            loader(&files),
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing fixture"), "{error}");
    }

    #[test]
    fn manifest_paths_resolve_beside_the_manifest() {
        let text = r#"{"body": "meshes/body.obj",
                       "layers": [{"id": "a", "order": 1, "obj": "meshes/a.obj"},
                                  {"id": "b", "order": 2, "obj": "/abs/b.obj"}]}"#;
        let stack = LayerStack::parse_json(text, Path::new("/work/outfit/stack.json")).unwrap();
        assert_eq!(
            stack.body,
            Some(PathBuf::from("/work/outfit/meshes/body.obj"))
        );
        assert_eq!(
            stack.layers[0].obj,
            PathBuf::from("/work/outfit/meshes/a.obj")
        );
        assert_eq!(stack.layers[1].obj, PathBuf::from("/abs/b.obj"));
    }
}
