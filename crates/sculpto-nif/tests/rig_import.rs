//! Raw source capture through the public reader.
//!
//! The synthetic files are written by this crate's own writers, so every
//! assertion describes the adapter projection: which imported nodes become rig
//! joints, what the per-shape source record holds, and that the existing shape
//! arrays are unchanged. The two real bodies are private host fixtures reached
//! through environment variables; without them the controls print a skip and
//! claim nothing.

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::REST_TOLERANCE;
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::rig::{JointId, LocalTransform};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::{NifDocument, NifRigError, read, yup_matrix_to_zup, yup_transform_to_zup};

fn triangle() -> MeshAsset {
    MeshAsset {
        name: "Body".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    }
}

fn skinned() -> MeshAsset {
    let mut asset = triangle();
    asset.name = "Skinned".into();
    asset.skin = Some(Skin {
        bones: vec!["Root".into(), "Spine".into()],
        // Empty means the identity fallback for every bone.
        bind_inverse: Vec::new(),
        influences: SkinWeights::from_quads(&[[0, 1, 0, 0]; 3], &[[0.75, 0.25, 0.0, 0.0]; 3])
            .expect("four-lane rows"),
    });
    asset
}

fn identity() -> [[f32; 4]; 4] {
    let mut matrix = [[0.0f32; 4]; 4];
    matrix[0][0] = 1.0;
    matrix[1][1] = 1.0;
    matrix[2][2] = 1.0;
    matrix[3][3] = 1.0;
    matrix
}

fn is_identity(matrix: &[[f32; 4]; 4]) -> bool {
    let unit = identity();
    matrix
        .iter()
        .flatten()
        .zip(unit.iter().flatten())
        .all(|(actual, expected)| (actual - expected).abs() <= 1e-6)
}

#[test]
fn an_unskinned_shape_captures_no_graph_and_no_skin_context() {
    let bytes = write_se_shape(&triangle()).expect("an unskinned shape writes");
    let document = read(&bytes).expect("the written file reads");

    assert_eq!(document.shapes.len(), 1);
    assert!(
        document.rig.is_none(),
        "a bare shape has no imported node, so it has no graph"
    );
    assert_eq!(document.sources.len(), 1);

    let source = &document.sources[0];
    assert_eq!(source.parent, None);
    assert_eq!(source.local, LocalTransform::default());
    assert_eq!(source.global_skin, None);
    assert_eq!(source.skeleton_root, None);
    assert!(document.shapes[0].skin.is_none());
    assert!(matches!(document.bind_rest(), Err(NifRigError::MissingRig)));
}

#[test]
fn a_skinned_shape_captures_the_root_graph_and_its_skin_context() {
    let bytes = write_se_shape(&skinned()).expect("a skinned shape writes");
    let document = read(&bytes).expect("the written file reads");

    let rig = document
        .rig
        .as_ref()
        .expect("a skinned shape hangs under an imported root node");
    assert_eq!(
        rig.joints().len(),
        1,
        "only nodes under the root are imported; the palette bone blocks are not"
    );
    assert_eq!(rig.roots(), &[JointId::new(0)]);
    assert!(
        rig.joint(JointId::new(0))
            .expect("root node")
            .name
            .is_none(),
        "this writer leaves the root node unnamed"
    );

    assert_eq!(document.sources.len(), document.shapes.len());
    let source = &document.sources[0];
    assert_eq!(source.parent, Some(JointId::new(0)));
    assert_eq!(source.local, LocalTransform::default());
    assert_eq!(
        source.skeleton_root.as_deref(),
        Some("Root"),
        "the skin names its first palette bone as the skeleton root"
    );
    assert_eq!(
        source.global_skin,
        Some(identity()),
        "this writer emits an identity global skin transform"
    );

    // The existing shape projection is unchanged by the capture.
    let shape = &document.shapes[0];
    assert_eq!(shape.name, "Skinned");
    let skin = shape.skin.as_ref().expect("the shape carries one skin");
    assert_eq!(skin.bones, vec!["Root".to_string(), "Spine".to_string()]);
    assert_eq!(skin.bind_inverse.len(), 2);
    let row = skin.influences.row(0).expect("vertex 0");
    assert_eq!(row.joints, &[0, 1]);
    assert_eq!(row.weights, &[0.75, 0.25]);
    assert!(matches!(
        document.bind_rest(),
        Err(NifRigError::Root { shape: 0, .. })
    ));
}

