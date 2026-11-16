//! Read-only archive member listing.
//!
//! The catalogue never extracts payloads into the catalogue: it records
//! member identity so a face or preset inside a supplied archive is
//! discoverable without unpacking it. Host `7z` lists 7z/zip/rar containers;
//! Skyrim `.bsa` archives go to the engine's own BSA parser, because host
//! `7z` misparses the container as gzip.

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

/// List members of one supported archive, dispatched by extension.
///
/// `.bsa` goes to the in-engine BSA parser ([`list_bsa`]) and is never handed
/// to `7z`; every other extension goes to the host `7z` route. An archive the
/// chosen reader refuses is a named error, not an empty list.
pub fn archive_members(path: &Path) -> Result<Vec<ArchiveEntry>, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    match extension.as_str() {
        "bsa" => list_bsa(path),
        _ => list_archive(path),
    }
}

/// List a Skyrim BSA's members through the engine's read-only index.
///
/// Member paths are the engine's normalized keys (`\`→`/`, ASCII lowercase),
/// sorted; sizes come from the index where they are readable. BSA records
/// carry a name hash, not a CRC, so `crc` is always `None`.
pub fn list_bsa(path: &Path) -> Result<Vec<ArchiveEntry>, String> {
    let index =
        crate::bsa::index_bsa(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(index
        .members()
        .map(|key| ArchiveEntry {
            path: key.to_string(),
            size: index.member_size(key),
            crc: None,
            is_directory: false,
        })
        .collect())
}

/// List an archive through the host `7z` binary.
///
/// RAR5 containers are not readable by the installed p7zip; when `7z` refuses
/// a `.rar`, fall back to the host `unrar lb` route. Both readers stay host
/// tools: the catalogue never extracts the payload.
pub fn list_archive(path: &Path) -> Result<Vec<ArchiveEntry>, String> {
    let attempt = Command::new("7z").arg("l").arg("-slt").arg(path).output();
    let seven_error = match attempt {
        Ok(output) if output.status.success() => {
            return Ok(parse_listing(&String::from_utf8_lossy(&output.stdout)));
        }
        Ok(output) => format!(
            "7z listing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("7z is unavailable: {error}"),
    };
    let is_rar = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rar"));
    if !is_rar {
        return Err(format!("{}: {seven_error}", path.display()));
    }
    list_rar(path).map_err(|unrar_error| {
        format!(
            "{}: {seven_error}; unrar fallback failed: {unrar_error}",
            path.display()
        )
    })
}

/// List a RAR through the host `unrar` binary.
///
/// `lb` prints bare member paths; sizes and CRCs are not part of that output.
fn list_rar(path: &Path) -> Result<Vec<ArchiveEntry>, String> {
    let output = Command::new("unrar")
        .arg("lb")
        .arg("-p-")
        .arg(path)
        .output()
        .map_err(|error| format!("unrar is unavailable: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "unrar listing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_unrar_listing(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Parse `unrar lb` output into member rows.
fn parse_unrar_listing(text: &str) -> Vec<ArchiveEntry> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let is_directory = line.ends_with('/') || line.ends_with('\\');
            ArchiveEntry {
                path: line.trim_end_matches(['/', '\\']).replace('\\', "/"),
                size: None,
                crc: None,
                is_directory,
            }
        })
        .filter(|entry| !entry.path.is_empty())
        .collect()
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
    use super::*;
    use crate::bsa::tests::{Build, Spec, build_bsa};

    fn write_bsa(label: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "boethos-catalogue-{label}-{}.bsa",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("fixture archive writes");
        path
    }

    #[test]
    fn bsa_dispatch_reads_the_engine_parser_and_never_7z() {
        // `BSA\0` bytes are no container host `7z` can open (it misparses a
        // BSA as gzip and fails), so an `Ok` with exactly these members can
        // only come from the engine's BSA index reader.
        let specs = [
            Spec {
                folder: "meshes\\actors\\character\\animations",
                name: "2hm_attackpowerforward.hkx",
                data: b"havok-spline-clip",
            },
            Spec {
                folder: "meshes\\actors\\character\\animations",
                name: "mt_walkidle.hkx",
                data: b"havok-behavior",
            },
            Spec {
                folder: "textures\\clutter",
                name: "rope.dds",
                data: b"DDS ",
            },
        ];
        let path = write_bsa("dispatch", &build_bsa(&specs, Build::default()));
        let entries = archive_members(&path).expect("engine parser lists the BSA");
        let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "meshes/actors/character/animations/2hm_attackpowerforward.hkx",
                "meshes/actors/character/animations/mt_walkidle.hkx",
                "textures/clutter/rope.dds",
            ],
            "normalized keys, sorted"
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.crc.is_none() && !entry.is_directory)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_bsa_is_a_named_refusal() {
        let path =
            std::env::temp_dir().join(format!("absent-catalogue-{}.bsa", std::process::id()));
        let error = archive_members(&path).expect_err("a missing archive refuses");
        assert!(error.to_string().contains("No such file"), "{error}");
        assert!(
            error
                .to_string()
                .contains(&path.to_string_lossy().to_string()),
            "the refusal names the archive: {error}"
        );
    }

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

    #[test]
    fn parses_the_unrar_lb_member_list() {
        let listing = "Meshes\\actors\\character\\animations\\male\\mt_walkForward.HkX\r\n\
Meshes/actors/character/animations/female/mt_walkForward.hkx\n\
textures/\n";
        let entries = parse_unrar_listing(listing);
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0].path,
            "Meshes/actors/character/animations/male/mt_walkForward.HkX"
        );
        assert_eq!(
            entries[1].path,
            "Meshes/actors/character/animations/female/mt_walkForward.hkx"
        );
        assert!(entries[2].is_directory);
        assert_eq!(entries[2].path, "textures");
        assert!(entries.iter().all(|entry| entry.size.is_none()));
    }
}
