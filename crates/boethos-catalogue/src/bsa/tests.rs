//! BSA index tests: synthetic SSE archives in temp directories plus one
//! env-gated real-archive gate. No fixture bytes enter version control.

use super::*;
use std::io::Write as _;

/// How the synthetic builder marks members and archive defaults.
#[derive(Default, Clone, Copy)]
pub(crate) struct Build {
    /// Set the archive's default-compressed flag (`0x4`).
    archive_compressed: bool,
    /// Set the per-member compression bits (`member XOR archive default`).
    member_compressed: bool,
    /// Set the full-name-prefix flag (`0x100`) and prefix each member's data.
    prefix: bool,
    /// Clear the path/file-name flags; used only by the flags refusal test.
    without_name_flags: bool,
}

/// One member specification.
pub(crate) struct Spec<'a> {
    /// Member folder, backslash separators allowed.
    pub(crate) folder: &'a str,
    /// Member file name.
    pub(crate) name: &'a str,
    /// Uncompressed payload bytes.
    pub(crate) data: &'a [u8],
}

/// Compress `data` into the LZ4 frame format real SSE members use.
fn lz4_frame(data: &[u8]) -> Vec<u8> {
    let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
    encoder.write_all(data).expect("frame encode");
    encoder.finish().expect("frame finish")
}

/// Effective compression for one member under a build.
fn effective(build: &Build) -> bool {
    build.member_compressed ^ build.archive_compressed
}

/// Stored bytes of one member's payload under a build.
fn stored_payload(spec: &Spec<'_>, build: &Build) -> Vec<u8> {
    if effective(build) {
        // The declared size is the format's u32; `usize::to_le_bytes` would
        // write eight bytes and corrupt every following field.
        let declared = u32::try_from(spec.data.len()).expect("test payload fits u32");
        let mut stored = declared.to_le_bytes().to_vec();
        stored.extend_from_slice(&lz4_frame(spec.data));
        stored
    } else {
        spec.data.to_vec()
    }
}

