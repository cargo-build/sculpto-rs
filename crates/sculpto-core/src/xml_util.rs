//! Small attribute helpers shared by the XML readers.

use roxmltree::Node;

use crate::error::{Error, ErrorKind};

pub(crate) fn element_path(node: Node<'_, '_>) -> String {
    let mut parts = Vec::new();
    let mut current = Some(node);
    while let Some(item) = current {
        let mut part = item.tag_name().name().to_string();
        if let Some(name) = item.attribute("name") {
            part.push_str("[name=");
            part.push_str(name);
            part.push(']');
        }
        parts.push(part);
        current = item.parent().filter(|parent| parent.is_element());
    }
    parts.reverse();
    parts.join("/")
}

pub(crate) fn required_attr(node: Node<'_, '_>, name: &str) -> crate::error::Result<String> {
    match node.attribute(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Some(_) => Err(Error::new(
            ErrorKind::MissingRequired,
            element_path(node),
            format!("attribute `{name}` is empty"),
        )),
        None => Err(Error::new(
            ErrorKind::MissingRequired,
            element_path(node),
            format!("attribute `{name}` is missing"),
        )),
    }
}

pub(crate) fn optional_attr(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.attribute(name)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

pub(crate) fn bool_attr(
    node: Node<'_, '_>,
    name: &str,
    default: bool,
) -> crate::error::Result<bool> {
    match node.attribute(name) {
        None => Ok(default),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            other => Err(Error::new(
                ErrorKind::InvalidValue,
                element_path(node),
                format!("attribute `{name}` is not a boolean: `{other}`"),
            )),
        },
    }
}

pub(crate) fn f32_attr(node: Node<'_, '_>, name: &str, default: f32) -> crate::error::Result<f32> {
    match node.attribute(name) {
        None => Ok(default),
        Some(value) => {
            let parsed: f32 = value.trim().parse().map_err(|_| {
                Error::new(
                    ErrorKind::InvalidValue,
                    element_path(node),
                    format!("attribute `{name}` is not a number: `{value}`"),
                )
            })?;
            if parsed.is_finite() {
                Ok(parsed)
            } else {
                Err(Error::new(
                    ErrorKind::InvalidValue,
                    element_path(node),
                    format!("attribute `{name}` is not finite: `{value}`"),
                ))
            }
        }
    }
}

pub(crate) fn required_text(node: Node<'_, '_>) -> crate::error::Result<String> {
    node.text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            Error::new(
                ErrorKind::MissingRequired,
                element_path(node),
                "element text is empty",
            )
        })
}

/// Optional element text: `None` when the element is absent or empty.
///
/// The reference clears an owning field when the element is missing *or* has no
/// text, so absence and emptiness are the same modeled state.
pub(crate) fn optional_text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}
