//! Ordered weight normalization and the strict final influence pack.
//!
//! `BoneWeightAutoNormalizer` adjusts three bone classes in priority order: a
//! locked weight is never rewritten, modified (copied) weights are clamped and
//! scaled first, normalizable bones absorb the remainder, and an opt-in spread
//! divides what is still missing (`WeightNorm.h:17-35`,
//! `WeightNorm.cpp:64-181`). Its comparisons use [`WEIGHT_EPSILON`], so the
//! arithmetic alone may leave a sum short of one. [`pack_influences`] is the
//! explicit stricter step that fits the four NIF influence slots and closes the
//! sum to [`SUM_TOLERANCE`].
//!
//! The reference drives the same arithmetic from an editor `UndoState`; here it
//! is a pure per-vertex operation, so a caller supplies the three classes and
//! the vertex map.

use std::collections::BTreeSet;
use std::fmt;

use crate::garment::Influences;

/// Native weight comparison threshold (`WeightNorm.h:37`).
pub const WEIGHT_EPSILON: f32 = 0.001;
/// Positive influences one NIF skin vertex stores.
pub const MAX_INFLUENCES: usize = 4;
/// Admitted error of the packed weight sum.
pub const SUM_TOLERANCE: f32 = 1e-4;

/// Why one influence is not an acceptable skin weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeightDefect {
    /// The weight is not finite.
    NonFinite { weight: f32 },
    /// The weight is negative.
    Negative { weight: f32 },
}

/// The first unacceptable influence in name order.
///
/// The accepted-weight rule has one owner; callers attach their own context
/// (the vertex, the input field) to the returned defect.
pub fn first_weight_defect(influences: &Influences) -> Option<(&str, WeightDefect)> {
    influences
        .iter()
        .find(|(_, weight)| !weight.is_finite() || **weight < 0.0)
        .map(|(bone, weight)| {
            let defect = match weight.is_finite() {
                true => WeightDefect::Negative { weight: *weight },
                false => WeightDefect::NonFinite { weight: *weight },
            };
            (bone.as_str(), defect)
        })
}

/// Why a weight class, one vertex, or the final pack was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum WeightError {
    /// A bone name is empty.
    EmptyBoneName,
    /// A bone appears twice in the normalize-bone order.
    DuplicateBone { name: String },
    /// A bone sits in two classes at once.
    AmbiguousClass { name: String },
    /// The modified count exceeds the normalize-bone order.
    ModifiedOutOfRange { modified: usize, bones: usize },
    /// An influence names a bone outside every declared class.
    UnclassifiedBone { name: String },
    /// An influence weight is not finite.
    NonFiniteWeight { bone: String, weight: f32 },
    /// An influence weight is negative.
    NegativeWeight { bone: String, weight: f32 },
    /// The locked mass leaves no usable budget.
    ImpossibleLockedMass { locked: f32 },
    /// More positive locked influences than the NIF stores.
    TooManyLocked { locked: usize, slots: usize },
    /// The locked mass exceeds one.
    LockedMassOverOne { locked: f32 },
    /// A positive remainder has no unlocked influence to carry it.
    MissingUnlockedMass { remainder: f32 },
}

impl fmt::Display for WeightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBoneName => write!(formatter, "a bone name is empty"),
            Self::DuplicateBone { name } => write!(formatter, "bone `{name}` appears twice"),
            Self::AmbiguousClass { name } => {
                write!(formatter, "bone `{name}` is in two weight classes")
            }
            Self::ModifiedOutOfRange { modified, bones } => write!(
                formatter,
                "{modified} modified bones exceed the {bones} normalize bones"
            ),
            Self::UnclassifiedBone { name } => {
                write!(formatter, "bone `{name}` is in no weight class")
            }
            Self::NonFiniteWeight { bone, weight } => {
                write!(formatter, "bone `{bone}` has non-finite weight {weight}")
            }
            Self::NegativeWeight { bone, weight } => {
                write!(formatter, "bone `{bone}` has negative weight {weight}")
            }
            Self::ImpossibleLockedMass { locked } => {
                write!(formatter, "the locked weight {locked} exceeds one")
            }
            Self::TooManyLocked { locked, slots } => write!(
                formatter,
                "{locked} locked influences exceed the {slots} NIF slots"
            ),
            Self::LockedMassOverOne { locked } => {
                write!(formatter, "the locked weights sum to {locked}, above one")
            }
            Self::MissingUnlockedMass { remainder } => write!(
                formatter,
                "no unlocked influence carries the {remainder} remainder"
            ),
        }
    }
}

impl std::error::Error for WeightError {}

