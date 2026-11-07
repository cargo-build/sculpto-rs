//! `sculpto` command-line entry point.
//!
//! The first slices are read-only: inspect a Sculpto input document and print
//! what the core model understood, so format questions are answered by the
//! same parser the adapters will use.

mod actor;
mod garment_control;
mod garment_uv;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sculpto_bake::BakePlan;
use sculpto_core::shape_data::MAGIC as SHAPE_DATA_MAGIC;
use sculpto_core::{Document, MorphContainer, SliderSize};
use sculpto_mesh::aura::{Aura, AuraMode, AuraParams};
use sculpto_mesh::check;
use sculpto_mesh::classify;
use sculpto_mesh::conform::Conform;
use sculpto_mesh::wrap;

use sculpto_nif::NifShape;

const USAGE: &str = "\
usage: sculpto <command> [options]

commands:
  actor sample [options] sample explicit GLB mesh nodes at REST or clip/time
  actor rig [options]   sample parts plus reduced rig graph/influences JSON
  actor pack-cloth [options] pack baked cloth into an unchanged GLB actor
  inspect <path>        parse a document or morph container and print a summary
  bake [options]        resolve a preset, apply it to a body NIF, write an OBJ
  batch [options]       bake every matching set in a project to OBJ files
  sliders [options]     print each slider's effective percentage
  presets [options]     list presets in a SliderPresets collection
  export-obj [options]  read a NIF and write one shape as an OBJ
  garment weights [options]  transfer the body's bone weights onto a fitted garment
  outfit install [options]  fit an outfit to a body, skin it and print the gate
  compare [options]     per-vertex delta between two same-topology OBJs
  garment fit [options] bind a garment to its authored body, then fit it to a bake
  garment follow [options] transport render positions through a deformed control
  garment aura [options] press a garment onto the body's cross-section aura
  garment classify [options] measure a garment into its GarmentKind and profile
  garment proxies [options] export the body's per-contour collision proxies
  garment check [options] count garment/body triangle intersections (measured gate)
  garment trim [options]  cut away garment triangles that intersect the body
  garment cover [options] body coverage per height band (<=2/<=8/<=20 units)
  garment control-exterior [options] keep the outward control surface of a thick shell
  garment control-repair [options] plan corner-flare and incidental-loop repair
  garment control-check [options] measure and gate a finished control surface
  garment control-separate [options] apply the declared bounded local contact repair
  garment control-cuts [options] validate declared source-opening cuts and print the plan
  garment uv-check [options] qualify triangle-corner UV area and atlas bounds
  version               print the Sculpto implementation version
  help                  print this help

bake options:
  --project <osp>    .osp project file
  --presets <xml>    SliderPresets collection
  --set <name>       slider set to bake
  --preset <name>    preset name
  --endpoint <big|small>
  --source <nif>     source body (defaults to the set\'s source under ShapeData)
  --shape <name>     shape to bake (defaults to the largest)
  --home <dir>       BodySlide home (defaults to the project\'s grandparent)
  --slider N=V       override one slider percentage (repeatable)
  --out <obj>        output path

batch options:
  --project <osp>  --presets <xml>  --preset <name>  --endpoint <big|small>
  --out <dir>      --filter <substring>  --limit <n>  [--home <dir>]

sliders options:
  --project <osp>  --set <name>  --preset <name>  --endpoint <big|small>

presets options:
  --presets <xml>   SliderPresets collection to list

export-obj options:
  --nif <path>       NIF to read
  --shape <name>     shape to export (defaults to the largest)
  --out <obj>        output path

garment fit options:
  --nif <path>         garment NIF
  --shape <name>       garment shape inside the NIF (required: the NIF also carries a body)
  --body-shape <name>  body shape the garment was authored on (required)
  --body <obj>         baked body OBJ, same topology as --body-shape
  --out <obj>          fitted garment OBJ
  --body-after <obj>   second bake; reports how body and garment moved
  --out-after <obj>    fitted garment for --body-after

garment follow options:
  --reference <obj>   original control surface
  --target <obj>      deformed control with identical ordered topology
  --garment <obj>     detailed render surface in the reference frame
  --out <obj>         transported positions/triangles; adapter retains materials/UVs

garment aura options:
  --body <obj>        baked body OBJ (positions + triangles)
  --garment <obj>     garment OBJ in the same frame
  --out <obj>         fitted garment OBJ
  --class <name>      auto | upper-body | trousers | skirt | dress | robe |
                      footwear | cape | accessory: resolve the corset-aura
                      profile first; explicit flags below still override
  --adherence <name>  snug | fitted | relaxed (classified, with --class)
  --clearance <f32>   minimum distance the body keeps inside the aura (0.6)
  --slack <f32>       linear ease on the smoothed contour; 0.05 = +5% perimeter
  --slices <n>        body sections (96)
  --bins <n>          angular samples per contour (192)
  --smooth <n>        circular smoothing window in bins (5)
  --mode <name>       tight | press | loose (press)
  --max-ease <f32>    press cap as a factor of the target radius (1.5)

garment intake options:
  --body <obj>        baked body OBJ (positions + triangles)
  --garment <obj>     garment OBJ in the same frame
  --require-fit <0|1> return failure when surface preparation is required (0)

garment strip options:
  --body <obj>        baked body OBJ (positions + triangles)
  --garment <obj>     garment OBJ in the same frame
  --out <obj>         repaired garment OBJ with interior triangles removed

garment proxies options:
  --body <obj>        baked body OBJ (positions + triangles)
  --out <obj>         proxy polylines OBJ, one loop per body contour
  --slices <n>        body sections (96)
  --bins <n>          angular samples per contour (192)

garment control-exterior options:
  --source <obj>      thick source shell OBJ (display frame)
  --profile <json>    control profile: guide, apertures, repair/reduction limits
  --out <obj>         outward control surface OBJ (display frame)
  --plan <json>       reconstruction decisions for the host adapter

garment control-repair options:
  --control <obj>     candidate control OBJ after native decimation
  --profile <json>    the same control profile
  --out <obj>         control OBJ after corner-flare pruning
  --plan <json>       repair actions: collapse or fill each incidental loop

garment control-check options:
  --control <obj>     finished control OBJ
  --source <obj>      thick source shell OBJ in the same frame
  --profile <json>    the same control profile
  --report <json>     measured topology, apertures, deviations and verdict

garment control-cuts options:
  --profile <json>    the same control profile (optional source_cuts)
  --report <json>     validated cut planes in the mesh frame for the host

garment control-separate options:
  --control <obj>     finished control OBJ that still contacts itself
  --profile <json>    the same control profile (contact policy and bounds)
  --out <obj>         control OBJ after the bounded local separation
  --report <json>     the chosen axis, moved vertices and bounds actually used
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "version" => {
            println!("sculpto {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [command, path] if command == "inspect" => inspect(Path::new(path)),
        [command, rest @ ..] if command == "actor" => actor::run(rest),
        [command, rest @ ..] if command == "bake" => bake(rest),
        [command, rest @ ..] if command == "sliders" => sliders(rest),
        [command, rest @ ..] if command == "batch" => batch(rest),
        [command, rest @ ..] if command == "presets" => presets(rest),
        [command, rest @ ..] if command == "export-obj" => export_obj(rest),
        [command, rest @ ..] if command == "compare" => compare(rest),
        [first, second, rest @ ..] if first == "garment" && second == "weights" => {
            garment_weights(rest)
        }
        [first, second, rest @ ..] if first == "outfit" && second == "install" => {
            outfit_install(rest)
        }
        [command, rest @ ..] if command == "garment" => garment(rest),
        [command] if command == "help" || command == "-h" || command == "--help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn inspect(path: &Path) -> ExitCode {
    if is_shape_data(path) {
        return match MorphContainer::parse_file(path) {
            Ok(container) => {
                print!("{}", render_shape_data(path, &container));
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    match Document::parse_file(path) {
        Ok(document) => {
            print!("{}", render(path, &document));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Render a stable, human-readable summary; tests compare this text.
fn render(path: &Path, document: &Document) -> String {
    match document {
        Document::Presets(presets) => {
            let values: usize = presets.iter().map(|preset| preset.values.len()).sum();
            let unknown: usize = presets.iter().map(|preset| preset.unknown.len()).sum();
            let mut out = format!(
                "{}: presets document, {} presets, {} values{}\n",
                path.display(),
                presets.len(),
                values,
                render_unknown(unknown)
            );
            for preset in presets {
                let set = preset.slider_set.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "  preset {:<24} set={:<20} groups={:<3} values={}\n",
                    preset.name,
                    set,
                    preset.groups.len(),
                    preset.values.len()
                ));
            }
            out
        }
        Document::Project(project) => {
            let sliders: usize = project.sets.iter().map(|set| set.sliders.len()).sum();
            let records: usize = project
                .sets
                .iter()
                .flat_map(|set| set.sliders.iter())
                .map(|slider| slider.data.len())
                .sum();
            let unresolved: usize = project
                .sets
                .iter()
                .flat_map(|set| set.sliders.iter())
                .flat_map(|slider| slider.data.iter())
                .filter(|data| data.file.is_none())
                .count();
            let unknown: usize = project.unknown.len()
                + project
                    .sets
                    .iter()
                    .map(|set| {
                        set.unknown.len()
                            + set.sliders.iter().map(|s| s.unknown.len()).sum::<usize>()
                    })
                    .sum::<usize>();
            let mut out = format!(
                "{}: SliderSetInfo v{}, {} sets, {} sliders, {} data records ({} without a container){}\n",
                path.display(),
                project.version,
                project.sets.len(),
                sliders,
                records,
                unresolved,
                render_unknown(unknown)
            );
            for set in &project.sets {
                out.push_str(&format!(
                    "  set {:<32} shapes={:<3} sliders={:<4} source={}\n",
                    set.name,
                    set.shapes.len(),
                    set.sliders.len(),
                    set.source_file
                ));
            }
            out
        }
    }
}

/// A `.osd`/`.bsd` container is binary, so the XML dispatcher must not see it.
fn is_shape_data(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map(|()| magic == SHAPE_DATA_MAGIC)
        .unwrap_or(false)
}

fn render_shape_data(path: &Path, container: &MorphContainer) -> String {
    let mut out = format!(
        "{}: morph container v{}, {} blocks, {} offsets\n",
        path.display(),
        container.version,
        container.blocks.len(),
        container.offset_count()
    );
    for block in &container.blocks {
        out.push_str(&format!(
            "  block {:<40} offsets={}\n",
            block.name,
            block.offsets.len()
        ));
    }
    out
}

/// Suffix for a summary line; an empty string keeps known dialects noise-free.
fn render_unknown(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(", {count} unknown elements")
    }
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut flags = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next() {
        let Some(name) = key.strip_prefix("--") else {
            return Err(format!("unexpected argument `{key}`"));
        };
        let Some(value) = iter.next() else {
            return Err(format!("`--{name}` needs a value"));
        };
        flags.insert(name.to_string(), value.clone());
    }
    Ok(flags)
}

fn required<'a>(flags: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    flags
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("`--{name}` is required"))
}

fn bake(args: &[String]) -> ExitCode {
    match bake_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bake: {error}");
            ExitCode::FAILURE
        }
    }
}

fn bake_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let project_path = PathBuf::from(required(&flags, "project")?);
    let presets_path = PathBuf::from(required(&flags, "presets")?);
    let set_name = required(&flags, "set")?;
    let preset_name = required(&flags, "preset")?;
    let endpoint = required(&flags, "endpoint")?;
    let out = PathBuf::from(required(&flags, "out")?);
    let size = match endpoint {
        "big" => SliderSize::Big,
        "small" => SliderSize::Small,
        other => return Err(format!("`--endpoint` must be big or small, got `{other}`")),
    };

    let project = match Document::parse_file(&project_path).map_err(|e| e.to_string())? {
        Document::Project(project) => project,
        Document::Presets(_) => return Err(format!("{} is not a project", project_path.display())),
    };
    let presets = match Document::parse_file(&presets_path).map_err(|e| e.to_string())? {
        Document::Presets(presets) => presets,
        Document::Project(_) => {
            return Err(format!("{} is not a presets file", presets_path.display()));
        }
    };
    let preset = presets
        .iter()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("preset `{preset_name}` not found"))?;

    let home = match flags.get("home") {
        Some(home) => PathBuf::from(home),
        None => project_path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| "cannot derive the BodySlide home; pass `--home`".to_string())?
            .to_path_buf(),
    };

    let mut overrides = BTreeMap::new();
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "--slider" {
            let (name, value) = args[index + 1]
                .split_once('=')
                .ok_or_else(|| "`--slider` expects Name=Value".to_string())?;
            let percent: f32 = value
                .parse()
                .map_err(|_| format!("`--slider {name}` value `{value}` is not a number"))?;
            overrides.insert(name.to_string(), percent);
        }
        index += 2;
    }
    let plan = BakePlan::resolve_overrides(&project, preset, set_name, size, &home, &overrides)
        .map_err(|e| e.to_string())?;
    let set = project
        .sets
        .iter()
        .find(|set| set.name == set_name)
        .ok_or_else(|| format!("set `{set_name}` not found"))?;
    let source = match flags.get("source") {
        Some(source) => PathBuf::from(source),
        None => home
            .join("ShapeData")
            .join(set.data_folder.replace('\\', "/"))
            .join(&set.source_file),
    };

    let bytes = std::fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    let document = sculpto_nif::read(&bytes).map_err(|e| e.to_string())?;
    let shape = pick_shape(&document.shapes, flags.get("shape").map(String::as_str))?;
    let baked = plan
        .apply(&shape.name, &shape.positions)
        .map_err(|e| e.to_string())?;
    write_obj(&out, &baked, &shape.triangles).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "{}: {} vertices, {} triangles, {} sliders, {} unresolved, {} deferred",
        out.display(),
        baked.len(),
        shape.triangles.len(),
        plan.sliders.len(),
        plan.unresolved.len(),
        plan.deferred.len()
    );
    Ok(())
}

