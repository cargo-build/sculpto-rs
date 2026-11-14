//! Read-only directory and archive indexing.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use boethos::{Put, Store};
use sha2::{Digest, Sha256};

use crate::archive::list_archive;
use crate::model::{Asset, Assets, Category, Sex, Status};

const ARCHIVE_EXTENSIONS: [&str; 3] = ["7z", "zip", "rar"];
const IGNORED_DIRS: [&str; 5] = [".git", ".jj", "__pycache__", "node_modules", "target"];
const UNSUPPORTED_ARCHIVES: [&str; 5] = ["gz", "tar", "tgz", "xz", "bz2"];

/// Indexing switches.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Read archive members for faces, presets and projects.
    pub archive_entries: bool,
    /// Delete rows under the root that this run did not see.
    pub prune: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            archive_entries: true,
            prune: false,
        }
    }
}

/// What one indexing run changed.
#[derive(Debug, Default, Clone)]
pub struct IndexReport {
    /// Regular files visited.
    pub files: usize,
    /// Archives visited.
    pub archives: usize,
    /// Archive members selected for the catalogue.
    pub members: usize,
    /// New identities.
    pub inserted: usize,
    /// Existing identities whose record changed.
    pub updated: usize,
    /// Existing identities whose record was already current.
    pub unchanged: usize,
    /// Rows removed by `prune`.
    pub pruned: usize,
    /// Directory entries skipped because they are symbolic links.
    pub symlinks: usize,
    /// Ignored directories such as `.git` and `target`.
    pub ignored_dirs: usize,
    /// Non-regular, non-directory entries such as sockets and devices.
    pub special: usize,
    /// Paths skipped because they are not valid UTF-8.
    pub non_utf8: usize,
    /// Compressed archives the indexer cannot list; recorded as plain files.
    pub unsupported_archives: usize,
    /// Per-path failures retained instead of aborting the run.
    pub errors: Vec<String>,
}

/// Index one root recursively into the store.
///
/// Identity is derived from the source label and the asset path, so two files
/// with equal bytes stay distinct records and a content change updates the same
/// record. The file digest remains in `sha256`.
pub fn index_root(store: &Store, root: &Path, source: &str, options: &IndexOptions) -> IndexReport {
    let mut report = IndexReport::default();
    if !root.is_dir() {
        report
            .errors
            .push(format!("{}: not a directory", root.display()));
        return report;
    }
    // Canonical paths keep identity stable when a root is named relatively,
    // absolutely or through a different working directory.
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let root = root.as_path();
    let readme_hashes = read_readme_hashes(root);
    let mut files = Vec::new();
    walk(root, &mut files, &mut report);
    let mut seen = HashSet::new();
    for path in files {
        if path.to_str().is_none() {
            report.non_utf8 += 1;
            report
                .errors
                .push(format!("{}: non-UTF-8 path skipped", path.display()));
            continue;
        }
        let extension = extension(&path);
        if ARCHIVE_EXTENSIONS.contains(&extension.as_str()) {
            report.archives += 1;
            index_archive(
                store,
                &path,
                source,
                &readme_hashes,
                options,
                &mut seen,
                &mut report,
            );
            continue;
        }
        report.files += 1;
        if UNSUPPORTED_ARCHIVES.contains(&extension.as_str()) {
            report.unsupported_archives += 1;
        }
        let category = classify(&path, &extension);
        let sha = match sha256_file(&path) {
            Ok(sha) => sha,
            Err(error) => {
                report.errors.push(error);
                continue;
            }
        };
        let path_text = path.to_string_lossy().into_owned();
        let bytes = fs::metadata(&path).ok().map(|meta| meta.len());
        let asset = Asset {
            id: identity(&["file", source, &path_text]),
            category,
            name: file_name(&path),
            path: path_text.clone(),
            source: source.to_string(),
            archive: None,
            entry: None,
            format: extension,
            sha256: Some(sha),
            bytes,
            sex: sex_from_path(&path_text),
            family: family_from_path(&path_text),
            tags: Vec::new(),
            status: status_from_path(&path_text),
        };
        write(store, &asset, &mut seen, &mut report);
    }
    if options.prune {
        prune(store, root, source, &seen, &mut report);
    }
    report
}

fn walk(directory: &Path, files: &mut Vec<PathBuf>, report: &mut IndexReport) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            report
                .errors
                .push(format!("{}: {error}", directory.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report
                    .errors
                    .push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                report.errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if file_type.is_symlink() {
            report.symlinks += 1;
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            if IGNORED_DIRS.iter().any(|ignored| name == *ignored) {
                report.ignored_dirs += 1;
                continue;
            }
            walk(&path, files, report);
        } else if file_type.is_file() {
            files.push(path);
        } else {
            report.special += 1;
        }
    }
}

