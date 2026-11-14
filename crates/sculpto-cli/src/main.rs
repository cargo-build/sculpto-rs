//! `sculpto` command-line entry point.
//!
//! The first slices are read-only: inspect a Sculpto input document and print
//! what the core model understood, so format questions are answered by the
//! same parser the adapters will use.

mod actor;
mod garment_control;
mod garment_uv;
mod mcp;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sculpto_bake::BakePlan;
use sculpto_core::shape_data::{MAGIC as SHAPE_DATA_MAGIC, MorphData};
use sculpto_core::{Document, SliderSize};
use sculpto_mesh::aura::{Aura, AuraMode, AuraParams};
use sculpto_mesh::check;
use sculpto_mesh::classify;
use sculpto_mesh::wrap::FitOptions;
use sculpto_mesh::wrap::repair::final_push::{FinalPushOptions, final_push};

use sculpto_engine::obj::write_obj;
use sculpto_engine::ops;

mod run;

const USAGE: &str = "\
usage: sculpto <command> [options]

commands:
  actor sample [options] sample explicit GLB mesh nodes at REST or clip/time
  actor rig [options]   sample parts plus reduced rig graph/influences JSON
  actor pack-cloth [options] pack baked cloth into an unchanged GLB actor
  inspect <path>        parse a document or a `.osd`/`.bsd` morph data file
  bake [options]        resolve a preset, apply it to a body NIF, write an OBJ
  batch [options]       bake every matching set in a project to OBJ files
  sliders [options]     print each slider's effective percentage
  presets [options]     list presets in a SliderPresets collection
  export-obj [options]  read a NIF and write one shape as an OBJ
  export-nif [options]  re-encode NIF shapes for a Skyrim target (one shape or a bundle)
  outfit inspect --nif <path> [--manifest <json>] report shape roles and cloth selection
  outfit conform --nif <path> --shape <name> --body-shape <name> --project <osp>
                 --presets <xml> --preset <name> --set <name> --out <osd>
                 [--body-nif <path>] [--home <path>]
  garment fit [options] bind a garment to its authored body, then fit it to a bake
  garment follow [options] transport render positions through a deformed control
  garment aura [options] press a garment onto the body's cross-section aura
  garment classify [options] measure a garment into its GarmentKind and profile
  garment proxies [options] export the body's per-contour collision proxies
  garment check [options] count garment/body triangle intersections (measured gate)
  garment depth [options] penetration depth of garment vertices inside a body
  garment push [options]  move visible depth violations out to just under a tolerance
  garment trim [options]  cut away garment triangles that intersect the body
  garment cover [options] body coverage per height band (<=2/<=8/<=20 units)
  garment control-exterior [options] keep the outward control surface of a thick shell
  garment control-repair [options] plan corner-flare and incidental-loop repair
  garment control-check [options] measure and gate a finished control surface
  garment control-separate [options] apply the declared bounded local contact repair
  garment control-cuts [options] validate declared source-opening cuts and print the plan
  garment uv-check [options] qualify triangle-corner UV area and atlas bounds
  run --cmd <id>        dispatch one command id and print its JSON envelope
  commands [--json]     list every command id with its mutation class
  mcp                   serve the MCP surface (JSON-RPC 2.0 over stdio)
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
  --list <1>         print every shape and its counts, then stop (no OBJ)

export-nif options:
  --nif <path>       NIF to read
  --out <path>       output NIF
  --target <se|le>   target game version
  --shape <name>     single shape to export (defaults to the largest)
  --shapes <name>    explicit shape list, repeatable (bundle mode)
  --all-shapes       export every document shape (bundle mode)
  --rig-template <path>  family skeleton NIF (single-shape mode only)

garment fit options:
  --nif <path>         garment NIF
  --shape <name>       garment shape inside the NIF (required: the NIF also carries a body)
  --body-shape <name>  body shape the garment was authored on (required)
  --body <obj>         baked body OBJ, same topology as --body-shape
  --out <obj>          fitted garment OBJ
  --body-after <obj>   second bake; reports how body and garment moved
  --out-after <obj>    fitted garment for --body-after
  --conform <osd>      preview a conformed garment slider
  --slider <name>      slider to preview with --conform
  --percent <number>   slider percentage for --conform
  --body-nif <path>    source body NIF for a slider or manifest fit, otherwise --nif
  --manifest <json>    fit ordinary regions and retain declared Blender Cloth geometry

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