fn export_obj(args: &[String]) -> ExitCode {
    match export_obj_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("export-obj: {error}");
            ExitCode::FAILURE
        }
    }
}

fn outfit_install(args: &[String]) -> ExitCode {
    match outfit_install_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("outfit install: {error}");
            ExitCode::FAILURE
        }
    }
}

/// One command for the whole wearable path: fit, skin, measure.
///
/// This is the operator-facing "install an outfit on a body preset" route. It
/// runs the same operations as the individual garment commands (regional
/// envelope, aura press, clip trim), transfers the body's bone weights and
/// prints the intersection gate, so a new outfit needs no per-piece
/// choreography.
fn outfit_install_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let outfit_path = PathBuf::from(required(&flags, "outfit")?);
    let out_dir = PathBuf::from(required(&flags, "out")?);
    let nif = PathBuf::from(required(&flags, "body-nif")?);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let (body_display, body_triangles) = read_obj(&body_path)?;
    let (outfit_display, outfit_triangles) = read_obj(&outfit_path)?;

    // The envelope measures height in the display frame (Y up); the aura runs in
    // the NIF source frame. Feeding source coordinates to the envelope made the
    // zone sweep the depth axis instead of the height axis.
    let envelope = wrap::EnvelopeOptions {
        zone_start: 0.0,
        zone_end: flag_f32(&flags, "zone-end", 0.35)?,
        gap: flag_f32(&flags, "gap", 0.5)?,
        blend: flag_f32(&flags, "blend", 6.0)?,
        ..wrap::EnvelopeOptions::default()
    };
    let (adapted_display, envelope_report) =
        wrap::adapt_envelope(&outfit_display, &body_display, &envelope)
            .ok_or("outfit install: empty body or outfit")?;
    let body = to_source(&body_display);
    let mut garment = to_source(&adapted_display);
    println!(
        "envelope: {} vertices moved, max move {:.2}",
        envelope_report.moved, envelope_report.max_move
    );

    let mode = match flags.get("mode").map(String::as_str) {
        None | Some("loose") => AuraMode::Loose,
        Some("press") => AuraMode::Press,
        Some("tight") => AuraMode::Tight,
        Some(other) => {
            return Err(format!(
                "--mode expects loose, press or tight, got `{other}`"
            ));
        }
    };
    let profile = {
        let mut verdict = classify::classify(&body, &body_triangles, &garment, &outfit_triangles)
            .map_err(|e| e.to_string())?;
        if let Some(name) = flags.get("class").filter(|name| *name != "auto") {
            verdict.kind = classify::GarmentKind::parse(name).map_err(|e| e.to_string())?;
        }
        verdict.profile()
    };
    let base = profile.map(|profile| profile.params).unwrap_or_default();
    let params = AuraParams {
        clearance: flag_f32(&flags, "clearance", base.clearance)?,
        slack: flag_f32(&flags, "slack", base.slack)?,
        slices: base.slices,
        bins: base.bins,
        smooth: base.smooth,
        mode: flags
            .get("mode")
            .map(|name| match name.as_str() {
                "loose" => Ok(AuraMode::Loose),
                "press" => Ok(AuraMode::Press),
                "tight" => Ok(AuraMode::Tight),
                other => Err(format!(
                    "--mode expects loose, press or tight, got `{other}`"
                )),
            })
            .transpose()?
            .unwrap_or(mode),
        max_ease: base.max_ease,
        bands: base.bands,
    };
    // Trim minimisation: a slightly larger ease can clear the body without
    // cutting a whole trouser leg open. Try a small ladder and keep the
    // candidate that needs the fewest cuts, then the smallest displacement.
    let ladder = [
        (params.clearance, params.slack),
        (params.clearance * 1.3, params.slack),
        (params.clearance * 1.3, params.slack + 0.05),
        (params.clearance * 1.6, params.slack + 0.05),
        (params.clearance * 2.0, params.slack + 0.10),
        (params.clearance * 2.5, params.slack + 0.15),
    ];
    // Keep cuts inside a small budget (1% of the outfit) and then prefer the
    // candidate that moves the garment least: fewer cuts is worthless if it
    // inflates the whole outfit away from the authored shape.
    let budget = (outfit_triangles.len() / 100).max(12);
    struct Candidate {
        cuts: usize,
        mean_move: f32,
        positions: Vec<[f32; 3]>,
        clearance: f32,
        slack: f32,
    }
    let mut best: Option<Candidate> = None;
    for (clearance, slack) in ladder {
        let candidate = AuraParams {
            clearance,
            slack,
            ..params
        };
        let aura = Aura::build(&body, &body_triangles, candidate).map_err(|e| e.to_string())?;
        let (fitted, report) = aura.apply(&garment);
        let pairs =
            check::intersection_pair_list(&fitted, &outfit_triangles, &body, &body_triangles);
        let doomed: std::collections::BTreeSet<usize> =
            pairs.iter().map(|(outfit, _)| *outfit).collect();
        let mean = fitted
            .iter()
            .zip(garment.iter())
            .map(|(a, b)| {
                let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .sum::<f32>()
            / fitted.len().max(1) as f32;
        println!(
            "candidate clearance {clearance:.2} slack {slack:.2}: inside {} -> {}, would trim {}, mean move {mean:.3}",
            report.inside_before,
            report.inside_after,
            doomed.len()
        );
        // The first candidate inside the cut budget wins: it is the one closest
        // to the profile the classifier chose. The ladder only rescues outfits
        // that would otherwise be cut open.
        let better = match &best {
            None => true,
            Some(previous) => previous.cuts > budget && doomed.len() < previous.cuts,
        };
        if better {
            best = Some(Candidate {
                cuts: doomed.len(),
                mean_move: mean,
                positions: fitted,
                clearance,
                slack,
            });
        }
    }
    let chosen = best.ok_or("outfit install: no aura candidate")?;
    let (cuts, mean_move, fitted, chosen_clearance, chosen_slack) = (
        chosen.cuts,
        chosen.mean_move,
        chosen.positions,
        chosen.clearance,
        chosen.slack,
    );
    garment = fitted;
    let pairs = check::intersection_pair_list(&garment, &outfit_triangles, &body, &body_triangles);
    let doomed: std::collections::BTreeSet<usize> =
        pairs.iter().map(|(outfit, _)| *outfit).collect();
    let kept: Vec<[u32; 3]> = outfit_triangles
        .iter()
        .enumerate()
        .filter_map(|(index, triangle)| (!doomed.contains(&index)).then_some(*triangle))
        .collect();
    println!(
        "trim: removed {} of {} triangles that intersect the body (mean move {mean_move:.3}, {cuts} cuts, clearance {chosen_clearance:.2}, slack {chosen_slack:.2}, budget {budget})",
        doomed.len(),
        outfit_triangles.len()
    );

    let fit_path = out_dir.join("fit.obj");
    let uvs = read_obj_uvs(&outfit_path)?;
    write_obj_keep_uv(&fit_path, &garment, &kept, &uvs)
        .map_err(|e| format!("{}: {e}", fit_path.display()))?;

    let bytes = std::fs::read(&nif).map_err(|e| format!("{}: {e}", nif.display()))?;
    let document = sculpto_nif::read(&bytes).map_err(|e| e.to_string())?;
    let body_shape = pick_shape(&document.shapes, None)?;
    let influences = sculpto_mesh::skin::transfer_weights(
        &body_shape.positions,
        &body_shape.triangles,
        &body_shape.bone_indices,
        &body_shape.weights,
        &garment,
    )
    .ok_or("outfit install: weight transfer failed")?;
    let mut weights = String::from("{\n  \"bones\": [");
    for (index, bone) in body_shape.bones.iter().enumerate() {
        if index > 0 {
            weights.push(',');
        }
        weights.push_str(&format!("{bone:?}"));
    }
    weights.push_str("],\n  \"vertices\": [");
    for (index, influence) in influences.iter().enumerate() {
        if index > 0 {
            weights.push(',');
        }
        let slots: Vec<String> = influence
            .bones
            .iter()
            .zip(influence.weights.iter())
            .filter(|(_, weight)| **weight > 0.0)
            .map(|(bone, weight)| format!("[{bone}, {weight:.6}]"))
            .collect();
        weights.push_str(&format!("\n    [{}]", slots.join(", ")));
    }
    weights.push_str("\n  ]\n}\n");
    let weights_path = out_dir.join("fit.weights.json");
    std::fs::write(&weights_path, weights)
        .map_err(|e| format!("{}: {e}", weights_path.display()))?;

    let remaining = check::intersection_pair_list(&garment, &kept, &body, &body_triangles).len();
    println!(
        "{}: {} triangles, {} vertices; {} body/outfit pairs after trim; weights -> {}",
        fit_path.display(),
        kept.len(),
        garment.len(),
        remaining,
        weights_path.display()
    );
    if let Some(reference) = flags.get("reference") {
        let (reference_display, _) = read_obj(Path::new(reference))?;
        let reference_positions = to_source(&reference_display);
        if reference_positions.len() != garment.len() {
            println!("reference: vertex count differs, height band table skipped");
        } else {
            // Source frame is Z-up: band by Z, the height axis.
            let low = reference_positions
                .iter()
                .map(|p| p[2])
                .fold(f32::INFINITY, f32::min);
            let high = reference_positions
                .iter()
                .map(|p| p[2])
                .fold(f32::NEG_INFINITY, f32::max);
            let bands = flag_usize(&flags, "bands", 11)?;
            let step = (high - low) / bands as f32;
            println!("height slices {low:.1}..{high:.1}, {bands} bands");
            for band in 0..bands {
                let from = low + step * band as f32;
                let to = if band + 1 == bands {
                    high + 1e-4
                } else {
                    from + step
                };
                let mut values: Vec<f32> = reference_positions
                    .iter()
                    .zip(garment.iter())
                    .filter(|(position, _)| position[2] >= from && position[2] < to)
                    .map(|(a, b)| {
                        let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                    })
                    .collect();
                values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                if values.is_empty() {
                    continue;
                }
                let p95 = values[(values.len() as f32 * 0.95) as usize];
                println!(
                    "  {from:6.1}-{to:6.1} n={:5} mean {:6.3} p95 {:6.3} max {:6.3}",
                    values.len(),
                    values.iter().sum::<f32>() / values.len() as f32,
                    p95,
                    values.last().copied().unwrap_or(0.0)
                );
            }
        }
    }
    Ok(())
}

fn garment_weights(args: &[String]) -> ExitCode {
    match garment_weights_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("garment weights: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Transfer the body's skin weights onto a garment, Outfit Studio style.
///
/// `OutfitProject::CopyBoneWeights` resamples the body's per-bone weights
/// through the conform proximity cache; this command does the same and writes
/// the influences as JSON beside the garment, so the fitted OBJ can be skinned.
fn garment_weights_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let nif = PathBuf::from(required(&flags, "body-nif")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let bytes = std::fs::read(&nif).map_err(|e| format!("{}: {e}", nif.display()))?;
    let document = sculpto_nif::read(&bytes).map_err(|e| e.to_string())?;
    let body = pick_shape(
        &document.shapes,
        flags.get("body-shape").map(String::as_str),
    )?;
    if body.weights.is_empty() || body.bones.is_empty() {
        return Err(format!("shape `{}` carries no skin weights", body.name));
    }
    let (garment_display, _) = read_obj(Path::new(required(&flags, "garment")?))?;
    let garment = to_source(&garment_display);
    let influences = sculpto_mesh::skin::transfer_weights(
        &body.positions,
        &body.triangles,
        &body.bone_indices,
        &body.weights,
        &garment,
    )
    .ok_or("weight transfer failed: empty or inconsistent body mesh")?;

    let mut text = String::from("{\n  \"bones\": [");
    for (index, bone) in body.bones.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&format!("{:?}", bone));
    }
    text.push_str("],\n  \"vertices\": [");
    for (index, influence) in influences.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        let slots: Vec<String> = influence
            .bones
            .iter()
            .zip(influence.weights.iter())
            .filter(|(_, weight)| **weight > 0.0)
            .map(|(bone, weight)| format!("[{}, {:.6}]", bone, weight))
            .collect();
        text.push_str(&format!("\n    [{}]", slots.join(", ")));
    }
    text.push_str("\n  ]\n}\n");
    std::fs::write(&out, text).map_err(|e| format!("{}: {e}", out.display()))?;

    let mut used = std::collections::BTreeSet::new();
    for influence in &influences {
        for (bone, weight) in influence.bones.iter().zip(influence.weights.iter()) {
            if *weight > 0.0 {
                used.insert(*bone);
            }
        }
    }
    println!(
        "{}: {} garment vertices skinned from `{}` ({} bones, {} influenced)",
        out.display(),
        influences.len(),
        body.name,
        body.bones.len(),
        used.len()
    );
    if flags.get("self-check").map(String::as_str).unwrap_or("0") != "0" {
        // Query the body's own vertices through the same transfer and compare
        // with their authored weights: a barycentric identity must hold.
        let same = sculpto_mesh::skin::transfer_weights(
            &body.positions,
            &body.triangles,
            &body.bone_indices,
            &body.weights,
            &body.positions,
        )
        .ok_or("self-check: empty body mesh")?;
        let mut error_sum = 0.0f64;
        let mut dominant_mismatch = 0usize;
        for (index, influence) in same.iter().enumerate() {
            // Compare per bone: the transfer sorts influences by weight, so the
            // slot order itself carries no meaning.
            let mut per_bone: std::collections::BTreeMap<u16, f32> =
                std::collections::BTreeMap::new();
            for slot in 0..4 {
                let weight = influence.weights[slot];
                if weight > 0.0 {
                    *per_bone.entry(influence.bones[slot]).or_insert(0.0) += weight;
                }
                let authored = body.weights[index][slot];
                if authored > 0.0 {
                    *per_bone
                        .entry(body.bone_indices[index][slot])
                        .or_insert(0.0) -= authored;
                }
            }
            error_sum += per_bone
                .values()
                .map(|delta| (*delta).abs() as f64)
                .sum::<f64>();
            let nearest = |bones: [u16; 4], weights: [f32; 4]| {
                let mut best = 0usize;
                for slot in 1..4 {
                    if weights[slot] > weights[best] {
                        best = slot;
                    }
                }
                bones[best]
            };
            if nearest(influence.bones, influence.weights)
                != nearest(body.bone_indices[index], body.weights[index])
            {
                dominant_mismatch += 1;
            }
        }
        println!(
            "self-check: mean per-bone weight error {:.6}, dominant-bone mismatches {} of {}",
            error_sum / same.len() as f64,
            dominant_mismatch,
            same.len()
        );
    }
    Ok(())
}

