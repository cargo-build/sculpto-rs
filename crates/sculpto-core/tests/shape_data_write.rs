//! `.osd` and bare `.bsd` morph data: exact wire bytes, read and written.
//!
//! The byte fixtures are hand-encoded here, independently of the writer, so a
//! layout regression cannot hide behind a shared encoder.

use std::path::PathBuf;

use sculpto_core::shape_data::MAGIC;
use sculpto_core::shape_data::{self, MorphData, RECORD_EPSILON, VertexOffset};
use sculpto_core::{ErrorKind, MorphBlock, MorphContainer};

/// One named block `Body` with two records: `(1, [0.5, -0.25, 0.0])` and
/// `(2, [0.0, 0.0, 1.0])`.
#[rustfmt::skip]
const OSD_BYTES: [u8; 47] = [
    0x00, 0x44, 0x53, 0x4f,             // tag 0x4f534400: NUL, 'D', 'S', 'O'
    0x01, 0x00, 0x00, 0x00,             // version 1
    0x01, 0x00, 0x00, 0x00,             // one block
    0x04,                               // name length in bytes
    b'B', b'o', b'd', b'y',             // "Body"
    0x02, 0x00,                         // two records
    0x01, 0x00,                         // index 1
    0x00, 0x00, 0x00, 0x3f,             // 0.5
    0x00, 0x00, 0x80, 0xbe,             // -0.25
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x02, 0x00,                         // index 2
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x00, 0x00, 0x80, 0x3f,             // 1.0
];

/// A bare record set with two records: `(1, [0.5, 0.0, 0.0])` and
/// `(9, [0.0, 0.0, -2.0])`. No tag, no version, no name.
#[rustfmt::skip]
const BSD_BYTES: [u8; 36] = [
    0x02, 0x00, 0x00, 0x00,             // two records
    0x01, 0x00, 0x00, 0x00,             // 32-bit index 1
    0x00, 0x00, 0x00, 0x3f,             // 0.5
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x09, 0x00, 0x00, 0x00,             // 32-bit index 9
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x00, 0x00, 0x00, 0x00,             // 0.0
    0x00, 0x00, 0x00, 0xc0,             // -2.0
];

fn offset(index: u16, delta: [f32; 3]) -> VertexOffset {
    VertexOffset { index, delta }
}