/// Serialize one minimal, structurally valid SSE v0x69 archive.
///
/// Layout follows the reference reader: header, folder records, then per
/// folder its length-prefixed name and file records, then the NUL-terminated
/// file-name block, then the member data blocks. Shared with the catalogue
/// index/dispatch tests so the synthetic format has one owner.
pub(crate) fn build_bsa(specs: &[Spec<'_>], build: Build) -> Vec<u8> {
    let mut archive_flags = 0x0000_0003;
    if build.archive_compressed {
        archive_flags |= FLAG_COMPRESSED;
    }
    if build.prefix {
        archive_flags |= FLAG_FULL_NAME_PREFIX;
    }
    if build.without_name_flags {
        archive_flags &= !(FLAG_PATH_NAMES | FLAG_FILE_NAMES);
    }
    let folders: Vec<&str> = specs
        .iter()
        .map(|spec| spec.folder)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    // Pass 1: measure, so records carry absolute offsets and the header
    // declares the true folder-name and file-name block lengths.
    let folder_records_len = folders.len() * FOLDER_RECORD_LEN;
    let file_records_len = specs.len() * FILE_RECORD_LEN;
    // One length byte, the name, and the NUL it counts (SSE style).
    let folder_names_len: usize = folders.iter().map(|folder| 2 + folder.len()).sum();
    let file_names_len: usize = specs.iter().map(|spec| spec.name.len() + 1).sum();
    let mut member_offsets = Vec::with_capacity(specs.len());
    let mut cursor =
        HEADER_LEN + folder_records_len + folder_names_len + file_records_len + file_names_len;
    for spec in specs {
        member_offsets.push(cursor);
        cursor += stored_payload(spec, &build).len();
        if build.prefix {
            cursor += 1 + format!("{}/{}", spec.folder, spec.name).len();
        }
    }

    let mut bytes = Vec::with_capacity(cursor);
    let push_u32 = |bytes: &mut Vec<u8>, value: u32| {
        bytes.extend_from_slice(&value.to_le_bytes());
    };
    push_u32(&mut bytes, MAGIC);
    push_u32(&mut bytes, SSE_VERSION);
    push_u32(&mut bytes, HEADER_LEN as u32);
    push_u32(&mut bytes, archive_flags);
    push_u32(&mut bytes, folders.len() as u32);
    push_u32(&mut bytes, specs.len() as u32);
    push_u32(&mut bytes, folder_names_len as u32);
    push_u32(&mut bytes, file_names_len as u32);
    push_u32(&mut bytes, 0);

    // Folder records; each folder's file records begin after every folder
    // record plus the earlier folders' names and file records.
    let mut folder_files_at = HEADER_LEN + folder_records_len;
    let mut starts = Vec::with_capacity(folders.len());
    for folder in &folders {
        let count = specs.iter().filter(|spec| spec.folder == *folder).count();
        starts.push(folder_files_at);
        folder_files_at += 2 + folder.len() + count * FILE_RECORD_LEN;
    }
    for (folder, start) in folders.iter().zip(starts) {
        let count = specs.iter().filter(|spec| spec.folder == *folder).count();
        bytes.extend_from_slice(&0u64.to_le_bytes());
        push_u32(&mut bytes, count as u32);
        push_u32(&mut bytes, 0);
        bytes.extend_from_slice(&(start as u64).to_le_bytes());
    }

    // Per folder: its name, then its file records with data offsets above.
    for folder in &folders {
        bytes.push(folder.len() as u8 + 1);
        bytes.extend_from_slice(folder.as_bytes());
        bytes.push(0);
        for (index, spec) in specs.iter().enumerate() {
            if spec.folder != *folder {
                continue;
            }
            let mut stored = stored_payload(spec, &build).len() as u32;
            if build.prefix {
                stored += 1 + format!("{}/{}", spec.folder, spec.name).len() as u32;
            }
            let size_flags = stored | (u32::from(build.member_compressed) * COMPRESS_MASK);
            bytes.extend_from_slice(&0u64.to_le_bytes());
            push_u32(&mut bytes, size_flags);
            push_u32(&mut bytes, member_offsets[index] as u32);
        }
    }

    // File-name block, NUL-terminated in record order (folder-sorted).
    for folder in &folders {
        for spec in specs.iter().filter(|spec| spec.folder == *folder) {
            bytes.extend_from_slice(spec.name.as_bytes());
            bytes.push(0);
        }
    }

    // Member data blocks.
    for (index, spec) in specs.iter().enumerate() {
        assert_eq!(bytes.len(), member_offsets[index], "layout drift");
        if build.prefix {
            let full = format!("{}/{}", spec.folder, spec.name);
            bytes.push(full.len() as u8);
            bytes.extend_from_slice(full.as_bytes());
        }
        bytes.extend_from_slice(&stored_payload(spec, &build));
    }
    bytes
}

/// Write one archive to a fresh, uniquely named temp file.
pub(crate) fn write_temp(label: &str, bytes: &[u8]) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "sculpto-bsa-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::write(&path, bytes).expect("temp archive writes");
    path
}

/// Write and index one synthetic archive.
fn index_bytes(label: &str, bytes: &[u8]) -> BsaIndex {
    index_bsa(&write_temp(label, bytes)).expect("synthetic archive indexes")
}

const ROBE: &[u8] = b"DDS \x00\x00\x00\x0frobe-diffuse";
const ROBE_N: &[u8] = b"DDS \x00\x00\x00\x0frobe-normal";
const BUCKET: &[u8] = b"Gamebryo File Format";
const NIGHTSHADE_FOLDER: &str = "textures\\divineelegance\\robes\\male\\nightshade";
const NIGHTSHADE: &[Spec<'static>] = &[
    Spec {
        folder: NIGHTSHADE_FOLDER,
        name: "robe.dds",
        data: ROBE,
    },
    Spec {
        folder: NIGHTSHADE_FOLDER,
        name: "robe_n.dds",
        data: ROBE_N,
    },
    Spec {
        folder: "meshes\\clutter",
        name: "bucket.nif",
        data: BUCKET,
    },
];