garment push options:
  --body <obj>        the surface to grade and push against (display frame)
  --garment <obj>     fitted garment OBJ (display frame)
  --tolerance <units> accepted visible depth; deeper graded vertices move out
  --passes <n>        pass budget (1, bounded by the fitter's 10)
  --out <obj>         pushed garment OBJ (display frame)
  --json              print the per-pass report as JSON
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
        [command, rest @ ..] if command == "export-nif" => export_nif(rest),
        [command, action, rest @ ..] if command == "outfit" && action == "inspect" => {
            outfit_inspect(rest)
        }
        [command, action, rest @ ..] if command == "outfit" && action == "conform" => {
            outfit_conform(rest)
        }
        [command, rest @ ..] if command == "garment" => garment(rest),
        [command, rest @ ..] if command == "run" => run::run(rest),
        [command, rest @ ..] if command == "commands" => run::commands(rest),
        [command] if command == "mcp" => mcp::serve(),
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
        return match MorphData::from_file(path) {
            Ok(data) => {
                print!("{}", render_shape_data(path, &data));
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

/// A `.osd`/`.bsd` data file is binary, so the XML dispatcher must not see it.
///
/// The tag decides for a named container; a bare `.bsd` has no tag at all, so
/// its suffix decides, exactly as the reference does (`SliderSet.cpp:376-380`).
fn is_shape_data(path: &Path) -> bool {
    use std::io::Read;
    let bare = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bsd"));
    if bare {
        return true;
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map(|()| magic == SHAPE_DATA_MAGIC)
        .unwrap_or(false)
}

fn render_shape_data(path: &Path, data: &MorphData) -> String {
    match data {
        MorphData::Named(container) => {
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
        MorphData::Bare(offsets) => {
            // A bare set stores no name; report the facts it does carry instead
            // of guessing a shape from the file name.
            let highest = offsets
                .iter()
                .map(|offset| offset.index)
                .max()
                .map(|index| index.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "{}: bare morph data, {} offsets, highest vertex index {}\n",
                path.display(),
                offsets.len(),
                highest
            )
        }
    }
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

fn outfit_inspect(args: &[String]) -> ExitCode {
    let input = parse_flags(args).and_then(|flags| {
        if let Some(key) = flags
            .keys()
            .find(|key| !matches!(key.as_str(), "nif" | "manifest"))
        {
            return Err(format!("unknown option `--{key}`"));
        }
        Ok(serde_json::json!({
            "nif": required(&flags, "nif")?,
            "manifest": flags.get("manifest"),
        }))
    });
    match input {
        Ok(input) => run::dispatch_payload("outfit.inspect", &input),
        Err(error) => {
            eprintln!("outfit inspect: {error}");
            ExitCode::from(2)
        }
    }
}

fn outfit_conform(args: &[String]) -> ExitCode {
    let input = parse_flags(args).and_then(|flags| {
        if let Some(key) = flags.keys().find(|key| {
            !matches!(
                key.as_str(),
                "nif"
                    | "shape"
                    | "body-nif"
                    | "body-shape"
                    | "project"
                    | "presets"
                    | "preset"
                    | "set"
                    | "home"
                    | "out"
            )
        }) {
            return Err(format!("unknown option `--{key}`"));
        }
        Ok(serde_json::json!({
            "nif": required(&flags, "nif")?,
            "shape": required(&flags, "shape")?,
            "body_nif": flags.get("body-nif"),
            "body_shape": required(&flags, "body-shape")?,
            "project": required(&flags, "project")?,
            "presets": required(&flags, "presets")?,
            "preset": required(&flags, "preset")?,
            "set": required(&flags, "set")?,
            "home": flags.get("home"),
            "out": required(&flags, "out")?,
        }))
    });
    match input {
        Ok(input) => run::dispatch_payload("outfit.conform", &input),
        Err(error) => {
            eprintln!("outfit conform: {error}");
            ExitCode::from(2)
        }
    }
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
    let mut sliders = BTreeMap::new();
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "--slider" {
            let (name, value) = args[index + 1]
                .split_once('=')
                .ok_or_else(|| "`--slider` expects Name=Value".to_string())?;
            let percent: f32 = value
                .parse()
                .map_err(|_| format!("`--slider {name}` value `{value}` is not a number"))?;
            sliders.insert(name.to_string(), percent);
        }
        index += 2;
    }
    let params = ops::BakeParams {
        project: PathBuf::from(required(&flags, "project")?),
        presets: PathBuf::from(required(&flags, "presets")?),
        set: required(&flags, "set")?.to_string(),
        preset: required(&flags, "preset")?.to_string(),
        endpoint: required(&flags, "endpoint")?.to_string(),
        out: PathBuf::from(required(&flags, "out")?),
        source: flags.get("source").map(PathBuf::from),
        shape: flags.get("shape").cloned(),
        home: flags.get("home").map(PathBuf::from),
        sliders,
    };
    let outcome = ops::bake(&params).map_err(|error| error.to_string())?;
    println!(
        "{}: {} vertices, {} triangles, {} sliders, {} unresolved, {} deferred",
        outcome.out.display(),
        outcome.vertices,
        outcome.triangles,
        outcome.sliders,
        outcome.unresolved,
        outcome.deferred
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

fn export_obj_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let params = ops::ExportObjParams {
        nif: PathBuf::from(required(&flags, "nif")?),
        shape: flags.get("shape").cloned(),
        list: flags.contains_key("list"),
        out: flags.get("out").map(PathBuf::from),
    };
    match ops::export_obj(&params).map_err(|error| error.to_string())? {
        ops::ExportObjOutcome::Listed(rows) => {
            for row in rows {
                println!(
                    "{}: {} vertices, {} triangles",
                    row.name, row.vertices, row.triangles
                );
            }
        }
        ops::ExportObjOutcome::Written {
            out,
            shape,
            vertices,
            triangles,
        } => {
            println!(
                "{}: {} vertices, {} triangles (shape `{}`)",
                out.display(),
                vertices,
                triangles,
                shape
            );
        }
    }
    Ok(())
}

fn export_nif(args: &[String]) -> ExitCode {
    match export_nif_input(args) {
        Ok((command, input)) => run::dispatch_payload(command, &input),
        Err(error) => {
            eprintln!("export-nif: {error}");
            ExitCode::from(2)
        }
    }
}

/// Translate argv flags into a registry input: bundle mode dispatches
/// `export.bundle`, the default single-shape mode keeps `export.nif`.
fn export_nif_input(args: &[String]) -> Result<(&'static str, serde_json::Value), String> {
    let mut flags: BTreeMap<String, String> = BTreeMap::new();
    let mut shapes: Vec<String> = Vec::new();
    let mut all_shapes = false;
    let mut iter = args.iter();
    while let Some(key) = iter.next() {
        let name = key
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument `{key}`"))?;
        if name == "all-shapes" {
            all_shapes = true;
            continue;
        }
        let value = iter
            .next()
            .ok_or_else(|| format!("`--{name}` needs a value"))?;
        match name {
            "shapes" => shapes.push(value.clone()),
            "nif" | "out" | "target" | "shape" | "rig-template" => {
                flags.insert(name.to_string(), value.clone());
            }
            other => return Err(format!("unknown option `--{other}`")),
        }
    }
    let bundle = all_shapes || !shapes.is_empty();
    if all_shapes && (!shapes.is_empty() || flags.contains_key("shape")) {
        return Err("`--all-shapes` cannot combine with `--shape` or `--shapes`".to_string());
    }
    if bundle && flags.contains_key("shape") {
        return Err("`--shapes` cannot combine with `--shape`".to_string());
    }
    if bundle && flags.contains_key("rig-template") {
        return Err("`--rig-template` applies to single-shape export only".to_string());
    }
    let mut input = serde_json::json!({
        "nif": required(&flags, "nif")?,
        "out": required(&flags, "out")?,
        "target": required(&flags, "target")?,
    });
    match bundle {
        true => {
            if !shapes.is_empty() {
                input["shapes"] = serde_json::json!(shapes);
            }
            Ok(("export.bundle", input))
        }
        false => {
            if let Some(shape) = flags.get("shape") {
                input["shape"] = serde_json::json!(shape);
            }
            if let Some(rig) = flags.get("rig-template") {
                input["rig_template"] = serde_json::json!(rig);
            }
            Ok(("export.nif", input))
        }
    }
}

/// Positions plus triangles, the shape an OBJ carries in the display frame.
type Mesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Inverse of the `(x, z, -y)` display conversion `write_obj` applies.
fn to_source(positions: &[[f32; 3]]) -> Vec<[f32; 3]> {
    positions.iter().map(|[x, y, z]| [*x, -*z, *y]).collect()
}

fn garment(args: &[String]) -> ExitCode {
    let usage = "usage: sculpto garment fit --nif <path> --shape <name> --body-shape <name> --body <obj> --out <obj> [--manifest <json>] [--collision <obj> --collision-frame source-scene]\n       sculpto garment aura --body <obj> --garment <obj> --out <obj> [--class auto|<kind>]\n       sculpto garment classify --body <obj> --garment <obj>\n       sculpto garment intake --body <obj> --garment <obj>\n       sculpto garment strip --body <obj> --garment <obj> --out <obj>\n       sculpto garment proxies --body <obj> --out <obj>\n       sculpto garment check --body <obj> --garment <obj> [--self 1]
       sculpto garment depth --body <obj> --garment <obj> [--json] [--tolerance <units>]
       sculpto garment push --body <obj> --garment <obj> --tolerance <units> --out <obj> [--passes <n>] [--json]
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
        [action, rest @ ..] if action == "depth" => garment_depth_inner(rest),
        [action, rest @ ..] if action == "push" => garment_push_inner(rest),
        [action, rest @ ..] if action == "trim" => garment_trim_inner(rest),
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
    let percent = flags
        .get("percent")
        .map(|value| value.parse::<f32>())
        .transpose()
        .map_err(|error| format!("invalid --percent: {error}"))?;
    let params = ops::FitParams {
        nif: PathBuf::from(required(&flags, "nif")?),
        shape: required(&flags, "shape")?.to_string(),
        body_shape: required(&flags, "body-shape")?.to_string(),
        body: PathBuf::from(required(&flags, "body")?),
        out: PathBuf::from(required(&flags, "out")?),
        body_after: flags.get("body-after").map(PathBuf::from),
        out_after: flags.get("out-after").map(PathBuf::from),
        conform: flags.get("conform").map(PathBuf::from),
        slider: flags.get("slider").cloned(),
        percent,
        body_nif: flags.get("body-nif").map(PathBuf::from),
        manifest: flags.get("manifest").map(PathBuf::from),
        collision: flags.get("collision").map(PathBuf::from),
        collision_frame: flags.get("collision-frame").cloned(),
    };
    let outcome = ops::garment_fit(&params).map_err(|error| error.to_string())?;
    println!(
        "{}: {} vertices from `{}` on `{}`, offset mean {:.6} max {:.6}",
        outcome.out.display(),
        outcome.vertices,
        outcome.garment,
        outcome.reference,
        outcome.offset_mean,
        outcome.offset_max
    );
    if let Some((body, garment, difference)) = outcome.follow {
        println!(
            "follow (bound vertices): body moved {:.6}, garment moved {:.6}, max difference {:.9}",
            body, garment, difference
        );
    }
    if let Some(regions) = &outcome.regions {
        match &regions.ordinary {
            Some(ordinary) => println!(
                "regions: ordinary {} faces / {} obstacle contacts / {} inside; Blender Cloth {} faces / {} obstacle contacts",
                regions.ordinary_triangles,
                ordinary.intersection_pairs,
                ordinary.inside_after,
                regions.cloth_triangles,
                regions.cloth_intersection_pairs,
            ),
            None => println!(
                "regions: ordinary 0 faces; Blender Cloth {} faces / {} obstacle contacts",
                regions.cloth_triangles, regions.cloth_intersection_pairs,
            ),
        }
        println!(
            "deformation: max displacement {:.6}, max edge strain {:.6}, collapsed edges {}",
            regions.max_displacement, regions.max_strain, regions.collapsed_edges,
        );
    }
    if let (Some(pairs), Some(inside)) = (outcome.intersection_pairs, outcome.inside_after) {
        println!("obstacle contacts {pairs}, inside groups {inside}");
    }
    if let Some(repair) = &outcome.repair {
        println!(
            "deformation: max displacement {:.6}, max edge strain {:.6}, collapsed edges {}; no-clipping fallback {}",
            repair.max_displacement,
            repair.max_strain,
            repair.collapsed_edges,
            repair.no_clipping_fallback_applied,
        );
    }
    if let Some(collision) = &outcome.collision {
        println!(
            "collision: `{}` ({}), {} vertices / {} triangles; unspecified parts are not implied",
            collision.frame, collision.scope, collision.vertices, collision.triangles
        );
    }
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
    write_obj(&out, &fitted, &garment_triangles).map_err(|e| format!("{}: {e}", out.display()))?;
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
    write_obj(&out, &garment_positions, &kept).map_err(|e| format!("{}: {e}", out.display()))?;
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
    let doomed: std::collections::BTreeSet<usize> =
        pairs.iter().map(|(garment, _)| *garment).collect();
    let kept: Vec<[u32; 3]> = garment_triangles
        .iter()
        .enumerate()
        .filter_map(|(index, triangle)| (!doomed.contains(&index)).then_some(*triangle))
        .collect();
    // positions are already in the OBJ display frame; write them verbatim
    {
        let mut text = String::new();
        for position in &garment_positions {
            text.push_str(&format!(
                "v {:.6} {:.6} {:.6}\n",
                position[0], position[1], position[2]
            ));
        }
        for triangle in &kept {
            text.push_str(&format!(
                "f {} {} {}\n",
                triangle[0] + 1,
                triangle[1] + 1,
                triangle[2] + 1
            ));
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

fn garment_depth_inner(args: &[String]) -> Result<(), String> {
    let mut json = false;
    let mut rest = Vec::with_capacity(args.len());
    for arg in args {
        if arg == "--json" {
            json = true;
        } else {
            rest.push(arg.clone());
        }
    }
    let flags = parse_flags(&rest)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let tolerance = flag_tolerance(&flags)?;
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    // Both inputs share the OBJ display frame, so pure geometric queries need
    // no conversion.
    let report = check::garment_depth_report(
        (&garment_positions, &garment_triangles),
        (&body_positions, &body_triangles),
    )
    .map_err(|error| error.to_string())?;
    let verdict = tolerance.map(|tolerance| report.visible_verdict(tolerance));
    if json {
        let above: Vec<serde_json::Value> = check::DEPTH_THRESHOLDS
            .iter()
            .zip(&report.above)
            .map(|(&threshold, &count)| {
                serde_json::json!({
                    "threshold": depth_number(threshold),
                    "count": count,
                })
            })
            .collect();
        let worst: Vec<serde_json::Value> = report
            .worst
            .iter()
            .map(|&(vertex, depth)| {
                serde_json::json!({
                    "vertex": vertex,
                    "depth": depth_number(depth),
                })
            })
            .collect();
        let mut payload = serde_json::json!({
            "body": body_path.display().to_string(),
            "garment": garment_path.display().to_string(),
            "units": "source",
            "semantics": "max(0, -dot(point - closest_body_point, body_triangle_normal))",
            "layers_semantics": "visible when dot(garment_vertex_normal, nearest_body_triangle_normal) >= 0, inner when < 0; hidden when the garment's own surface is first along the nearest body point's outward normal and the hit is not the penetrating vertex's own triangle; visible_occluded = visible and not hidden, the layered graded set",
            "garment_vertices": report.vertices,
            "garment_triangles": garment_triangles.len(),
            "body_triangles": body_triangles.len(),
            "body_pairs": report.body_pairs,
            "self_pairs": report.self_pairs,
            "contact_vertices": report.contacts,
            "inside_vertices": report.inside,
            "depth": {
                "vertices": report.inside,
                "mean": depth_number(report.mean),
                "median": depth_number(report.median),
                "p90": depth_number(report.p90),
                "p99": depth_number(report.p99),
                "max": depth_number(report.max),
            },
            "layers": {
                "visible": depth_layer_json(&report.visible),
                "inner": depth_layer_json(&report.inner),
                "hidden": depth_layer_json(&report.hidden),
                "visible_occluded": depth_layer_json(&report.visible_occluded),
            },
            "above": above,
            "worst": worst,
        });
        if let Some(verdict) = &verdict {
            let violations: Vec<serde_json::Value> = verdict
                .violations
                .iter()
                .map(|&(vertex, depth)| {
                    serde_json::json!({
                        "vertex": vertex,
                        "depth": depth_number(depth),
                    })
                })
                .collect();
            payload["verdict"] = serde_json::json!({
                "result": if verdict.passed { "pass" } else { "fail" },
                "tolerance": depth_number(verdict.tolerance),
                "visible_max": depth_number(report.visible_occluded.max),
                "violations": violations,
            });
        }
        println!("{payload}");
        return Ok(());
    }
    println!(
        "{}: {} garment vertices vs {} body triangles; body pairs {}, self pairs {}",
        garment_path.display(),
        report.vertices,
        body_triangles.len(),
        report.body_pairs,
        report.self_pairs
    );
    println!(
        "contact vertices {} (inside {}); depth units: source",
        report.contacts, report.inside
    );
    println!(
        "depth mean {:.6}, median {:.6}, p90 {:.6}, p99 {:.6}, max {:.6}",
        report.mean, report.median, report.p90, report.p99, report.max
    );
    println!(
        "layers: visible {} (max {:.6}, mean {:.6}, p90 {:.6}), inner {} (max {:.6}, mean {:.6}, p90 {:.6})",
        report.visible.count,
        report.visible.max,
        report.visible.mean,
        report.visible.p90,
        report.inner.count,
        report.inner.max,
        report.inner.mean,
        report.inner.p90
    );
    println!(
        "occlusion: hidden {} (max {:.6}, mean {:.6}, p90 {:.6}), visible_occluded {} (max {:.6}, mean {:.6}, p90 {:.6})",
        report.hidden.count,
        report.hidden.max,
        report.hidden.mean,
        report.hidden.p90,
        report.visible_occluded.count,
        report.visible_occluded.max,
        report.visible_occluded.mean,
        report.visible_occluded.p90
    );
    let bands: Vec<String> = check::DEPTH_THRESHOLDS
        .iter()
        .zip(&report.above)
        .map(|(threshold, count)| format!("{threshold}: {count}"))
        .collect();
    println!("above depth: {}", bands.join(", "));
    if report.worst.is_empty() {
        println!("worst (index, depth): none");
    } else {
        let worst: Vec<String> = report
            .worst
            .iter()
            .map(|(vertex, depth)| format!("({vertex}, {depth:.6})"))
            .collect();
        println!("worst (index, depth): {}", worst.join(", "));
    }
    if let Some(verdict) = &verdict {
        if verdict.passed {
            println!(
                "verdict: pass (tolerance {:.6}, visible max {:.6})",
                verdict.tolerance, report.visible_occluded.max
            );
        } else {
            println!(
                "verdict: fail (tolerance {:.6}, visible max {:.6}, {} above)",
                verdict.tolerance,
                report.visible_occluded.max,
                verdict.violations.len()
            );
            let violating: Vec<String> = verdict
                .violations
                .iter()
                .map(|(vertex, depth)| format!("({vertex}, {depth:.6})"))
                .collect();
            println!(
                "violating visible vertices (index, depth): {}",
                violating.join(", ")
            );
        }
    }
    println!(
        "note: inside = behind the nearest body triangle's normal; visible/inner split by the cosine of the garment vertex normal there (>= 0 visible); hidden = the garment's own surface is first along that body point's outward normal and it is not the vertex's own triangle (DECL0015 contact point); visible_occluded = visible and not hidden, the graded set; body winding supplies outward and one-sided depth is not containment"
    );
    Ok(())
}

/// The fitter's opt-in final push-out stage over saved geometry.
///
/// `garment depth` says what fails; this moves exactly the graded violations the
/// fit's post-repair stage moves and writes the result, so a fitted OBJ can be
/// re-graded against the same tolerance. Both inputs share the OBJ display
/// frame; positions are converted to the source frame the writer expects.
fn garment_push_inner(args: &[String]) -> Result<(), String> {
    let mut json = false;
    let mut rest = Vec::with_capacity(args.len());
    for arg in args {
        if arg == "--json" {
            json = true;
        } else {
            rest.push(arg.clone());
        }
    }
    let flags = parse_flags(&rest)?;
    let body_path = PathBuf::from(required(&flags, "body")?);
    let garment_path = PathBuf::from(required(&flags, "garment")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let tolerance = flag_tolerance(&flags)?
        .ok_or("`--tolerance` is required: it states the accepted visible depth")?;
    let passes = flag_usize(&flags, "passes", 1)?;
    let passes = u32::try_from(passes)
        .map_err(|_| format!("`--passes` {passes} is above the supported range"))?;
    let (body_positions, body_triangles) = read_obj(&body_path)?;
    let (garment_positions, garment_triangles) = read_obj(&garment_path)?;
    let (pushed, report) = final_push(
        (&to_source(&garment_positions), &garment_triangles),
        (&to_source(&body_positions), &body_triangles),
        None,
        &FitOptions::default(),
        &FinalPushOptions { passes, tolerance },
    )
    .map_err(|error| error.to_string())?;
    write_obj(&out, &pushed, &garment_triangles)
        .map_err(|error| format!("{}: {error}", out.display()))?;
    if json {
        let per_pass: Vec<serde_json::Value> = report
            .per_pass
            .iter()
            .map(|pass| {
                serde_json::json!({
                    "pass": pass.pass,
                    "moved": pass.moved,
                    "violations": pass.violations,
                    "max": depth_number(pass.max),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "body": body_path.display().to_string(),
                "garment": garment_path.display().to_string(),
                "out": out.display().to_string(),
                "units": "source",
                "semantics": "each pass moves the welded group of every graded vertex deeper than the tolerance along the nearest body triangle's outward normal by depth - tolerance + 1e-4",
                "tolerance": depth_number(report.tolerance),
                "budget": report.budget,
                "passes": report.passes,
                "before_violations": report.before_violations,
                "before_max": depth_number(report.before_max),
                "per_pass": per_pass,
                "violations": report.violations,
                "max": depth_number(report.max),
                "held": report.held,
            })
        );
        return Ok(());
    }
    println!(
        "{}: {} garment vertices, pass budget {}",
        out.display(),
        pushed.len(),
        report.budget
    );
    println!(
        "visible violations {} (max {:.6}) at tolerance {:.6} -> {} (max {:.6}) after {} passes; held {}",
        report.before_violations,
        report.before_max,
        report.tolerance,
        report.violations,
        report.max,
        report.passes,
        report.held
    );
    for pass in &report.per_pass {
        println!(
            "pass {}: moved {}, violations {}, max {:.6}",
            pass.pass, pass.moved, pass.violations, pass.max
        );
    }
    println!(
        "note: graded vertices are `garment depth`'s visible_occluded set; a fully locked vertex, a rigid part or a vertex whose nearest body triangle is degenerate is held, and welded copies move together; the tolerance stays the caller's"
    );
    Ok(())
}

/// JSON view of one penetration layer.
fn depth_layer_json(layer: &check::DepthLayer) -> serde_json::Value {
    serde_json::json!({
        "count": layer.count,
        "mean": depth_number(layer.mean),
        "p90": depth_number(layer.p90),
        "max": depth_number(layer.max),
    })
}

/// Parse the optional acceptance tolerance in source units; without it there
/// is no verdict. Rejects values that cannot name a depth bound.
fn flag_tolerance(flags: &BTreeMap<String, String>) -> Result<Option<f32>, String> {
    let Some(value) = flags.get("tolerance") else {
        return Ok(None);
    };
    let tolerance = value
        .parse::<f32>()
        .map_err(|_| format!("`--tolerance` must be a number, got `{value}`"))?;
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(format!(
            "`--tolerance` must be a finite non-negative number, got `{value}`"
        ));
    }
    Ok(Some(tolerance))
}

/// JSON depths are rounded to 1e-6 source units: f32's binary expansion adds
/// noise far below the measurement's precision.
fn depth_number(value: f32) -> f64 {
    (f64::from(value) * 1e6).round() / 1e6
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

/// Read an OBJ as display-frame positions plus triangles; polygons are
/// fan-triangulated by the engine parser both surfaces share.
fn read_obj(path: &Path) -> Result<Mesh, String> {
    sculpto_engine::obj::read_obj_mesh(path).map_err(|error| error.to_string())
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
