//! Read a GLB model via the `gltf` crate and report mesh and skin counts.

use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [p] => p.as_str(),
        _ => "tools/bevy-viewer/tests/fixtures/synthetic.glb",
    };

    let bytes = fs::read(path).unwrap_or_else(|err| panic!("failed to read '{path}': {err}"));
    let parsed = gltf::Gltf::from_slice(&bytes)
        .unwrap_or_else(|err| panic!("failed to parse '{path}': {err}"));
    let doc = parsed.document;

    let mut total_verts = 0;
    let mut total_tris = 0;
    for mesh in doc.meshes() {
        for prim in mesh.primitives() {
            if let Some(pos) = prim.get(&gltf::Semantic::Positions) {
                total_verts += pos.count();
            }
            if let Some(ind) = prim.indices() {
                total_tris += ind.count() / 3;
            }
        }
    }
    let total_joints: usize = doc.skins().map(|s| s.joints().count()).sum();
    let total_materials: usize = doc.materials().filter(|m| m.index().is_some()).count();

    println!(
        "GLTF_CRATE: vertices={total_verts} triangles={total_tris} joints={total_joints} materials={total_materials}"
    );
}
