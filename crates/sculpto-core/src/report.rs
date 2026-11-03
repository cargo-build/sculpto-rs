//! Non-fatal observations from a successful parse.
//!
//! Upstream adds children to its dialects without bumping `SliderSetInfo`
//! version 1, so a reader that rejects an unrecognized child stops reading
//! every pack that uses a newer authoring feature. Unknown children are
//! therefore accepted and reported here; document identity (root element,
//! version) and invariants (missing required values, duplicate names) stay
//! fatal. Keeping the element bytes for re-emission belongs to the writer,
//! which is not accepted yet (DECL0003).

use roxmltree::Node;

use crate::xml_util::element_path;

/// An element inside a known container that this crate does not model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnknownElement {
    /// Path with authored names, e.g. `SliderSetInfo/SliderSet[name=Body]/New`.
    pub path: String,
}

impl UnknownElement {
    pub(crate) fn record(node: Node<'_, '_>) -> Self {
        Self {
            path: element_path(node),
        }
    }
}