fn index_archive(
    store: &Store,
    path: &Path,
    source: &str,
    readme_hashes: &HashMap<String, String>,
    options: &IndexOptions,
    seen: &mut HashSet<String>,
    report: &mut IndexReport,
) {
    let name = file_name(path);
    let path_text = path.to_string_lossy().into_owned();
    let computed = match sha256_file(path) {
        Ok(sha) => Some(sha),
        Err(error) => {
            report.errors.push(error);
            None
        }
    };
    if let (Some(computed), Some(declared)) = (&computed, readme_hashes.get(&name))
        && computed != declared
    {
        report.errors.push(format!(
            "{path_text}: README hash {declared} does not match computed {computed}; using computed"
        ));
    }
    let sha = computed.or_else(|| readme_hashes.get(&name).cloned());
    let archive_id = identity(&["archive", source, &path_text]);
    let archive_asset = Asset {
        id: archive_id.clone(),
        category: Category::Archive,
        name: name.clone(),
        path: path_text.clone(),
        source: source.to_string(),
        archive: None,
        entry: None,
        format: extension(path),
        sha256: sha,
        bytes: fs::metadata(path).ok().map(|meta| meta.len()),
        sex: None,
        family: family_from_path(&name),
        tags: Vec::new(),
        status: Status::Raw,
    };
    write(store, &archive_asset, seen, report);
    if !options.archive_entries {
        return;
    }
    let entries = match list_archive(path) {
        Ok(entries) => entries,
        Err(error) => {
            report.errors.push(error);
            return;
        }
    };
    for entry in entries {
        if entry.is_directory {
            continue;
        }
        let Some(category) = member_category(&entry.path) else {
            continue;
        };
        let member_identity = identity(&["member", &archive_id, &entry.path]);
        let mut tags = Vec::new();
        if let Some(crc) = &entry.crc {
            tags.push(format!("crc:{crc}"));
        }
        if entry.path.to_lowercase().contains("facegeom") {
            tags.push("facegeom".to_string());
        }
        let member_name = {
            let mut parts = entry.path.rsplit('/').take(2).collect::<Vec<_>>();
            parts.reverse();
            parts.join("/")
        };
        let sex = sex_from_context(&format!("{name} {member_name} {}", entry.path));
        let family = family_from_path(&name);
        let asset = Asset {
            id: member_identity,
            category,
            name: member_name,
            path: format!("{path_text}::{}", entry.path),
            source: source.to_string(),
            archive: Some(path_text.clone()),
            entry: Some(entry.path.clone()),
            format: extension(Path::new(&entry.path)),
            sha256: None,
            bytes: entry.size,
            sex,
            family,
            tags,
            status: Status::Raw,
        };
        report.members += 1;
        write(store, &asset, seen, report);
    }
}

fn write(store: &Store, asset: &Asset, seen: &mut HashSet<String>, report: &mut IndexReport) {
    seen.insert(asset.id.clone());
    match store.get::<Assets>(&asset.id) {
        Ok(Some(existing)) if &existing == asset => {
            report.unchanged += 1;
            return;
        }
        Ok(_) => {}
        Err(error) => {
            report.errors.push(format!("{}: {error}", asset.path));
            return;
        }
    }
    match store.upsert::<Assets>(&asset.id, asset) {
        Ok(Put::Inserted) => report.inserted += 1,
        Ok(Put::Replaced) => report.updated += 1,
        Ok(Put::Exists) => report.unchanged += 1,
        Err(error) => report.errors.push(format!("{}: {error}", asset.path)),
    }
}

fn prune(
    store: &Store,
    root: &Path,
    source: &str,
    seen: &HashSet<String>,
    report: &mut IndexReport,
) {
    let root_text = root.to_string_lossy();
    let root_text = root_text.trim_end_matches('/');
    let prefix = format!("{root_text}/");
    let rows = match store.iter_kind::<Assets>() {
        Ok(rows) => rows,
        Err(error) => {
            report.errors.push(format!("{root_text}: {error}"));
            return;
        }
    };
    for (id, asset) in rows {
        if seen.contains(&id) || asset.source != source {
            continue;
        }
        let under_path = asset.path == root_text || asset.path.starts_with(&prefix);
        let under_archive = asset
            .archive
            .as_deref()
            .is_some_and(|archive| archive == root_text || archive.starts_with(&prefix));
        if !(under_path || under_archive) {
            continue;
        }
        match store.delete::<Assets>(&id) {
            Ok(true) => report.pruned += 1,
            Ok(false) => {}
            Err(error) => report.errors.push(format!("{}: {error}", asset.path)),
        }
    }
}

