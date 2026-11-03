//! Named slider presets (`SliderPresets` documents).
//!
//! A preset stores one percentage per slider and body-weight endpoint. It has
//! no geometry of its own: applying it needs the owning slider set, which is
//! why the target set name is preserved even when it is not resolvable here.

use std::collections::BTreeSet;
use std::path::Path;

use roxmltree::Node;

use crate::error::{Error, ErrorKind, Result};
use crate::report::UnknownElement;
use crate::xml_util::{element_path, f32_attr, optional_attr, required_attr};

/// Body-weight endpoint a preset value applies to.
///
/// `small` is the low-weight endpoint and `big` the high-weight endpoint; they
/// are not LOD levels and both are baked by BodySlide at build time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SliderSize {
    /// Low-weight body endpoint (`_0` output).
    Small,
    /// High-weight body endpoint (`_1` output).
    Big,
}

impl SliderSize {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "small" => Some(Self::Small),
            "big" => Some(Self::Big),
            _ => None,
        }
    }
}

/// One slider value inside a preset, stored as the authored percentage.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetValue {
    /// Slider name as written by the preset author.
    pub slider: String,
    /// Endpoint this value drives.
    pub size: SliderSize,
    /// Authored percentage in `0.0..=100.0`.
    pub percent: f32,
}

impl PresetValue {
    /// Normalized slider position used by morph application.
    pub fn fraction(&self) -> f32 {
        self.percent / 100.0
    }
}

/// A named preset with optional group and owning slider set.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// Preset name shown in the tool.
    pub name: String,
    /// Slider set the preset targets, when the author recorded one.
    pub slider_set: Option<String>,
    /// Slider category groups the preset belongs to.
    pub groups: Vec<String>,
    /// Values in document order.
    pub values: Vec<PresetValue>,
    /// Children this reader does not model, in document order.
    pub unknown: Vec<UnknownElement>,
}

impl Preset {
    /// Parse every preset in a `SliderPresets` root.
    pub(crate) fn parse_all(root: Node<'_, '_>, origin: &Path) -> Result<Vec<Preset>> {
        let mut presets = Vec::new();
        let mut names = BTreeSet::new();
        for node in root.children().filter(Node::is_element) {
            if node.tag_name().name() != "Preset" {
                return Err(Error::new(
                    ErrorKind::UnsupportedRoot,
                    element_path(node),
                    "expected only `Preset` children",
                )
                .with_path(origin));
            }
            let preset = Self::parse(node, origin)?;
            if !names.insert(preset.name.clone()) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    element_path(node),
                    format!("duplicate preset name `{}`", preset.name),
                )
                .with_path(origin));
            }
            presets.push(preset);
        }
        Ok(presets)
    }

    fn parse(node: Node<'_, '_>, origin: &Path) -> Result<Self> {
        let name = required_attr(node, "name").map_err(|e| e.with_path(origin))?;
        let slider_set = optional_attr(node, "set");
        let mut groups = Vec::new();
        let mut values = Vec::new();
        let mut unknown = Vec::new();
        let mut seen = BTreeSet::new();
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "Group" => {
                    groups.push(required_attr(child, "name").map_err(|e| e.with_path(origin))?)
                }
                "SetSlider" => {
                    let value = Self::parse_value(child, origin)?;
                    let key = (value.slider.clone(), value.size);
                    if !seen.insert(key) {
                        return Err(Error::new(
                            ErrorKind::InvalidValue,
                            element_path(child),
                            format!(
                                "slider `{}` repeats the same endpoint {:?}",
                                value.slider, value.size
                            ),
                        )
                        .with_path(origin));
                    }
                    values.push(value);
                }
                _ => unknown.push(UnknownElement::record(child)),
            }
        }
        Ok(Self {
            name,
            slider_set,
            groups,
            values,
            unknown,
        })
    }

    fn parse_value(node: Node<'_, '_>, origin: &Path) -> Result<PresetValue> {
        let slider = required_attr(node, "name").map_err(|e| e.with_path(origin))?;
        let size_token = required_attr(node, "size").map_err(|e| e.with_path(origin))?;
        let size = SliderSize::parse(&size_token).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidValue,
                element_path(node),
                format!("unknown size `{size_token}`; expected `small` or `big`"),
            )
            .with_path(origin)
        })?;
        let percent = f32_attr(node, "value", 0.0).map_err(|e| e.with_path(origin))?;
        if !(0.0..=100.0).contains(&percent) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                element_path(node),
                format!("value `{percent}` is outside 0..=100"),
            )
            .with_path(origin));
        }
        Ok(PresetValue {
            slider,
            size,
            percent,
        })
    }
}
