//! Native acceptance of our written **multi-shape** Skyrim NIFs.
//!
//! A three-shape bundle (body + hands + feet, one shared skeleton, distinct
//! geometry and materials) is written once for SE and once for LE, then handed
//! to the native Outfit Studio automation (`ImportFile` -> `ExportFile` with
//! `WithRef`). The re-export is read back with [`sculpto_nif::read`] and must
//! agree with the input bundle on shape count, names, per-shape vertex and
//! triangle counts, and bone lists. The negative control writes only two of the
//! three shapes and requires the native load to report two, not three.
//!
//! Host-gated: the test needs a private Outfit Studio home
//! (`SCULPTO_OS_HOME`, default `/tmp/opencode/c15-bodyslide`) containing the
//! native `OutfitStudio` executable with its `res/` tree, the maintained
//! `skyrim-body` launcher (`SCULPTO_OS_LAUNCHER`), and a display
//! (`SCULPTO_OS_DISPLAY`, else `DISPLAY`). Without any of them the test skips
//! instead of pretending to pass; the recorded evidence names the env used.
//!
//! The Outfit Studio automation exits the tool after the last step, so both
//! targets run to completion without a GUI session.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId, ShaderKind, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::write::le_bundle::write_le_bundle;
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_bundle::write_se_bundle;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::write::skin::identity;
use sculpto_nif::{NifDocument, read};

/// Bone names from `res/skeleton_male_sse.nif`, the skeleton Outfit Studio
/// resolves at startup; a synthetic palette has to name real skeleton nodes or
/// the native import cannot bind the skin.
const BONES: [&str; 3] = ["NPC Root [Root]", "NPC Spine [Spn0]", "NPC Neck [Neck]"];

/// Which writer a target uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Se,
    Le,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::Se => "se",
            Target::Le => "le",
        }
    }

    fn write(
        self,
        shapes: &[MeshAsset],
    ) -> Result<Vec<u8>, sculpto_nif::write::header::NifWriteError> {
        match self {
            Target::Se => write_se_bundle(shapes),
            Target::Le => write_le_bundle(shapes),
        }
    }

    fn write_single(
        self,
        shape: &MeshAsset,
    ) -> Result<Vec<u8>, sculpto_nif::write::header::NifWriteError> {
        match self {
            Target::Se => write_se_shape(shape),
            Target::Le => write_le_shape(shape),
        }
    }
}

/// One skinned shape with explicit geometry, its own influence pair and an
/// optional material; `base` keeps the three shapes apart in space.
fn shape(
    name: &str,
    base: [f32; 3],
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    influence: ([u16; 4], [f32; 4]),
    material: Option<MaterialSlot>,
) -> MeshAsset {
    let vertices = positions.len();
    let positions = positions
        .into_iter()
        .map(|position| {
            [
                position[0] + base[0],
                position[1] + base[1],
                position[2] + base[2],
            ]
        })
        .collect::<Vec<_>>();
    let asset = MeshAsset {
        name: name.to_string(),
        normals: vec![[0.0, 0.0, 1.0]; vertices],
        uvs: (0..vertices)
            .map(|vertex| [f32::from(vertex as u8) * 0.1, 0.0])
            .collect(),
        positions,
        triangles: triangles.clone(),
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        skin: Some(Skin {
            bones: BONES.iter().map(|bone| bone.to_string()).collect(),
            bind_inverse: vec![identity(); BONES.len()],
            influences: SkinWeights::from_quads(
                &vec![influence.0; vertices],
                &vec![influence.1; vertices],
            )
            .expect("four-lane rows"),
        }),
        materials: material.into_iter().collect(),
        ..MeshAsset::default()
    };
    asset.validate().expect("the fixture is a valid model");
    asset
}

