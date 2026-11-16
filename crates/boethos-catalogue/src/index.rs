//! Read-only directory and archive indexing.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use boethos::{Put, Store};
use sha2::{Digest, Sha256};

use crate::archive::archive_members;
use crate::model::{Asset, Assets, Category, Sex, Status};

const ARCHIVE_EXTENSIONS: [&str; 4] = ["7z", "zip", "rar", "bsa"];
const IGNORED_DIRS: [&str; 5] = [".git", ".jj", "__pycache__", "node_modules", "target"];
const UNSUPPORTED_ARCHIVES: [&str; 5] = ["gz", "tar", "tgz", "xz", "bz2"];

/// Controlled garment-type tags. A garment may carry several of them.
const GARMENT_WORD_TAGS: &[(&str, &[&str])] = &[
    (
        "wear:underwear",
        &[
            "bra",
            "panty",
            "panties",
            "briefs",
            "underwear",
            "undergarment",
        ],
    ),
    ("wear:top", &["bra", "shirt", "blouse", "top", "vest"]),
    (
        "wear:bottom",
        &["shorts", "short", "pants", "trousers", "skirt", "panty"],
    ),
    ("wear:onepiece", &["dress", "robe", "gown", "jumpsuit"]),
    ("wear:shoes", &["boots", "shoes", "sandals", "heels"]),
    ("wear:headwear", &["helmet", "hat", "hood", "circlet"]),
    (
        "wear:armor",
        &["armor", "armour", "cuirass", "plate", "mail"],
    ),
    ("wear:sport", &["sport", "athletic", "training", "gym"]),
    (
        "wear:accessory",
        &[
            "gloves",
            "gauntlets",
            "belt",
            "cape",
            "cloak",
            "necklace",
            "amulet",
            "jewelry",
            "ring",
        ],
    ),
];

/// Exact garment-part tags; unlike `wear:*`, these name the piece.
const GARMENT_PART_TAGS: &[(&str, &[&str])] = &[
    ("garment:bra", &["bra"]),
    ("garment:panty", &["panty", "panties", "briefs"]),
    ("garment:shorts", &["shorts", "short"]),
    ("garment:dress", &["dress", "gown"]),
    ("garment:robe", &["robe"]),
    ("garment:boots", &["boots"]),
    ("garment:helmet", &["helmet"]),
];

const TIGHT_TOKENS: [&str; 8] = [
    "bra", "panty", "panties", "briefs", "shorts", "short", "leggings", "tights",
];

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
    /// Selected-member count per archive path, in visit order. A refused
    /// archive records 0 so a silent miss stays visible next to its error.
    pub members_per_archive: Vec<(String, usize)>,
    /// Animation clips embedded in indexed `glb`/`gltf` files.
    pub clips: usize,
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
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        index_one(
            store,
            &path,
            &relative,
            source,
            &readme_hashes,
            options,
            &mut seen,
            &mut report,
        );
    }
    if options.prune {
        prune(store, root, source, &seen, &mut report);
    }
    report
}

/// Index one explicit file without walking a directory.
///
/// Use it when a hand-picked set matters more than a whole root: the row
/// identity is the same canonical path the recursive walk would produce, and
/// archive files still gain their members.
pub fn index_file(store: &Store, path: &Path, source: &str, options: &IndexOptions) -> IndexReport {
    let mut report = IndexReport::default();
    // `prune` deletes every row of a source that the walk did not see, and a single file
    // is not a boundary: honouring it here would wipe the rest of the source. Refuse
    // rather than accept a flag we would have to ignore.
    if options.prune {
        report.errors.push(format!(
            "{}: --prune applies to --root walks; a single file has no boundary to prune against",
            path.display()
        ));
        return report;
    }
    if !path.is_file() {
        report
            .errors
            .push(format!("{}: not a file", path.display()));
        return report;
    }
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let relative = Path::new(path.file_name().unwrap_or(path.as_os_str())).to_path_buf();
    let mut seen = HashSet::new();
    index_one(
        store,
        &path,
        &relative,
        source,
        &HashMap::new(),
        options,
        &mut seen,
        &mut report,
    );
    report
}

