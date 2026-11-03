//! Document detection by XML root element.
//!
//! BodySlide-family directories hold several XML dialects with different
//! roots; the root element is the authoritative discriminator, never the file
//! extension, because presets and sets are both ordinary `.xml` in the wild.

use std::path::Path;

use roxmltree::Document as Xml;

use crate::error::{Error, ErrorKind, Result};
use crate::preset::Preset;
use crate::project::Project;

/// A parsed Sculpto input document.
#[derive(Debug, Clone, PartialEq)]
pub enum Document {
    /// A `SliderPresets` collection of named presets.
    Presets(Vec<Preset>),
    /// A `.osp` `SliderSetInfo` project.
    Project(Project),
}

impl Document {
    /// Parse a document from text. `origin` is used only in error messages.
    pub fn parse_str(text: &str, origin: impl AsRef<Path>) -> Result<Self> {
        let origin = origin.as_ref();
        let xml = Xml::parse(text).map_err(|e| {
            Error::new(ErrorKind::Malformed, "document", e.to_string()).with_path(origin)
        })?;
        let root = xml.root_element();
        match root.tag_name().name() {
            // BodySlide writes `<SliderPresets>`; pipeline-generated preset
            // files (the operator's 3BA defaults) use `<Vector>` with the same
            // `Preset` children. Accept the alias but keep the same strict
            // child validation.
            "SliderPresets" | "Vector" => Preset::parse_all(root, origin).map(Document::Presets),
            "SliderSetInfo" => Project::parse(root, origin).map(Document::Project),
            other => Err(Error::new(
                ErrorKind::UnsupportedRoot,
                other,
                "expected SliderPresets or SliderSetInfo",
            )
            .with_path(origin)),
        }
    }

    /// Parse a document from disk.
    pub fn parse_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::new(ErrorKind::Io, "document", e.to_string()).with_path(path))?;
        Self::parse_str(&text, path)
    }
}
