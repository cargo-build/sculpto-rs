//! MOD-02: a real mod NIF's shader and texture set survives read → write → read.
//!
//! The source is the Apachii Divine Elegance nightshade robe the pipeline census
//! reads. Every assertion is a fact the source authored: the slot table, the
//! shader feature gates, the alpha property and the scalar shader values.
//!
//! `SCULPTO_MOD_FIXTURE` gates both tests; unset, each prints a skip notice and
//! returns, so a missing host fixture is never a silent pass. @req DECL0008

use std::path::{Path, PathBuf};

use sculpto_core::mesh::MeshAsset;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::{NifDocument, NifShape};

/// The catalogued mod fixture root.
const ROOT_VARIABLE: &str = "SCULPTO_MOD_FIXTURE";
/// The nightshade robe: four shapes, one of them the robe body.
const ROBE: &str = "apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif";

/// The fixture NIF, or `None` with a printed skip notice.
fn robe_path() -> Option<PathBuf> {
    let Some(root) = std::env::var_os("SCULPTO_MOD_FIXTURE") else {
        eprintln!("SKIP material roundtrip: {ROOT_VARIABLE} unset");
        return None;
    };
    Some(Path::new(&root).join(ROBE))
}

/// Read the fixture, or `None` when the fixture is unset.
fn document() -> Option<NifDocument> {
    let path = robe_path()?;
    let bytes = std::fs::read(&path).expect("fixture NIF is readable");
    Some(sculpto_nif::read(&bytes).expect("fixture NIF parses"))
}

/// The one shape the robe NIF authors a shader block for.
fn robe(document: &NifDocument) -> &NifShape {
    document
        .shapes
        .iter()
        .find(|shape| !shape.materials.is_empty())
        .expect("the fixture authors a shader block on the robe")
}

#[test]
fn robe_shape_carries_the_authored_texture_set() {
    let Some(document) = document() else {
        return;
    };
    assert_eq!(
        document.shapes.len(),
        4,
        "nightshade robef_1.nif shape count"
    );
    let slot = &robe(&document).materials[0];
    let diffuse = slot
        .texture_set
        .first()
        .expect("the source authors a diffuse slot");
    assert!(
        diffuse.starts_with("textures\\"),
        "authored diffuse path is `{diffuse}`"
    );
    assert!(
        !slot.name.is_empty(),
        "the slot is labelled from the shape or the shader block"
    );
}

#[test]
fn material_roundtrip() {
    let Some(document) = document() else {
        return;
    };
    let source = robe(&document);
    let authored = &source.materials[0];
    assert!(
        !authored.texture_set.is_empty(),
        "the source authors a texture-set table"
    );

    // One authored SE NIF per source shape, read back through the same adapter.
    let bytes = write_se_shape(&MeshAsset::from(source)).expect("the robe shape writes");
    let roundtripped = sculpto_nif::read(&bytes).expect("the written NIF parses");
    let read = &roundtripped.shapes[0].materials[0];

    assert_eq!(
        read.texture_set, authored.texture_set,
        "texture paths keep their authored slot order"
    );
    assert_eq!(read.flags, authored.flags, "shader feature gates");
    assert_eq!(read.alpha, authored.alpha, "alpha property");
    assert_eq!(read.specular_power, authored.specular_power);
    assert_eq!(read.emissive_color, authored.emissive_color);
    assert_eq!(read.emissive_multiple, authored.emissive_multiple);
    assert_eq!(read.skin_tint, authored.skin_tint);

    // The role view is the vendored slot table's reading of the same table, so
    // it must survive too — the specular role only resolves under the source's
    // own model-space-normal or back-lighting rule.
    assert_eq!(read.glow, authored.glow);
    assert_eq!(read.environment, authored.environment);
    assert_eq!(read.environment_mask, authored.environment_mask);
    assert_eq!(read.specular, authored.specular);

    // A per-shape model projection keeps the slot: the export model is what the
    // writers read.
    let model = MeshAsset::from(source);
    assert_eq!(model.materials.len(), 1);
    assert_eq!(model.materials[0], *authored);
}
