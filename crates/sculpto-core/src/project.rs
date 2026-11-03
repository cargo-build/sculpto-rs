//! Buildable slider projects (`.osp` `SliderSetInfo` documents).
//!
//! A project owns one or more slider sets. Each set names the source mesh, the
//! output file and the sliders that deform it; slider records point at
//! external `.osd`/`.bsd` morph data by file and record name. Sculpto keeps
//! that reference unresolved: resolving paths is a build-time concern.

use std::collections::BTreeSet;
use std::path::Path;

use roxmltree::Node;

use crate::error::{Error, ErrorKind, Result};
use crate::report::UnknownElement;
use crate::xml_util::{
    bool_attr, element_path, f32_attr, optional_attr, required_attr, required_text,
};

/// The only `SliderSetInfo` version this crate reads.
pub const SUPPORTED_VERSION: u32 = 1;

/// Default seam-normal smoothing angle used by BodySlide when unspecified.
pub const DEFAULT_SMOOTH_SEAM_NORMALS_ANGLE: f32 = 60.0;

/// Output file settings of a slider set.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputFile {
    /// Output file stem without extension, e.g. `femalebody`.
    pub name: String,
    /// Emit skin weights into the built mesh.
    pub gen_weights: bool,
    /// Skip writing the `.tri` morph file next to the built mesh.
    pub prevent_morph_file: bool,
    /// Keep geometry that zap sliders would otherwise delete.
    pub keep_zapped_shapes: bool,
}

/// A mesh shape a slider set can target.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeRef {
    /// Shape name inside the source mesh.
    pub name: String,
    /// Target shape name used by slider records.
    pub target: String,
    /// Optional per-shape data folder override.
    pub data_folder: Option<String>,
    /// Smooth seams when generating normals.
    pub smooth_seam_normals: bool,
    /// Angle threshold for seam smoothing, in degrees.
    pub smooth_seam_normals_angle: f32,
    /// Keep authored normals instead of regenerating them.
    pub lock_normals: bool,
}

/// One morph record reference for a slider and target shape.
#[derive(Debug, Clone, PartialEq)]
pub struct SliderDataRef {
    /// Record name as addressed by the author, e.g. `HIMBO - FirstLean`.
    pub name: String,
    /// Target shape this record deforms.
    pub target: String,
    /// Resolve inside the slider set's own data folder.
    pub local: bool,
    /// Data file name, e.g. `HIMBO Body 1st.osd`. `None` when the project
    /// records the reference without a container; BodySlide treats such an
    /// entry as unresolved, and Sculpto preserves it for round-trip fidelity.
    pub file: Option<String>,
    /// Record inside a multi-record container; `None` for single-record `.bsd`.
    pub record: Option<String>,
}

impl SliderDataRef {
    /// Split the authored `file.osd\\record` form into file and record.
    fn split_reference(text: &str) -> (String, Option<String>) {
        match text.rfind(['\\', '/']) {
            Some(index) => {
                let file = text[..index].to_string();
                let record = text[index + 1..].to_string();
                (file, (!record.is_empty()).then_some(record))
            }
            None => (text.to_string(), None),
        }
    }
}

/// A slider definition with its endpoints and flags.
#[derive(Debug, Clone, PartialEq)]
pub struct SliderDef {
    /// Slider name, unique within its set.
    pub name: String,
    /// Reverse the morph direction for this slider.
    pub invert: bool,
    /// Default low-weight value in percent.
    pub small: f32,
    /// Default high-weight value in percent.
    pub big: f32,
    /// Optional clamped default used by generated sets.
    pub default: Option<f32>,
    /// Hidden from the ordinary slider list.
    pub hidden: bool,
    /// Clamp the value to the authored default.
    pub clamp: bool,
    /// Delete tagged geometry instead of deforming it.
    pub zap: bool,
    /// Offset UVs rather than vertex positions.
    pub uv: bool,
    /// Morph records owned by this slider, in document order.
    pub data: Vec<SliderDataRef>,
    /// Children this reader does not model, in document order.
    pub unknown: Vec<UnknownElement>,
}

