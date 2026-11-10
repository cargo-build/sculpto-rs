//! Buildable slider projects (`.osp` `SliderSetInfo` documents).
//!
//! A project owns one or more slider sets. Each set names the source mesh, the
//! output file and the sliders that deform it; slider records point at
//! external `.osd`/`.bsd` morph data by file and record name. Sculpto keeps
//! that reference unresolved: resolving paths is a build-time concern.
//!
//! [`Project::to_osp_xml`] writes the same model back. It covers every field
//! this module models, in document order, with the attribute style of
//! `SliderSet::WriteSliderSet` (`SliderSet.cpp:522-660`): a boolean equal to the
//! reader default is omitted, because the reader restores it. Content the reader
//! could not model is refused by name instead of being dropped — a project that
//! accepts a file and then narrows it is not a lossless writer.

use std::collections::BTreeSet;
use std::path::Path;

use roxmltree::Node;

use crate::error::{Error, ErrorKind, Result};
use crate::report::UnknownElement;
use crate::xml_util::{
    bool_attr, element_path, f32_attr, optional_attr, optional_text, required_attr, required_text,
};

/// The only `SliderSetInfo` version this crate reads and writes.
pub const SUPPORTED_VERSION: u32 = 1;

/// Default seam-normal smoothing angle used by BodySlide when unspecified.
pub const DEFAULT_SMOOTH_SEAM_NORMALS_ANGLE: f32 = 60.0;

/// Indent of one emitted XML level; the tinyxml2 default printer uses four spaces.
const INDENT: &str = "    ";

/// Reference project a slider set was derived from (`ReferenceInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceInfo {
    /// Referenced `.osp` file, e.g. `CBBE.osp`.
    pub project_file: Option<String>,
    /// Referenced slider set name inside that file.
    pub project_name: Option<String>,
    /// Referenced shape name inside that set.
    pub shape_name: Option<String>,
}

/// Output file settings of a slider set.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputFile {
    /// Output file stem without extension, e.g. `femalebody`.
    pub name: String,
    /// Generate low/high body-weight variants instead of a single output mesh.
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
    /// Sliders a zap toggle switches, in authored order. Only a zap slider
    /// carries them; the reference reads the attribute under the same condition
    /// (`SliderData.cpp:70-74`).
    pub zap_toggles: Vec<String>,
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
    /// Reference project metadata this set was derived from.
    pub reference: Option<ReferenceInfo>,
    /// Starfield morph output folder; `None` when the set writes no morph.
    pub sf_morph_path: Option<String>,
    /// Starfield morph target shape; `None` when no morph target is written.
    pub sf_morph_target_shape: Option<String>,
    /// Free-form authoring note.
    pub notes: Option<String>,
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

    /// Serialize as a complete `SliderSetInfo` document.
    ///
    /// `Err` covers every input the writer cannot round-trip: a version this
    /// crate does not write, unmodeled children reported by the reader, duplicate
    /// declarations, empty required values, padded text the reader would trim and
    /// illegal XML characters. The error names the element path in the reader's
    /// own `SliderSetInfo/SliderSet[name=…]/…` form.
    pub fn to_osp_xml(&self) -> Result<String> {
        if self.version != SUPPORTED_VERSION {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "SliderSetInfo",
                format!(
                    "version {}; this crate writes version {SUPPORTED_VERSION}",
                    self.version
                ),
            ));
        }
        refuse_unknown("SliderSetInfo", &self.unknown)?;

        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        open(
            &mut out,
            0,
            &format!("<SliderSetInfo version=\"{}\">", self.version),
        );
        let mut names = BTreeSet::new();
        for set in &self.sets {
            if !names.insert(set.name.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    "SliderSetInfo",
                    format!("duplicate slider set name `{}`", set.name),
                ));
            }
            write_set(&mut out, set)?;
        }
        open(&mut out, 0, "</SliderSetInfo>");
        Ok(out)
    }
}