fn compare(args: &[String]) -> ExitCode {
    match compare_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compare: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Distance summary for one layer of one height slice.
struct LayerDelta {
    count: usize,
    mean: f32,
    p50: f32,
    p95: f32,
    max: f32,
}

impl LayerDelta {
    fn from(mut values: Vec<f32>) -> Self {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let count = values.len();
        if count == 0 {
            return Self {
                count,
                mean: 0.0,
                p50: 0.0,
                p95: 0.0,
                max: 0.0,
            };
        }
        Self {
            count,
            mean: values.iter().sum::<f32>() / count as f32,
            p50: values[count / 2],
            p95: values[(count as f32 * 0.95) as usize],
            max: *values.last().unwrap_or(&0.0),
        }
    }

    fn print(&self, label: &str) {
        if self.count == 0 {
            println!("  {label:5} -");
            return;
        }
        println!(
            "  {label:5} n={:5} mean {:6.3} p50 {:6.3} p95 {:6.3} max {:6.3}",
            self.count, self.mean, self.p50, self.p95, self.max
        );
    }
}

/// Index-wise distance when the topologies match, nearest-surface otherwise.
fn layer_distances(a: &[[f32; 3]], b: &[[f32; 3]]) -> (Vec<f32>, &'static str) {
    if a.len() == b.len() {
        let values = a
            .iter()
            .zip(b)
            .map(|(p, q)| {
                let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .collect();
        return (values, "index-wise");
    }
    let values = a
        .iter()
        .map(|p| {
            b.iter()
                .map(|q| {
                    let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
                })
                .fold(f32::INFINITY, f32::min)
                .sqrt()
        })
        .collect();
    (values, "nearest-surface")
}

fn compare_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let (a_positions, _) = read_obj(Path::new(required(&flags, "a")?))?;
    let (b_positions, _) = read_obj(Path::new(required(&flags, "b")?))?;
    let cloth = match (flags.get("a-cloth"), flags.get("b-cloth")) {
        (Some(a), Some(b)) => {
            let (a_cloth, _) = read_obj(Path::new(a))?;
            let (b_cloth, _) = read_obj(Path::new(b))?;
            Some((a_cloth, b_cloth))
        }
        (None, None) => None,
        _ => return Err("--a-cloth and --b-cloth must be given together".to_string()),
    };
    let bands: usize = match flags.get("bands") {
        Some(value) => value
            .parse()
            .map_err(|_| "--bands must be a positive integer".to_string())?,
        None => 12,
    };
    if bands == 0 {
        return Err("--bands must be a positive integer".to_string());
    }

    let (body_values, body_mode) = layer_distances(&a_positions, &b_positions);
    let cloth_layer = cloth.as_ref().map(|(a_cloth, b_cloth)| {
        let (values, mode) = layer_distances(a_cloth, b_cloth);
        (values, mode)
    });

    println!(
        "compare body: {} vs {} vertices ({body_mode})",
        a_positions.len(),
        b_positions.len()
    );
    LayerDelta::from(body_values.clone()).print("all");
    if let Some((values, mode)) = &cloth_layer {
        println!("compare outfit: {mode}");
        LayerDelta::from(values.clone()).print("all");
    }

    // Height slices: Y is the display frame's up axis in these OBJ files.
    let low = a_positions
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min);
    let high = a_positions
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max);
    let step = (high - low) / bands as f32;
    if step <= 0.0 {
        return Ok(());
    }
    println!("height slices {low:.1}..{high:.1}, {bands} bands of {step:.2} (display units)");
    println!("  band (y)        body                         outfit");
    for band in 0..bands {
        let from = low + step * band as f32;
        let to = if band + 1 == bands {
            high + 1e-4
        } else {
            from + step
        };
        let inside = |p: &[f32; 3]| p[1] >= from && p[1] < to;
        let body_band: Vec<f32> = a_positions
            .iter()
            .zip(body_values.iter())
            .filter(|(position, _)| inside(position))
            .map(|(_, value)| *value)
            .collect();
        let cloth_band: Vec<f32> = match (&cloth, &cloth_layer) {
            (Some((a_cloth, _)), Some((values, _))) => a_cloth
                .iter()
                .zip(values.iter())
                .filter(|(position, _)| inside(position))
                .map(|(_, value)| *value)
                .collect(),
            _ => Vec::new(),
        };
        println!("  {from:6.1}-{to:6.1}");
        LayerDelta::from(body_band).print("body");
        if cloth.is_some() {
            LayerDelta::from(cloth_band).print("cloth");
        }
    }
    Ok(())
}

