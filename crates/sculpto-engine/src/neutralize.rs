//! BodySlide batch orchestration for the mesh neutralizer and static GLB writer.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use sculpto_bake::BakePlan;
use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_core::{Preset, Project, SliderSet, SliderSize};
use sculpto_gltf::{review::smooth_normals, write_glb};
use sculpto_mesh::neutralize::{NeutralizeError, Neutralizer};
use sculpto_nif::NifShape;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::obj::{pick_shape_index, source_to_display};
use crate::ops::{BakedShape, bake_home, read_bake_presets, read_bake_project, read_bake_source};
use crate::viewer::UNITS_PER_METRE;
use crate::{CommandError, ErrorKind, errors};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    /// BodySlide projects. Preset set names resolve across these files.
    projects: Vec<PathBuf>,
    /// BodySlide SliderPresets collections.
    presets: Vec<PathBuf>,
    /// Exact preset names in output order. Empty selects all collection entries.
    #[serde(default)]
    names: Vec<String>,
    /// Set override for all selections. Defaults to each preset's authored set.
    set: Option<String>,
    /// Body shape override. The largest shape is the default, as in bake.
    shape: Option<String>,
    /// BodySlide home override. Defaults to each resolved project's grandparent.
    home: Option<PathBuf>,
    /// Weight endpoints. `both` selects `_0` and `_1` when authored.
    #[serde(default)]
    endpoint: Endpoint,
    /// Output directory for the per-preset GLBs.
    out: PathBuf,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Endpoint {
    #[default]
    Both,
    Small,
    Big,
}

impl Endpoint {
    fn sizes(self, gen_weights: bool) -> &'static [SliderSize] {
        match (self, gen_weights) {
            (Self::Both, true) => &[SliderSize::Small, SliderSize::Big],
            (Self::Both | Self::Big, _) => &[SliderSize::Big],
            (Self::Small, true) => &[SliderSize::Small],
            (Self::Small, false) => &[],
        }
    }
}

struct InputProject {
    path: PathBuf,
    home: PathBuf,
    project: Project,
}

struct PreparedBody<'a> {
    project: &'a InputProject,
    set: &'a SliderSet,
    source: PathBuf,
    shape: NifShape,
    neutralizer: Neutralizer,
}

impl<'a> PreparedBody<'a> {
    fn load(
        project: &'a InputProject,
        set: &'a SliderSet,
        shape: Option<&str>,
    ) -> Result<Self, CommandError> {
        let (source, mut document) = read_bake_source(&project.home, set, None)?;
        let index = pick_shape_index(&document.shapes, shape)?;
        let shape = document.shapes.swap_remove(index);
        let neutralizer =
            Neutralizer::bind(&shape.positions, &shape.triangles).map_err(neutralize_error)?;
        Ok(Self {
            project,
            set,
            source,
            shape,
            neutralizer,
        })
    }

    fn export(
        &self,
        preset: &Preset,
        size: SliderSize,
        path: &Path,
    ) -> Result<Value, CommandError> {
        let plan = BakePlan::resolve(
            &self.project.project,
            preset,
            &self.set.name,
            size,
            &self.project.home,
        )
        .map_err(errors::bake)?;
        let target = self
            .set
            .target_for_shape(&self.shape.name)
            .unwrap_or(&self.shape.name);
        if let Some(zap) = plan
            .zaps
            .iter()
            .find(|zap| zap.target == target && !zap.indices.is_empty())
        {
            return Err(CommandError::new(
                ErrorKind::Unsupported,
                "topology_changed_by_zap",
                format!("{} removes vertices from {}", zap.slider, self.shape.name),
            ));
        }
        if let Some(unresolved) = plan
            .unresolved
            .iter()
            .find(|reference| reference.target == target)
        {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "unresolved_body_morph",
                format!("{}: {:?}", unresolved.slider, unresolved.reason),
            ));
        }
        let baked = BakedShape::apply(&plan, &self.shape, target)?;
        let mut normals = smooth_normals(&baked.positions, &self.shape.triangles)
            .into_iter()
            .map(source_to_display)
            .collect::<Vec<_>>();
        let mut positions = baked
            .positions
            .into_iter()
            .map(|row| source_to_display(row).map(|v| v / UNITS_PER_METRE))
            .collect::<Vec<_>>();
        // Qualify the f32 positions in the same frame and units the GLB stores.
        let report = self
            .neutralizer
            .apply(&mut positions, &mut normals)
            .map_err(neutralize_error)?;
        let asset = MeshAsset {
            name: preset.name.clone(),
            positions,
            normals,
            uvs: baked.source.uvs.clone(),
            triangles: baked.source.triangles.clone(),
            triangle_partitions: vec![PartitionId(0); baked.source.triangles.len()],
            ..MeshAsset::default()
        };
        let bytes = write_glb(&asset).map_err(|error| {
            CommandError::new(ErrorKind::Refused, "neutral_glb_export", error.to_string())
        })?;
        write_new_glb(path, &bytes)?;
        Ok(json!({
            "preset": preset.name,
            "endpoint": endpoint_label(size),
            "set": self.set.name,
            "shape": self.shape.name,
            "project": self.project.path,
            "source": self.source,
            "out": path,
            "vertices": asset.positions.len(),
            "triangles": asset.triangles.len(),
            "units": "metres",
            "deferred_sliders": plan.deferred.iter().map(|slider| &slider.name).collect::<Vec<_>>(),
            "geometry": report,
        }))
    }
}

pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(Params))
}