impl SliderSet {
    fn parse(node: Node<'_, '_>, origin: &Path) -> Result<Self> {
        let name = required_attr(node, "name").map_err(|e| e.with_path(origin))?;
        let mut data_folder = None;
        let mut source_file = None;
        let mut output_path = None;
        let mut output_file = None;
        let mut reference = None;
        let mut sf_morph_path = None;
        let mut sf_morph_target_shape = None;
        let mut notes = None;
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
                "ReferenceInfo" => reference = Some(ReferenceInfo::parse(child)),
                "SFMorphPath" => sf_morph_path = optional_text(child),
                "SFMorphTargetShape" => sf_morph_target_shape = optional_text(child),
                "Notes" => notes = optional_text(child),
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
                // `NormalsGeneration` is a nested normal-generation layer tree
                // this crate does not model; recording it keeps the writer from
                // accepting a file and then dropping it.
                _ => unknown.push(UnknownElement::record(child)),
            }
        }

        Ok(Self {
            name,
            data_folder: data_folder.ok_or_else(|| missing(origin, &node, "DataFolder"))?,
            source_file: source_file.ok_or_else(|| missing(origin, &node, "SourceFile"))?,
            output_path: output_path.ok_or_else(|| missing(origin, &node, "OutputPath"))?,
            output_file: output_file.ok_or_else(|| missing(origin, &node, "OutputFile"))?,
            reference,
            sf_morph_path,
            sf_morph_target_shape,
            notes,
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

impl ReferenceInfo {
    fn parse(node: Node<'_, '_>) -> Self {
        Self {
            project_file: optional_attr(node, "ProjectFile"),
            project_name: optional_attr(node, "ProjectName"),
            shape_name: optional_attr(node, "ShapeName"),
        }
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
        let zap = bool_attr(node, "zap", false).map_err(|e| e.with_path(origin))?;
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
            zap,
            zap_toggles: if zap {
                node.attribute("zaptoggles")
                    .map(|text| {
                        text.split(';')
                            .filter(|toggle| !toggle.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
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

fn write_set(out: &mut String, set: &SliderSet) -> Result<()> {
    let element = format!("SliderSetInfo/SliderSet[name={}]", set.name);
    let set_name = attr_value(&element, "name", &set.name)?;
    refuse_unknown(&element, &set.unknown)?;

    open(out, 1, &format!("<SliderSet name=\"{set_name}\">"));
    if let Some(reference) = &set.reference {
        write_reference_info(out, &element, reference)?;
    }
    text_element(out, 2, &element, "DataFolder", &set.data_folder)?;
    text_element(out, 2, &element, "SourceFile", &set.source_file)?;
    text_element(out, 2, &element, "OutputPath", &set.output_path)?;
    write_output_file(out, &element, &set.output_file)?;
    if let Some(path) = &set.sf_morph_path {
        text_element(out, 2, &element, "SFMorphPath", path)?;
    }
    if let Some(shape) = &set.sf_morph_target_shape {
        text_element(out, 2, &element, "SFMorphTargetShape", shape)?;
    }

    let mut shape_names = BTreeSet::new();
    for shape in &set.shapes {
        if !shape_names.insert(shape.name.as_str()) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                &element,
                format!("duplicate shape `{}`", shape.name),
            ));
        }
        write_shape(out, &element, shape)?;
    }

    let mut slider_names = BTreeSet::new();
    for slider in &set.sliders {
        if !slider_names.insert(slider.name.as_str()) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                &element,
                format!("duplicate slider `{}`", slider.name),
            ));
        }
        write_slider(out, &element, slider)?;
    }

    if let Some(notes) = &set.notes {
        text_element(out, 2, &element, "Notes", notes)?;
    }
    open(out, 1, "</SliderSet>");
    Ok(())
}

fn write_reference_info(
    out: &mut String,
    set_element: &str,
    reference: &ReferenceInfo,
) -> Result<()> {
    let element = format!("{set_element}/ReferenceInfo");
    let mut attrs = String::new();
    for (name, value) in [
        ("ProjectFile", &reference.project_file),
        ("ProjectName", &reference.project_name),
        ("ShapeName", &reference.shape_name),
    ] {
        if let Some(value) = value {
            let value = attr_value(&element, name, value)?;
            attrs.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    open(out, 2, &format!("<ReferenceInfo{attrs}/>"));
    Ok(())
}

fn write_output_file(out: &mut String, set_element: &str, output: &OutputFile) -> Result<()> {
    let element = format!("{set_element}/OutputFile");
    let name = text_value(&element, "OutputFile", &output.name)?;
    let mut attrs = String::new();
    if !output.gen_weights {
        attrs.push_str(" GenWeights=\"false\"");
    }
    if output.prevent_morph_file {
        attrs.push_str(" PreventMorphFile=\"true\"");
    }
    if output.keep_zapped_shapes {
        attrs.push_str(" KeepZappedShapes=\"true\"");
    }
    open(out, 2, &format!("<OutputFile{attrs}>{name}</OutputFile>"));
    Ok(())
}

fn write_shape(out: &mut String, set_element: &str, shape: &ShapeRef) -> Result<()> {
    let element = format!("{set_element}/Shape[name={}]", shape.name);
    let name = text_value(&element, "Shape", &shape.name)?;
    let target = attr_value(&element, "target", &shape.target)?;
    let mut attrs = format!(" target=\"{target}\"");
    if let Some(folder) = &shape.data_folder {
        let folder = attr_value(&element, "DataFolder", folder)?;
        attrs.push_str(&format!(" DataFolder=\"{folder}\""));
    }
    if !shape.smooth_seam_normals {
        attrs.push_str(" SmoothSeamNormals=\"false\"");
    }
    if shape.smooth_seam_normals_angle != DEFAULT_SMOOTH_SEAM_NORMALS_ANGLE {
        let angle = number(
            &element,
            "SmoothSeamNormalsAngle",
            shape.smooth_seam_normals_angle,
        )?;
        attrs.push_str(&format!(" SmoothSeamNormalsAngle=\"{angle}\""));
    }
    if shape.lock_normals {
        attrs.push_str(" LockNormals=\"true\"");
    }
    open(out, 2, &format!("<Shape{attrs}>{name}</Shape>"));
    Ok(())
}

fn write_slider(out: &mut String, set_element: &str, slider: &SliderDef) -> Result<()> {
    let element = format!("{set_element}/Slider[name={}]", slider.name);
    let name = attr_value(&element, "name", &slider.name)?;
    refuse_unknown(&element, &slider.unknown)?;

    let mut attrs = format!(" name=\"{name}\"");
    if slider.invert {
        attrs.push_str(" invert=\"true\"");
    }
    // Both endpoint defaults are always written: the reference writes either
    // `small`/`big` or `default` depending on `GenWeights`, and ignores what it
    // does not read, so writing all modeled values is the lossless choice.
    let small = number(&element, "small", slider.small)?;
    let big = number(&element, "big", slider.big)?;
    attrs.push_str(&format!(" small=\"{small}\" big=\"{big}\""));
    if let Some(default) = slider.default {
        let default = number(&element, "default", default)?;
        attrs.push_str(&format!(" default=\"{default}\""));
    }
    if slider.hidden {
        attrs.push_str(" hidden=\"true\"");
    }
    if slider.clamp {
        attrs.push_str(" clamp=\"true\"");
    }
    if slider.zap {
        attrs.push_str(" zap=\"true\"");
    }
    if !slider.zap_toggles.is_empty() {
        if !slider.zap {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                &element,
                "zap toggles on a non-zap slider would not be read back",
            ));
        }
        let mut toggles = String::new();
        for toggle in &slider.zap_toggles {
            if toggle.is_empty() || toggle.contains(';') {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &element,
                    format!("zap toggle `{toggle}` cannot be expressed in the `;`-separated list"),
                ));
            }
            toggles.push_str(&attr_value(&element, "zaptoggles", toggle)?);
            toggles.push(';');
        }
        attrs.push_str(&format!(" zaptoggles=\"{toggles}\""));
    }
    if slider.uv {
        attrs.push_str(" uv=\"true\"");
    }

    if slider.data.is_empty() {
        open(out, 2, &format!("<Slider{attrs}/>"));
        return Ok(());
    }
    open(out, 2, &format!("<Slider{attrs}>"));
    let mut records = BTreeSet::new();
    for reference in &slider.data {
        if !records.insert((reference.name.as_str(), reference.target.as_str())) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                &element,
                format!(
                    "record `{}` repeats for target `{}`",
                    reference.name, reference.target
                ),
            ));
        }
        write_data_ref(out, &element, reference)?;
    }
    open(out, 2, "</Slider>");
    Ok(())
}

