use std::path::{Path, PathBuf};

use sculpto_core::document::Document;
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::shape_data::{MorphBlock, MorphContainer, SUPPORTED_VERSION, VertexOffset};
use sculpto_core::skin::SkinWeights;
use sculpto_engine::{dispatch, obj};
use sculpto_gltf::{Clip, ClipChannel, write_glb_with};
use sculpto_mesh::actor::{Actor, Pose};
use sculpto_mesh::check::self_intersection_pairs;
use sculpto_mesh::garment::{Influences, TransferOptions, transfer};
use sculpto_nif::read;
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::json;

const PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body Set">
    <DataFolder>Test</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="BodyData">Body</Shape>
    <Slider name="Lift" small="0" big="0">
      <Data target="BodyData" local="true">source.osd\LiftData</Data>
    </Slider>
    <Slider name="Reverse" small="0" big="0" invert="true">
      <Data target="BodyData" local="true">source.osd\LiftData</Data>
    </Slider>
    <Slider name="Texture" small="0" big="0" uv="true"/>
    <Slider name="Hide" small="0" big="0" zap="true"/>
  </SliderSet>
</SliderSetInfo>"#;
const PRESETS: &str = r#"<SliderPresets><Preset name="Neutral" set="Body Set"/></SliderPresets>"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("sculpto-slider-{tag}-{}", std::process::id()));
        std::fs::create_dir(&path).expect("unique fixture root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    scratch: Scratch,
    body: MeshAsset,
    garment: MeshAsset,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let scratch = Scratch::new(tag);
        let root = scratch.path();
        let data = root.join("ShapeData/Test");
        std::fs::create_dir_all(&data).expect("local data directory");
        std::fs::write(root.join("project.osp"), PROJECT).expect("project");
        std::fs::write(root.join("presets.xml"), PRESETS).expect("preset");
        let body = patch("Body", 0.0);
        let garment = patch("Garment", 0.5);
        std::fs::write(
            root.join("body.nif"),
            write_se_shape(&body).expect("body bytes"),
        )
        .expect("body file");
        std::fs::write(
            root.join("garment.nif"),
            write_se_shape(&garment).expect("garment bytes"),
        )
        .expect("garment file");
        let source = MorphContainer {
            version: SUPPORTED_VERSION,
            blocks: vec![MorphBlock {
                name: "LiftData".into(),
                offsets: (0..4)
                    .map(|index| VertexOffset {
                        index,
                        delta: [0.0, 0.0, 1.0],
                    })
                    .collect(),
            }],
        };
        std::fs::write(
            data.join("source.osd"),
            source.to_osd_bytes().expect("source data"),
        )
        .expect("morph file");
        Self {
            scratch,
            body,
            garment,
        }
    }

    fn conform(&self) -> serde_json::Value {
        dispatch("outfit.conform", &self.conform_request()).expect("conform named body offsets")
    }

    fn conform_request(&self) -> serde_json::Value {
        let root = self.scratch.path();
        json!({
            "nif": root.join("garment.nif"),
            "shape": "Garment",
            "body_nif": root.join("body.nif"),
            "body_shape": "Body",
            "project": root.join("project.osp"),
            "presets": root.join("presets.xml"),
            "preset": "Neutral",
            "set": "Body Set",
            "home": root,
            "out": root.join("conformed.osd"),
        })
    }

    fn preview(&self, percent: u32) -> (Vec<[f32; 3]>, serde_json::Value) {
        let root = self.scratch.path();
        let body = root.join(format!("body-{percent}.obj"));
        let positions = self
            .body
            .positions
            .iter()
            .map(|&[x, y, z]| [x, y, z + percent as f32 / 100.0])
            .collect::<Vec<_>>();
        dispatch(
            "bake",
            &json!({
                "project": root.join("project.osp"), "presets": root.join("presets.xml"),
                "set": "Body Set", "preset": "Neutral", "endpoint": "big",
                "home": root, "source": root.join("body.nif"), "shape": "Body",
                "sliders": { "Lift": percent }, "out": body,
            }),
        )
        .expect("bake body through its target alias");
        assert_eq!(
            obj::read_obj_positions(&body).expect("body endpoint OBJ"),
            positions
        );
        let out = root.join(format!("garment-{percent}.obj"));
        let result = dispatch(
            "outfit.fit",
            &json!({
                "nif": root.join("garment.nif"),
                "shape": "Garment",
                "body_nif": root.join("body.nif"),
                "body_shape": "Body",
                "body": body,
                "out": out,
                "conform": root.join("conformed.osd"),
                "slider": "Lift",
                "percent": percent,
            }),
        )
        .expect("fit from an authored slider diff");
        (
            obj::read_obj_positions(&out).expect("fitted output"),
            result,
        )
    }
}

