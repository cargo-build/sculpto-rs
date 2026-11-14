//! A hidden shape keeps its geometry and changes exactly the `NiAVObject`
//! flags word. @req BLD-01
//!
//! Native marks a fully zapped shape under `KeepZappedShapes` by setting flag
//! bit 0 (BodySlideApp.cpp:4420-4421) and changes nothing else about the shape.
//! The reader does not surface flags yet, so this suite observes the emission
//! at the byte level: hiding a shape must flip exactly one `u32` in the file,
//! from zero to one, on every writer that emits a shape block (SE and LE,
//! single shape and bundle).

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::write::header::NifTarget;
use sculpto_nif::write::le_bundle::write_le_bundle;
use sculpto_nif::write::se_bundle::write_se_bundle;
use sculpto_nif::write::write_shape;

fn asset() -> MeshAsset {
    MeshAsset {
        name: "Body".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        triangle_partitions: vec![PartitionId(0), PartitionId(0)],
        ..MeshAsset::default()
    }
}

/// The hidden flag must flip exactly one byte — the flags word's low byte,
/// from 0 to 1 — and change nothing else.
fn assert_one_flags_word(visible: &[u8], hidden: &[u8]) {
    assert_eq!(visible.len(), hidden.len(), "hiding adds no bytes");
    let differing: Vec<usize> = visible
        .iter()
        .zip(hidden)
        .enumerate()
        .filter_map(|(index, (a, b))| (a != b).then_some(index))
        .collect();
    assert_eq!(
        differing.len(),
        1,
        "exactly the flags word's low byte differs"
    );
    assert_eq!(hidden[differing[0]], 1, "the flags word is bit 0 only");
    assert_eq!(visible[differing[0]], 0, "the shape was visible before");
}

#[test]
fn hiding_flips_only_the_se_flags_word() {
    let visible = write_shape(&asset(), NifTarget::Se).expect("visible SE writes");
    let mut shape = asset();
    shape.hidden = true;
    let hidden = write_shape(&shape, NifTarget::Se).expect("hidden SE writes");
    assert_one_flags_word(&visible, &hidden);
    // No read-back here on purpose: the vendored importer culls APP_CULLED
    // (flags & 0x01) shapes at import (byroredux import/walk/mod.rs:296), so
    // `sculpto_nif::read` legitimately surfaces no shape for a hidden file.
    // Surfacing kept-but-hidden shapes is a reader-scope change owned
    // elsewhere; this suite witnesses the emission at the byte level.
}

#[test]
fn hiding_flips_only_the_le_flags_word() {
    let visible = write_shape(&asset(), NifTarget::Le).expect("visible LE writes");
    let mut shape = asset();
    shape.hidden = true;
    let hidden = write_shape(&shape, NifTarget::Le).expect("hidden LE writes");
    assert_one_flags_word(&visible, &hidden);
}

#[test]
fn hiding_flips_only_the_bundle_flags_words() {
    let mut second = asset();
    second.name = "Body.1".into();
    let shapes = vec![asset(), second];
    for target in [NifTarget::Se, NifTarget::Le] {
        let visible = match target {
            NifTarget::Se => write_se_bundle(&shapes).expect("visible SE bundle"),
            NifTarget::Le => write_le_bundle(&shapes).expect("visible LE bundle"),
        };
        let hidden_shapes = shapes
            .iter()
            .map(|shape| {
                let mut hidden = shape.clone();
                hidden.hidden = true;
                hidden
            })
            .collect::<Vec<_>>();
        let hidden = match target {
            NifTarget::Se => write_se_bundle(&hidden_shapes).expect("hidden SE bundle"),
            NifTarget::Le => write_le_bundle(&hidden_shapes).expect("hidden LE bundle"),
        };
        assert_eq!(visible.len(), hidden.len(), "{target:?} bundle length");
        // Both shapes hide, so exactly two flags bytes differ.
        let differing: Vec<usize> = visible
            .iter()
            .zip(&hidden)
            .enumerate()
            .filter_map(|(index, (a, b))| (a != b).then_some(index))
            .collect();
        assert_eq!(differing.len(), 2, "{target:?}: one byte per shape");
        assert!(
            differing
                .iter()
                .all(|&index| hidden[index] == 1 && visible[index] == 0)
        );
    }
}