#[test]
fn index_counts_members_and_reports_sorted_keys() {
    let index = index_bytes("counts", &build_bsa(NIGHTSHADE, Build::default()));
    assert_eq!(index.len(), 3);
    let members: Vec<&str> = index.members().collect();
    assert_eq!(
        members,
        [
            "meshes/clutter/bucket.nif",
            "textures/divineelegance/robes/male/nightshade/robe.dds",
            "textures/divineelegance/robes/male/nightshade/robe_n.dds",
        ]
    );
    assert!(index.contains("meshes/clutter/bucket.nif"));
}

#[test]
fn lookup_normalizes_separators_and_case() {
    let index = index_bytes("normalize", &build_bsa(NIGHTSHADE, Build::default()));
    let mixed = "TEXTURES\\DivineElegance\\Robes\\MALE\\Nightshade\\Robe.DDS";
    assert!(index.contains(mixed));
    assert_eq!(
        index.read_member(mixed).expect("mixed-case key reads"),
        ROBE.to_vec()
    );
}

#[test]
fn member_size_is_uncompressed_size() {
    let build = Build {
        member_compressed: true,
        ..Build::default()
    };
    let index = index_bytes("size", &build_bsa(NIGHTSHADE, build));
    let size = index
        .member_size("meshes/clutter/bucket.nif")
        .expect("size peeks");
    assert_eq!(size, BUCKET.len() as u64);
}

#[test]
fn read_member_roundtrips_uncompressed_bytes() {
    let index = index_bytes("plain", &build_bsa(NIGHTSHADE, Build::default()));
    assert_eq!(
        index
            .read_member("textures\\divineelegance\\robes\\male\\nightshade\\robe_n.dds")
            .expect("member reads"),
        ROBE_N.to_vec()
    );
}

#[test]
fn read_member_decompresses_lz4_frames() {
    let build = Build {
        member_compressed: true,
        ..Build::default()
    };
    let index = index_bytes("lz4", &build_bsa(NIGHTSHADE, build));
    for spec in NIGHTSHADE {
        let key = format!("{}/{}", spec.folder, spec.name);
        assert_eq!(
            index.read_member(&key).expect("compressed member decodes"),
            spec.data.to_vec(),
            "member {key}"
        );
    }
}

#[test]
fn a_toggled_compression_bit_keeps_the_member_uncompressed() {
    // Archive default compressed and member bits set: member XOR default is
    // false, so the payload stays stored bytes.
    let build = Build {
        archive_compressed: true,
        member_compressed: true,
        ..Build::default()
    };
    let index = index_bytes("toggle-off", &build_bsa(NIGHTSHADE, build));
    assert_eq!(
        index
            .read_member("meshes/clutter/bucket.nif")
            .expect("the XOR rule leaves the member stored"),
        BUCKET.to_vec()
    );
}

#[test]
fn prefixed_members_roundtrip_with_and_without_compression() {
    for compressed in [false, true] {
        let build = Build {
            archive_compressed: compressed,
            member_compressed: compressed,
            prefix: true,
            ..Build::default()
        };
        let index = index_bytes("prefix", &build_bsa(NIGHTSHADE, build));
        let key = "textures/divineelegance/robes/male/nightshade/robe.dds";
        assert_eq!(
            index.read_member(key).expect("prefix member reads"),
            ROBE.to_vec()
        );
        assert_eq!(
            index
                .member_size(key)
                .expect("prefix size peeks past the name"),
            ROBE.len() as u64
        );
    }
}

#[test]
fn missing_member_is_named() {
    let index = index_bytes("missing", &build_bsa(NIGHTSHADE, Build::default()));
    assert!(!index.contains("textures/nope.dds"));
    assert_eq!(index.member_size("textures/nope.dds"), None);
    let error = index.read_member("textures/nope.dds").expect_err("absent");
    assert_eq!(error, BsaError::MemberNotFound("textures/nope.dds".into()));
    assert!(error.to_string().contains("textures/nope.dds"));
}

