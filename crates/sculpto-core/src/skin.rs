//! Authored skin weights: compressed sparse rows over (palette index, weight).
//!
//! A NIF stores variable per-vertex influences — LE keeps a sparse `f32` map per
//! bone and can exceed four entries, SE keeps four packed `f16` lanes — so a
//! fixed four-lane array is not a source representation. The file's values are
//! what the authoring tool wrote, quantization included, and this module keeps
//! them exactly: [`SkinWeights`] owns `offsets`/`joints`/`values` and only ever
//! *projects* them.
//!
//! Two projections exist for different consumers:
//! - [`SkinWeights::four_lane`] copies one row into the four hardware lanes
//!   without changing values, refusing a row that does not fit.
//! - [`SkinWeights::normalized_four_lane`] divides by the row sum first, which is
//!   the unit-sum view a glTF or pose consumer needs.
//!
//! Stored sums are data, not an invariant: half-float packing on SE drifts, and
//! the unit-sum contract belongs to the derived view. `@req DECL0008`

use std::fmt;

/// One vertex's authored influences in a [`SkinWeights`] row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row<'a> {
    /// Palette indices, in source order.
    pub joints: &'a [u16],
    /// Authored weights, parallel to [`Self::joints`].
    pub weights: &'a [f32],
}

/// One row copied onto the four hardware influence lanes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FourLane {
    /// Palette indices, zero-padded past the row length.
    pub joints: [u16; 4],
    /// Values in the same order as [`Self::joints`], zero-padded.
    pub weights: [f32; 4],
}

/// A skin's per-vertex influences in compressed sparse row form.
///
/// Row `i` owns `joints[offsets[i]..offsets[i + 1]]` and the parallel
/// `values[..]`. Buffers are private so an offset edit cannot desynchronize the
/// three arrays; mutation goes through [`SkinWeights::normalize`] only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinWeights {
    offsets: Vec<u32>,
    joints: Vec<u16>,
    values: Vec<f32>,
}

impl SkinWeights {
    /// Build rows from explicit CSR buffers and validate them.
    ///
    /// # Errors
    /// See [`SkinWeights::validate`]; `palette` is the skin's bone count.
    pub fn new(
        offsets: Vec<u32>,
        joints: Vec<u16>,
        values: Vec<f32>,
        palette: usize,
    ) -> Result<Self, SkinWeightError> {
        let weights = Self {
            offsets,
            joints,
            values,
        };
        weights.validate(palette)?;
        Ok(weights)
    }

    /// Build rows from four-lane source arrays, dropping the zero padding.
    ///
    /// A lane is kept when its weight is not exactly zero, so a negative or
    /// non-finite lane is refused instead of being silently discarded as
    /// padding. Palette references are checked by the model, which owns the bone
    /// list ([`SkinWeights::validate`]); this constructor cannot know it.
    /// @req DECL0008
    ///
    /// # Errors
    /// A lane count mismatch, or any structural or per-value defect.
    pub fn from_quads(quads: &[[u16; 4]], weights: &[[f32; 4]]) -> Result<Self, SkinWeightError> {
        if quads.len() != weights.len() {
            return Err(SkinWeightError::EntryCount {
                joints: quads.len(),
                values: weights.len(),
            });
        }
        let mut offsets = Vec::with_capacity(quads.len() + 1);
        let mut joints = Vec::with_capacity(quads.len());
        let mut values = Vec::with_capacity(quads.len());
        offsets.push(0);
        for (quad, lanes) in quads.iter().zip(weights) {
            for (joint, weight) in quad.iter().zip(lanes) {
                if *weight != 0.0 {
                    joints.push(*joint);
                    values.push(*weight);
                }
            }
            offsets.push(u32::try_from(joints.len()).map_err(|_| SkinWeightError::Overflow)?);
        }
        let rows = Self {
            offsets,
            joints,
            values,
        };
        rows.validate_rows()?;
        Ok(rows)
    }

    /// Number of vertex rows.
    pub fn rows(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// One row, or `None` when the index is outside the row count.
    ///
    /// This is the only public row accessor: it refuses an index instead of
    /// panicking, so a stale row number is a `None`, never a slice built from an
    /// offset that does not exist.
    pub fn row(&self, row: usize) -> Option<Row<'_>> {
        let start = *self.offsets.get(row)? as usize;
        let end = *self.offsets.get(row.checked_add(1)?)? as usize;
        if start > end || end > self.joints.len() || end > self.values.len() {
            return None;
        }
        Some(Row {
            joints: &self.joints[start..end],
            weights: &self.values[start..end],
        })
    }