#[test]
fn the_le_writer_captures_the_same_root_shape() {
    let bytes = write_le_shape(&skinned()).expect("an LE skinned shape writes");
    let document = read(&bytes).expect("the LE file reads");

    assert_eq!(document.bsver, 83);
    let rig = document.rig.as_ref().expect("the LE skin root is imported");
    assert_eq!(rig.joints().len(), 1);
    assert_eq!(rig.roots(), &[JointId::new(0)]);
    assert_eq!(document.sources.len(), 1);
    assert_eq!(document.sources[0].skeleton_root.as_deref(), Some("Root"));
}

#[test]
fn placement_and_global_skin_share_one_basis_rule() {
    // A Y-up placement converts its translation the same way a Y-up matrix does:
    // +5 along the Y-up axis is +5 along source Z.
    let transform = yup_transform_to_zup([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], 1.0);
    assert_eq!(transform.translation, [1.0, -3.0, 2.0]);

    let mut matrix = identity();
    matrix[3] = [0.0, 5.0, 0.0, 1.0];
    let moved = yup_matrix_to_zup(&matrix);
    assert_eq!([moved[3][0], moved[3][1], moved[3][2]], [0.0, 0.0, 5.0]);
}

fn palette_counts(document: &NifDocument) -> Vec<usize> {
    let mut counts: Vec<usize> = document
        .shapes
        .iter()
        .map(|shape| shape.skin.as_ref().map_or(0, |skin| skin.bones.len()))
        .collect();
    counts.sort_unstable();
    counts
}

fn print_capture(label: &str, document: &NifDocument) {
    let rig = document.rig.as_ref().expect("a captured graph");
    eprintln!(
        "{label}: graph {} nodes, roots {:?}, palettes {:?}",
        rig.joints().len(),
        rig.roots(),
        palette_counts(document)
    );
    for (index, source) in document.sources.iter().enumerate() {
        let shape = &document.shapes[index];
        eprintln!(
            "  shape[{}] `{}` parent {:?} skeleton_root {:?}",
            index, shape.name, source.parent, source.skeleton_root
        );
        if let Some(global) = source.global_skin {
            eprintln!(
                "    G translation {:?}",
                [global[3][0], global[3][1], global[3][2]]
            );
        }
    }
}

fn measure_bound_rest(document: &NifDocument, expects_correction: bool) {
    let binding = document
        .bind_rest()
        .expect("source rig reconstructs at rest");
    assert!(std::ptr::eq(
        binding.rig(),
        document.rig.as_ref().expect("source rig")
    ));
    let basis = binding.basis();
    assert_eq!(
        basis != identity().map(|column| column.map(f64::from)),
        expects_correction
    );
    assert_eq!(binding.skins().len(), document.shapes.len());
    for shape in binding.skins() {
        let source = shape.source();
        assert_eq!(shape.palette().len(), source.skin.bones.len());
        let measurement = shape.measurement();
        eprintln!(
            "  rest `{}`: {} vertices, joint diagnostic {:.9e}, influence {:.9e}, vertex {:.9e}",
            document.shapes[source.shape].name,
            source.positions.len(),
            measurement.max_joint,
            measurement.max_influence,
            measurement.max_vertex
        );
        assert!(measurement.max_influence <= REST_TOLERANCE);
        assert!(measurement.max_vertex <= REST_TOLERANCE);
    }
}

#[test]
fn himbo_body_raw_capture() {
    let Ok(path) = std::env::var("SCULPTO_HIMBO_NIF") else {
        eprintln!("SCULPTO_HIMBO_NIF not set; skipping the raw HIMBO capture control");
        return;
    };
    let bytes = std::fs::read(&path).expect("the HIMBO fixture is readable");
    let document = read(&bytes).expect("the HIMBO body reads");
    print_capture("HIMBO", &document);
    measure_bound_rest(&document, false);

    let rig = document
        .rig
        .as_ref()
        .expect("a body NIF carries imported nodes");
    assert_eq!(
        rig.joints().len(),
        25,
        "the raw HIMBO body graph has 25 imported nodes"
    );
    assert!(!rig.roots().is_empty(), "a graph has at least one root");
    assert!(
        rig.roots()
            .iter()
            .all(|root| rig.joint(*root).expect("root").parent.is_none()),
        "every root is parentless"
    );
    assert_eq!(
        palette_counts(&document),
        vec![5, 24],
        "the HIMBO body carries a 24-bone body palette and a 5-bone Boxers palette"
    );
    assert_eq!(document.shapes.len(), 2);
    assert_eq!(document.sources.len(), document.shapes.len());
    assert!(
        rig.joints().len() > 24,
        "the graph carries nodes the largest palette does not name"
    );
    for source in &document.sources {
        if let Some(parent) = source.parent {
            assert!(
                parent.index() < rig.joints().len(),
                "parent stays in the graph"
            );
        }
    }
}