/// What one [`WeightNormalizer::adjust`] produced, in native order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormReport {
    /// Locked mass the reference subtracts, after its epsilon rounding.
    pub locked_mass: f32,
    /// Accepted modified mass after clamping and scaling.
    pub modified_mass: f32,
    /// Normalizable mass after scaling and clamping.
    pub normalizable_mass: f32,
    /// Remainder added to every normalizable bone, zero without spread.
    pub spread_mass: f32,
}

/// One native normalization configuration (`WeightNorm.cpp:9-41`).
///
/// `bones` is the normalize-bone order: the first `n_modified` entries are the
/// modified (copied) class and the rest are normalizable. `locked` must be
/// disjoint from `bones`. A locked weight is never rewritten, so its f32 bits
/// survive a call unchanged.
#[derive(Debug, Clone)]
pub struct WeightNormalizer {
    modified: Vec<String>,
    normalizable: Vec<String>,
    locked: BTreeSet<String>,
    spread: bool,
}

impl WeightNormalizer {
    /// Validate the three classes before any vertex is adjusted.
    ///
    /// # Errors
    ///
    /// Fails on an empty or duplicated bone name, a bone in two classes, and a
    /// modified count outside `bones`.
    pub fn new(
        bones: &[String],
        locked: &BTreeSet<String>,
        n_modified: usize,
        spread: bool,
    ) -> Result<Self, WeightError> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for name in bones {
            if name.is_empty() {
                return Err(WeightError::EmptyBoneName);
            }
            if !seen.insert(name.as_str()) {
                return Err(WeightError::DuplicateBone { name: name.clone() });
            }
        }
        if n_modified > bones.len() {
            return Err(WeightError::ModifiedOutOfRange {
                modified: n_modified,
                bones: bones.len(),
            });
        }
        for name in locked {
            if name.is_empty() {
                return Err(WeightError::EmptyBoneName);
            }
            if seen.contains(name.as_str()) {
                return Err(WeightError::AmbiguousClass { name: name.clone() });
            }
        }
        Ok(Self {
            modified: bones[..n_modified].to_vec(),
            normalizable: bones[n_modified..].to_vec(),
            locked: locked.clone(),
            spread,
        })
    }

    /// Normalize one vertex in the reference's locked/modified/normalizable order.
    ///
    /// # Errors
    ///
    /// Fails on an influence outside the three classes, a weight that is not a
    /// finite non-negative number, and a locked mass above one. Locked weights
    /// are left bit-identical.
    pub fn adjust(&self, influences: &mut Influences) -> Result<NormReport, WeightError> {
        match first_weight_defect(influences) {
            Some((bone, WeightDefect::NonFinite { weight })) => {
                return Err(WeightError::NonFiniteWeight {
                    bone: bone.to_string(),
                    weight,
                });
            }
            Some((bone, WeightDefect::Negative { weight })) => {
                return Err(WeightError::NegativeWeight {
                    bone: bone.to_string(),
                    weight,
                });
            }
            None => {}
        }
        match influences
            .keys()
            .find(|bone| !self.classified(bone.as_str()))
        {
            Some(name) => {
                return Err(WeightError::UnclassifiedBone { name: name.clone() });
            }
            None => {}
        }

        let exact_locked: f32 = self
            .locked
            .iter()
            .filter_map(|bone| influences.get(bone))
            .sum();
        if exact_locked > 1.0 {
            return Err(WeightError::ImpossibleLockedMass {
                locked: exact_locked,
            });
        }
        let locked_mass = match exact_locked < WEIGHT_EPSILON {
            true => 0.0,
            false => exact_locked,
        };
        let available = match 1.0 - locked_mass < WEIGHT_EPSILON {
            true => 0.0,
            false => 1.0 - locked_mass,
        };

        let normalizable_total: f32 = self
            .normalizable
            .iter()
            .filter_map(|bone| influences.get(bone))
            .sum();

        let mut modified_mass = 0.0;
        for bone in &self.modified {
            let Some(weight) = influences.get_mut(bone) else {
                continue;
            };
            if *weight < WEIGHT_EPSILON {
                *weight = 0.0;
            }
            // Native comparison: `w - 1.0 > -WEIGHT_EPSILON`.
            if *weight - 1.0 > -WEIGHT_EPSILON {
                *weight = 1.0;
            }
            modified_mass += *weight;
        }
        if modified_mass > available {
            let factor = match modified_mass <= WEIGHT_EPSILON {
                true => 0.0,
                false => available / modified_mass,
            };
            modified_mass = 0.0;
            for bone in &self.modified {
                let Some(weight) = influences.get_mut(bone) else {
                    continue;
                };
                *weight *= factor;
                modified_mass += *weight;
            }
            if modified_mass > available {
                modified_mass = available;
            }
        }

        let normalizing = match normalizable_total <= WEIGHT_EPSILON {
            true => 0.0,
            false => (available - modified_mass) / normalizable_total,
        };
        let mut normalizable_mass = 0.0;
        for bone in &self.normalizable {
            let Some(weight) = influences.get_mut(bone) else {
                continue;
            };
            *weight *= normalizing;
            if *weight < WEIGHT_EPSILON {
                *weight = 0.0;
            }
            // Native comparison: `w - 1.0 > -WEIGHT_EPSILON`.
            if *weight - 1.0 > -WEIGHT_EPSILON {
                *weight = 1.0;
            }
            normalizable_mass += *weight;
        }

        let remainder = 1.0 - normalizable_mass - modified_mass - locked_mass;
        let spread_mass =
            match remainder > WEIGHT_EPSILON && self.spread && !self.normalizable.is_empty() {
                true => {
                    let share = remainder / self.normalizable.len() as f32;
                    self.normalizable.iter().for_each(|bone| {
                        *influences.entry(bone.clone()).or_insert(0.0) += share;
                    });
                    share
                }
                false => 0.0,
            };
        Ok(NormReport {
            locked_mass,
            modified_mass,
            normalizable_mass,
            spread_mass,
        })
    }

    /// A bone belongs to exactly one of the three classes.
    fn classified(&self, bone: &str) -> bool {
        // The class lists hold a handful of bones, so a linear scan is cheaper
        // than a second membership index.
        self.locked.contains(bone)
            || self.modified.iter().any(|name| name == bone)
            || self.normalizable.iter().any(|name| name == bone)
    }
}

