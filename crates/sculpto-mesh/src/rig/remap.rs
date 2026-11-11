//! Neutral skin remap from a checked source binding onto a checked target rig.
//!
//! The transfer keeps the source's skin-local vertices and global-to-skin
//! matrix, resolves every positive influence through the existing
//! [`BoneMap`](crate::rig::map::BoneMap), and encodes one bind per destination
//! as `inverse(Bt*Wt)*inverse(Gs)` so the output proves at rest under the
//! target's own basis `Bt` — never a substitute basis. The strict four-slot
//! cap and normalization stay with
//! [`pack_influences`](crate::weight_norm::pack_influences). @req DECL0008

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use glam::DMat4;
use sculpto_core::mesh::Skin;
use sculpto_core::rig::rest::{
    BoundRestSkin, RestBindError, RestBinding, RestMeasurement, SkinRestSource,
};
use sculpto_core::rig::{JointId, Rig, RigError};
use sculpto_core::skin::{SkinWeightError, SkinWeights};

use crate::garment::Influences;
use crate::rig::map::{BoneMap, BoneMapError, Destination};
use crate::weight_norm::{WeightError, pack_influences};

/// Required shape selections and source joint locks for one remap.
///
/// Every field is declared by the caller: a missing or ambiguous shape
/// selection is a refusal, never a default.
#[derive(Debug, Clone)]
pub struct RemapSettings {
    /// Identity of the source shape to transfer.
    pub source_shape: usize,
    /// Identity of the target reference shape, which supplies the output
    /// skeleton root and the rest context of the transfer.
    pub target_shape: usize,
    /// Source joints whose positive weights remain bit-exact and whose
    /// zero weights remain zero.
    pub locked_source_joints: Vec<JointId>,
}

/// One checked skin transferred onto the target graph.
#[derive(Debug, Clone, PartialEq)]
pub struct RemappedSkin<'a> {
    /// Target graph that owns the output palette identities.
    pub rig: &'a Rig,
    /// Skeleton subtree the output palette's joints lie under.
    pub root: JointId,
    /// Source global-to-skin matrix, kept unchanged.
    pub global_to_skin: [[f32; 4]; 4],
    /// Encoded output skin over the target graph's joint names.
    pub skin: Skin,
    /// Collapse, discard and rest evidence of the transfer.
    pub report: RemapReport,
}

/// Evidence of one checked skin remap, in raw source weight units.
///
/// Source rows may sum to other values than one, so every reported mass is a
/// raw pre-pack weight, never a normalized share.
#[derive(Debug, Clone, PartialEq)]
pub struct RemapReport {
    /// Rest residuals of the encoded output measured at the target basis.
    pub measurement: RestMeasurement,
    /// Destinations reached through explicit collapses or merged source weights.
    pub collapses: Vec<CollapsedWeights>,
    /// Positive aggregated destination weights the four-slot pack dropped.
    pub discarded: Vec<DiscardedWeight>,
}

/// Positive weights routed through a declared collapse or merge at one vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct CollapsedWeights {
    /// Vertex carrying the merged influence.
    pub vertex: usize,
    /// Destination joint, in the target graph.
    pub destination: JointId,
    /// Contributing source joints with their raw weights, in source id order.
    pub sources: Vec<(JointId, f32)>,
    /// Sum of the contributing raw weights, in source weight units.
    pub source_sum: f32,
}

/// One positive aggregated destination weight lost to the four-slot cap.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscardedWeight {
    /// Vertex that lost the influence.
    pub vertex: usize,
    /// Destination joint, in the target graph.
    pub destination: JointId,
    /// Raw pre-pack aggregated weight, in source weight units; source rows
    /// may be non-unit, so this is not a normalized share.
    pub weight: f32,
}