fn export_obj_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let nif = PathBuf::from(required(&flags, "nif")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let bytes = std::fs::read(&nif).map_err(|e| format!("{}: {e}", nif.display()))?;
    let document = sculpto_nif::read(&bytes).map_err(|e| e.to_string())?;
    let shape = pick_shape(&document.shapes, flags.get("shape").map(String::as_str))?;
    write_obj_with_uv(&out, &shape.positions, &shape.uvs, &shape.triangles)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let material_path = out.with_extension("material.json");
    let material = format!(
        "{{\"shape\": {:?}, \"base_color\": {:?}, \"normal\": {:?}}}\n",
        shape.name, shape.texture_base_color, shape.texture_normal
    );
    std::fs::write(&material_path, material)
        .map_err(|e| format!("{}: {e}", material_path.display()))?;
    if let Some(path) = flags.get("weights-json") {
        let path = PathBuf::from(path);
        let vertices: Vec<Vec<[serde_json::Value; 2]>> = shape
            .bone_indices
            .iter()
            .zip(shape.weights.iter())
            .map(|(bones, weights)| {
                bones
                    .iter()
                    .zip(weights.iter())
                    .filter(|(_, weight)| **weight > 0.0)
                    .map(|(bone, weight)| [serde_json::json!(bone), serde_json::json!(weight)])
                    .collect()
            })
            .collect();
        let payload = serde_json::json!({ "bones": shape.bones, "vertices": vertices });
        std::fs::write(&path, format!("{payload}\n"))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        println!(
            "{}: {} bones, {} skinned vertices (shape `{}`)",
            path.display(),
            shape.bones.len(),
            vertices.iter().filter(|row| !row.is_empty()).count(),
            shape.name
        );
    }
    println!(
        "{}: {} vertices, {} triangles (shape `{}`, base color `{}`)",
        out.display(),
        shape.positions.len(),
        shape.triangles.len(),
        shape.name,
        shape.texture_base_color
    );
    Ok(())
}

/// Write an OBJ in the display frame with `vt` lines and `v/vt` corners.
/// Read the `vt` block of an OBJ. The garment transforms keep vertex order, so
/// the authored per-vertex UVs stay valid for the fitted output.
fn read_obj_uvs(path: &Path) -> Result<Vec<[f32; 2]>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut uvs = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("vt ") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let parse = |value: Option<&str>| -> Result<f32, String> {
            value
                .ok_or_else(|| format!("{}: incomplete `vt` line `{line}`", path.display()))?
                .parse::<f32>()
                .map_err(|e| format!("{}: {e}", path.display()))
        };
        uvs.push([parse(parts.next())?, parse(parts.next())?]);
    }
    Ok(uvs)
}

/// Write source-frame positions with the authored UVs when they line up.
fn write_obj_keep_uv(
    path: &Path,
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    uvs: &[[f32; 2]],
) -> std::io::Result<()> {
    if uvs.len() == positions.len() {
        write_obj_with_uv(path, positions, uvs, triangles)
    } else {
        write_obj(path, positions, triangles)
    }
}

fn write_obj_with_uv(
    path: &Path,
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    triangles: &[[u32; 3]],
) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        out,
        "# sculpto: Y-up display frame from source Z-up (x, z, -y)"
    )?;
    for [x, y, z] in positions {
        writeln!(out, "v {x} {z} {}", -y)?;
    }
    let with_uv = uvs.len() == positions.len();
    if with_uv {
        for [u, v] in uvs {
            writeln!(out, "vt {u} {v}")?;
        }
    }
    for [a, b, c] in triangles {
        if with_uv {
            writeln!(
                out,
                "f {}/{} {}/{} {}/{}",
                a + 1,
                a + 1,
                b + 1,
                b + 1,
                c + 1,
                c + 1
            )?;
        } else {
            writeln!(out, "f {} {} {}", a + 1, b + 1, c + 1)?;
        }
    }
    out.flush()
}

/// Positions plus triangles, the shape an OBJ carries in the display frame.
type Mesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Inverse of the `(x, z, -y)` display conversion `write_obj` applies.
fn to_source(positions: &[[f32; 3]]) -> Vec<[f32; 3]> {
    positions.iter().map(|[x, y, z]| [*x, -*z, *y]).collect()
}

fn garment(args: &[String]) -> ExitCode {
    let usage = "usage: sculpto garment fit --nif <path> --shape <name> --body-shape <name> --body <obj> --out <obj>\n       sculpto garment aura --body <obj> --garment <obj> --out <obj> [--class auto|<kind>]\n       sculpto garment classify --body <obj> --garment <obj>\n       sculpto garment intake --body <obj> --garment <obj>\n       sculpto garment strip --body <obj> --garment <obj> --out <obj>\n       sculpto garment proxies --body <obj> --out <obj>\n       sculpto garment check --body <obj> --garment <obj> [--self 1]
       sculpto garment control-exterior --source <obj> --profile <json> --out <obj> --plan <json>
       sculpto garment control-repair --control <obj> --profile <json> --out <obj> --plan <json>
       sculpto garment control-check --control <obj> --source <obj> --profile <json> --report <json>
       sculpto garment control-separate --control <obj> --profile <json> --out <obj> --report <json>
       sculpto garment control-cuts --profile <json> --report <json>
       sculpto garment uv-check --uv <json> --report <json>";

    let result = match args {
        [action, rest @ ..] if action == "fit" => garment_fit_inner(rest),
        [action, rest @ ..] if action == "follow" => garment_follow_inner(rest),
        [action, rest @ ..] if action == "aura" => garment_aura_inner(rest),
        [action, rest @ ..] if action == "classify" => garment_classify_inner(rest),
        [action, rest @ ..] if action == "intake" => garment_intake_inner(rest),
        [action, rest @ ..] if action == "strip" => garment_strip_inner(rest),
        [action, rest @ ..] if action == "proxies" => garment_proxies_inner(rest),
        [action, rest @ ..] if action == "check" => garment_check_inner(rest),
        [action, rest @ ..] if action == "trim" => garment_trim_inner(rest),
        [action, rest @ ..] if action == "conform" => garment_conform_inner(rest),
        [action, rest @ ..] if action == "fix-clipping" => garment_fix_clipping_inner(rest),
        [action, rest @ ..] if action == "adapt-envelope" => garment_adapt_envelope_inner(rest),
        [action, rest @ ..] if action == "fix-pairs" => garment_fix_pairs_inner(rest),
        [action, rest @ ..] if action == "cover" => garment_cover_inner(rest),
        [action, rest @ ..] if action == "control-exterior" => {
            garment_control::exterior_inner(rest)
        }
        [action, rest @ ..] if action == "control-repair" => garment_control::repair_inner(rest),
        [action, rest @ ..] if action == "control-check" => garment_control::check_inner(rest),
        [action, rest @ ..] if action == "control-separate" => {
            garment_control::separate_inner(rest)
        }
        [action, rest @ ..] if action == "control-cuts" => garment_control::cuts_inner(rest),
        [action, rest @ ..] if action == "uv-check" => garment_uv::check(rest),
        _ => Err(usage.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("garment: {error}");
            ExitCode::FAILURE
        }
    }
}