    /// Sum of one row's authored values, or `None` outside the row count.
    pub fn row_sum(&self, row: usize) -> Option<f32> {
        self.row(row).map(|row| row.weights.iter().sum())
    }

    /// Row slices for an index the caller has already bounded.
    ///
    /// Private: every public row access goes through [`SkinWeights::row`], which
    /// checks the index and the span. @req DECL0008
    fn raw_row(&self, row: usize) -> (&[u16], &[f32]) {
        let start = self.offsets[row] as usize;
        let end = self.offsets[row + 1] as usize;
        (&self.joints[start..end], &self.values[start..end])
    }

    /// Copy one row onto the four hardware lanes, values unchanged.
    ///
    /// # Errors
    /// [`SkinWeightError::TooManyInfluences`] when the row does not fit.
    pub fn four_lane(&self, row: usize) -> Result<FourLane, SkinWeightError> {
        let Some(view) = self.row(row) else {
            return Err(SkinWeightError::RowOutOfRange {
                row,
                rows: self.rows(),
            });
        };
        if view.joints.len() > 4 {
            return Err(SkinWeightError::TooManyInfluences {
                row,
                influences: view.joints.len(),
            });
        }
        let mut lane = FourLane {
            joints: [0; 4],
            weights: [0.0; 4],
        };
        lane.joints[..view.joints.len()].copy_from_slice(view.joints);
        lane.weights[..view.weights.len()].copy_from_slice(view.weights);
        Ok(lane)
    }

    /// Copy one row onto four lanes after dividing by the row sum.
    ///
    /// # Errors
    /// [`SkinWeightError::TooManyInfluences`] when the row does not fit, or a
    /// non-positive/non-finite sum.
    pub fn normalized_four_lane(&self, row: usize) -> Result<FourLane, SkinWeightError> {
        let mut lane = self.four_lane(row)?;
        lane.weights
            .iter_mut()
            .zip(self.normalized_row(row)?)
            .for_each(|(cell, (_, weight))| *cell = weight as f32);
        Ok(lane)
    }