#[allow(clippy::too_many_arguments)]
fn index_one(
    store: &Store,
    path: &Path,
    relative: &Path,
    source: &str,
    readme_hashes: &HashMap<String, String>,
    options: &IndexOptions,
    seen: &mut HashSet<String>,
    report: &mut IndexReport,
) {
    let extension = extension(path);
    if ARCHIVE_EXTENSIONS.contains(&extension.as_str()) {
        report.archives += 1;
        index_archive(store, path, source, readme_hashes, options, seen, report);
        return;
    }
    report.files += 1;
    if UNSUPPORTED_ARCHIVES.contains(&extension.as_str()) {
        report.unsupported_archives += 1;
    }
    let relative_text = relative.to_string_lossy();
    let category = classify(relative, &extension);
    let sha = match sha256_file(path) {
        Ok(sha) => sha,
        Err(error) => {
            report.errors.push(error);
            return;
        }
    };
    let path_text = path.to_string_lossy().into_owned();
    let bytes = fs::metadata(path).ok().map(|meta| meta.len());
    let status = status_from_path(&relative_text);
    let asset = Asset {
        id: crate::identity(&["file", source, &path_text]),
        category,
        name: file_name(path),
        path: path_text.clone(),
        source: source.to_string(),
        archive: None,
        entry: None,
        format: extension,
        shape: None,
        vertices: None,
        triangles: None,
        clip: None,
        sha256: Some(sha),
        source_sha256: None,
        bytes,
        sex: sex_from_path(&relative_text),
        family: family_from_path(&relative_text),
        tags: garment_tags(&relative_text, category, source),
        status,
    };
    write(store, &asset, seen, report);
    if matches!(asset.format.as_str(), "glb" | "gltf") {
        index_clips(store, path, &asset, seen, report);
    }
}