/// Why a checked skin could not be remapped onto the target rig.
#[derive(Debug, Clone, PartialEq)]
pub enum RemapError {
    /// A map graph owner is not the rig of the supplied binding.
    GraphOwner { side: GraphSide },
    /// The requested shape is absent from a binding.
    MissingShape { side: GraphSide, shape: usize },
    /// A binding declares the requested shape identity twice.
    DuplicateShape { side: GraphSide, shape: usize },
    /// A locked source joint is outside the source rig.
    UnknownLock { joint: JointId, joints: usize },
    /// A source joint is locked twice.
    DuplicateLock { joint: JointId },
    /// A positive source joint has no supported destination.
    Map(BoneMapError),
    /// A locked source joint shares its destination with another source
    /// weight, so its independent value cannot survive the pack.
    LockCollapse {
        vertex: usize,
        locked: JointId,
        destination: JointId,
    },
    /// The strict four-slot pack refused a vertex row.
    Pack { vertex: usize, error: WeightError },
    /// The encoded output rows are not a valid skin.
    Encode(SkinWeightError),
    /// A target graph query failed.
    Rig(RigError),
    /// The encoded output failed its explicit-basis rest proof.
    Rest(RestBindError),
}

/// Which bound graph or shape selection a refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphSide {
    /// The source binding's graph or shape.
    Source,
    /// The target binding's graph or shape.
    Target,
}

/// One vertex's aggregated destination weights and locked destinations.
struct VertexTransfer {
    vertex: usize,
    aggregated: BTreeMap<JointId, f32>,
    locked: BTreeSet<JointId>,
}