/// The three-part body bundle: 6/4, 5/3 and 4/2 vertices/triangles.
fn bundle() -> Vec<MeshAsset> {
    vec![
        shape(
            "Body",
            [0.0, 0.0, 0.0],
            vec![
                [0.0, 0.0, 0.0],
                [0.2, 0.0, 0.0],
                [0.0, 0.2, 0.0],
                [0.2, 0.2, 0.0],
                [0.0, 0.4, 0.0],
                [0.2, 0.4, 0.0],
            ],
            vec![[0, 1, 2], [1, 3, 2], [2, 3, 4], [3, 5, 4]],
            ([0, 1, 2, 0], [0.5, 0.3, 0.2, 0.0]),
            Some(MaterialSlot {
                name: "shp06_body".into(),
                shader: ShaderKind::Cloth,
                diffuse: Some("textures/actors/character/shp06_body_d.dds".into()),
                normal: Some("textures/actors/character/shp06_body_n.dds".into()),
                specular: None,
                double_sided: true,
            }),
        ),
        shape(
            "Hands",
            [0.5, 0.0, 0.0],
            vec![
                [0.0, 0.0, 0.0],
                [0.1, 0.0, 0.0],
                [0.0, 0.1, 0.0],
                [0.1, 0.1, 0.0],
                [0.0, 0.2, 0.0],
            ],
            vec![[0, 1, 2], [1, 3, 2], [2, 3, 4]],
            ([2, 0, 1, 0], [0.4, 0.4, 0.2, 0.0]),
            Some(MaterialSlot {
                name: "shp06_hands".into(),
                shader: ShaderKind::Skin,
                diffuse: Some("textures/actors/character/shp06_hands_d.dds".into()),
                normal: None,
                specular: None,
                double_sided: false,
            }),
        ),
        shape(
            "Feet",
            [1.0, 0.0, 0.0],
            vec![
                [0.0, 0.0, 0.0],
                [0.3, 0.0, 0.0],
                [0.0, 0.15, 0.0],
                [0.3, 0.15, 0.0],
            ],
            vec![[0, 1, 2], [1, 3, 2]],
            ([1, 2, 0, 0], [0.6, 0.3, 0.1, 0.0]),
            None,
        ),
    ]
}

/// Per-shape counts read from an input or re-exported document, keyed by name.
fn census(document: &NifDocument) -> BTreeMap<String, (usize, usize)> {
    document
        .shapes
        .iter()
        .map(|shape| {
            (
                shape.name.clone(),
                (shape.positions.len(), shape.triangles.len()),
            )
        })
        .collect()
}

/// Per-shape bone names, keyed by shape name; `None` for an unskinned shape.
fn palettes(document: &NifDocument) -> BTreeMap<String, Option<Vec<String>>> {
    document
        .shapes
        .iter()
        .map(|shape| {
            (
                shape.name.clone(),
                shape
                    .skin
                    .as_ref()
                    .map(|skin| skin.bones.iter().map(|bone| bone.to_string()).collect()),
            )
        })
        .collect()
}

/// The host fixture: Outfit Studio home, launcher and display.
struct Host {
    home: PathBuf,
    launcher: PathBuf,
    display: String,
}

/// Resolve the host fixture, or `None` when this machine cannot run it.
fn host() -> Option<Host> {
    let home = std::env::var_os("SCULPTO_OS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/opencode/c15-bodyslide"));
    if !home.join("OutfitStudio").exists() {
        eprintln!("no native OutfitStudio under {home:?}; skipping");
        return None;
    }
    let launcher = std::env::var_os("SCULPTO_OS_LAUNCHER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join("src/work/just-poker-wrapper/scripts/skyrim-body")
        });
    if !launcher.exists() {
        eprintln!("no skyrim-body launcher at {launcher:?}; skipping");
        return None;
    }
    let display = std::env::var("SCULPTO_OS_DISPLAY")
        .or_else(|_| std::env::var("DISPLAY"))
        .ok();
    let Some(display) = display.filter(|display| !display.is_empty()) else {
        eprintln!("no DISPLAY for the private X server; skipping");
        return None;
    };
    Some(Host {
        home,
        launcher,
        display,
    })
}