fn garment_follow_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let (reference, reference_triangles) = read_obj(Path::new(required(&flags, "reference")?))?;
    let (target, target_triangles) = read_obj(Path::new(required(&flags, "target")?))?;
    let (render, render_triangles) = read_obj(Path::new(required(&flags, "garment")?))?;
    let output = Path::new(required(&flags, "out")?);
    let binding =
        sculpto_mesh::surface::SurfaceBinding::bind(&render, &reference, &reference_triangles)
            .map_err(|error| error.to_string())?;
    let transported = binding
        .apply(&target, &target_triangles)
        .map_err(|error| error.to_string())?;
    write_obj(output, &to_source(&transported), &render_triangles)
        .map_err(|error| format!("{}: {error}", output.display()))?;
    println!(
        "{}: {} render vertices, {} triangles, signed normal offsets {:?}; collision verification required",
        output.display(),
        transported.len(),
        render_triangles.len(),
        binding.normal_offset_range()
    );
    Ok(())
}

fn garment_fit_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let nif = PathBuf::from(required(&flags, "nif")?);
    let shape_name = required(&flags, "shape")?;
    let body_shape_name = required(&flags, "body-shape")?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let bytes = std::fs::read(&nif).map_err(|e| format!("{}: {e}", nif.display()))?;
    let document = sculpto_nif::read(&bytes).map_err(|e| e.to_string())?;
    let garment = pick_shape(&document.shapes, Some(shape_name))?;
    let reference = pick_shape(&document.shapes, Some(body_shape_name))?;
    let body = read_obj_positions(&body_path)?;
    let conform =
        Conform::bind(&garment.positions, &reference.positions).map_err(|e| e.to_string())?;
    let fitted = conform.apply(&body).map_err(|e| e.to_string())?;
    write_obj(&out, &fitted, &garment.triangles).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "{}: {} vertices from `{}` on `{}`, offset mean {:.6} max {:.6}",
        out.display(),
        fitted.len(),
        garment.name,
        reference.name,
        conform.offset_mean(),
        conform.offset_max()
    );

    let Some(after_body) = flags.get("body-after") else {
        return Ok(());
    };
    let out_after = PathBuf::from(required(&flags, "out-after")?);
    let after = read_obj_positions(Path::new(after_body))?;
    let fitted_after = conform.apply(&after).map_err(|e| e.to_string())?;
    write_obj(&out_after, &fitted_after, &garment.triangles)
        .map_err(|e| format!("{}: {e}", out_after.display()))?;
    let follow = conform.follow(&body, &after).map_err(|e| e.to_string())?;
    println!(
        "follow (bound vertices): body moved {:.6}, garment moved {:.6}, max difference {:.9}",
        follow.body, follow.garment, follow.difference
    );
    Ok(())
}

fn garment_aura_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_display, body_triangles) = read_obj(&body_path)?;
    let (garment_display, garment_triangles) = read_obj(&garment_path)?;
    // OBJ files are written in the Y-up display frame; the aura runs in the NIF
    // source frame so `write_obj` can convert the result back unchanged.
    let body_positions = to_source(&body_display);
    let garment_positions = to_source(&garment_display);

    // `--class` resolves the corset-aura profile first; explicit numeric
    // flags below still override whatever the profile picked.
    let profile = match flags.get("class").map(String::as_str) {
        None => None,
        Some(requested) => {
            let mut verdict = classify::classify(
                &body_positions,
                &body_triangles,
                &garment_positions,
                &garment_triangles,
            )
            .map_err(|e| e.to_string())?;
            if requested != "auto" {
                verdict.kind =
                    classify::GarmentKind::parse(requested).map_err(|e| e.to_string())?;
            }
            if let Some(name) = flags.get("adherence") {
                verdict.adherence = classify::Adherence::parse(name).map_err(|e| e.to_string())?;
            }
            print_classification(&verdict);
            Some(
                verdict
                    .profile()
                    .ok_or_else(|| classification_blocked(&verdict))?,
            )
        }
    };
    let base = profile.map(|profile| profile.params).unwrap_or_default();
    let mut params = AuraParams {
        clearance: flag_f32(&flags, "clearance", base.clearance)?,
        slack: flag_f32(&flags, "slack", base.slack)?,
        slices: flag_usize(&flags, "slices", base.slices)?,
        bins: flag_usize(&flags, "bins", base.bins)?,
        smooth: flag_usize(&flags, "smooth", base.smooth)?,
        mode: base.mode,
        max_ease: flag_f32(&flags, "max-ease", base.max_ease)?,
        bands: base.bands,
    };
    if let Some(name) = flags.get("mode") {
        let mode = AuraMode::parse(name).map_err(|e| e.to_string())?;
        params.mode = mode;
        if let Some(bands) = params.bands.as_mut() {
            bands.above.mode = mode;
            bands.below.mode = mode;
        }
    }
    let garment_positions = match profile.filter(|p| p.fit == classify::PartFit::ScaleToPart) {
        None => garment_positions,
        Some(_) => {
            let verdict = classify::classify(
                &body_positions,
                &body_triangles,
                &garment_positions,
                &garment_triangles,
            )
            .map_err(|e| e.to_string())?;
            let (scaled, report) = classify::scale_to_part(
                &body_positions,
                &body_triangles,
                &garment_positions,
                &garment_triangles,
                &verdict,
            );
            if !report.applied {
                return Err("scale-to-part found no body contours in the band".to_string());
            }
            println!(
                "scaled to part: factor {:.3} about ({:.1}, {:.1})",
                report.scale, report.anchor[0], report.anchor[1]
            );
            scaled
        }
    };
    let aura = Aura::build(&body_positions, &body_triangles, params).map_err(|e| e.to_string())?;
    let (fitted, report) = aura.apply(&garment_positions);
    let garment_uvs = read_obj_uvs(&garment_path)?;
    write_obj_keep_uv(&out, &fitted, &garment_triangles, &garment_uvs)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let axis = ["x", "y", "z"][report.axis];
    println!(
        "aura: axis {axis}, {} slices, up to {} contours per slice, clearance {:.3}, slack {:.3} (perimeter)",
        report.slices, report.max_contours, params.clearance, params.slack
    );
    if let Some(bands) = params.bands {
        println!(
            "bands: split {:.1}, above {} / {}, below {} / {}",
            bands.split,
            layout_label(bands.above.layout),
            mode_label(bands.above.mode),
            layout_label(bands.below.layout),
            mode_label(bands.below.mode),
        );
    }
    println!(
        "{}: {} vertices, mode {}, moved {} of {} (mean {:.4}, p50 {:.4}, p95 {:.4}, max {:.4}), inside body {} -> {}",
        out.display(),
        fitted.len(),
        mode_label(params.mode),
        report.moved,
        garment_positions.len(),
        report.moved_mean,
        report.moved_p50,
        report.moved_p95,
        report.moved_max,
        report.inside_before,
        report.inside_after
    );
    if report.inside_after > 0 {
        return Err(format!(
            "{} vertices still sit inside the body contour",
            report.inside_after
        ));
    }
    Ok(())
}

fn classification_blocked(verdict: &classify::Classified) -> String {
    match verdict.kind {
        classify::GarmentKind::Composite => format!(
            "composite outfit: fit each part ({}); the union resolves no single profile",
            verdict
                .parts
                .iter()
                .map(|part| format!("{} ({})", part.kind.name(), part.adherence.name()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => format!(
            "classification ended as `{}` and resolves no profile; pass --class with the \
             intended kind",
            verdict.kind.name()
        ),
    }
}

fn print_classification(verdict: &classify::Classified) {
    println!(
        "class: {} ({})",
        verdict.kind.name(),
        verdict.adherence.name()
    );
    println!(
        "features: bottom {:.2} top {:.2} coverage {:.2}, columns {} (leg {}, body {}), merged {}, radial {:.2}, top-ratio {:.2}, angular {:.2}",
        verdict.features.bottom,
        verdict.features.top,
        verdict.features.coverage,
        verdict.features.columns,
        verdict.features.leg_columns,
        verdict.features.body_columns,
        verdict.features.merged,
        verdict.features.radial_ratio,
        verdict.features.top_ratio,
        verdict.features.angular
    );
    for line in &verdict.evidence {
        println!("evidence: {line}");
    }
    if let Some(profile) = verdict.profile() {
        println!(
            "corset-aura: mode {}, clearance {:.3}, slack {:.3}, max-ease {:.2}, anchor {}, fit {}",
            mode_label(profile.params.mode),
            profile.params.clearance,
            profile.params.slack,
            profile.params.max_ease,
            profile.anchor.name(),
            fit_label(profile.fit)
        );
        if let Some(bands) = profile.params.bands {
            println!(
                "bands: split {:.1}, above {} / {}, below {} / {}",
                bands.split,
                layout_label(bands.above.layout),
                mode_label(bands.above.mode),
                layout_label(bands.below.layout),
                mode_label(bands.below.mode),
            );
        }
    }
}

fn layout_label(layout: sculpto_mesh::aura::Layout) -> &'static str {
    match layout {
        sculpto_mesh::aura::Layout::PerContour => "per-contour",
        sculpto_mesh::aura::Layout::Merged => "merged",
    }
}

fn fit_label(fit: classify::PartFit) -> &'static str {
    match fit {
        classify::PartFit::Radial => "radial",
        classify::PartFit::ScaleToPart => "scale-to-part",
    }
}

fn garment_classify_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let (body_display, body_triangles) = read_obj(&body_path)?;
    let (garment_display, garment_triangles) = read_obj(&garment_path)?;
    let verdict = classify::classify(
        &to_source(&body_display),
        &body_triangles,
        &to_source(&garment_display),
        &garment_triangles,
    )
    .map_err(|e| e.to_string())?;
    print_classification(&verdict);
    if verdict.profile().is_none() {
        println!("{}", classification_blocked(&verdict));
    }
    Ok(())
}