/// Transfer a checked source skin onto the target rig through `map`. @req DECL0008
///
/// The output keeps the source vertices and global-to-skin matrix, encodes
/// `inverse(Bt*Wt)*inverse(Gs)` per destination joint, and proves at rest
/// under the target's own basis exactly — a failing encoded proof refuses the
/// transfer instead of hiding under another basis.
///
/// # Errors
/// See [`RemapError`]; raw inputs are never mutated by a refusal.
pub fn remap_skin<'a>(
    source: &RestBinding<'_>,
    map: &BoneMap<'a>,
    target: &'a RestBinding<'a>,
    settings: &RemapSettings,
) -> Result<RemappedSkin<'a>, RemapError> {
    if !std::ptr::eq(map.source(), source.rig()) {
        return Err(RemapError::GraphOwner {
            side: GraphSide::Source,
        });
    }
    if !std::ptr::eq(map.target(), target.rig()) {
        return Err(RemapError::GraphOwner {
            side: GraphSide::Target,
        });
    }
    let locked = locked_set(source.rig(), settings)?;
    let locked_targets: Vec<_> = locked
        .iter()
        .filter_map(|&joint| match map.entries()[joint.index()].destination {
            Destination::Direct(target) | Destination::Collapsed(target) => Some((joint, target)),
            Destination::Unsupported => None,
        })
        .collect();
    let source_skin = select_shape(source.skins(), GraphSide::Source, settings.source_shape)?;
    let target_skin = select_shape(target.skins(), GraphSide::Target, settings.target_shape)?;

    let global_to_skin = source_skin.source().global_to_skin;
    let source_influences = &source_skin.source().skin.influences;
    let source_palette = source_skin.palette();
    let mut names = BTreeMap::<JointId, String>::new();
    let mut transfers = Vec::with_capacity(source_influences.rows());
    let mut collapses = Vec::new();

    for vertex in 0..source_influences.rows() {
        let row = source_influences
            .row(vertex)
            .expect("row index below the checked row count");
        // Raw positive weights per source joint; single-entry sums keep their
        // f32 bits, so a locked weight survives aggregation unchanged.
        let mut per_source = BTreeMap::<JointId, f32>::new();
        for (&slot, &weight) in row.joints.iter().zip(row.weights) {
            if weight <= 0.0 {
                continue;
            }
            let joint = source_palette[usize::from(slot)];
            *per_source.entry(joint).or_insert(0.0) += weight;
        }
        let mut aggregated = BTreeMap::<JointId, f32>::new();
        let mut contributors = BTreeMap::<JointId, Vec<(JointId, f32)>>::new();
        for (joint, weight) in &per_source {
            let destination = map.resolve(*joint).map_err(RemapError::Map)?;
            names
                .entry(destination)
                .or_insert_with(|| destination_name(target.rig(), destination));
            *aggregated.entry(destination).or_insert(0.0) += *weight;
            contributors
                .entry(destination)
                .or_default()
                .push((*joint, *weight));
        }
        for &(locked_joint, destination) in &locked_targets {
            if contributors
                .get(&destination)
                .is_some_and(|sources| sources.iter().any(|(joint, _)| *joint != locked_joint))
            {
                return Err(RemapError::LockCollapse {
                    vertex,
                    locked: locked_joint,
                    destination,
                });
            }
        }
        let locked_destinations = contributors
            .iter()
            .filter(|(_, sources)| sources.iter().any(|(joint, _)| locked.contains(joint)))
            .map(|(destination, _)| *destination)
            .collect();
        for (destination, sources) in contributors.iter().filter(|(_, sources)| {
            sources.len() >= 2
                || sources.iter().any(|(joint, _)| {
                    matches!(
                        map.entries()[joint.index()].destination,
                        Destination::Collapsed(_)
                    )
                })
        }) {
            collapses.push(CollapsedWeights {
                vertex,
                destination: *destination,
                sources: sources.clone(),
                source_sum: sources.iter().map(|(_, weight)| *weight).sum(),
            });
        }
        transfers.push(VertexTransfer {
            vertex,
            aggregated,
            locked: locked_destinations,
        });
    }

    // Deterministic output palette: used destinations in target joint order.
    let destinations: Vec<JointId> = names.keys().copied().collect();
    let slots: BTreeMap<JointId, u16> = destinations
        .iter()
        .enumerate()
        .map(|(index, joint)| {
            // The destination count is bounded by the source palette, which
            // SkinWeights addresses with u16 slots.
            (
                *joint,
                u16::try_from(index).expect("destination count fits the u16 palette"),
            )
        })
        .collect();
    let bones: Vec<String> = destinations
        .iter()
        .map(|joint| names[joint].clone())
        .collect();

    let mut offsets = Vec::with_capacity(transfers.len() + 1);
    let mut joints = Vec::new();
    let mut values = Vec::new();
    let mut discarded = Vec::new();
    offsets.push(0);
    for transfer in &transfers {
        let influences: Influences = transfer
            .aggregated
            .iter()
            .map(|(joint, weight)| (names[joint].clone(), *weight))
            .collect();
        let locked_names: BTreeSet<String> = transfer
            .locked
            .iter()
            .map(|joint| names[joint].clone())
            .collect();
        let packed =
            pack_influences(&influences, &locked_names).map_err(|error| RemapError::Pack {
                vertex: transfer.vertex,
                error,
            })?;
        for (destination, weight) in &transfer.aggregated {
            if !packed.contains_key(names[destination].as_str()) {
                discarded.push(DiscardedWeight {
                    vertex: transfer.vertex,
                    destination: *destination,
                    weight: *weight,
                });
            }
        }
        for destination in transfer.aggregated.keys() {
            if let Some(weight) = packed.get(names[destination].as_str()) {
                joints.push(slots[destination]);
                values.push(*weight);
            }
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| RemapError::Encode(SkinWeightError::Overflow))?,
        );
    }

    let influences =
        SkinWeights::new(offsets, joints, values, bones.len()).map_err(RemapError::Encode)?;
    let rig = target.rig();
    let basis = DMat4::from_cols_array_2d(&target.basis());
    let global_inverse =
        DMat4::from_cols_array_2d(&global_to_skin.map(|column| column.map(f64::from))).inverse();
    let bind_inverse = destinations
        .iter()
        .map(|joint| {
            let world =
                DMat4::from_cols_array_2d(&rig.rest_world(*joint).map_err(RemapError::Rig)?);
            let encoded = (basis * world).inverse() * global_inverse;
            Ok(encoded
                .to_cols_array_2d()
                .map(|column| column.map(|x| x as f32)))
        })
        .collect::<Result<Vec<_>, RemapError>>()?;
    let output = Skin {
        bones,
        bind_inverse,
        influences,
    };

    let root = target_skin.source().root;
    let proof_source = SkinRestSource {
        shape: settings.source_shape,
        root,
        positions: source_skin.source().positions,
        skin: &output,
        global_to_skin,
    };
    let proof = RestBinding::prove_with_basis(rig, rig.frame(), &[proof_source], target.basis())
        .map_err(RemapError::Rest)?;
    let measurement = proof.skins()[0].measurement();
    Ok(RemappedSkin {
        rig,
        root,
        global_to_skin,
        skin: output,
        report: RemapReport {
            measurement,
            collapses,
            discarded,
        },
    })
}