/// One buildable slider set.
#[derive(Debug, Clone, PartialEq)]
pub struct SliderSet {
    /// Set name shown in the tool.
    pub name: String,
    /// Folder holding local morph data and source meshes.
    pub data_folder: String,
    /// Source mesh file in `data_folder`.
    pub source_file: String,
    /// Output folder relative to the game data root.
    pub output_path: String,
    /// Output file settings.
    pub output_file: OutputFile,
    /// Shapes the set can deform.
    pub shapes: Vec<ShapeRef>,
    /// Sliders in document order.
    pub sliders: Vec<SliderDef>,
    /// Children this reader does not model, in document order.
    pub unknown: Vec<UnknownElement>,
}

/// A parsed `.osp` project.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// Declared schema version; always [`SUPPORTED_VERSION`] after parsing.
    pub version: u32,
    /// Slider sets in document order.
    pub sets: Vec<SliderSet>,
    /// Children of the root this reader does not model, in document order.
    pub unknown: Vec<UnknownElement>,
}

impl Project {
    /// Parse a `SliderSetInfo` root.
    pub(crate) fn parse(root: Node<'_, '_>, origin: &Path) -> Result<Self> {
        let version = root
            .attribute("version")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::MissingRequired,
                    element_path(root),
                    "attribute `version` is missing or not an integer",
                )
                .with_path(origin)
            })?;
        if version != SUPPORTED_VERSION {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                element_path(root),
                format!("version {version}; this crate reads version {SUPPORTED_VERSION}"),
            )
            .with_path(origin));
        }
        let mut sets = Vec::new();
        let mut names = BTreeSet::new();
        let mut unknown = Vec::new();
        for node in root.children().filter(Node::is_element) {
            if node.tag_name().name() != "SliderSet" {
                unknown.push(UnknownElement::record(node));
                continue;
            }
            let set = SliderSet::parse(node, origin)?;
            if !names.insert(set.name.clone()) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    element_path(node),
                    format!("duplicate slider set name `{}`", set.name),
                )
                .with_path(origin));
            }
            sets.push(set);
        }
        Ok(Self {
            version,
            sets,
            unknown,
        })
    }
}

impl SliderSet {
    fn parse(node: Node<'_, '_>, origin: &Path) -> Result<Self> {
        let name = required_attr(node, "name").map_err(|e| e.with_path(origin))?;
        let mut data_folder = None;
        let mut source_file = None;
        let mut output_path = None;
        let mut output_file = None;
        let mut shapes = Vec::new();
        let mut sliders = Vec::new();
        let mut shape_names = BTreeSet::new();
        let mut slider_names = BTreeSet::new();
        let mut unknown = Vec::new();

        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "DataFolder" => {
                    data_folder = Some(required_text(child).map_err(|e| e.with_path(origin))?)
                }
                "SourceFile" => {
                    source_file = Some(required_text(child).map_err(|e| e.with_path(origin))?)
                }
                "OutputPath" => {
                    output_path = Some(required_text(child).map_err(|e| e.with_path(origin))?)
                }
                "OutputFile" => output_file = Some(OutputFile::parse(child, origin)?),
                "Shape" => {
                    let shape = ShapeRef::parse(child, origin)?;
                    if !shape_names.insert(shape.name.clone()) {
                        return Err(Error::new(
                            ErrorKind::InvalidValue,
                            element_path(child),
                            format!("duplicate shape `{}`", shape.name),
                        )
                        .with_path(origin));
                    }
                    shapes.push(shape);
                }
                // Known upstream children that carry no buildable geometry here.
                // They are skipped deliberately; an unknown element still fails.
                "SFMorphPath" | "SFMorphTargetShape" | "Notes" | "ReferenceInfo"
                | "NormalsGeneration" => {}
                "Slider" => {
                    let slider = SliderDef::parse(child, origin)?;
                    if !slider_names.insert(slider.name.clone()) {
                        return Err(Error::new(
                            ErrorKind::InvalidValue,
                            element_path(child),
                            format!("duplicate slider `{}`", slider.name),
                        )
                        .with_path(origin));
                    }
                    sliders.push(slider);
                }
                _ => unknown.push(UnknownElement::record(child)),
            }
        }

        Ok(Self {
            name,
            data_folder: data_folder.ok_or_else(|| missing(origin, &node, "DataFolder"))?,
            source_file: source_file.ok_or_else(|| missing(origin, &node, "SourceFile"))?,
            output_path: output_path.ok_or_else(|| missing(origin, &node, "OutputPath"))?,
            output_file: output_file.ok_or_else(|| missing(origin, &node, "OutputFile"))?,
            shapes,
            sliders,
            unknown,
        })
    }
}