fn write_data_ref(out: &mut String, slider_element: &str, reference: &SliderDataRef) -> Result<()> {
    let element = format!("{slider_element}/Data[name={}]", reference.name);
    let name = attr_value(&element, "name", &reference.name)?;
    let target = attr_value(&element, "target", &reference.target)?;
    let local = if reference.local {
        " local=\"true\""
    } else {
        ""
    };
    let text = match (&reference.file, &reference.record) {
        (Some(file), Some(record)) => {
            let file = raw_text(&element, "file", file)?;
            let record = raw_text(&element, "record", record)?;
            if record.contains(['\\', '/']) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &element,
                    format!(
                        "record `{record}` contains a path separator the reference would split on"
                    ),
                ));
            }
            escape_text(&element, "reference", &format!("{file}\\{record}"))?
        }
        (Some(file), None) => {
            if file.contains(['\\', '/']) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &element,
                    "a file-only reference contains a path separator the reader would split as file\\record",
                ));
            }
            text_value(&element, "file", file)?
        }
        (None, Some(_)) => {
            return Err(Error::new(
                ErrorKind::MissingRequired,
                &element,
                "a record name without a container file has no `file\\record` text",
            ));
        }
        (None, None) => String::new(),
    };
    if text.is_empty() {
        open(
            out,
            3,
            &format!("<Data name=\"{name}\" target=\"{target}\"{local}/>"),
        );
    } else {
        open(
            out,
            3,
            &format!("<Data name=\"{name}\" target=\"{target}\"{local}>{text}</Data>"),
        );
    }
    Ok(())
}

