//! Named `.osd` morph-container decoding. Bare `.bsd` record sets are covered by
//! `shape_data_write.rs`, which owns the writer and the format split.

use sculpto_core::{ErrorKind, MorphContainer, shape_data::MAGIC};

type Offset = (u16, [f32; 3]);
type Block<'a> = (&'a str, &'a [Offset]);

fn container(blocks: &[Block<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (name, offsets) in blocks {
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(offsets.len() as u16).to_le_bytes());
        for (index, delta) in *offsets {
            out.extend_from_slice(&index.to_le_bytes());
            for value in *delta {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    out
}

#[test]
fn parses_blocks_and_offsets() {
    let bytes = container(&[
        (
            "HIMBO Body - Vanilla",
            &[(3, [0.1, -0.2, 0.3]), (42, [0.0, 1.0, -1.0])],
        ),
        ("HIMBO Body - Lean", &[(7, [0.5, 0.5, 0.5])]),
    ]);
    let parsed = MorphContainer::parse(&bytes).expect("container parses");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.blocks.len(), 2);
    assert_eq!(parsed.blocks[0].name, "HIMBO Body - Vanilla");
    assert_eq!(parsed.blocks[0].offsets[1].index, 42);
    assert_eq!(parsed.blocks[0].offsets[1].delta, [0.0, 1.0, -1.0]);
    assert_eq!(parsed.offset_count(), 3);
    assert_eq!(parsed.max_index(), Some(42));
}

#[test]
fn empty_container_is_valid() {
    let parsed = MorphContainer::parse(&container(&[])).expect("container parses");
    assert!(parsed.blocks.is_empty());
    assert_eq!(parsed.offset_count(), 0);
    assert_eq!(parsed.max_index(), None);
}

#[test]
fn bad_magic_is_malformed() {
    let mut bytes = container(&[]);
    bytes[0] = b'X';
    let error = MorphContainer::parse(&bytes).expect_err("magic must be checked");
    assert_eq!(error.kind(), ErrorKind::Malformed);
}

#[test]
fn unsupported_version_is_rejected() {
    let mut bytes = container(&[]);
    bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
    let error = MorphContainer::parse(&bytes).expect_err("version 2 must fail");
    assert_eq!(error.kind(), ErrorKind::UnsupportedVersion);
}

#[test]
fn truncated_record_is_malformed() {
    let mut bytes = container(&[("Body", &[(1, [1.0, 2.0, 3.0])])]);
    bytes.truncate(bytes.len() - 1);
    let error = MorphContainer::parse(&bytes).expect_err("truncation must fail");
    assert_eq!(error.kind(), ErrorKind::Malformed);
}

#[test]
fn trailing_bytes_are_malformed() {
    let mut bytes = container(&[("Body", &[(1, [0.0, 0.0, 0.0])])]);
    bytes.push(0);
    let error = MorphContainer::parse(&bytes).expect_err("the walk must end at EOF");
    assert_eq!(error.kind(), ErrorKind::Malformed);
    assert!(error.to_string().contains("trailing"));
}

#[test]
fn duplicate_block_is_rejected() {
    let bytes = container(&[("Body", &[]), ("Body", &[])]);
    let error = MorphContainer::parse(&bytes).expect_err("duplicate block must fail");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("duplicate block"));
}

#[test]
fn duplicate_vertex_offset_is_rejected() {
    let bytes = container(&[("Body", &[(5, [0.0, 0.0, 0.0]), (5, [1.0, 1.0, 1.0])])]);
    let error = MorphContainer::parse(&bytes).expect_err("duplicate index must fail");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("vertex 5"));
}

#[test]
fn block_count_beyond_file_is_malformed() {
    let mut bytes = container(&[]);
    bytes[8..12].copy_from_slice(&1000u32.to_le_bytes());
    let error = MorphContainer::parse(&bytes).expect_err("absurd block count must fail");
    assert_eq!(error.kind(), ErrorKind::Malformed);
}