/// Write `bytes` as `name` into `dir`, removing any stale copy first.
fn fresh_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    fs::write(&path, bytes).expect("write the fixture NIF");
    path
}

/// Write the automation script and run the native tool once.
fn run_native(host: &Host, output_dir: &Path, script: &str, import: &Path, export: &Path) {
    let automations = host.home.join("Automations");
    fs::create_dir_all(&automations).expect("create Automations");
    let _ = fs::remove_file(export);
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<AutomationScript>
  <Step type="ImportFile">
    <FilePath>{}</FilePath>
  </Step>
  <Step type="ExportFile">
    <FilePath>{}</FilePath>
    <WithRef>true</WithRef>
  </Step>
</AutomationScript>
"#,
        import.display(),
        export.display()
    );
    let script_path = automations.join(format!("{script}.xml"));
    fs::write(&script_path, xml).expect("write automation XML");

    let log = output_dir.join(format!("{script}_launcher.log"));
    let status = Command::new(&host.launcher)
        .args(["native", "outfitstudio", "--home"])
        .arg(&host.home)
        .args(["--display", &host.display, "--log"])
        .arg(&log)
        .args(["--", "-a", script])
        .env("GDK_BACKEND", "x11")
        .env_remove("WAYLAND_DISPLAY")
        .status()
        .expect("run the skyrim-body launcher");
    let launcher_log = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.success(),
        "Outfit Studio automation {script} failed: {status:?}\n{launcher_log}"
    );
    assert!(export.exists(), "native export {export:?} must exist");
}

/// Run one target end to end and return the native re-export facts.
fn native_roundtrip(host: &Host, output_dir: &Path, target: Target) -> (PathBuf, Vec<u8>) {
    let shapes = bundle();
    let written = target.write(&shapes).expect("the bundle writes");
    let name = format!("shp06_{}_three_shape", target.label());
    let input = fresh_file(output_dir, &format!("{name}.nif"), &written);
    let export = output_dir.join(format!("{name}_reexport.nif"));
    run_native(host, output_dir, &name, &input, &export);
    let reexport = fs::read(&export).expect("read the native re-export");
    eprintln!(
        "{target}: wrote {} bytes, native re-export {} bytes",
        written.len(),
        reexport.len(),
        target = target.label()
    );
    (export, reexport)
}

/// The four comparisons the task owns, plus the pre-flight read of our input.
fn assert_match(written: &[u8], reexport: &[u8], target: Target) {
    let input = read(written).expect("read our own bundle");
    assert_eq!(input.shapes.len(), 3, "our SE/LE file must hold 3 shapes");
    let expected = census(&input);
    let expected_palettes = palettes(&input);

    let document = read(reexport).expect("read the native re-export");
    assert!(
        !document.truncated,
        "native re-export must parse completely"
    );
    assert_eq!(
        document.shapes.len(),
        expected.len(),
        "{target}: shape count after the native load",
        target = target.label()
    );
    let actual = census(&document);
    assert_eq!(
        actual,
        expected,
        "{target}: name/vertex/triangle census",
        target = target.label()
    );
    assert_eq!(
        palettes(&document),
        expected_palettes,
        "{target}: bone lists after the native load",
        target = target.label()
    );
    for (name, (vertices, triangles)) in &actual {
        eprintln!(
            "{target}: shape {name} -> {vertices} vertices / {triangles} triangles, {} bones",
            document
                .shapes
                .iter()
                .find(|shape| &shape.name == name)
                .and_then(|shape| shape.skin.as_ref())
                .map_or(0, |skin| skin.bones.len()),
            target = target.label()
        );
    }
}

