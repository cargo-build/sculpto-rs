//! Read-only archive member listing through the host `7z` tool.
//!
//! The catalogue never extracts payloads: it records member identity so a face
//! or preset inside a supplied archive is discoverable without unpacking it.

use std::path::Path;
use std::process::Command;

/// One member reported by `7z l -slt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Member path using `/` separators.
    pub path: String,
    /// Uncompressed size, when reported.
    pub size: Option<u64>,
    /// CRC-32 reported by the archive, when present.
    pub crc: Option<String>,
    /// Directory marker.
    pub is_directory: bool,
}

/// List an archive through the host `7z` binary.
pub fn list_archive(path: &Path) -> Result<Vec<ArchiveEntry>, String> {
    let output = Command::new("7z")
        .arg("l")
        .arg("-slt")
        .arg(path)
        .output()
        .map_err(|error| format!("{}: 7z is unavailable: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{}: 7z listing failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_listing(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse `7z l -slt` output into member rows.
pub fn parse_listing(text: &str) -> Vec<ArchiveEntry> {
    let mut entries = Vec::new();
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut flush = |fields: &mut Vec<(String, String)>| {
        if fields.is_empty() {
            return;
        }
        let value = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let path = value("Path");
        let kind = value("Type");
        let is_directory = value("Folder").is_some_and(|folder| folder == "+")
            || value("Attributes").is_some_and(|attributes| attributes.starts_with('D'));
        let size = value("Size").and_then(|size| size.parse::<u64>().ok());
        let crc = value("CRC").filter(|crc| !crc.is_empty());
        if let Some(path) = path {
            let is_archive_header = kind.is_some() && size.is_none();
            if !is_archive_header {
                entries.push(ArchiveEntry {
                    path,
                    size,
                    crc,
                    is_directory,
                });
            }
        }
        fields.clear();
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut fields);
            continue;
        }
        if let Some((key, value)) = line.split_once(" = ") {
            fields.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    flush(&mut fields);
    entries
}

#[cfg(test)]
mod tests {
    use super::parse_listing;

    #[test]
    fn parses_members_and_skips_the_archive_header() {
        let listing = "\
Listing archive: sample.7z

--
Path = sample.7z
Type = 7z
Physical Size = 12

----------
Path = FaceGeom/skyrim.esm/0001.NIF
Size = 2048
CRC = A1B2C3D4
Attributes = A

Path = meshes/
Folder = +
Size = 0

";
        let entries = parse_listing(listing);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "FaceGeom/skyrim.esm/0001.NIF");
        assert_eq!(entries[0].size, Some(2048));
        assert_eq!(entries[0].crc.as_deref(), Some("A1B2C3D4"));
        assert!(!entries[0].is_directory);
        assert!(entries[1].is_directory);
    }
}