#[test]
fn bijin_body_raw_capture() {
    let Ok(path) = std::env::var("SCULPTO_BIJIN_NIF") else {
        eprintln!("SCULPTO_BIJIN_NIF not set; skipping the raw Bijin capture control");
        return;
    };
    let bytes = std::fs::read(&path).expect("the Bijin fixture is readable");
    let document = read(&bytes).expect("the Bijin body reads");
    print_capture("Bijin", &document);
    measure_bound_rest(&document, true);

    let rig = document
        .rig
        .as_ref()
        .expect("a body NIF carries imported nodes");
    assert_eq!(
        rig.joints().len(),
        23,
        "the raw Bijin body graph has 23 imported nodes"
    );
    assert!(!rig.roots().is_empty(), "a graph has at least one root");
    assert_eq!(palette_counts(&document), vec![22]);
    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.sources.len(), 1);

    let source = &document.sources[0];
    let global = source.global_skin.expect("the Bijin body is skinned");
    assert!(
        !is_identity(&global),
        "the accepted Bijin body carries a nonidentity G, measured {:?}",
        [global[3][0], global[3][1], global[3][2]]
    );
    assert!(
        rig.joints().len() > document.shapes[0].skin.as_ref().expect("skin").bones.len(),
        "the graph carries nodes the palette does not name"
    );
}

#[test]
fn three_ba_body_rest_reconstructs_in_its_authored_frame() {
    let Ok(path) = std::env::var("SCULPTO_3BA_NIF") else {
        eprintln!("SCULPTO_3BA_NIF not set; skipping the configured 3BA rest control");
        return;
    };
    let bytes = std::fs::read(path).expect("the 3BA fixture is readable");
    let document = read(&bytes).expect("the 3BA body reads");
    let body = document
        .shapes
        .iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a body");
    assert_eq!(body.positions.len(), 18436);
    assert_eq!(
        body.skin.as_ref().expect("the body has a skin").bones.len(),
        49
    );
    assert_eq!(document.shapes.len(), 3);
    measure_bound_rest(&document, false);
}

#[test]
fn configured_family_templates_restore_hierarchy_and_preserve_body_rest() {
    let Some(root) = std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES") else {
        eprintln!("SCULPTO_RIG_TEMPLATE_FIXTURES not set; skipping family skeleton completion");
        return;
    };
    let root = std::path::PathBuf::from(root);
    for (body_file, template_file, body_root, template_root, count, expects_correction) in [
        ("himbo.nif", "skeleton_male_sse.nif", "Scene Root", "Scene Root", 514, false),
        ("bijin.nif", "skeleton_female_sse.nif", "CalienteArmor.00", "skeleton_female.nif", 363, true),
        ("3ba.nif", "skeleton_female_sse.nif", "Scene Root", "skeleton_female.nif", 363, false),
    ] {
        let body = read(&std::fs::read(root.join(body_file)).expect("configured body")).expect("body reads");
        let template = read(&std::fs::read(root.join(template_file)).expect("configured family template"))
            .expect("template reads");
        let source_rig = body.rig.as_ref().expect("authored graph");
        let template_rig = template.rig.as_ref().expect("family graph");
        let completed = complete_family_rig(source_rig, template_rig, TemplateRoots {
            authored: source_rig.joint_by_name(body_root).expect("authored scene root"),
            template: template_rig.joint_by_name(template_root).expect("family scene root"),
        }).expect("compatible family graph");
        assert_eq!(completed.joints().len(), count);
        assert_eq!(completed.roots(), source_rig.roots());
        for &id in source_rig.topological_order() {
            assert_eq!(source_rig.joint(id).expect("source").name, completed.joint(id).expect("completed").name);
        }
        let thigh = completed.joint_by_name("NPC L Thigh [LThg]").expect("left thigh");
        let foot = completed.joint_by_name("NPC L Foot [Lft ]").expect("left foot");
        assert!(completed.is_ancestor(thigh, foot).expect("valid limb chain"));
        let binding = body.bind_rest_on(&completed).expect("completed rig keeps the authored rest");
        assert_eq!(
            binding.basis() != identity().map(|column| column.map(f64::from)),
            expects_correction,
        );
        for skin in binding.skins() {
            let source = skin.source();
            let measured = skin.measurement();
            assert!(std::ptr::eq(source.skin, body.shapes[source.shape].skin.as_ref().expect("authored skin")));
            assert!(measured.max_influence <= REST_TOLERANCE && measured.max_vertex <= REST_TOLERANCE);
            eprintln!(
                "{body_file}: graph {} -> {}, shape {}, {} vertices, rest error {:.9e}",
                source_rig.joints().len(),
                completed.joints().len(),
                source.shape,
                source.positions.len(),
                measured.max_vertex,
            );
        }
    }
}