/// Validate lock declarations against the source rig.
fn locked_set(rig: &Rig, settings: &RemapSettings) -> Result<BTreeSet<JointId>, RemapError> {
    let mut locked = BTreeSet::new();
    for joint in &settings.locked_source_joints {
        if rig.joint(*joint).is_none() {
            return Err(RemapError::UnknownLock {
                joint: *joint,
                joints: rig.joints().len(),
            });
        }
        if !locked.insert(*joint) {
            return Err(RemapError::DuplicateLock { joint: *joint });
        }
    }
    Ok(locked)
}

/// The one skin whose declared shape identity matches, refusing the rest.
fn select_shape<'a>(
    skins: &'a [BoundRestSkin<'a>],
    side: GraphSide,
    shape: usize,
) -> Result<&'a BoundRestSkin<'a>, RemapError> {
    let mut matches = skins.iter().filter(|skin| skin.source().shape == shape);
    let Some(skin) = matches.next() else {
        return Err(RemapError::MissingShape { side, shape });
    };
    if matches.next().is_some() {
        return Err(RemapError::DuplicateShape { side, shape });
    }
    Ok(skin)
}

/// Every map destination resolves by name, so the palette can name it.
fn destination_name(rig: &Rig, joint: JointId) -> String {
    rig.joint(joint)
        .and_then(|joint| joint.name.clone())
        .expect("map destinations resolve by joint name")
}

impl fmt::Display for GraphSide {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source => write!(formatter, "source"),
            Self::Target => write!(formatter, "target"),
        }
    }
}

impl fmt::Display for RemapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GraphOwner { side } => write!(
                formatter,
                "{side} map graph is not the rig of the supplied binding"
            ),
            Self::MissingShape { side, shape } => {
                write!(formatter, "{side} binding has no shape {shape}")
            }
            Self::DuplicateShape { side, shape } => {
                write!(formatter, "{side} binding declares shape {shape} twice")
            }
            Self::UnknownLock { joint, joints } => write!(
                formatter,
                "locked source joint {joint} is outside the rig's {joints} joints"
            ),
            Self::DuplicateLock { joint } => {
                write!(formatter, "source joint {joint} is locked twice")
            }
            Self::Map(error) => write!(formatter, "{error}"),
            Self::LockCollapse {
                vertex,
                locked,
                destination,
            } => write!(
                formatter,
                "vertex {vertex}: locked source joint {locked} shares destination \
                 {destination} with another source weight"
            ),
            Self::Pack { vertex, error } => write!(formatter, "vertex {vertex}: {error}"),
            Self::Encode(error) => write!(formatter, "encoded output: {error}"),
            Self::Rig(error) => write!(formatter, "{error}"),
            Self::Rest(error) => write!(formatter, "encoded rest proof: {error}"),
        }
    }
}

impl std::error::Error for RemapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Map(error) => Some(error),
            Self::Pack { error, .. } => Some(error),
            Self::Encode(error) => Some(error),
            Self::Rig(error) => Some(error),
            Self::Rest(error) => Some(error),
            _ => None,
        }
    }
}