/// Cap one vertex at the four NIF influence slots around its locked mass.
///
/// Locked positives are kept bit-exactly and consume their exact mass, not the
/// normalizer's epsilon-rounded total. The largest unlocked positives take the
/// remaining slots, ties resolving by name, and only they are scaled to the
/// exact remainder. A remainder inside [`SUM_TOLERANCE`] is left unassigned
/// because the sum already meets the guarantee.
///
/// # Errors
///
/// Fails on a weight that is not a finite non-negative number, more than
/// [`MAX_INFLUENCES`] locked positives, a locked mass above one, and a positive
/// remainder with no slot or no unlocked weight to carry it.
pub fn pack_influences(
    influences: &Influences,
    locked: &BTreeSet<String>,
) -> Result<Influences, WeightError> {
    match first_weight_defect(influences) {
        Some((bone, WeightDefect::NonFinite { weight })) => {
            return Err(WeightError::NonFiniteWeight {
                bone: bone.to_string(),
                weight,
            });
        }
        Some((bone, WeightDefect::Negative { weight })) => {
            return Err(WeightError::NegativeWeight {
                bone: bone.to_string(),
                weight,
            });
        }
        None => {}
    }

    let locked_positive: Vec<(String, f32)> = influences
        .iter()
        .filter(|(bone, weight)| locked.contains(bone.as_str()) && **weight > 0.0)
        .map(|(bone, weight)| (bone.clone(), *weight))
        .collect();
    if locked_positive.len() > MAX_INFLUENCES {
        return Err(WeightError::TooManyLocked {
            locked: locked_positive.len(),
            slots: MAX_INFLUENCES,
        });
    }
    let locked_mass: f32 = locked_positive.iter().map(|(_, weight)| *weight).sum();
    if locked_mass > 1.0 {
        return Err(WeightError::LockedMassOverOne {
            locked: locked_mass,
        });
    }
    let remainder = 1.0 - locked_mass;
    let mut packed: Influences = locked_positive.into_iter().collect();
    if remainder <= SUM_TOLERANCE {
        return Ok(packed);
    }

    let slots = MAX_INFLUENCES - packed.len();
    let mut unlocked: Vec<(String, f32)> = influences
        .iter()
        .filter(|(bone, weight)| !locked.contains(bone.as_str()) && **weight > 0.0)
        .map(|(bone, weight)| (bone.clone(), *weight))
        .collect();
    // Largest first, name ascending on a tie: a rerun chooses the same bones.
    unlocked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    let chosen: Vec<(String, f32)> = unlocked.into_iter().take(slots).collect();
    let chosen_mass: f32 = chosen.iter().map(|(_, weight)| *weight).sum();
    if slots == 0 || chosen_mass <= 0.0 {
        return Err(WeightError::MissingUnlockedMass { remainder });
    }
    let scale = remainder / chosen_mass;
    packed.extend(
        chosen
            .into_iter()
            .map(|(bone, weight)| (bone, weight * scale)),
    );
    Ok(packed)
}