impl OutputFile {
    fn parse(node: Node<'_, '_>, origin: &Path) -> Result<Self> {
        Ok(Self {
            name: required_text(node).map_err(|e| e.with_path(origin))?,
            gen_weights: bool_attr(node, "GenWeights", true).map_err(|e| e.with_path(origin))?,
            prevent_morph_file: bool_attr(node, "PreventMorphFile", false)
                .map_err(|e| e.with_path(origin))?,
            keep_zapped_shapes: bool_attr(node, "KeepZappedShapes", false)
                .map_err(|e| e.with_path(origin))?,
        })
    }
}

impl ShapeRef {
    fn parse(node: Node<'_, '_>, origin: &Path) -> Result<Self> {
        Ok(Self {
            name: required_text(node).map_err(|e| e.with_path(origin))?,
            target: required_attr(node, "target").map_err(|e| e.with_path(origin))?,
            data_folder: optional_attr(node, "DataFolder"),
            smooth_seam_normals: bool_attr(node, "SmoothSeamNormals", true)
                .map_err(|e| e.with_path(origin))?,
            smooth_seam_normals_angle: f32_attr(
                node,
                "SmoothSeamNormalsAngle",
                DEFAULT_SMOOTH_SEAM_NORMALS_ANGLE,
            )
            .map_err(|e| e.with_path(origin))?,
            lock_normals: bool_attr(node, "LockNormals", false).map_err(|e| e.with_path(origin))?,
        })
    }
}

impl SliderDef {
    fn parse(node: Node<'_, '_>, origin: &Path) -> Result<Self> {
        let name = required_attr(node, "name").map_err(|e| e.with_path(origin))?;
        let mut data = Vec::new();
        let mut unknown = Vec::new();
        let mut seen = BTreeSet::new();
        for child in node.children().filter(Node::is_element) {
            if child.tag_name().name() != "Data" {
                unknown.push(UnknownElement::record(child));
                continue;
            }
            let reference = SliderDataRef::parse(child, &name, origin)?;
            if !seen.insert((reference.name.clone(), reference.target.clone())) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    element_path(child),
                    format!(
                        "record `{}` repeats for target `{}`",
                        reference.name, reference.target
                    ),
                )
                .with_path(origin));
            }
            data.push(reference);
        }
        Ok(Self {
            name,
            invert: bool_attr(node, "invert", false).map_err(|e| e.with_path(origin))?,
            small: f32_attr(node, "small", 0.0).map_err(|e| e.with_path(origin))?,
            big: f32_attr(node, "big", 0.0).map_err(|e| e.with_path(origin))?,
            default: node
                .attribute("default")
                .map(|_| f32_attr(node, "default", 0.0))
                .transpose()
                .map_err(|e| e.with_path(origin))?,
            hidden: bool_attr(node, "hidden", false).map_err(|e| e.with_path(origin))?,
            clamp: bool_attr(node, "clamp", false).map_err(|e| e.with_path(origin))?,
            zap: bool_attr(node, "zap", false).map_err(|e| e.with_path(origin))?,
            uv: bool_attr(node, "uv", false).map_err(|e| e.with_path(origin))?,
            data,
            unknown,
        })
    }
}

impl SliderDataRef {
    fn parse(node: Node<'_, '_>, slider_name: &str, origin: &Path) -> Result<Self> {
        let name = optional_attr(node, "name").unwrap_or_else(|| slider_name.to_string());
        let target = required_attr(node, "target").map_err(|e| e.with_path(origin))?;
        let (file, record) = match node.text().map(str::trim).filter(|text| !text.is_empty()) {
            Some(text) => {
                let (file, record) = Self::split_reference(text);
                (Some(file), record)
            }
            None => (None, None),
        };
        Ok(Self {
            name,
            target,
            local: bool_attr(node, "local", false).map_err(|e| e.with_path(origin))?,
            file,
            record,
        })
    }
}

fn missing(origin: &Path, node: &Node<'_, '_>, element: &str) -> Error {
    Error::new(
        ErrorKind::MissingRequired,
        element_path(*node),
        format!("missing `{element}` element"),
    )
    .with_path(origin)
}