pub(crate) fn run(input: &Value) -> Result<Value, CommandError> {
    let params: Params = serde_json::from_value(input.clone())
        .map_err(|error| errors::invalid("body.neutralize", error.to_string()))?;
    if params.projects.is_empty() || params.presets.is_empty() || params.out.as_os_str().is_empty()
    {
        return Err(errors::invalid(
            "body.neutralize",
            "projects, preset collections and output directory are required",
        ));
    }
    let projects = params
        .projects
        .iter()
        .map(|path| {
            Ok(InputProject {
                path: path.clone(),
                home: bake_home(path, params.home.as_deref())?,
                project: read_bake_project(path)?,
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    let presets = params
        .presets
        .iter()
        .map(|path| read_bake_presets(path))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let selections: Vec<_> = if params.names.is_empty() {
        presets
            .iter()
            .map(|preset| (preset.name.as_str(), Ok(preset)))
            .collect()
    } else {
        params
            .names
            .iter()
            .map(|name| {
                (
                    name.as_str(),
                    unique_match(
                        "preset",
                        name,
                        presets.iter().filter(|preset| preset.name == *name),
                    ),
                )
            })
            .collect()
    };
    if selections.is_empty() {
        return Err(errors::invalid(
            "presets",
            "no presets in the supplied collections",
        ));
    }
    fs::create_dir_all(&params.out).map_err(|error| errors::io(&params.out, error))?;
    let mut sources = HashMap::new();
    let mut processed = Vec::new();
    let mut skipped = Vec::new();
    for (ordinal, (name, request)) in selections.into_iter().enumerate() {
        let (preset, (project_index, set)) = match request.and_then(|preset| {
            find_set(&projects, preset, params.set.as_deref()).map(|set| (preset, set))
        }) {
            Ok(selection) => selection,
            Err(error) => {
                skipped.push(skipped_row(name, None, None, &error));
                continue;
            }
        };
        let sizes = params.endpoint.sizes(set.output_file.gen_weights);
        if sizes.is_empty() {
            let error = CommandError::new(
                ErrorKind::Unsupported,
                "endpoint_unavailable",
                "the set authors a single weight",
            );
            skipped.push(skipped_row(
                name,
                Some(SliderSize::Small),
                Some(&set.name),
                &error,
            ));
            continue;
        }
        let source = sources
            .entry((project_index, set.name.as_str()))
            .or_insert_with(|| {
                PreparedBody::load(&projects[project_index], set, params.shape.as_deref())
            });
        for &size in sizes {
            let path = params.out.join(output_name(ordinal + 1, name, size));
            match source
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|body| body.export(preset, size, &path))
            {
                Ok(row) => processed.push(row),
                Err(error) => skipped.push(skipped_row(name, Some(size), Some(&set.name), &error)),
            }
        }
    }
    Ok(json!({"processed": processed, "skipped": skipped}))
}

fn find_set<'a>(
    projects: &'a [InputProject],
    preset: &Preset,
    override_set: Option<&str>,
) -> Result<(usize, &'a SliderSet), CommandError> {
    let name = override_set
        .or(preset.slider_set.as_deref())
        .ok_or_else(|| {
            errors::invalid(
                "set",
                "the preset does not declare a set; pass a set override",
            )
        })?;
    let sets = projects.iter().enumerate().flat_map(|(index, input)| {
        input
            .project
            .sets
            .iter()
            .filter(move |set| set.name == name)
            .map(move |set| (index, set))
    });
    unique_match("set", name, sets)
}

fn unique_match<T>(
    what: &str,
    name: &str,
    mut matches: impl Iterator<Item = T>,
) -> Result<T, CommandError> {
    let value = matches
        .next()
        .ok_or_else(|| errors::not_found(what, name))?;
    if matches.next().is_some() {
        return Err(errors::invalid(
            what,
            format!("ambiguous name `{name}` in the supplied files"),
        ));
    }
    Ok(value)
}

fn endpoint_label(size: SliderSize) -> &'static str {
    match size {
        SliderSize::Small => "small",
        SliderSize::Big => "big",
    }
}

fn output_name(ordinal: usize, preset: &str, size: SliderSize) -> String {
    let slug: String = preset
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .take(100)
        .collect();
    let slug = if slug.is_empty() { "preset" } else { &slug };
    let weight = usize::from(size == SliderSize::Big);
    format!("{ordinal:03}_{slug}_{weight}.glb")
}

fn skipped_row(
    name: &str,
    size: Option<SliderSize>,
    set: Option<&str>,
    error: &CommandError,
) -> Value {
    json!({
        "preset": name,
        "endpoint": size.map(endpoint_label),
        "set": set,
        "code": error.code(),
        "message": error.message(),
    })
}

fn neutralize_error(error: NeutralizeError) -> CommandError {
    match error {
        NeutralizeError::UnsupportedTopology { .. } => CommandError::new(
            ErrorKind::Unsupported,
            "unsupported_topology",
            error.to_string(),
        ),
        _ => CommandError::new(
            ErrorKind::Refused,
            "neutralization_refused",
            error.to_string(),
        ),
    }
}

fn write_new_glb(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    let pending = path.with_extension(format!("glb.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|error| errors::io(&pending, error))?;
    let written = file.write_all(bytes);
    drop(file);
    // Publish the completed file atomically and preserve any existing result.
    let result = written.and_then(|()| fs::hard_link(&pending, path));
    let _ = fs::remove_file(&pending);
    result.map_err(|error| errors::io(path, error))
}