/// Append one indented line.
fn open(out: &mut String, depth: usize, line: &str) {
    out.push_str(&INDENT.repeat(depth));
    out.push_str(line);
    out.push('\n');
}

/// Append one text-carrying child element.
fn text_element(
    out: &mut String,
    depth: usize,
    parent: &str,
    name: &str,
    value: &str,
) -> Result<()> {
    let element = format!("{parent}/{name}");
    let value = text_value(&element, name, value)?;
    open(out, depth, &format!("<{name}>{value}</{name}>"));
    Ok(())
}

/// Reject unmodeled children instead of silently narrowing the document.
fn refuse_unknown(element: &str, unknown: &[UnknownElement]) -> Result<()> {
    match unknown.first() {
        Some(item) => Err(Error::new(
            ErrorKind::InvalidValue,
            element,
            format!("element `{}` has no writer; refusing to drop it", item.path),
        )),
        None => Ok(()),
    }
}

/// Validate a required text without escaping it, for callers that compose it.
fn raw_text<'a>(element: &str, field: &str, value: &'a str) -> Result<&'a str> {
    if value.trim().is_empty() {
        return Err(Error::new(
            ErrorKind::MissingRequired,
            element,
            format!("{field} is empty"),
        ));
    }
    if value.trim() != value {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            element,
            format!("{field} `{value}` has whitespace the reader would trim"),
        ));
    }
    Ok(value)
}

/// Escape and validate a required element text.
fn text_value(element: &str, field: &str, value: &str) -> Result<String> {
    escape_text(element, field, raw_text(element, field, value)?)
}

/// Escape and validate a required attribute value.
fn attr_value(element: &str, field: &str, value: &str) -> Result<String> {
    escape_attr(element, field, raw_text(element, field, value)?)
}

/// Format a model float so the reader parses back the identical value.
fn number(element: &str, field: &str, value: f32) -> Result<String> {
    if value.is_finite() {
        Ok(value.to_string())
    } else {
        Err(Error::new(
            ErrorKind::InvalidValue,
            element,
            format!("{field} is not finite"),
        ))
    }
}

fn escape_text(element: &str, field: &str, value: &str) -> Result<String> {
    escape(element, field, value, false)
}

fn escape_attr(element: &str, field: &str, value: &str) -> Result<String> {
    escape(element, field, value, true)
}

/// Escape XML character data or an attribute value, refusing what XML 1.0
/// cannot represent at all.
fn escape(element: &str, field: &str, value: &str, attribute: bool) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        if !is_xml_char(character) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                element,
                format!(
                    "{field} contains the illegal XML character U+{:04X}",
                    u32::from(character)
                ),
            ));
        }
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' if attribute => out.push_str("&#9;"),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// XML 1.0 `Char`, minus the C0 controls that are not whitespace.
fn is_xml_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r')
        || (character >= ' ' && !matches!(character, '\u{fffe}' | '\u{ffff}'))
}