fn patch(name: &str, z: f32) -> MeshAsset {
    MeshAsset {
        name: name.into(),
        positions: vec![[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    }
}

#[test]
fn conformed_slider_follows_the_body_at_zero_and_one_hundred() {
    let fixture = Fixture::new("endpoints");
    let result = fixture.conform();
    assert_eq!(result["sliders"], json!(["Lift", "Reverse"]));
    assert_eq!(
        result["deferred"],
        json!([
            { "name": "Texture", "reason": "uv" },
            { "name": "Hide", "reason": "zap" },
        ])
    );
    let path = fixture.scratch.path().join("conformed.osd");
    let file = MorphContainer::parse_file(&path).expect("ordinary named OSD");
    assert_eq!(file.blocks.len(), 2);
    assert_eq!(file.blocks[0].name, "GarmentLift");
    assert_eq!(file.blocks[0].offsets.len(), 4);
    assert_eq!(file.blocks[1].name, "GarmentReverse");
    assert!(
        file.blocks[1]
            .offsets
            .iter()
            .all(|offset| (offset.delta[2] + 1.0).abs() < 1e-6)
    );
    let (zero, zero_report) = fixture.preview(0);
    let (hundred, hundred_report) = fixture.preview(100);
    assert_eq!(zero, fixture.garment.positions);
    for (before, after) in zero.iter().zip(hundred) {
        assert!((after[2] - before[2] - 1.0).abs() < 1e-6);
        assert_eq!(&before[..2], &after[..2]);
    }
    assert_eq!(zero_report["intersection_pairs"], 0);
    assert_eq!(hundred_report["intersection_pairs"], 0);
}

#[test]
fn a_missing_selected_body_diff_leaves_no_output() {
    let fixture = Fixture::new("missing-body-diff");
    let source = MorphContainer {
        version: SUPPORTED_VERSION,
        blocks: Vec::new(),
    };
    std::fs::write(
        fixture.scratch.path().join("ShapeData/Test/source.osd"),
        source.to_osd_bytes().expect("empty OSD"),
    )
    .expect("replace the fixture's source morphs");
    let error =
        dispatch("outfit.conform", &fixture.conform_request()).expect_err("missing body diff");
    assert_eq!(error.code(), "conform_unresolved_reference");
    assert!(error.message().contains("Lift"));
    assert!(!fixture.scratch.path().join("conformed.osd").exists());
}

#[test]
fn configured_sentinel_slider_endpoints_are_clear() {
    let (Some(home), Some(fixture)) = (
        std::env::var_os("SCULPTO_BODYSLIDE_HOME"),
        std::env::var_os("SCULPTO_FIT_FIXTURE"),
    ) else {
        eprintln!(
            "SKIP configured_sentinel_slider_endpoints_are_clear: configure both fixture roots"
        );
        return;
    };
    let home = Path::new(&home);
    let garment_nif = Path::new(&fixture).join("tunic.nif");
    let source =
        read(&std::fs::read(&garment_nif).expect("Sentinel NIF")).expect("Sentinel source");
    let garment = source
        .shapes
        .iter()
        .find(|shape| shape.name == "Armor001_1")
        .expect("Sentinel form");
    let body_nif = home.join("ShapeData/HIMBO Body/Vanilla/Vanilla NeverNude.nif");
    let project = home.join("SliderSets/HIMBO.osp");
    let presets = home.join("SliderPresets/HIMBO.xml");
    let set_name = "HIMBO Body - Vanilla Nevernude";
    let Document::Project(document) = Document::parse_file(&project).expect("HIMBO project") else {
        panic!("expected a HIMBO slider-set project");
    };
    let set = document
        .sets
        .iter()
        .find(|set| set.name == set_name)
        .expect("HIMBO set");
    let scratch = Scratch::new("real-sentinel");
    let conform = scratch.path().join("sentinel.osd");
    let report = dispatch(
        "outfit.conform",
        &json!({
            "nif": garment_nif, "shape": garment.name, "body_nif": body_nif,
            "body_shape": "HIMBO - Body", "project": project, "presets": presets,
            "preset": "HIMBO Chonky", "set": set_name, "home": home, "out": conform,
        }),
    )
    .expect("conform real Sentinel");
    assert_eq!(
        report["sliders"].as_array().expect("named sliders").len(),
        124
    );
    assert_eq!(report["deferred"].as_array().expect("deferrals").len(), 2);
    let mut previews = Vec::new();
    for percent in [0, 100] {
        let body = scratch.path().join(format!("body-{percent}.obj"));
        let out = scratch.path().join(format!("garment-{percent}.obj"));
        let sliders: serde_json::Map<_, _> = set
            .sliders
            .iter()
            .map(|slider| {
                (
                    slider.name.clone(),
                    json!(if slider.name == "Lean" { percent } else { 0 }),
                )
            })
            .collect();
        dispatch(
            "bake",
            &json!({
                "project": project, "presets": presets, "set": set_name, "preset": "HIMBO Chonky",
                "endpoint": "big", "out": body, "source": body_nif, "shape": "HIMBO - Body",
                "home": home, "sliders": sliders,
            }),
        )
        .expect("bake the matching body endpoint");
        let report = dispatch(
            "outfit.fit",
            &json!({
                "nif": garment_nif, "shape": garment.name, "body_nif": body_nif,
                "body_shape": "HIMBO - Body", "body": body, "out": out,
                "conform": conform, "slider": "Lean", "percent": percent,
            }),
        )
        .expect("preview the real endpoint");
        assert_eq!(report["inside_after"], 0);
        assert_eq!(report["intersection_pairs"], 0);
        let positions = obj::read_obj_positions(&out).expect("preview OBJ");
        assert_eq!(positions.len(), garment.positions.len());
        assert_eq!(self_intersection_pairs(&positions, &garment.triangles), 0);
        let obj = std::fs::read_to_string(&out).expect("preview topology");
        assert_eq!(
            obj.lines().filter(|line| line.starts_with("f ")).count(),
            garment.triangles.len()
        );
        eprintln!(
            "Sentinel Lean={percent}: {} vertices, {} faces, zero body and self contacts",
            positions.len(),
            garment.triangles.len()
        );
        previews.push(positions);
    }
    assert_ne!(previews[0], previews[1]);
}

#[test]
fn posed_garment_max_error_below_bound() {
    let mut body = patch("Body", 0.0);
    let mut garment = patch("Garment", 0.5);
    for asset in [&mut body, &mut garment] {
        let second: Vec<_> = asset
            .positions
            .iter()
            .map(|&[x, y, z]| [x + 25.0, y, z])
            .collect();
        asset.positions.extend(second);
        asset.triangles.extend([[4, 5, 6], [4, 6, 7]]);
        asset.triangle_partitions.extend([PartitionId(0); 2]);
    }
    let identity = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let quad = |joint| [joint, 0, 0, 0];
    body.skin = Some(Skin {
        bones: vec!["Left".into(), "Right".into()],
        bind_inverse: vec![identity; 2],
        influences: SkinWeights::from_quads(
            &[
                quad(0),
                quad(0),
                quad(0),
                quad(0),
                quad(1),
                quad(1),
                quad(1),
                quad(1),
            ],
            &[[1.0, 0.0, 0.0, 0.0]; 8],
        )
        .expect("two bone regions"),
    });
    let native = read(&write_se_shape(&body).expect("native source")).expect("read source binding");
    let body = MeshAsset::from(&native.shapes[0]);
    let body_skin = body.skin.as_ref().expect("native body skin");
    let weights: Vec<Influences> = (0..body.positions.len())
        .map(|vertex| {
            let row = body_skin
                .influences
                .row(vertex)
                .expect("native influence row");
            row.joints
                .iter()
                .zip(row.weights)
                .map(|(&joint, &weight)| (body_skin.bones[usize::from(joint)].clone(), weight))
                .collect()
        })
        .collect();
    let copied = transfer(
        &garment.positions,
        &body.positions,
        &weights,
        &TransferOptions::default(),
    )
    .expect("copy and normalize authored weights");
    let (mut offsets, mut joints, mut values) = (vec![0], Vec::new(), Vec::new());
    for row in &copied {
        for (bone, &weight) in row {
            let index = body_skin
                .bones
                .iter()
                .position(|name| name == bone)
                .expect("source palette name");
            joints.push(u16::try_from(index).expect("native palette width"));
            values.push(weight);
        }
        offsets.push(u32::try_from(joints.len()).expect("fixture row count"));
    }
    garment.skin = Some(Skin {
        bones: body_skin.bones.clone(),
        bind_inverse: body_skin.bind_inverse.clone(),
        influences: SkinWeights::new(offsets, joints, values, body_skin.bones.len())
            .expect("copied garment skin"),
    });
    let clip = Clip {
        name: "Separate".into(),
        times: vec![0.0, 1.0],
        channels: vec![
            ClipChannel::Translation {
                bone: 0,
                values: vec![[0.0; 3], [1.0, 0.0, 0.0]],
            },
            ClipChannel::Translation {
                bone: 1,
                values: vec![[0.0; 3], [-2.0, 0.0, 0.0]],
            },
        ],
    };
    let sample = |asset: &MeshAsset| {
        let bytes = write_glb_with(asset, std::slice::from_ref(&clip)).expect("export skinned GLB");
        let actor = Actor::from_glb(&bytes).expect("read the authored clip");
        (
            actor
                .sample(&[asset.name.as_str()], Pose::Rest)
                .expect("rest"),
            actor
                .sample(
                    &[asset.name.as_str()],
                    Pose::Clip {
                        name: "Separate",
                        time: 1.0,
                    },
                )
                .expect("posed"),
        )
    };
    let (body_rest, body_pose) = sample(&body);
    let (garment_rest, garment_pose) = sample(&garment);
    let mut maximum = 0.0_f64;
    let mut detached = 0;
    for vertex in 0..body.positions.len() {
        let body_delta = std::array::from_fn::<_, 3, _>(|axis| {
            body_pose.vertices[vertex][axis] - body_rest.vertices[vertex][axis]
        });
        let garment_delta = std::array::from_fn::<_, 3, _>(|axis| {
            garment_pose.vertices[vertex][axis] - garment_rest.vertices[vertex][axis]
        });
        let error = body_delta
            .into_iter()
            .zip(garment_delta)
            .map(|(body, garment)| (body - garment).powi(2))
            .sum::<f64>()
            .sqrt();
        let expected = if vertex < 4 { 1.0 } else { -2.0 };
        assert!((body_delta[0] - expected).abs() < 1e-6);
        maximum = maximum.max(error);
        detached += usize::from(garment_delta[0].abs() <= 0.1);
    }
    eprintln!("posed garment: max displacement error={maximum:.9}, detached={detached}");
    assert!(maximum < 0.05);
    assert_eq!(detached, 0);
}
