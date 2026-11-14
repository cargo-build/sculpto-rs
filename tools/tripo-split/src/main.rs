//! CLI for `tripo-split`: one source GLB in, one GLB per figure plus a manifest.

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut glb = None;
    let mut out_dir = None;
    let mut manifest = None;
    let mut source = "tripo3d-parts".to_string();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--glb" => glb = arguments.next(),
            "--out-dir" => out_dir = arguments.next(),
            "--manifest" => manifest = arguments.next(),
            "--source" => {
                source = arguments
                    .next()
                    .ok_or_else(|| "--source needs a label".to_string())?;
            }
            "--help" | "-h" => {
                println!(
                    "usage: tripo-split --glb PATH --out-dir DIR --manifest PATH [--source LABEL]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let glb = glb.ok_or_else(|| "--glb PATH is required".to_string())?;
    let out_dir = out_dir.ok_or_else(|| "--out-dir DIR is required".to_string())?;
    let manifest = manifest.ok_or_else(|| "--manifest PATH is required".to_string())?;
    let report =
        tripo_split::split_glb(std::path::Path::new(&glb), std::path::Path::new(&out_dir))?;
    tripo_split::write_manifest(&report, std::path::Path::new(&manifest), &source)?;
    println!(
        "split {} into {} part(s); manifest {}",
        report.source.display(),
        report.parts.len(),
        manifest
    );
    for part in &report.parts {
        println!(
            "  {} {} v / {} t -> {}",
            part.part,
            part.vertices,
            part.triangles,
            part.path.display()
        );
    }
    Ok(())
}
