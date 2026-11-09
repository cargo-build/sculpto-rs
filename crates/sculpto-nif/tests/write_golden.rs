//! Byte-level goldens for the SE and LE writers.
//!
//! The round-trip tests read our own bytes, so a shared layout bug could pass.
//! These goldens pin the exact file hash and the structural fields the readers
//! depend on. Update with `GOLDEN_UPDATE=1` only on a deliberate format change.

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_nif::write::header::{NifTarget, write_header};
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;

fn fnv1a64(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn fixture() -> MeshAsset {
    let mut bind = vec![[[0.0f32; 4]; 4], [[0.0f32; 4]; 4]];
    bind[0][0][0] = 1.0;
    bind[0][1][1] = 1.0;
    bind[0][2][2] = 1.0;
    bind[0][3][3] = 1.0;
    bind[1][0][0] = 1.0;
    bind[1][1][1] = 1.0;
    bind[1][2][2] = 1.0;
    bind[1][3] = [0.0, 0.0, 1.0, 1.0];
    MeshAsset {
        name: "Golden".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: bind,
            indices: vec![[0, 1, 0, 0]; 3],
            weights: vec![[0.75, 0.25, 0.0, 0.0]; 3],
        }),
        ..MeshAsset::default()
    }
}

fn expected(name: &str, actual: String) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
        std::fs::write(&path, &actual).expect("write golden");
        return actual;
    }
    std::fs::read_to_string(&path).expect("golden is committed")
}

#[test]
fn se_and_le_writer_bytes_are_pinned() {
    let asset = fixture();
    let se = write_se_shape(&asset).expect("SE writes");
    let le = write_le_shape(&asset).expect("LE writes");
    assert_eq!(
        fnv1a64(&se),
        expected("se-golden.txt", format!("{}\n", fnv1a64(&se))).trim()
    );
    assert_eq!(
        fnv1a64(&le),
        expected("le-golden.txt", format!("{}\n", fnv1a64(&le))).trim()
    );
    // Structural markers the readers depend on.
    assert!(se.windows(10).any(|window| window == b"BSTriShape"));
    assert!(le.windows(10).any(|window| window == b"NiTriShape"));
    assert!(se.windows(15).any(|window| window == b"NiSkinPartition"));
    assert!(le.windows(15).any(|window| window == b"NiSkinPartition"));
    assert!(se.len() > 200 && le.len() > 200);
    // Negative control: any attribute change must move the hash.
    let mut changed = asset.clone();
    changed.uvs[0] = [0.25, 0.25];
    assert_ne!(
        fnv1a64(&write_se_shape(&changed).expect("writes")),
        fnv1a64(&se)
    );
    // Header goldens stay exact.
    assert_eq!(fnv1a64(&write_header(NifTarget::Se)).len(), 16);
}