fn classify(path: &Path, extension: &str) -> Category {
    let text = path.to_string_lossy().to_lowercase();
    if text.contains("facegeom") && extension == "nif" {
        return Category::Face;
    }
    if text.contains("sliderpresets") && extension == "xml" {
        return Category::Preset;
    }
    if extension == "jslot" {
        return Category::FacePreset;
    }
    if matches!(extension, "osp" | "osd" | "bsd") {
        return Category::Project;
    }
    if matches!(extension, "nif" | "glb" | "gltf" | "obj") {
        if text.contains("hair") || text.contains("hairdos") {
            return Category::Hair;
        }
        if text.contains("body") || text.contains("baseshape") {
            return Category::Body;
        }
        if text.contains("clothes") || text.contains("armor") || text.contains("outfit") {
            return Category::Outfit;
        }
    }
    if extension == "nif" {
        return Category::Mesh;
    }
    if extension == "dds" {
        return Category::Texture;
    }
    if extension == "fbx" && text.contains("mixamo/animations") {
        return Category::Animation;
    }
    if matches!(extension, "glb" | "gltf" | "obj" | "fbx" | "tri") {
        return Category::Mesh;
    }
    if matches!(extension, "hkx") || text.contains("/animations/") {
        return Category::Animation;
    }
    if matches!(extension, "png" | "jpg" | "jpeg" | "webp") {
        return Category::Image;
    }
    Category::Other
}

fn member_category(path: &str) -> Option<Category> {
    let text = path.to_lowercase();
    if text.contains("facegeom") && text.ends_with(".nif") {
        return Some(Category::Face);
    }
    if text.contains("sliderpresets/") && text.ends_with(".xml") {
        return Some(Category::Preset);
    }
    if text.contains("slidersets/")
        && (text.ends_with(".osp") || text.ends_with(".osd") || text.ends_with(".bsd"))
    {
        return Some(Category::Project);
    }
    if text.ends_with(".jslot") {
        return Some(Category::FacePreset);
    }
    None
}

fn sex_from_path(text: &str) -> Option<Sex> {
    sex_from_context(text)
}

fn sex_from_context(text: &str) -> Option<Sex> {
    let lower = text.to_lowercase();
    if lower.contains("female")
        || lower.contains("bijin")
        || lower.contains("warmaidens")
        || lower.contains("wives")
    {
        Some(Sex::Female)
    } else if lower.contains("male")
        || lower.contains("pride")
        || lower.contains("pan_npcs")
        || lower.contains("himbo")
    {
        Some(Sex::Male)
    } else {
        None
    }
}

fn family_from_path(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for family in [
        "bijin", "himbo", "3ba", "cbbe", "pride", "sentinel", "apachii", "rustic",
    ] {
        if lower.contains(family) {
            return Some(family.to_string());
        }
    }
    // `pan` is a whole token: a path such as `pants/` must not become PAN_NPCS.
    if lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == "pan")
    {
        return Some("pan".to_string());
    }
    None
}

fn status_from_path(text: &str) -> Status {
    let lower = text.to_lowercase();
    if lower.contains("accepted") {
        Status::Accepted
    } else if lower.contains("/outputs/") || lower.contains("/built/") || lower.contains("prepared")
    {
        Status::Prepared
    } else {
        Status::Raw
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unnamed")
        .to_string()
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn identity(parts: &[&str]) -> String {
    sha256_bytes(parts.join("\u{1f}").as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_readme_hashes(root: &Path) -> HashMap<String, String> {
    let path = root.join("README.md");
    let Ok(text) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let mut hashes = HashMap::new();
    for line in text.lines() {
        let mut cells = line.split('|').map(str::trim);
        let _leading = cells.next();
        let Some(name) = cells.next() else { continue };
        let Some(name) = name
            .strip_prefix('`')
            .and_then(|name| name.strip_suffix('`'))
        else {
            continue;
        };
        let mut hash = None;
        for cell in cells {
            if let Some(value) = cell
                .strip_prefix('`')
                .and_then(|cell| cell.strip_suffix('`'))
                && value.len() == 64
                && value.chars().all(|character| character.is_ascii_hexdigit())
            {
                hash = Some(value.to_lowercase());
            }
        }
        if let Some(hash) = hash {
            hashes.insert(name.to_string(), hash);
        }
    }
    hashes
}