#[test]
fn bad_magic_is_refused() {
    let mut bytes = build_bsa(NIGHTSHADE, Build::default());
    bytes[0..4].copy_from_slice(b"BTDX");
    let error = index_bsa(&write_temp("magic", &bytes)).expect_err("not a BSA");
    assert_eq!(error, BsaError::Magic { found: 0x5844_5442 });
}

#[test]
fn unsupported_version_is_named() {
    let mut bytes = build_bsa(NIGHTSHADE, Build::default());
    bytes[4..8].copy_from_slice(&0x67u32.to_le_bytes());
    let error = index_bsa(&write_temp("version", &bytes)).expect_err("LE version");
    assert_eq!(error, BsaError::Version { found: 0x67 });
}

#[test]
fn missing_name_flags_refused() {
    let build = Build {
        without_name_flags: true,
        ..Build::default()
    };
    let error =
        index_bsa(&write_temp("flags", &build_bsa(NIGHTSHADE, build))).expect_err("no name flags");
    assert!(matches!(error, BsaError::Flags { .. }));
}

#[test]
fn truncated_header_refused() {
    let error = index_bsa(&write_temp("short", &[0u8; 20])).expect_err("short file");
    assert_eq!(
        error,
        BsaError::Truncated { what: "header" },
        "20 bytes cannot hold the 36-byte header"
    );
}

#[test]
fn folder_count_mismatch_refused() {
    let mut bytes = build_bsa(NIGHTSHADE, Build::default());
    bytes[20..24].copy_from_slice(&4u32.to_le_bytes());
    let error = index_bsa(&write_temp("counts", &bytes)).expect_err("counts disagree");
    assert_eq!(
        error,
        BsaError::CountMismatch {
            declared: 4,
            folders: 3,
        }
    );
}

#[test]
fn offset_past_eof_is_refused_at_index_time() {
    // Patch the first folder-sorted file record's data offset far past the
    // file: folder records end at 36+2*24=84; the sorted first folder
    // `meshes\clutter` adds its 16-byte name entry (length byte + name +
    // NUL), so its record starts at 100.
    let mut bytes = build_bsa(NIGHTSHADE, Build::default());
    let record_at = HEADER_LEN + 2 * FOLDER_RECORD_LEN + 2 + "meshes\\clutter".len();
    assert_eq!(record_at, 100);
    bytes[record_at + 12..record_at + 16].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
    let error = index_bsa(&write_temp("offset", &bytes)).expect_err("offset out of range");
    assert_eq!(
        error,
        BsaError::Truncated {
            what: "member data"
        }
    );
}

#[test]
fn corrupt_compressed_payload_refused() {
    // One member, compressed: header(36) + folder record(24) + folder name
    // (1+11) + file record(16) + file name (6+1) = 95; the payload's declared
    // size starts there. Corrupting it makes the decoded length mismatch.
    let spec = &[Spec {
        folder: "textures\\x",
        name: "a.dds",
        data: ROBE,
    }];
    let build = Build {
        member_compressed: true,
        ..Build::default()
    };
    let mut bytes = build_bsa(spec, build);
    let data_at = HEADER_LEN
        + FOLDER_RECORD_LEN
        + 2
        + spec[0].folder.len()
        + FILE_RECORD_LEN
        + spec[0].name.len()
        + 1;
    assert_eq!(bytes.len(), data_at + 4 + lz4_frame(ROBE).len());
    bytes[data_at] = 0xFF;
    let index = index_bsa(&write_temp("corrupt", &bytes)).expect("indexing stays cheap");
    let error = index
        .read_member("textures\\x\\a.dds")
        .expect_err("payload does not decode");
    assert!(matches!(error, BsaError::Decompress { .. }), "{error}");
}