fn garment_intake_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let (body_display, body_triangles) = read_obj(&body_path)?;
    let (garment_display, garment_triangles) = read_obj(&garment_path)?;
    let report = sculpto_mesh::intake::audit(
        &to_source(&body_display),
        &body_triangles,
        &to_source(&garment_display),
        &garment_triangles,
    )
    .map_err(|e| e.to_string())?;
    let shares = report
        .shares
        .iter()
        .map(|share| format!("{share:.0}%"))
        .collect::<Vec<_>>()
        .join("/");
    println!(
        "intake: {} vertices, {} triangles, {} major component(s) ({})",
        report.vertices, report.triangles, report.major_components, shares
    );
    println!(
        "openings: {} loops, up to {} edges",
        report.openings.len(),
        report.openings.iter().map(|o| o.edges).max().unwrap_or(0)
    );
    println!(
        "chains inside the outer shell per section: up to {}, self-intersections: {}",
        report.max_interior_chains, report.self_intersections
    );
    let defect = match report.route {
        sculpto_mesh::intake::Route::Repair { defect } => format!(" — {defect}"),
        _ => String::new(),
    };
    println!("route: {}{defect}", report.route.name());
    for line in &report.evidence {
        println!("evidence: {line}");
    }
    match flags.get("require-fit").map(String::as_str) {
        None | Some("0") => {}
        Some("1") if report.route == sculpto_mesh::intake::Route::Fit => {}
        Some("1") => {
            return Err(format!(
                "garment preparation required: {}{defect}",
                report.route.name()
            ));
        }
        Some(value) => return Err(format!("--require-fit expects 0 or 1, got `{value}`")),
    }
    Ok(())
}

fn garment_strip_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_display, body_triangles) = read_obj(&body_path)?;
    let (garment_display, garment_triangles) = read_obj(&garment_path)?;
    let garment_positions = to_source(&garment_display);
    let (kept, report) = sculpto_mesh::intake::strip_interior(
        &to_source(&body_display),
        &body_triangles,
        &garment_positions,
        &garment_triangles,
    )
    .map_err(|e| e.to_string())?;
    let garment_uvs = read_obj_uvs(&garment_path)?;
    write_obj_keep_uv(&out, &garment_positions, &kept, &garment_uvs)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "{}: stripped {} of {} triangles across {} bands (kept {})",
        out.display(),
        report.removed,
        garment_triangles.len(),
        report.bands,
        report.kept
    );
    if report.kept == 0 {
        return Err("every triangle was interior; the body frame is wrong".to_string());
    }
    Ok(())
}

fn garment_proxies_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_display, body_triangles) = read_obj(&body_path)?;
    let params = AuraParams {
        slices: flag_usize(&flags, "slices", 96)?,
        bins: flag_usize(&flags, "bins", 192)?,
        ..AuraParams::default()
    };
    let aura = Aura::build(&to_source(&body_display), &body_triangles, params)
        .map_err(|e| e.to_string())?;
    let sections = aura.proxies();
    let mut text = String::new();
    let mut index = 1usize;
    for section in &sections {
        for contour in &section.contours {
            for point in contour {
                // Proxies are body-frame polylines; write them in the display
                // frame like every other OBJ this tool emits.
                let [x, y, z] = *point;
                let (display_y, display_z) = (z, -y);
                text.push_str(&format!("v {x:.6} {display_y:.6} {display_z:.6}\n"));
                index += 1;
            }
            let first = index - contour.len();
            let mut line = String::from("l");
            for step in 0..=contour.len() {
                line.push_str(&format!(" {}", first + (step % contour.len())));
            }
            text.push_str(line.trim_end());
            text.push('\n');
        }
    }
    std::fs::write(&out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    let contours: usize = sections.iter().map(|section| section.contours.len()).sum();
    println!(
        "{}: {} proxy contours over {} sections",
        out.display(),
        contours,
        sections.len()
    );
    Ok(())
}

fn garment_cover_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let (body_positions, _) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let distances = sculpto_mesh::check::nearest_distances(
        &body_positions,
        &garment_positions,
        &garment_triangles,
    );
    let y_min = body_positions
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min);
    let y_max = body_positions
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max);
    let mut bands = [[0usize; 4]; 10];
    for (position, distance) in body_positions.iter().zip(&distances) {
        let band = (((position[1] - y_min) / (y_max - y_min) * 10.0) as usize).min(9);
        bands[band][0] += 1;
        bands[band][1] += usize::from(*distance <= 2.0);
        bands[band][2] += usize::from(*distance <= 8.0);
        bands[band][3] += usize::from(*distance <= 20.0);
    }
    println!(
        "coverage of {} body points by {}:",
        body_positions.len(),
        garment_path.display()
    );
    for (index, band) in bands.iter().enumerate() {
        let total = band[0].max(1);
        println!(
            "  y {:>3}-{:>3}%: {} points, <=2: {:>5.1}%, <=8: {:>5.1}%, <=20: {:>5.1}%",
            index * 10,
            index * 10 + 10,
            band[0],
            band[1] as f32 / total as f32 * 100.0,
            band[2] as f32 / total as f32 * 100.0,
            band[3] as f32 / total as f32 * 100.0
        );
    }
    Ok(())
}

fn garment_fix_pairs_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let mut options = wrap::PairFixOptions::default();
    if let Some(value) = flags.get("gap") {
        options.gap = value.parse().map_err(|_| "--gap must be a number")?;
    }
    if let Some(value) = flags.get("passes") {
        options.passes = value.parse().map_err(|_| "--passes must be an integer")?;
    }
    if let Some(value) = flags.get("rings") {
        options.rings = value.parse().map_err(|_| "--rings must be an integer")?;
    }
    if flags.get("drop-invalid").is_some_and(|v| v == "0") {
        options.drop_invalid = false;
    }
    let (positions, faces, report) = wrap::fix_pairs(
        &garment_positions,
        &garment_triangles,
        &body_positions,
        &body_triangles,
        &options,
    )
    .ok_or("fix-pairs: empty body or garment mesh")?;
    let garment_uvs = read_obj_uvs(&garment_path)?;
    write_obj_keep_uv(&out, &to_source(&positions), &faces, &garment_uvs)
        .map_err(|error| format!("{}: {error}", out.display()))?;
    println!(
        "{}: {} vertices, pairs {} -> {}, {} rounds, dropped {} malformed triangles",
        out.display(),
        positions.len(),
        report.pairs_before,
        report.pairs_after,
        report.rounds,
        report.dropped
    );
    Ok(())
}

fn garment_adapt_envelope_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_positions, _body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let mut options = wrap::EnvelopeOptions::default();
    if let Some(value) = flags.get("zone-start") {
        options.zone_start = value.parse().map_err(|_| "--zone-start must be a number")?;
    }
    if let Some(value) = flags.get("gap") {
        options.gap = value.parse().map_err(|_| "--gap must be a number")?;
    }
    if let Some(value) = flags.get("zone-end") {
        options.zone_end = value.parse().map_err(|_| "--zone-end must be a number")?;
    }
    if let Some(value) = flags.get("blend") {
        options.blend = value.parse().map_err(|_| "--blend must be a number")?;
    }
    if let Some(value) = flags.get("band") {
        options.band = value.parse().map_err(|_| "--band must be a number")?;
    }
    let (positions, report) = wrap::adapt_envelope(&garment_positions, &body_positions, &options)
        .ok_or("adapt-envelope: empty body or garment mesh")?;
    let garment_uvs = read_obj_uvs(&garment_path)?;
    write_obj_keep_uv(
        &out,
        &to_source(&positions),
        &garment_triangles,
        &garment_uvs,
    )
    .map_err(|error| format!("{}: {error}", out.display()))?;
    println!(
        "{}: {} vertices, moved {}, max move {:.4}",
        out.display(),
        positions.len(),
        report.moved,
        report.max_move
    );
    Ok(())
}

fn garment_fix_clipping_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let mut options = wrap::ClipFixOptions::default();
    if let Some(value) = flags.get("strength") {
        options.strength = value.parse().map_err(|_| "--strength must be a number")?;
    }
    if let Some(value) = flags.get("passes") {
        options.passes = value.parse().map_err(|_| "--passes must be an integer")?;
    }
    let (mut positions, report) = wrap::fix_clipping(
        &garment_positions,
        &garment_triangles,
        &body_positions,
        &body_triangles,
        &options,
    )
    .ok_or("fix-clipping: empty body or garment mesh")?;
    // `--band <y_min:y_max>` bounds the push to one height window of the
    // display frame (Y is the height axis), so a local defect can be repaired
    // without moving the rest of the garment.
    if let Some(spec) = flags.get("band") {
        let (low, high) = spec.split_once(':').ok_or("--band needs `y_min:y_max`")?;
        let low: f32 = low.parse().map_err(|_| "--band bounds must be numbers")?;
        let high: f32 = high.parse().map_err(|_| "--band bounds must be numbers")?;
        let mut moved = 0usize;
        for (source, pushed) in garment_positions.iter().zip(positions.iter_mut()) {
            // both are in the display frame here: fix_clipping works on the
            // OBJ coordinates as read
            let height = source[1];
            if !(low..=high).contains(&height) {
                *pushed = *source;
            } else if pushed
                .iter()
                .zip(source.iter())
                .any(|(a, b)| (a - b).abs() > 1e-6)
            {
                moved += 1;
            }
        }
        println!("band: {moved} vertices moved inside y {low}..{high}");
    }
    let garment_uvs = read_obj_uvs(&garment_path)?;
    write_obj_keep_uv(
        &out,
        &to_source(&positions),
        &garment_triangles,
        &garment_uvs,
    )
    .map_err(|error| format!("{}: {error}", out.display()))?;
    println!(
        "{}: {} vertices, pushed {}, max push {:.4}",
        out.display(),
        positions.len(),
        report.pushed,
        report.max_push
    );
    Ok(())
}

