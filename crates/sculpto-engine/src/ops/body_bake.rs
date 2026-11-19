//! Read BodySlide inputs and carry the source attributes through the position bake.

use std::path::{Path, PathBuf};

use sculpto_bake::BakePlan;
use sculpto_core::{Document, Preset, Project, SliderSet};
use sculpto_nif::{NifDocument, NifShape};

use crate::{CommandError, errors, obj::write_obj};

pub fn read_bake_project(path: &Path) -> Result<Project, CommandError> {
    match Document::parse_file(path).map_err(errors::document)? {
        Document::Project(project) => Ok(project),
        Document::Presets(_) => Err(errors::invalid(
            "project",
            format!("{} is not a project", path.display()),
        )),
    }
}

pub fn read_bake_presets(path: &Path) -> Result<Vec<Preset>, CommandError> {
    match Document::parse_file(path).map_err(errors::document)? {
        Document::Presets(presets) => Ok(presets),
        Document::Project(_) => Err(errors::invalid(
            "presets",
            format!("{} is not a presets file", path.display()),
        )),
    }
}

pub fn bake_home(project: &Path, home: Option<&Path>) -> Result<PathBuf, CommandError> {
    home.or_else(|| project.parent().and_then(Path::parent))
        .map(Path::to_path_buf)
        .ok_or_else(|| errors::invalid("home", "cannot derive the BodySlide home; pass it"))
}

pub fn read_bake_source(
    home: &Path,
    set: &SliderSet,
    source: Option<&Path>,
) -> Result<(PathBuf, NifDocument), CommandError> {
    let path = source.map(Path::to_path_buf).unwrap_or_else(|| {
        home.join("ShapeData")
            .join(set.data_folder.replace('\\', "/"))
            .join(&set.source_file)
    });
    let bytes = std::fs::read(&path).map_err(|error| errors::io(&path, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    Ok((path, document))
}

/// Positions after bake, with the ordered source indices and attributes still in scope.
pub struct BakedShape<'a> {
    pub source: &'a NifShape,
    pub positions: Vec<[f32; 3]>,
}

impl<'a> BakedShape<'a> {
    pub fn apply(
        plan: &BakePlan,
        source: &'a NifShape,
        target: &str,
    ) -> Result<Self, CommandError> {
        let positions = plan
            .apply(target, &source.positions)
            .map_err(errors::bake)?;
        Ok(Self { source, positions })
    }

    pub fn write_obj(&self, path: &Path) -> Result<(), CommandError> {
        write_obj(path, &self.positions, &self.source.triangles)
            .map_err(|error| errors::io(path, error))
    }
}