#[test]
fn se_and_le_multi_shape_survive_the_native_load() {
    let Some(host) = host() else {
        return;
    };
    let output_dir = PathBuf::from("/tmp/opencode/shp06-native");
    fs::create_dir_all(&output_dir).expect("create output dir");
    for target in [Target::Se, Target::Le] {
        let (_, reexport) = native_roundtrip(&host, &output_dir, target);
        let written = target.write(&bundle()).expect("the bundle writes");
        assert_match(&written, &reexport, target);
    }
}

/// One skinned shape in its own two-bone palette: the single-shape writers
/// share the graph layout this file pins.
fn single_shape() -> MeshAsset {
    let asset = MeshAsset {
        name: "SingleShape".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: BONES[..2].iter().map(|bone| bone.to_string()).collect(),
            bind_inverse: vec![identity(); 2],
            influences: SkinWeights::from_quads(&[[0, 1, 0, 0]; 3], &[[0.75, 0.25, 0.0, 0.0]; 3])
                .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    };
    asset.validate().expect("the fixture is a valid model");
    asset
}

/// The single-shape writers use the same root/bone-node layout, so the palette
/// has to survive the native load there too.
#[test]
fn single_shape_palette_survives_the_native_load() {
    let Some(host) = host() else {
        return;
    };
    let output_dir = PathBuf::from("/tmp/opencode/shp06-native");
    fs::create_dir_all(&output_dir).expect("create output dir");
    let shape = single_shape();
    let expected = shape.skin.as_ref().expect("skin").bones.clone();
    for target in [Target::Se, Target::Le] {
        let written = target.write_single(&shape).expect("the shape writes");
        let name = format!("shp06_{}_single_shape", target.label());
        let input = fresh_file(&output_dir, &format!("{name}.nif"), &written);
        let export = output_dir.join(format!("{name}_reexport.nif"));
        run_native(&host, &output_dir, &name, &input, &export);
        let reexport = fs::read(&export).expect("read the native re-export");
        let document = read(&reexport).expect("read the native single-shape re-export");
        assert_eq!(
            document.shapes.len(),
            1,
            "{target}: one shape",
            target = target.label()
        );
        let bones = document.shapes[0]
            .skin
            .as_ref()
            .map(|skin| skin.bones.clone())
            .expect("the native re-export keeps the skin");
        assert_eq!(
            bones,
            expected,
            "{target}: bone list",
            target = target.label()
        );
        eprintln!(
            "{target}: single shape kept {} bones from {} bytes, re-export {} bytes",
            bones.len(),
            written.len(),
            reexport.len(),
            target = target.label()
        );
    }
}

#[test]
fn negative_control_two_shape_bundle_loads_two_shapes() {
    let Some(host) = host() else {
        return;
    };
    let output_dir = PathBuf::from("/tmp/opencode/shp06-native");
    fs::create_dir_all(&output_dir).expect("create output dir");
    let mut shapes = bundle();
    shapes.remove(2);
    for target in [Target::Se, Target::Le] {
        let written = target.write(&shapes).expect("the two-shape bundle writes");
        let name = format!("shp06_{}_two_shape", target.label());
        let input = fresh_file(&output_dir, &format!("{name}.nif"), &written);
        let export = output_dir.join(format!("{name}_reexport.nif"));
        run_native(&host, &output_dir, &name, &input, &export);
        let reexport = fs::read(&export).expect("read the native re-export");
        let document = read(&reexport).expect("read the native two-shape re-export");
        let names = document
            .shapes
            .iter()
            .map(|shape| shape.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            document.shapes.len(),
            2,
            "{target}: the native load must report 2 shapes, not 3 (names: {names:?})",
            target = target.label()
        );
        assert_eq!(
            census(&document),
            census(&read(&written).expect("read our input")),
            "{target}: counts",
            target = target.label()
        );
        eprintln!(
            "{target}: negative control loaded {names:?} from {} bytes and re-exported {} bytes",
            written.len(),
            reexport.len(),
            target = target.label()
        );
    }
}