fn garment_conform_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let mut options = wrap::FitOptions::default();
    if let Some(value) = flags.get("gap-mode") {
        options.gap_mode = match value.as_str() {
            "constant" => wrap::GapMode::Constant,
            "keep" => wrap::GapMode::Keep,
            "from-shape" => wrap::GapMode::FromShape,
            _ => return Err("--gap-mode must be `constant`, `keep` or `from-shape`".to_string()),
        };
    }
    if let Some(value) = flags.get("gap") {
        options.gap = value.parse().map_err(|_| "--gap must be a number")?;
    }
    if let Some(value) = flags.get("min-gap") {
        options.min_gap = value.parse().map_err(|_| "--min-gap must be a number")?;
    }
    if let Some(value) = flags.get("max-distance") {
        options.max_distance = value
            .parse()
            .map_err(|_| "--max-distance must be a number")?;
    }
    if let Some(value) = flags.get("min-normal-agreement") {
        options.min_normal_agreement = value
            .parse()
            .map_err(|_| "--min-normal-agreement must be a number")?;
    }
    if let Some(value) = flags.get("stiffness") {
        options.stiffness = value.parse().map_err(|_| "--stiffness must be a number")?;
    }
    if let Some(value) = flags.get("stages") {
        options.stages = value.parse().map_err(|_| "--stages must be an integer")?;
    }
    if let Some(value) = flags.get("iterations") {
        options.iterations = value
            .parse()
            .map_err(|_| "--iterations must be an integer")?;
    }
    if let Some(value) = flags.get("max-strain") {
        options.max_strain = value.parse().map_err(|_| "--max-strain must be a number")?;
    }
    if let Some(value) = flags.get("clipping-passes") {
        options.clipping_passes = value
            .parse()
            .map_err(|_| "--clipping-passes must be an integer")?;
    }
    if let Some(value) = flags.get("relief") {
        options.relief = value.parse().map_err(|_| "--relief must be an integer")?;
    }
    if flags.get("pokes").is_some_and(|v| v == "0") {
        options.pokes = false;
    }
    if let Some(value) = flags.get("flip-rings") {
        options.flip_rings = value
            .parse()
            .map_err(|_| "--flip-rings must be an integer")?;
    }
    if let Some(value) = flags.get("flip-strength") {
        options.flip_strength = value
            .parse()
            .map_err(|_| "--flip-strength must be a number")?;
    }
    let gap_from = match flags.get("gap-from") {
        Some(path) => Some(read_obj(Path::new(path))?),
        None => None,
    };
    let gap_source = match flags.get("gap-source") {
        // Same display frame as the garment it copies.
        Some(path) => Some(read_obj(Path::new(path))?.0),
        None => None,
    };
    if options.gap_mode == wrap::GapMode::FromShape && gap_from.is_none() {
        return Err("--gap-mode from-shape needs --gap-from <obj>".to_string());
    }
    let (positions, report) = wrap::fit_to_body_from(
        &garment_positions,
        &garment_triangles,
        &body_positions,
        &body_triangles,
        gap_from
            .as_ref()
            .map(|(positions, faces)| (positions.as_slice(), faces.as_slice())),
        gap_source.as_deref(),
        &options,
    )
    .ok_or("conform: empty body or garment mesh")?;
    let garment_uvs = read_obj_uvs(&garment_path)?;
    write_obj_keep_uv(
        &out,
        &to_source(&positions),
        &garment_triangles,
        &garment_uvs,
    )
    .map_err(|error| format!("{}: {error}", out.display()))?;
    println!(
        "{}: {} vertices, pulled {}, inside {} -> {}, kept form {}, flipped {}, displacement mean {:.4} max {:.4}",
        out.display(),
        report.vertices,
        report.pulled,
        report.inside_before,
        report.inside_after,
        report.kept_form,
        report.flipped,
        report.mean_displacement,
        report.max_displacement
    );
    Ok(())
}

fn garment_trim_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let pairs = check::intersection_pair_list(
        &garment_positions,
        &garment_triangles,
        &body_positions,
        &body_triangles,
    );
    // Interface band: triangles whose centroid lies in it are kept even when
    // crossing, so a part boundary shared with another garment piece is not cut
    // open. `--keep-band <y_min:y_max>` in the display frame.
    let keep_band: Option<(f32, f32)> = match flags.get("keep-band") {
        Some(spec) => {
            let (low, high) = spec
                .split_once(':')
                .ok_or("--keep-band needs `y_min:y_max`")?;
            Some((
                low.parse()
                    .map_err(|_| "--keep-band bounds must be numbers")?,
                high.parse()
                    .map_err(|_| "--keep-band bounds must be numbers")?,
            ))
        }
        None => None,
    };
    let centroid_y = |triangle: &[u32; 3]| -> f32 {
        (garment_positions[triangle[0] as usize][1]
            + garment_positions[triangle[1] as usize][1]
            + garment_positions[triangle[2] as usize][1])
            / 3.0
    };
    let doomed: std::collections::BTreeSet<usize> = pairs
        .iter()
        .filter_map(|(garment, _)| {
            let triangle = &garment_triangles[*garment];
            match keep_band {
                Some((low, high)) if (low..=high).contains(&centroid_y(triangle)) => None,
                _ => Some(*garment),
            }
        })
        .collect();
    let kept: Vec<[u32; 3]> = garment_triangles
        .iter()
        .enumerate()
        .filter_map(|(index, triangle)| (!doomed.contains(&index)).then_some(*triangle))
        .collect();
    let garment_uvs = read_obj_uvs(&garment_path)?;
    // positions are already in the OBJ display frame; write them verbatim
    {
        let mut text = String::new();
        for position in &garment_positions {
            text.push_str(&format!(
                "v {:.6} {:.6} {:.6}\n",
                position[0], position[1], position[2]
            ));
        }
        let with_uv = garment_uvs.len() == garment_positions.len();
        if with_uv {
            for [u, v] in &garment_uvs {
                text.push_str(&format!("vt {u} {v}\n"));
            }
        }
        for triangle in &kept {
            text.push_str(&if with_uv {
                format!(
                    "f {0}/{0} {1}/{1} {2}/{2}\n",
                    triangle[0] + 1,
                    triangle[1] + 1,
                    triangle[2] + 1
                )
            } else {
                format!(
                    "f {} {} {}\n",
                    triangle[0] + 1,
                    triangle[1] + 1,
                    triangle[2] + 1
                )
            });
        }
        std::fs::write(&out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    println!(
        "{}: removed {} of {} triangles that intersect the body, kept {} -> {}",
        garment_path.display(),
        doomed.len(),
        garment_triangles.len(),
        kept.len(),
        out.display()
    );
    Ok(())
}

fn garment_check_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    // Both inputs share the OBJ display frame, so pure geometric queries need
    // no conversion.
    let started = std::time::Instant::now();
    let cleaned = flags
        .get("ignore-body-internal")
        .is_some_and(|value| value == "1" || value == "true");
    let body_triangles: Vec<[u32; 3]> = if cleaned {
        let flagged = check::internally_crossing_triangles(&body_positions, &body_triangles);
        let skip: std::collections::BTreeSet<u32> = flagged.into_iter().collect();
        let kept: Vec<[u32; 3]> = body_triangles
            .iter()
            .enumerate()
            .filter_map(|(i, t)| (!skip.contains(&(i as u32))).then_some(*t))
            .collect();
        eprintln!(
            "body cleanup: skipped {} internally crossing triangles",
            skip.len()
        );
        kept
    } else {
        body_triangles
    };
    let body_pairs = check::intersection_pairs(
        &garment_positions,
        &garment_triangles,
        &body_positions,
        &body_triangles,
    );
    let self_requested = flags
        .get("self")
        .is_some_and(|value| value == "1" || value == "true");
    let self_pairs = if self_requested {
        check::self_intersection_pairs(&garment_positions, &garment_triangles)
    } else {
        0
    };
    if let Some(path) = flags.get("pairs-out") {
        let pairs = check::intersection_pair_list(
            &garment_positions,
            &garment_triangles,
            &body_positions,
            &body_triangles,
        );
        let mut text = String::new();
        for (garment, body) in &pairs {
            text.push_str(&format!("{garment} {body}\n"));
        }
        std::fs::write(path, text).map_err(|error| format!("{path}: {error}"))?;
        eprintln!("pairs written: {} -> {path}", pairs.len());
    }
    if flags.contains_key("where") {
        let pairs = check::intersection_pair_list(
            &garment_positions,
            &garment_triangles,
            &body_positions,
            &body_triangles,
        );
        for (a, _b) in pairs.iter().take(6) {
            let tri = garment_triangles[*a];
            let centroid: [f32; 3] = std::array::from_fn(|k| {
                (garment_positions[tri[0] as usize][k]
                    + garment_positions[tri[1] as usize][k]
                    + garment_positions[tri[2] as usize][k])
                    / 3.0
            });
            println!(
                "  pair at ({:.1}, {:.1}, {:.1})",
                centroid[0], centroid[1], centroid[2]
            );
        }
    }
    println!(
        "{}: {} garment triangles vs {} body triangles, body pairs {}, self pairs {} ({:?})",
        garment_path.display(),
        garment_triangles.len(),
        body_triangles.len(),
        body_pairs,
        self_pairs,
        started.elapsed()
    );
    if body_pairs > 0 {
        return Err(format!(
            "{body_pairs} garment/body triangle pairs intersect"
        ));
    }
    if self_requested && self_pairs > 0 {
        return Err(format!(
            "{self_pairs} garment triangle pairs intersect beyond shared features"
        ));
    }
    Ok(())
}