    /// Normalize the complete row for evaluation or a wire projection. @req DECL0008
    /// A wide sum/divide avoids overflow in a reciprocal of a small positive f32 sum.
    pub fn normalized_row(
        &self,
        row: usize,
    ) -> Result<impl ExactSizeIterator<Item = (u16, f64)> + '_, SkinWeightError> {
        let view = self.row(row).ok_or(SkinWeightError::RowOutOfRange {
            row,
            rows: self.rows(),
        })?;
        let sum = normalization_sum(view.weights, row)?;
        Ok(view
            .joints
            .iter()
            .copied()
            .zip(view.weights.iter().copied())
            .map(move |(joint, weight)| (joint, f64::from(weight) / sum)))
    }

    /// Divide every row by its own sum, in place.
    ///
    /// This is the explicit authoring mutation: it rewrites stored values and is
    /// never implied by validation, projection or export. @req DECL0008
    ///
    /// # Errors
    /// A row with a non-positive or non-finite sum.
    pub fn normalize(&mut self) -> Result<(), SkinWeightError> {
        for row in 0..self.rows() {
            let (_, weights) = self.raw_row(row);
            let sum = normalization_sum(weights, row)?;
            let start = self.offsets[row] as usize;
            let end = self.offsets[row + 1] as usize;
            for value in &mut self.values[start..end] {
                *value = (f64::from(*value) / sum) as f32;
            }
        }
        Ok(())
    }

    /// Copy the named rows into a new value, preserving order and values.
    ///
    /// Used by shape splitting, where the output rows must follow the kept
    /// vertices. @req DECL0008
    ///
    /// # Errors
    /// A row index outside this value, or a buffer above the `u32` offset range.
    pub fn gather(&self, rows: &[u32]) -> Result<Self, SkinWeightError> {
        let mut offsets = Vec::with_capacity(rows.len() + 1);
        let mut joints = Vec::new();
        let mut values = Vec::new();
        offsets.push(0);
        for &row in rows {
            let index = row as usize;
            let Some(view) = self.row(index) else {
                return Err(SkinWeightError::RowOutOfRange {
                    row: index,
                    rows: self.rows(),
                });
            };
            joints.extend_from_slice(view.joints);
            values.extend_from_slice(view.weights);
            offsets.push(u32::try_from(joints.len()).map_err(|_| SkinWeightError::Overflow)?);
        }
        Ok(Self {
            offsets,
            joints,
            values,
        })
    }

    /// Check every invariant that does not need the palette.
    ///
    /// Structural: offsets start at zero, are monotone and cover exactly the
    /// entry arrays. Per row: at least one entry, finite non-negative values and
    /// a finite positive sum. The sum is not required to be one; that belongs to
    /// [`SkinWeights::normalized_four_lane`].
    ///
    /// # Errors
    /// The first named defect, with its row.
    pub fn validate_rows(&self) -> Result<(), SkinWeightError> {
        if self.offsets.first() != Some(&0) {
            return Err(SkinWeightError::OffsetsStart);
        }
        if self.joints.len() != self.values.len() {
            return Err(SkinWeightError::EntryCount {
                joints: self.joints.len(),
                values: self.values.len(),
            });
        }
        let mut previous = 0_u32;
        for (row, &offset) in self.offsets.iter().enumerate() {
            if offset < previous {
                return Err(SkinWeightError::OffsetsOrder {
                    row,
                    previous,
                    offset,
                });
            }
            previous = offset;
        }
        let last = self.offsets.last().copied().unwrap_or(0);
        if last as usize != self.joints.len() {
            return Err(SkinWeightError::OffsetsCover {
                offset: last,
                entries: self.joints.len(),
            });
        }
        for row in 0..self.rows() {
            let (joints, weights) = self.raw_row(row);
            if joints.is_empty() {
                return Err(SkinWeightError::EmptyRow { row });
            }
            for (entry, weight) in weights.iter().enumerate() {
                if !weight.is_finite() {
                    return Err(SkinWeightError::NonFinite {
                        row,
                        entry,
                        weight: *weight,
                    });
                }
                if *weight < 0.0 {
                    return Err(SkinWeightError::Negative {
                        row,
                        entry,
                        weight: *weight,
                    });
                }
            }
            let sum: f32 = weights.iter().sum();
            match sum.is_finite() && sum > 0.0 {
                true => {}
                false => return Err(SkinWeightError::BadSum { row, sum }),
            }
        }
        Ok(())
    }

    /// Check the structural invariants and every row's palette references.
    ///
    /// # Errors
    /// Any [`SkinWeights::validate_rows`] defect, or the first palette index
    /// outside the `palette`-bone list.
    pub fn validate(&self, palette: usize) -> Result<(), SkinWeightError> {
        self.validate_rows()?;
        for row in 0..self.rows() {
            for (entry, joint) in self.raw_row(row).0.iter().enumerate() {
                if *joint as usize >= palette {
                    return Err(SkinWeightError::PaletteIndex {
                        row,
                        entry,
                        joint: *joint,
                        palette,
                    });
                }
            }
        }
        Ok(())
    }
}

fn normalization_sum(weights: &[f32], row: usize) -> Result<f64, SkinWeightError> {
    let sum = weights.iter().copied().map(f64::from).sum::<f64>();
    match sum.is_finite() && sum > 0.0 {
        true => Ok(sum),
        false => Err(SkinWeightError::BadSum {
            row,
            sum: sum as f32,
        }),
    }
}