fn block(name: &str, offsets: Vec<VertexOffset>) -> MorphBlock {
    MorphBlock {
        name: name.to_string(),
        offsets,
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sculpto-shape-data-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn osd_bytes_are_exact() {
    let parsed = MorphContainer::parse(&OSD_BYTES).expect("fixture parses");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.blocks[0].name, "Body");
    assert_eq!(
        parsed.blocks[0].offsets,
        vec![offset(1, [0.5, -0.25, 0.0]), offset(2, [0.0, 0.0, 1.0])]
    );
    assert_eq!(parsed.to_osd_bytes().expect("writes"), OSD_BYTES);
}

#[test]
fn bare_bsd_bytes_are_exact() {
    let parsed = shape_data::parse_bare_bsd(&BSD_BYTES).expect("fixture parses");
    assert_eq!(
        parsed,
        vec![offset(1, [0.5, 0.0, 0.0]), offset(9, [0.0, 0.0, -2.0])]
    );
    assert_eq!(
        shape_data::to_bsd_bytes(&parsed).expect("writes"),
        BSD_BYTES
    );
    assert_eq!(
        MorphData::Bare(parsed).to_bytes().expect("writes"),
        BSD_BYTES
    );
}

#[test]
fn the_two_layouts_are_not_interchangeable() {
    assert_eq!(OSD_BYTES[..4], MAGIC);
    assert_ne!(BSD_BYTES[..4], MAGIC);
    // A bare set is never re-read as a named container, and an OSD is never a
    // bare set: neither layout is guessed from the bytes alone.
    let error = MorphContainer::parse(&BSD_BYTES).expect_err("a bare set has no tag");
    assert_eq!(error.kind(), ErrorKind::Malformed);
    let error = shape_data::parse_bare_bsd(&OSD_BYTES).expect_err("an OSD has no count header");
    assert_eq!(error.kind(), ErrorKind::Malformed);
}

#[test]
fn empty_payloads_are_valid() {
    let empty_osd = MorphContainer {
        version: 1,
        blocks: Vec::new(),
    };
    let bytes = empty_osd.to_osd_bytes().expect("writes");
    assert_eq!(bytes.len(), 12);
    assert_eq!(
        MorphContainer::parse(&bytes).expect("empty container parses"),
        empty_osd
    );

    let empty_bare = shape_data::to_bsd_bytes(&[]).expect("writes");
    assert_eq!(empty_bare, 0u32.to_le_bytes());
    assert!(
        shape_data::parse_bare_bsd(&empty_bare)
            .expect("empty set parses")
            .is_empty()
    );
}

#[test]
fn parse_write_parse_round_trips_both_layouts() {
    let model = MorphContainer {
        version: 1,
        blocks: vec![
            block("HIMBO Body - Vanilla", vec![offset(3, [0.1, -0.2, 0.3])]),
            block("Empty", Vec::new()),
        ],
    };
    let bytes = model.to_osd_bytes().expect("writes");
    assert_eq!(MorphContainer::parse(&bytes).expect("parses"), model);

    let offsets = vec![offset(0, [1.0, 0.0, 0.0]), offset(65535, [0.0, 0.0, 2.0])];
    let bytes = shape_data::to_bsd_bytes(&offsets).expect("writes");
    assert_eq!(shape_data::parse_bare_bsd(&bytes).expect("parses"), offsets);
}

#[test]
fn records_snap_epsilon_on_read_in_both_layouts() {
    let model = MorphContainer {
        version: 1,
        blocks: vec![block(
            "Body",
            vec![offset(4, [0.00009, RECORD_EPSILON, -RECORD_EPSILON])],
        )],
    };
    let parsed = MorphContainer::parse(&model.to_osd_bytes().expect("writes"))
        .expect("parses")
        .blocks[0]
        .offsets[0]
        .delta;
    assert_eq!(
        parsed[0].to_bits(),
        0.0f32.to_bits(),
        "below epsilon reads back as +0.0"
    );
    assert_eq!(parsed[1], RECORD_EPSILON, "the boundary itself survives");
    assert_eq!(parsed[2], -RECORD_EPSILON);

    let offsets = vec![offset(2, [-0.00005, 0.00005, 0.5])];
    let parsed = shape_data::parse_bare_bsd(&shape_data::to_bsd_bytes(&offsets).expect("writes"))
        .expect("parses");
    assert_eq!(parsed[0].delta[0].to_bits(), 0.0f32.to_bits());
    assert_eq!(parsed[0].delta[1].to_bits(), 0.0f32.to_bits());
    assert_eq!(parsed[0].delta[2], 0.5);
}

#[test]
fn osd_write_refuses_what_the_reader_cannot_accept() {
    let long_name = "x".repeat(256);
    for model in [
        MorphContainer {
            version: 2,
            blocks: Vec::new(),
        },
        MorphContainer {
            version: 1,
            blocks: vec![block("", Vec::new())],
        },
        MorphContainer {
            version: 1,
            blocks: vec![block(&long_name, Vec::new())],
        },
        MorphContainer {
            version: 1,
            blocks: vec![block("Body", Vec::new()), block("Body", Vec::new())],
        },
        MorphContainer {
            version: 1,
            blocks: vec![block("Body", vec![offset(1, [f32::NAN, 0.0, 0.0])])],
        },
        MorphContainer {
            version: 1,
            blocks: vec![block(
                "Body",
                vec![offset(1, [0.0, 0.0, 0.0]), offset(1, [1.0, 1.0, 1.0])],
            )],
        },
    ] {
        assert!(
            model.to_osd_bytes().is_err(),
            "the writer must refuse a model the reader rejects: {model:?}"
        );
    }

    // Exactly `u16::MAX + 1` records exceed the per-block wire count.
    let too_many: Vec<VertexOffset> = (0..=u16::MAX)
        .map(|index| offset(index, [0.0, 0.0, 0.0]))
        .collect();
    let error = MorphContainer {
        version: 1,
        blocks: vec![block("Body", too_many)],
    }
    .to_osd_bytes()
    .expect_err("a record count above u16::MAX has no wire form");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("u16"), "{error}");
}

#[test]
fn osd_parse_refuses_malformed_input() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("tag", {
            let mut bytes = OSD_BYTES.to_vec();
            bytes[0] = b'X';
            bytes
        }),
        ("version", {
            let mut bytes = OSD_BYTES.to_vec();
            bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
            bytes
        }),
        ("truncated", OSD_BYTES[..OSD_BYTES.len() - 1].to_vec()),
        ("trailing", {
            let mut bytes = OSD_BYTES.to_vec();
            bytes.push(0);
            bytes
        }),
        ("impossible block count", {
            let mut bytes = OSD_BYTES.to_vec();
            bytes[8..12].copy_from_slice(&1000u32.to_le_bytes());
            bytes
        }),
        ("duplicate index", {
            let mut bytes = OSD_BYTES.to_vec();
            // First record index 1 -> 2 duplicates the second record's index.
            bytes[19] = 0x02;
            bytes
        }),
        ("non-finite delta", {
            let mut bytes = OSD_BYTES.to_vec();
            // First record x component.
            bytes[21..25].copy_from_slice(&f32::NAN.to_le_bytes());
            bytes
        }),
    ];
    let mut kinds = Vec::new();
    for (what, bytes) in cases {
        let error = MorphContainer::parse(&bytes).expect_err(what);
        kinds.push((what, error.kind()));
    }
    assert_eq!(
        kinds,
        vec![
            ("tag", ErrorKind::Malformed),
            ("version", ErrorKind::UnsupportedVersion),
            ("truncated", ErrorKind::Malformed),
            ("trailing", ErrorKind::Malformed),
            ("impossible block count", ErrorKind::Malformed),
            ("duplicate index", ErrorKind::InvalidValue),
            ("non-finite delta", ErrorKind::InvalidValue),
        ]
    );
}