fn mode_label(mode: AuraMode) -> &'static str {
    match mode {
        AuraMode::Tight => "tight",
        AuraMode::Press => "press",
        AuraMode::Loose => "loose",
    }
}

fn flag_f32(flags: &BTreeMap<String, String>, name: &str, default: f32) -> Result<f32, String> {
    match flags.get(name) {
        Some(value) => value
            .parse::<f32>()
            .map_err(|_| format!("`--{name}` must be a number, got `{value}`")),
        None => Ok(default),
    }
}

fn flag_usize(
    flags: &BTreeMap<String, String>,
    name: &str,
    default: usize,
) -> Result<usize, String> {
    match flags.get(name) {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| format!("`--{name}` must be a whole number, got `{value}`")),
        None => Ok(default),
    }
}

/// Read an OBJ as positions plus triangles; polygons are fan-triangulated.
fn read_obj(path: &Path) -> Result<Mesh, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("v ") {
            let mut parts = rest.split_whitespace();
            let mut coordinate = || -> Result<f32, String> {
                parts
                    .next()
                    .ok_or_else(|| format!("{}: incomplete vertex line `{line}`", path.display()))?
                    .parse::<f32>()
                    .map_err(|e| format!("{}: {e}", path.display()))
            };
            positions.push([coordinate()?, coordinate()?, coordinate()?]);
        } else if let Some(rest) = line.strip_prefix("f ") {
            let mut corners = Vec::new();
            for corner in rest.split_whitespace() {
                let index = corner
                    .split('/')
                    .next()
                    .ok_or_else(|| format!("{}: empty face corner", path.display()))?;
                let one_based = index.parse::<u32>().map_err(|_| {
                    format!(
                        "{}: face corner `{index}` is not a vertex index",
                        path.display()
                    )
                })?;
                corners.push(one_based.checked_sub(1).ok_or_else(|| {
                    format!("{}: face corner `{index}` is not 1-based", path.display())
                })?);
            }
            if corners.len() < 3 {
                return Err(format!(
                    "{}: face has fewer than three corners",
                    path.display()
                ));
            }
            triangles.extend(
                corners[1..]
                    .windows(2)
                    .map(|edge| [corners[0], edge[0], edge[1]]),
            );
        }
    }
    if positions.is_empty() {
        return Err(format!("{}: no vertex lines", path.display()));
    }
    if let Some(vertex) = triangles
        .iter()
        .flatten()
        .find(|&&index| index as usize >= positions.len())
    {
        return Err(format!(
            "{}: face references vertex {}, but only {} vertices exist",
            path.display(),
            u64::from(*vertex) + 1,
            positions.len()
        ));
    }
    Ok((positions, triangles))
}

/// Source NIF positions are Z-up; every OBJ here uses the `(x, z, -y)` display
/// frame, so garment, reference shape and baked body stay comparable.
fn read_obj_positions(path: &Path) -> Result<Vec<[f32; 3]>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut positions = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("v ") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let mut coordinate = || -> Result<f32, String> {
            parts
                .next()
                .ok_or_else(|| format!("{}: incomplete vertex line `{line}`", path.display()))?
                .parse::<f32>()
                .map_err(|e| format!("{}: {e}", path.display()))
        };
        let [x, y, z] = [coordinate()?, coordinate()?, coordinate()?];
        positions.push([x, -z, y]);
    }
    if positions.is_empty() {
        return Err(format!("{}: no vertex lines", path.display()));
    }
    Ok(positions)
}

fn pick_shape<'a>(shapes: &'a [NifShape], name: Option<&str>) -> Result<&'a NifShape, String> {
    match name {
        Some(name) => shapes
            .iter()
            .find(|shape| shape.name == name)
            .ok_or_else(|| format!("shape `{name}` not found")),
        None => shapes
            .iter()
            .max_by_key(|shape| shape.positions.len())
            .ok_or_else(|| "the NIF has no shapes".to_string()),
    }
}

/// Wavefront OBJ in a Y-up display frame: source positions are Z-up, so they are
/// converted as `(x, z, -y)` here. Both comparison exports use the same frame.
fn write_obj(path: &Path, positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        out,
        "# sculpto: Y-up display frame from source Z-up (x, z, -y)"
    )?;
    for [x, y, z] in positions {
        writeln!(out, "v {x} {z} {}", -y)?;
    }
    for [a, b, c] in triangles {
        writeln!(out, "f {} {} {}", a + 1, b + 1, c + 1)?;
    }
    out.flush()
}

fn sliders(args: &[String]) -> ExitCode {
    match sliders_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sliders: {error}");
            ExitCode::FAILURE
        }
    }
}

fn sliders_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let project_path = PathBuf::from(required(&flags, "project")?);
    let presets_path = PathBuf::from(required(&flags, "presets")?);
    let set_name = required(&flags, "set")?;
    let preset_name = required(&flags, "preset")?;
    let size = match required(&flags, "endpoint")? {
        "big" => SliderSize::Big,
        "small" => SliderSize::Small,
        other => return Err(format!("`--endpoint` must be big or small, got `{other}`")),
    };
    let project = match Document::parse_file(&project_path).map_err(|e| e.to_string())? {
        Document::Project(project) => project,
        Document::Presets(_) => return Err(format!("{} is not a project", project_path.display())),
    };
    let presets = match Document::parse_file(&presets_path).map_err(|e| e.to_string())? {
        Document::Presets(presets) => presets,
        Document::Project(_) => {
            return Err(format!("{} is not a presets file", presets_path.display()));
        }
    };
    let preset = presets
        .iter()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("preset `{preset_name}` not found"))?;
    for (name, percent) in
        BakePlan::percents(&project, preset, set_name, size).map_err(|e| e.to_string())?
    {
        println!("{name}\t{percent}");
    }
    Ok(())
}

fn presets(args: &[String]) -> ExitCode {
    match presets_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("presets: {error}");
            ExitCode::FAILURE
        }
    }
}

fn presets_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let path = PathBuf::from(required(&flags, "presets")?);
    let presets = match Document::parse_file(&path).map_err(|e| e.to_string())? {
        Document::Presets(presets) => presets,
        Document::Project(_) => return Err(format!("{} is not a presets file", path.display())),
    };
    for preset in presets {
        println!(
            "{}\t{}\t{}",
            preset.name,
            preset.slider_set.unwrap_or_else(|| "-".to_string()),
            preset.values.len()
        );
    }
    Ok(())
}

fn batch(args: &[String]) -> ExitCode {
    match batch_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("batch: {error}");
            ExitCode::FAILURE
        }
    }
}

fn batch_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let project_path = PathBuf::from(required(&flags, "project")?);
    let presets_path = PathBuf::from(required(&flags, "presets")?);
    let preset_name = required(&flags, "preset")?;
    let endpoint = required(&flags, "endpoint")?;
    let out_dir = PathBuf::from(required(&flags, "out")?);
    let filter = flags.get("filter").map(String::as_str);
    let limit: Option<usize> = flags
        .get("limit")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| "`--limit` must be a number".to_string())?;
    let size = match endpoint {
        "big" => SliderSize::Big,
        "small" => SliderSize::Small,
        other => return Err(format!("`--endpoint` must be big or small, got `{other}`")),
    };
    let project = match Document::parse_file(&project_path).map_err(|e| e.to_string())? {
        Document::Project(project) => project,
        Document::Presets(_) => return Err(format!("{} is not a project", project_path.display())),
    };
    let presets = match Document::parse_file(&presets_path).map_err(|e| e.to_string())? {
        Document::Presets(presets) => presets,
        Document::Project(_) => {
            return Err(format!("{} is not a presets file", presets_path.display()));
        }
    };
    let preset = presets
        .iter()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("preset `{preset_name}` not found"))?;
    let home = match flags.get("home") {
        Some(home) => PathBuf::from(home),
        None => project_path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| "cannot derive the BodySlide home; pass `--home`".to_string())?
            .to_path_buf(),
    };
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let mut built = 0usize;
    let mut failed = 0usize;
    let mut filtered = 0usize;
    for set in &project.sets {
        if let Some(needle) = filter
            && !set.name.contains(needle)
        {
            filtered += 1;
            continue;
        }
        if let Some(limit) = limit
            && built + failed >= limit
        {
            break;
        }
        let plan = match BakePlan::resolve(&project, preset, &set.name, size, &home) {
            Ok(plan) => plan,
            Err(error) => {
                println!("FAIL {}: {error}", set.name);
                failed += 1;
                continue;
            }
        };
        let source = home
            .join("ShapeData")
            .join(set.data_folder.replace('\\', "/"))
            .join(&set.source_file);
        let bytes = match std::fs::read(&source) {
            Ok(bytes) => bytes,
            Err(error) => {
                println!("FAIL {}: {}: {error}", set.name, source.display());
                failed += 1;
                continue;
            }
        };
        let document = match sculpto_nif::read(&bytes) {
            Ok(document) => document,
            Err(error) => {
                println!("FAIL {}: {error}", set.name);
                failed += 1;
                continue;
            }
        };
        let mut shapes_written = 0usize;
        for shape in &document.shapes {
            let baked = match plan.apply(&shape.name, &shape.positions) {
                Ok(baked) => baked,
                Err(error) => {
                    println!("FAIL {} / {}: {error}", set.name, shape.name);
                    continue;
                }
            };
            let file = out_dir.join(format!(
                "{}__{}.obj",
                sanitize(&set.name),
                sanitize(&shape.name)
            ));
            if let Err(error) = write_obj(&file, &baked, &shape.triangles) {
                println!("FAIL {}: {}: {error}", set.name, file.display());
                continue;
            }
            shapes_written += 1;
        }
        println!(
            "{}: {} shapes, {} sliders, {} unresolved",
            set.name,
            shapes_written,
            plan.sliders.len(),
            plan.unresolved.len()
        );
        built += 1;
    }
    println!("batch: {built} sets built, {failed} failed, {filtered} filtered out");
    if failed > 0 {
        return Err(format!("{failed} sets failed"));
    }
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}