fn index_clips(
    store: &Store,
    path: &Path,
    actor: &Asset,
    seen: &mut HashSet<String>,
    report: &mut IndexReport,
) {
    let document = match crate::clip::read_clips(path) {
        Ok(document) => document,
        Err(error) => {
            report.errors.push(error);
            return;
        }
    };
    for (index, clip) in document.clips.iter().enumerate() {
        let asset = crate::clip::clip_asset(
            &actor.source,
            &actor.path,
            actor.sha256.as_deref(),
            index,
            clip,
            actor.status,
        );
        report.clips += 1;
        write(store, &asset, seen, report);
    }
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
    let archive_id = crate::identity(&["archive", source, &path_text]);
    let archive_asset = Asset {
        id: archive_id.clone(),
        category: Category::Archive,
        name: name.clone(),
        path: path_text.clone(),
        source: source.to_string(),
        archive: None,
        entry: None,
        format: extension(path),
        shape: None,
        vertices: None,
        triangles: None,
        clip: None,
        sha256: sha,
        source_sha256: None,
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
    let entries = match archive_members(path) {
        Ok(entries) => entries,
        Err(error) => {
            report.errors.push(error);
            report.members_per_archive.push((path_text.clone(), 0));
            return;
        }
    };
    let mut selected = 0usize;
    for entry in entries {
        if entry.is_directory {
            continue;
        }
        let Some(category) = member_category(&entry.path, &name) else {
            continue;
        };
        let member_identity = crate::identity(&["member", &archive_id, &entry.path]);
        let mut tags = Vec::new();
        if let Some(crc) = &entry.crc {
            tags.push(format!("crc:{crc}"));
        }
        if entry.path.to_lowercase().contains("facegeom") {
            tags.push("facegeom".to_string());
        }
        tags.extend(garment_tags(&entry.path, category, source));
        tags.sort();
        tags.dedup();
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
            shape: None,
            vertices: None,
            triangles: None,
            clip: None,
            sha256: None,
            source_sha256: None,
            bytes: entry.size,
            sex,
            family,
            tags,
            status: Status::Raw,
        };
        report.members += 1;
        selected += 1;
        write(store, &asset, seen, report);
    }
    report
        .members_per_archive
        .push((path_text.clone(), selected));
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
    if matches!(extension, "nif" | "glb" | "gltf" | "obj")
        || (extension == "fbx" && !text.contains("animations"))
    {
        if has_hair_token(&text) {
            return Category::Hair;
        }
        if text.contains("body") || text.contains("baseshape") {
            return Category::Body;
        }
        if has_garment_token(&text)
            || text.contains("clothes")
            || text.contains("armor")
            || text.contains("outfit")
        {
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
    if extension == "hkx" {
        return Category::SkyrimAnimation;
    }
    if text.contains("/animations/") {
        return Category::Animation;
    }
    if matches!(extension, "png" | "jpg" | "jpeg" | "webp") {
        return Category::Image;
    }
    Category::Other
}

fn member_category(path: &str, archive_name: &str) -> Option<Category> {
    let text = path.to_lowercase();
    let file = text.rsplit('/').next().unwrap_or("");
    // Havok clips and behavior graphs: the extension owns this mapping, so a
    // `.hkx` member is never dropped as unclassified.
    if text.ends_with(".hkx") {
        return Some(Category::SkyrimAnimation);
    }
    if text.contains("underwear")
        && ["body", "hands", "feet", "head", "tail", "ears", "mouth"]
            .iter()
            .any(|part| file.contains(part))
    {
        return None;
    }
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
    // Supplied hair packs keep meshes under a `hair`/`hairdos` token or, like
    // KS Hairdos, directly under an archive whose name carries the token.
    let mesh_member = text.starts_with("meshes/") || text.contains("/meshes/");
    if text.ends_with(".nif")
        && mesh_member
        && (has_hair_token(&text) || has_hair_token(&archive_name.to_lowercase()))
    {
        return Some(Category::Hair);
    }
    if text.ends_with(".nif") && mesh_member && has_garment_token(&text) {
        return Some(Category::Outfit);
    }
    None
}

fn has_hair_token(text: &str) -> bool {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == "hair" || token.starts_with("hairdo") || token == "hairline")
}

fn garment_tags(text: &str, category: Category, source: &str) -> Vec<String> {
    if category != Category::Outfit {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let tokens: HashSet<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    let mut tags: Vec<String> = Vec::new();
    for (tag, words) in GARMENT_WORD_TAGS {
        if words.iter().any(|word| tokens.contains(word)) {
            tags.push((*tag).to_string());
        }
    }
    for (tag, words) in GARMENT_PART_TAGS {
        if words.iter().any(|word| tokens.contains(word)) {
            tags.push((*tag).to_string());
        }
    }
    if TIGHT_TOKENS.iter().any(|word| tokens.contains(word)) {
        tags.push("fit:tight".to_string());
    }
    if let Some(sex) = sex_from_context(text) {
        tags.push(format!("sex:{}", sex.as_str()));
    }
    if let Some(family) = family_from_path(text)
        && matches!(family.as_str(), "cbbe" | "3ba" | "himbo")
    {
        tags.push(format!("body:{family}"));
    }
    if let Some(origin) = crate::origin_tag(source, text) {
        tags.push(origin.to_string());
    }
    if tags.iter().any(|tag| {
        matches!(
            tag.as_str(),
            "wear:underwear"
                | "wear:top"
                | "wear:bottom"
                | "wear:onepiece"
                | "wear:shoes"
                | "wear:headwear"
                | "wear:accessory"
        )
    }) {
        tags.retain(|tag| tag != "wear:armor");
    }
    if tags.iter().any(|tag| tag == "wear:underwear") {
        tags.push("layer:inner".to_string());
    } else if tags
        .iter()
        .any(|tag| tag == "wear:top" || tag == "wear:bottom")
    {
        tags.push("layer:mid".to_string());
    }
    tags.sort();
    tags.dedup();
    tags
}

fn has_garment_token(text: &str) -> bool {
    let lower = text.to_lowercase();
    let tokens: HashSet<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    GARMENT_WORD_TAGS
        .iter()
        .chain(GARMENT_PART_TAGS.iter())
        .any(|(_, words)| words.iter().any(|word| tokens.contains(word)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsa::tests::{Build, Spec, build_bsa};

    #[test]
    fn hkx_members_land_in_the_skyrim_animation_category() {
        assert_eq!(
            member_category(
                "meshes/actors/character/animations/2hm_attackpowerforward.hkx",
                "Skyrim - Animations.bsa"
            ),
            Some(Category::SkyrimAnimation)
        );
        assert_eq!(
            member_category("behaviors/0_master.hkx", "Skyrim - Animations.bsa"),
            Some(Category::SkyrimAnimation)
        );
        assert_eq!(
            member_category(
                "meshes/animationsetdata/2hm_attackpowerforward.txt",
                "Skyrim - Animations.bsa"
            ),
            None,
            "non-Havok members keep their existing routing"
        );
        assert_eq!(
            classify(Path::new("meshes/animations/idle.hkx"), "hkx"),
            Category::SkyrimAnimation,
            "the extension table owns loose `.hkx` too"
        );
        assert_eq!(
            Category::parse("skyrim-animation"),
            Some(Category::SkyrimAnimation)
        );
        assert_eq!(Category::SkyrimAnimation.as_str(), "skyrim-animation");
    }

    #[test]
    fn a_bsa_root_yields_member_rows_counts_and_named_refusals() {
        let root =
            std::env::temp_dir().join(format!("boethos-catalogue-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("test root");

        let specs = [
            Spec {
                folder: "meshes\\actors\\character\\animations",
                name: "2hm_attackpowerforward.hkx",
                data: b"havok-spline-clip",
            },
            Spec {
                folder: "meshes\\actors\\character\\_1stperson\\animations",
                name: "mt_walkidle.hkx",
                data: b"havok-first-person",
            },
            Spec {
                folder: "textures\\clutter",
                name: "rope.dds",
                data: b"DDS ",
            },
        ];
        let animations = root.join("Skyrim - Animations.bsa");
        fs::write(&animations, build_bsa(&specs, Build::default())).expect("synthetic BSA");
        fs::write(root.join("loose behavior.hkx"), b"loose havok").expect("loose member");
        let broken = root.join("encrypted.bsa");
        fs::write(&broken, b"ENCRYPTED-PAYLOAD-BY-TEST-0000000000001").expect("broken fixture");

        let store_path = std::env::temp_dir().join(format!(
            "boethos-catalogue-store-{}.redb",
            std::process::id()
        ));
        let store = boethos::Store::open(&store_path).expect("test store");
        let report = index_root(&store, &root, "test-root", &IndexOptions::default());

        // Per-archive member counts make a silent miss visible: the refused
        // archive records 0 beside its named error.
        assert_eq!(report.archives, 2, "both .bsa files are archive candidates");
        assert_eq!(report.members, 2, "only the `.hkx` members are selected");
        assert_eq!(
            report.members_per_archive,
            vec![
                (animations.to_string_lossy().into_owned(), 2),
                (broken.to_string_lossy().into_owned(), 0),
            ]
        );
        let refusal = report
            .errors
            .iter()
            .find(|error| error.contains("encrypted.bsa"))
            .expect("the broken archive is a named refusal");
        assert!(refusal.contains("not a BSA"), "{refusal}");

        let rows: Vec<Asset> = store
            .iter_kind::<Assets>()
            .expect("rows read")
            .into_iter()
            .map(|(_, asset)| asset)
            .collect();
        let skyrim_rows: Vec<&Asset> = rows
            .iter()
            .filter(|asset| asset.category == Category::SkyrimAnimation)
            .collect();
        assert_eq!(
            skyrim_rows.len(),
            3,
            "two BSA members plus the loose `.hkx`"
        );
        let power_attack = skyrim_rows
            .iter()
            .find(|asset| {
                asset.entry.as_deref()
                    == Some("meshes/actors/character/animations/2hm_attackpowerforward.hkx")
            })
            .expect("the named clip is indexed");
        assert_eq!(
            power_attack.path,
            format!(
                "{}::meshes/actors/character/animations/2hm_attackpowerforward.hkx",
                animations.display()
            )
        );
        assert_eq!(
            power_attack.archive.as_deref(),
            Some(animations.to_string_lossy().as_ref())
        );
        assert_eq!(power_attack.format, "hkx");

        let archive_rows: Vec<&Asset> = rows
            .iter()
            .filter(|asset| asset.category == Category::Archive)
            .collect();
        assert_eq!(archive_rows.len(), 2);
        assert!(archive_rows.iter().all(|asset| asset.format == "bsa"));

        drop(store);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&store_path);
    }

    #[test]
    fn classifies_hair_members_without_chair_false_positive() {
        assert_eq!(
            member_category("Data/meshes/KS Hairdo's/911.nif", "KS Hairdos SSE.7z"),
            Some(Category::Hair)
        );
        assert_eq!(
            member_category("meshes/actors/character/hair/hair01.nif", "pack.7z"),
            Some(Category::Hair)
        );
        assert_eq!(
            member_category("meshes/elsewhere/hair01.nif", "KS Hairdos SSE.7z"),
            Some(Category::Hair)
        );
        assert_ne!(
            member_category("meshes/furniture/chair.nif", "pack.7z"),
            Some(Category::Hair)
        );
        assert_eq!(
            member_category("meshes/armor/cuirass.nif", "pack.7z"),
            Some(Category::Outfit)
        );
        assert_eq!(
            member_category("meshes/clothes/shirt.nif", "pack.7z"),
            Some(Category::Outfit)
        );
        assert_ne!(
            member_category("meshes/furniture/table.nif", "pack.7z"),
            Some(Category::Outfit)
        );
        assert_ne!(
            member_category(
                "meshes/actors/character/character assets/underwear/femalebody_0.nif",
                "pack.7z"
            ),
            Some(Category::Outfit)
        );
        assert_ne!(
            member_category("textures/KS Hairdo's/hair.dds", "KS Hairdos SSE.7z"),
            Some(Category::Hair)
        );
        assert_ne!(
            member_category("Data/KS Hairdo's.esp", "KS Hairdos SSE.7z"),
            Some(Category::Hair)
        );
        assert_eq!(
            member_category("meshes/FaceGeom/skyrim.esm/0001.nif", "KS Hairdos SSE.7z"),
            Some(Category::Face)
        );
        assert_eq!(
            classify(Path::new("white-haired-anime-elf-figure-8k.fbx"), "fbx"),
            Category::Mesh
        );
        assert_eq!(
            classify(Path::new("hair/hair01.nif"), "nif"),
            Category::Hair
        );
    }
}