#[test]
fn resolve_texture_prefers_loose_files_over_archives() {
    let dir = std::env::temp_dir().join(format!("sculpto-bsa-loose-{}", std::process::id()));
    let loose = dir.join("textures/divineelegance/robes/male/nightshade/robe.dds");
    std::fs::create_dir_all(loose.parent().expect("parent")).expect("loose dir");
    std::fs::write(&loose, b"loose").expect("loose file");
    let index = index_bytes("loose", &build_bsa(NIGHTSHADE, Build::default()));
    let source = resolve_texture(
        "textures\\divineelegance\\robes\\male\\nightshade\\robe.dds",
        std::slice::from_ref(&dir),
        std::slice::from_ref(&index),
    )
    .expect("loose wins");
    assert_eq!(source, TextureSource::Loose(loose));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_texture_uses_archive_order_first_match_wins() {
    let only_robe = &[Spec {
        folder: NIGHTSHADE_FOLDER,
        name: "robe.dds",
        data: b"second archive copy",
    }];
    let first = index_bytes("order-a", &build_bsa(NIGHTSHADE, Build::default()));
    let second = index_bytes("order-b", &build_bsa(only_robe, Build::default()));
    let path = "textures\\divineelegance\\robes\\male\\nightshade\\robe.dds";

    // The member only in the second archive resolves there.
    let lone = resolve_texture(
        "textures\\divineelegance\\robes\\male\\nightshade\\robe_n.dds",
        &[],
        std::slice::from_ref(&second),
    );
    assert_eq!(lone, None, "the second archive lacks the normal map");

    // Both archives hold the member: the earlier index wins.
    let first_path = PathBuf::from(first.path());
    let both = resolve_texture(path, &[], &[first, second]).expect("first match wins");
    assert_eq!(
        both,
        TextureSource::Archive {
            bsa: first_path,
            member: "textures/divineelegance/robes/male/nightshade/robe.dds".into(),
        }
    );
}

#[test]
fn resolve_texture_missing_is_none() {
    let index = index_bytes("none", &build_bsa(NIGHTSHADE, Build::default()));
    assert_eq!(
        resolve_texture("textures/missing.dds", &[], std::slice::from_ref(&index)),
        None
    );
    assert_eq!(resolve_texture("", &[], &[]), None);
}

#[test]
fn missing_archive_file_is_an_io_error() {
    let error = index_bsa(Path::new("/nonexistent/no.bsa")).expect_err("no file");
    assert!(matches!(error, BsaError::Io { .. }), "{error}");
}

/// Real-archive gate (tier 2): the configured Apachii textures BSA. The
/// expected member count comes from the archive header; extraction proves a
/// real LZ4 member decodes to DDS bytes of its declared size.
#[test]
fn real_archive_indexes_and_extracts_members() {
    let Some(path) = std::env::var_os("SCULPTO_MOD_BSA") else {
        eprintln!("SCULPTO_MOD_BSA not set; skipping real BSA indexing gate");
        return;
    };
    let index = index_bsa(Path::new(&path)).expect("real archive indexes");
    println!("real BSA {} holds {} members", path.display(), index.len());
    assert_eq!(index.len(), 1693, "header-declared member count");
    let key = "textures\\divineelegance\\robes\\male\\nightshade\\robe.dds";
    assert!(index.contains(key), "nightshade diffuse is indexed");
    let size = index.member_size(key).expect("member size peeks");
    let bytes = index.read_member(key).expect("real member extracts");
    assert_eq!(
        bytes.len() as u64,
        size,
        "decoded length equals the declared size"
    );
    assert!(bytes.starts_with(b"DDS "), "a DDS payload round-trips");
    println!("member {key}: {size} bytes, magic {:?}", &bytes[..4]);

    // Second real archive (meshes) exercises a second SSE variant when the
    // sibling exists; its absence skips only this half.
    let sibling = index
        .path()
        .parent()
        .expect("data dir")
        .join("Apachii_DivineEleganceStore.bsa");
    if sibling.is_file() {
        let meshes = index_bsa(&sibling).expect("sibling mesh archive indexes");
        println!(
            "sibling BSA {} holds {} members",
            sibling.display(),
            meshes.len()
        );
        assert_eq!(meshes.len(), 2175, "header-declared member count");
        assert!(meshes.members().any(|member| member.ends_with(".nif")));
    } else {
        eprintln!("sibling meshes archive absent; skipping the second-archive half");
    }
}
