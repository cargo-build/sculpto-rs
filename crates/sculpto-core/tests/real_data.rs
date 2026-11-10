//! Conformance against a real BodySlide editing home.
//!
//! The test is opt-in: it runs only when `SCULPTO_BODYSLIDE_HOME` points at a
//! BodySlide directory with `SliderSets/` and `SliderPresets/`. It checks
//! structural invariants rather than fixed counts, so the same test also
//! qualifies a different body pack (3BA, HIMBO, a future custom set).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sculpto_core::Document;
use sculpto_core::shape_data::{MorphContainer, MorphData, parse_bare_bsd};

fn home() -> Option<PathBuf> {
    std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from)
}

fn xml_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("xml") || ext.eq_ignore_ascii_case("osp")
            })
        })
        .collect();
    files.sort();
    files
}

#[test]
fn real_body_sets_and_presets_hold_structural_invariants() {
    let Some(home) = home() else {
        eprintln!("skipping: set SCULPTO_BODYSLIDE_HOME to a BodySlide directory");
        return;
    };
    let set_files = xml_files(&home.join("SliderSets"));
    let preset_files = xml_files(&home.join("SliderPresets"));
    assert!(
        !set_files.is_empty(),
        "no .osp files under {}",
        home.join("SliderSets").display()
    );
    assert!(
        !preset_files.is_empty(),
        "no preset files under {}",
        home.join("SliderPresets").display()
    );

    let mut set_count = 0;
    let mut slider_count = 0;
    for path in &set_files {
        let document = Document::parse_file(path).expect("real set parses");
        let Document::Project(project) = document else {
            panic!("{} is not a project", path.display());
        };
        let written = project
            .to_osp_xml()
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert_eq!(
            Document::parse_str(&written, path.display().to_string())
                .expect("written project parses"),
            Document::Project(project.clone()),
            "{}: OSP round trip lost data",
            path.display()
        );
        for set in &project.sets {
            set_count += 1;
            let shapes: BTreeSet<&str> = set
                .shapes
                .iter()
                .map(|shape| shape.target.as_str())
                .collect();
            assert!(
                !set.shapes.is_empty(),
                "{}: set `{}` has no shapes",
                path.display(),
                set.name
            );
            assert!(
                !set.sliders.is_empty(),
                "{}: set `{}` has no sliders",
                path.display(),
                set.name
            );
            for slider in &set.sliders {
                slider_count += 1;
                assert!(
                    !slider.data.is_empty(),
                    "{}: set `{}` slider `{}` has no morph data",
                    path.display(),
                    set.name,
                    slider.name
                );
                for data in &slider.data {
                    assert!(
                        shapes.contains(data.target.as_str()),
                        "{}: set `{}` slider `{}` targets undeclared shape `{}`",
                        path.display(),
                        set.name,
                        slider.name,
                        data.target
                    );
                    if let Some(file) = &data.file {
                        let lower = file.to_ascii_lowercase();
                        assert!(
                            lower.ends_with(".osd") || lower.ends_with(".bsd"),
                            "{}: unexpected morph container `{file}`",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    let mut preset_count = 0;
    let mut foreign_roots = 0;
    for path in &preset_files {
        match Document::parse_file(path) {
            Ok(Document::Presets(presets)) => {
                for preset in &presets {
                    preset_count += 1;
                    assert!(!preset.name.is_empty());
                    for value in &preset.values {
                        assert!(
                            !value.slider.is_empty(),
                            "{}: preset `{}` has an unnamed slider",
                            path.display(),
                            preset.name
                        );
                    }
                }
            }
            Ok(Document::Project(project)) => {
                panic!(
                    "{} stayed a project, not a preset collection: {:?}",
                    path.display(),
                    project.version
                )
            }
            Err(error) if error.kind() == sculpto_core::ErrorKind::UnsupportedRoot => {
                foreign_roots += 1;
                eprintln!("{}: skipped foreign root in SliderPresets/", path.display());
            }
            Err(error) => panic!("{}: {error}", path.display()),
        }
    }

    eprintln!(
        "parsed {} sets, {} sliders, {} presets from {} ({} foreign documents skipped)",
        set_count,
        slider_count,
        preset_count,
        home.display(),
        foreign_roots
    );
    assert!(set_count > 0 && slider_count > 0 && preset_count > 0);
}

#[test]
fn morph_containers_decode() {
    let Some(home) = home() else {
        return;
    };
    let mut files = Vec::new();
    collect_containers(&home.join("ShapeData"), &mut files);
    assert!(!files.is_empty(), "no .osd/.bsd files under ShapeData");

    let mut blocks = 0usize;
    let mut offsets = 0usize;
    for path in &files {
        let container = MorphData::from_file(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let encoded = container.to_bytes().expect("real morph data encodes");
        let reparsed = match &container {
            MorphData::Named(named) => {
                assert!(
                    !named.blocks.is_empty(),
                    "{}: empty container",
                    path.display()
                );
                blocks += named.blocks.len();
                MorphData::Named(MorphContainer::parse(&encoded).expect("written OSD parses"))
            }
            MorphData::Bare(_) => {
                blocks += 1;
                MorphData::Bare(parse_bare_bsd(&encoded).expect("written BSD parses"))
            }
        };
        assert_eq!(
            reparsed,
            container,
            "{}: morph round trip lost data",
            path.display()
        );
        offsets += container.offset_count();
    }
    eprintln!(
        "decoded {} containers, {} blocks, {} offsets from {}",
        files.len(),
        blocks,
        offsets,
        home.display()
    );
    assert!(offsets > 0);
}

fn collect_containers(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_containers(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("osd") || ext.eq_ignore_ascii_case("bsd"))
        {
            out.push(path);
        }
    }
}