#[test]
fn bare_parse_refuses_malformed_input() {
    let cases: Vec<(&str, Vec<u8>, ErrorKind)> = vec![
        (
            "count beyond file",
            {
                let mut bytes = BSD_BYTES.to_vec();
                bytes[0..4].copy_from_slice(&100u32.to_le_bytes());
                bytes
            },
            ErrorKind::Malformed,
        ),
        (
            "truncated",
            BSD_BYTES[..BSD_BYTES.len() - 1].to_vec(),
            ErrorKind::Malformed,
        ),
        (
            "trailing",
            {
                let mut bytes = BSD_BYTES.to_vec();
                bytes.push(0);
                bytes
            },
            ErrorKind::Malformed,
        ),
        (
            "duplicate index",
            {
                let mut bytes = BSD_BYTES.to_vec();
                bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
                bytes
            },
            ErrorKind::InvalidValue,
        ),
        (
            "non-finite delta",
            {
                let mut bytes = BSD_BYTES.to_vec();
                bytes[8..12].copy_from_slice(&f32::INFINITY.to_le_bytes());
                bytes
            },
            ErrorKind::InvalidValue,
        ),
    ];
    for (what, bytes, kind) in cases {
        let error = shape_data::parse_bare_bsd(&bytes).expect_err(what);
        assert_eq!(error.kind(), kind, "{what}");
    }
}

#[test]
fn bare_index_above_u16_max_is_refused_not_truncated() {
    // One record at index 65536: the wire carries it, the shared 16-bit offset
    // model cannot, and native's `static_cast<uint16_t>` would silently make it 0.
    #[rustfmt::skip]
    let bytes: [u8; 20] = [
        0x01, 0x00, 0x00, 0x00,             // one record
        0x00, 0x00, 0x01, 0x00,             // 32-bit index 65536
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let error = shape_data::parse_bare_bsd(&bytes).expect_err("width must be checked");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("65536"), "{error}");

    // The boundary itself is representable.
    let offsets = vec![offset(65535, [0.0, 0.0, 0.0])];
    let parsed = shape_data::parse_bare_bsd(&shape_data::to_bsd_bytes(&offsets).expect("writes"))
        .expect("65535 fits");
    assert_eq!(parsed[0].index, 65535);
}

#[test]
fn format_dispatch_uses_the_bsd_suffix_only() {
    let dir = temp_dir("dispatch");

    let named = dir.join("body.osd");
    std::fs::write(&named, OSD_BYTES).expect("fixture");
    let data = MorphData::from_file(&named).expect("named container reads");
    assert!(matches!(data, MorphData::Named(_)));
    assert_eq!(data.offsets("Body").expect("named block").len(), 2);
    assert!(data.offsets("Missing").is_none(), "a name can miss");
    assert_eq!(data.offset_count(), 2);
    assert_eq!(data.max_index(), Some(2));
    assert_eq!(data.to_bytes().expect("writes"), OSD_BYTES);

    let bare = dir.join("body.bsd");
    std::fs::write(&bare, BSD_BYTES).expect("fixture");
    let data = MorphData::from_file(&bare).expect("bare set reads");
    assert!(matches!(data, MorphData::Bare(_)));
    assert_eq!(
        data.offsets("AnyAuthoredName").expect("bare answers").len(),
        2,
        "a bare set answers for the name the project reference supplies"
    );
    assert_eq!(data.offset_count(), 2);
    assert_eq!(data.max_index(), Some(9));
    assert_eq!(data.to_bytes().expect("writes"), BSD_BYTES);

    // A truncated `.osd` stays a broken named container: the suffix, not the
    // bytes, decides the layout, so nothing is reinterpreted as a bare set.
    let truncated = dir.join("truncated.osd");
    std::fs::write(&truncated, &OSD_BYTES[..OSD_BYTES.len() - 1]).expect("fixture");
    let error = MorphData::from_file(&truncated).expect_err("truncation must fail");
    assert_eq!(error.kind(), ErrorKind::Malformed);
    assert!(error.to_string().contains("truncated.osd"), "{error}");
}