/// Why a weight row set is not a usable authored skin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkinWeightError {
    /// The offset array does not start at zero.
    OffsetsStart,
    /// Offsets are not monotone.
    OffsetsOrder {
        /// Row whose offset moved backwards.
        row: usize,
        /// Previous offset.
        previous: u32,
        /// This row's offset.
        offset: u32,
    },
    /// The final offset does not cover the entry arrays.
    OffsetsCover {
        /// Final offset.
        offset: u32,
        /// Entry count.
        entries: usize,
    },
    /// The joint and value arrays differ in length.
    EntryCount {
        /// Joint count.
        joints: usize,
        /// Value count.
        values: usize,
    },
    /// An entry count does not fit a `u32` offset.
    Overflow,
    /// A requested row index is outside the value.
    RowOutOfRange {
        /// Requested row.
        row: usize,
        /// Row count.
        rows: usize,
    },
    /// A row carries no influence at all.
    EmptyRow {
        /// The row.
        row: usize,
    },
    /// A palette index is outside the skin's bone list.
    PaletteIndex {
        /// The row.
        row: usize,
        /// Entry inside the row.
        entry: usize,
        /// The refused index.
        joint: u16,
        /// Palette length.
        palette: usize,
    },
    /// A value is not finite.
    NonFinite {
        /// The row.
        row: usize,
        /// Entry inside the row.
        entry: usize,
        /// The refused value.
        weight: f32,
    },
    /// A value is negative.
    Negative {
        /// The row.
        row: usize,
        /// Entry inside the row.
        entry: usize,
        /// The refused value.
        weight: f32,
    },
    /// A row sum is zero, negative or not finite.
    BadSum {
        /// The row.
        row: usize,
        /// The refused sum.
        sum: f32,
    },
    /// A row has more influences than the four hardware lanes hold.
    TooManyInfluences {
        /// The row.
        row: usize,
        /// Its influence count.
        influences: usize,
    },
}

impl fmt::Display for SkinWeightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetsStart => write!(formatter, "weight offsets do not start at zero"),
            Self::OffsetsOrder {
                row,
                previous,
                offset,
            } => write!(
                formatter,
                "weight offsets decrease at row {row}: {offset} after {previous}"
            ),
            Self::OffsetsCover { offset, entries } => write!(
                formatter,
                "weight offsets end at {offset} but cover {entries} entries"
            ),
            Self::EntryCount { joints, values } => write!(
                formatter,
                "weight rows hold {joints} palette indices and {values} values"
            ),
            Self::Overflow => write!(formatter, "weight entry count exceeds the offset range"),
            Self::RowOutOfRange { row, rows } => {
                write!(formatter, "weight row {row} is outside the {rows} rows")
            }
            Self::EmptyRow { row } => write!(formatter, "vertex {row} has no skin influence"),
            Self::PaletteIndex {
                row,
                entry,
                joint,
                palette,
            } => write!(
                formatter,
                "vertex {row} influence {entry} names bone {joint} outside the {palette} palette"
            ),
            Self::NonFinite { row, entry, weight } => write!(
                formatter,
                "vertex {row} influence {entry} weight {weight} is not finite"
            ),
            Self::Negative { row, entry, weight } => write!(
                formatter,
                "vertex {row} influence {entry} weight {weight} is negative"
            ),
            Self::BadSum { row, sum } => {
                write!(formatter, "vertex {row} weights sum to {sum}")
            }
            Self::TooManyInfluences { row, influences } => write!(
                formatter,
                "vertex {row} has {influences} influences, above the four hardware lanes"
            ),
        }
    }
}

impl std::error::Error for SkinWeightError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn five_row() -> SkinWeights {
        SkinWeights::new(
            vec![0, 5, 6],
            vec![3, 7, 1, 2, 9, 4],
            vec![0.5, 0.25, 0.125, 0.125, 2.0e-16, 1.0],
            10,
        )
        .expect("a five-influence and a one-influence row are valid")
    }

    #[test]
    fn rows_keep_every_authored_influence() {
        let weights = five_row();
        assert_eq!(weights.rows(), 2);
        let row = weights.row(0).expect("row 0");
        assert_eq!(row.joints, &[3, 7, 1, 2, 9]);
        assert_eq!(row.weights[4], 2.0e-16);
        assert_eq!(weights.row_sum(1), Some(1.0));
        assert_eq!(weights.row(2), None, "an index past the rows is refused");
        assert_eq!(weights.row_sum(2), None);
    }

    #[test]
    fn a_five_influence_row_does_not_fit_the_hardware_lanes() {
        let weights = five_row();
        assert_eq!(
            weights.four_lane(0),
            Err(SkinWeightError::TooManyInfluences {
                row: 0,
                influences: 5
            })
        );
        let lane = weights.four_lane(1).expect("one influence fits");
        assert_eq!(lane.joints, [4, 0, 0, 0]);
        assert_eq!(lane.weights, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn normalization_is_a_projection_and_never_rewrites_the_source() {
        let weights =
            SkinWeights::new(vec![0, 2], vec![1, 2], vec![0.75, 0.5], 3).expect("a valid row");
        let lane = weights
            .normalized_four_lane(0)
            .expect("the row fits four lanes");
        assert!((lane.weights[0] - 0.6).abs() < 1e-6);
        assert!((lane.weights[1] - 0.4).abs() < 1e-6);
        assert_eq!(weights.row(0).expect("row 0").weights, &[0.75, 0.5]);
        assert_eq!(weights.row_sum(0), Some(1.25));
    }

    #[test]
    fn quads_drop_padding_but_keep_a_tiny_positive_value() {
        let weights = SkinWeights::from_quads(&[[0, 1, 2, 0]], &[[0.5, 0.25, 0.25, 0.0]])
            .expect("three positive lanes");
        assert_eq!(weights.row(0).expect("row 0").joints, &[0, 1, 2]);

        let tiny = SkinWeights::from_quads(&[[0, 1, 0, 0]], &[[0.5, 1.0e-16, 0.5, 0.0]])
            .expect("a tiny positive lane is data");
        assert_eq!(tiny.rows(), 1);
        assert_eq!(tiny.row(0).expect("row 0").joints, &[0, 1, 0]);
    }

    #[test]
    fn malformed_rows_are_named() {
        assert_eq!(
            SkinWeights::new(vec![1, 1], Vec::new(), Vec::new(), 1),
            Err(SkinWeightError::OffsetsStart)
        );
        assert_eq!(
            SkinWeights::new(vec![0, 2, 1], vec![0, 0, 0], vec![1.0, 1.0, 1.0], 1),
            Err(SkinWeightError::OffsetsOrder {
                row: 2,
                previous: 2,
                offset: 1
            })
        );
        assert_eq!(
            SkinWeights::new(vec![0, 2], vec![0], vec![1.0], 1),
            Err(SkinWeightError::OffsetsCover {
                offset: 2,
                entries: 1
            })
        );
        assert_eq!(
            SkinWeights::new(vec![0, 1], vec![0, 0], vec![1.0], 1),
            Err(SkinWeightError::EntryCount {
                joints: 2,
                values: 1
            })
        );
        assert_eq!(
            SkinWeights::new(vec![0, 1], vec![4], vec![1.0], 4),
            Err(SkinWeightError::PaletteIndex {
                row: 0,
                entry: 0,
                joint: 4,
                palette: 4
            })
        );
        assert!(matches!(
            SkinWeights::new(vec![0, 1], vec![0], vec![f32::NAN], 1),
            Err(SkinWeightError::NonFinite { .. })
        ));
        assert!(matches!(
            SkinWeights::new(vec![0, 1], vec![0], vec![-0.5], 1),
            Err(SkinWeightError::Negative { .. })
        ));
        assert!(matches!(
            SkinWeights::new(vec![0, 1], vec![0], vec![0.0], 1),
            Err(SkinWeightError::BadSum { .. })
        ));
        assert!(matches!(
            SkinWeights::new(vec![0, 1, 1], vec![0], vec![1.0], 1),
            Err(SkinWeightError::EmptyRow { row: 1 })
        ));
    }

    #[test]
    fn gather_keeps_row_order_and_values() {
        let weights = five_row();
        let gathered = weights.gather(&[1, 0]).expect("both rows exist");
        assert_eq!(gathered.rows(), 2);
        assert_eq!(gathered.row(0).expect("row 0").joints, &[4]);
        assert_eq!(gathered.row(1).expect("row 1").joints, &[3, 7, 1, 2, 9]);
        assert_eq!(
            weights.gather(&[2]),
            Err(SkinWeightError::RowOutOfRange { row: 2, rows: 2 })
        );
    }

    #[test]
    fn explicit_normalization_rewrites_only_when_called() {
        let mut weights =
            SkinWeights::from_quads(&[[0, 1, 0, 0]], &[[0.75, 0.5, 0.0, 0.0]]).expect("two lanes");
        assert_eq!(weights.row_sum(0), Some(1.25));
        weights.normalize().expect("a positive sum normalizes");
        assert!((weights.row_sum(0).expect("row 0") - 1.0).abs() < 1e-6);
    }
}
