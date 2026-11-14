//! Read-only Skyrim BSA indexing and loose-first texture resolution.
//!
//! The format facts are reimplemented from the BSD-licensed NifTools FSEngine
//! reader (`lib/FSEngine/FSBSA.{h,cpp}` of the local BodySlide checkout):
//! 36-byte header, 24-byte SSE folder records, 16-byte file records, the
//! 1-byte-length folder names and trailing NUL-terminated file-name block, the
//! `sizeFlags` compression rule (`member XOR archive default`), the LZ4-frame
//! payload contract and the `\`→`/` + lowercase lookup normalization. No
//! BSD source text is copied.
//!
//! Only the SSE version (0x69) is indexed; 0x67/0x68 are named refusals until
//! a real local LE fixture exists (MOD-03 D-1). Indexing reads the header,
//! record and name tables only — no decompression — while
//! [`BsaIndex::read_member`] reads one member on demand. Every offset and
//! length is bounded against the file size before it is used.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};

/// `BSA\0` as a little-endian `u32`.
const MAGIC: u32 = 0x0041_5342;
/// Skyrim Special Edition BSA version.
const SSE_VERSION: u32 = 0x69;
/// Archive flag: folder (path) names are present; required for lookup.
const FLAG_PATH_NAMES: u32 = 0x0000_0001;
/// Archive flag: file names are present; required for lookup.
const FLAG_FILE_NAMES: u32 = 0x0000_0002;
/// Archive flag: members default to compressed.
const FLAG_COMPRESSED: u32 = 0x0000_0004;
/// Archive flag: member data begins with a length-prefixed name.
const FLAG_FULL_NAME_PREFIX: u32 = 0x0000_0100;
/// `sizeFlags` mask: the member's stored size.
const SIZE_MASK: u32 = 0x3FFF_FFFF;
/// `sizeFlags` mask: the per-member compression flag.
const COMPRESS_MASK: u32 = 0xC000_0000;
/// Header size: magic, version and seven `u32` fields.
const HEADER_LEN: usize = 36;
/// SSE folder-record size: hash, file count, unknown, offset.
const FOLDER_RECORD_LEN: usize = 24;
/// File-record size: hash, sizeFlags, offset.
const FILE_RECORD_LEN: usize = 16;

/// Why a BSA could not be indexed or a member could not be extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BsaError {
    /// The archive could not be opened or read.
    Io {
        /// Archive path.
        path: PathBuf,
        /// Failure description.
        message: String,
    },
    /// The magic is not `BSA\0`.
    Magic {
        /// Magic word that was found instead.
        found: u32,
    },
    /// The BSA version is not the indexed SSE version.
    Version {
        /// Version word that was found.
        found: u32,
    },
    /// The archive lacks the path-name or file-name flag.
    Flags {
        /// Archive flags that were found.
        found: u32,
    },
    /// A structure ended before its declared content; `what` names it.
    Truncated {
        /// Which structure was short.
        what: &'static str,
    },
    /// The folder records' file counts disagree with the header's file count.
    CountMismatch {
        /// File count declared by the header.
        declared: u32,
        /// File count summed over the folder records.
        folders: u32,
    },
    /// The file-name block overran its length or holds no valid name.
    NameTable,
    /// The requested member is not in the index.
    MemberNotFound(String),
    /// A compressed member's payload did not decode to its declared size.
    Decompress {
        /// Normalized member key.
        member: String,
        /// Failure description.
        message: String,
    },
}

impl fmt::Display for BsaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::Magic { found } => {
                write!(
                    formatter,
                    "not a BSA: magic 0x{found:08x}, expected `BSA\\0`"
                )
            }
            Self::Version { found } => {
                write!(
                    formatter,
                    "unsupported BSA version 0x{found:02x}; only SSE 0x69"
                )
            }
            Self::Flags { found } => write!(
                formatter,
                "archive flags 0x{found:08x} lack the path-name or file-name flag"
            ),
            Self::Truncated { what } => write!(formatter, "archive ends inside the {what}"),
            Self::CountMismatch { declared, folders } => write!(
                formatter,
                "header declares {declared} files but the folders sum to {folders}"
            ),
            Self::NameTable => write!(formatter, "file-name block overruns its length"),
            Self::MemberNotFound(key) => write!(formatter, "no member `{key}` in the archive"),
            Self::Decompress { member, message } => {
                write!(formatter, "member `{member}` did not decode: {message}")
            }
        }
    }
}

impl std::error::Error for BsaError {}

/// One indexed member: the record fields extraction needs.
#[derive(Debug, Clone, Copy)]
struct Member {
    /// Raw `sizeFlags`: size plus the compression flag bits.
    size_flags: u32,
    /// Absolute offset of the member's data block.
    offset: u32,
}

impl Member {
    /// Stored size on disk, name prefix and compression header included.
    fn stored_len(self) -> u32 {
        self.size_flags & SIZE_MASK
    }

    /// Per-member compression flag, before the archive default applies.
    fn compressed(self) -> bool {
        (self.size_flags & COMPRESS_MASK) != 0
    }

    /// Whether extraction must decode this member: `member XOR archive
    /// default`, the reference reader's rule.
    fn effective_compression(self, archive_default: bool) -> bool {
        self.compressed() ^ archive_default
    }
}

/// Read-only index of one Skyrim SSE BSA.
///
/// Lookups normalize like the reference reader: `\` becomes `/` and the key is
/// ASCII-lowercased. When several archives are consulted, the first that
/// contains the member wins; ambiguity is not detected (documented reference
/// behaviour, MOD-03 R-6).
#[derive(Debug)]
pub struct BsaIndex {
    path: PathBuf,
    members: BTreeMap<String, Member>,
    /// Archive-level compression default (`FLAG_COMPRESSED`).
    compress_toggle: bool,
    /// Member data starts with a length-prefixed name (`FLAG_FULL_NAME_PREFIX`).
    name_prefix: bool,
    /// File size at index time; every later read is bounded against it.
    file_len: u64,
}

/// Build the index of one BSA.
///
/// # Errors
/// Refuses unreadable files, non-BSA or non-SSE data, missing name flags,
/// structures that end inside the file, name tables that overrun and folder
/// counts that disagree with the header.
pub fn index_bsa(path: &Path) -> Result<BsaIndex, BsaError> {
    let io = |error: std::io::Error| BsaError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    };
    let file_len = std::fs::metadata(path).map_err(io)?.len();
    let handle = std::fs::File::open(path).map_err(io)?;
    let mut reader = std::io::BufReader::new(handle);

    let mut header = [0u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_| BsaError::Truncated { what: "header" })?;
    let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("slice"));
    if u32_at(0) != MAGIC {
        return Err(BsaError::Magic { found: u32_at(0) });
    }
    let version = u32_at(4);
    if version != SSE_VERSION {
        return Err(BsaError::Version { found: version });
    }
    let folder_record_offset = u32_at(8);
    let archive_flags = u32_at(12);
    if archive_flags & (FLAG_PATH_NAMES | FLAG_FILE_NAMES) != (FLAG_PATH_NAMES | FLAG_FILE_NAMES) {
        return Err(BsaError::Flags {
            found: archive_flags,
        });
    }
    let folder_count = u32_at(16);
    let file_count = u32_at(20);
    let file_name_len = u64::from(u32_at(28));
    if u64::from(folder_record_offset) < HEADER_LEN as u64
        || u64::from(folder_record_offset) >= file_len
        || u64::from(folder_record_offset) + u64::from(folder_count) * FOLDER_RECORD_LEN as u64
            > file_len
    {
        return Err(BsaError::Truncated {
            what: "folder records",
        });
    }
    reader
        .seek(std::io::SeekFrom::Start(u64::from(folder_record_offset)))
        .map_err(io)?;
    let mut folder_records = vec![[0u8; FOLDER_RECORD_LEN]; folder_count as usize];
    for record in &mut folder_records {
        reader.read_exact(record).map_err(|_| BsaError::Truncated {
            what: "folder record",
        })?;
    }

    let compress_toggle = archive_flags & FLAG_COMPRESSED != 0;
    let name_prefix = archive_flags & FLAG_FULL_NAME_PREFIX != 0;
    let mut members = BTreeMap::new();
    let mut folder_total: u32 = 0;
    let mut folder_files: Vec<(String, Vec<Member>)> = Vec::with_capacity(folder_count as usize);
    for record in &folder_records {
        let files_here = u32::from_le_bytes(record[8..12].try_into().expect("slice"));
        folder_total = folder_total.saturating_add(files_here);
        let folder = read_sized_name(&mut reader, "folder name")?;
        let mut files = Vec::with_capacity(files_here as usize);
        for _ in 0..files_here {
            let mut file_record = [0u8; FILE_RECORD_LEN];
            reader
                .read_exact(&mut file_record)
                .map_err(|_| BsaError::Truncated {
                    what: "file record",
                })?;
            let member = Member {
                size_flags: u32::from_le_bytes(file_record[8..12].try_into().expect("slice")),
                offset: u32::from_le_bytes(file_record[12..16].try_into().expect("slice")),
            };
            bound_member(member, file_len, compress_toggle, name_prefix)?;
            files.push(member);
        }
        folder_files.push((folder, files));
    }
    if folder_total != file_count {
        return Err(BsaError::CountMismatch {
            declared: file_count,
            folders: folder_total,
        });
    }
    // The NUL-terminated file-name block sits after the last folder's records
    // and is read whole, like the reference reader does; names are assigned in
    // record order and a block that ends without one is a refusal.
    let mut names_block = vec![0u8; file_name_len as usize];
    reader
        .read_exact(&mut names_block)
        .map_err(|_| BsaError::NameTable)?;
    let mut names = Names::new(&names_block);
    for (folder, files) in folder_files {
        for member in files {
            let name = names.next()?;
            members.insert(normalize(&format!("{folder}/{name}")), member);
        }
    }
    Ok(BsaIndex {
        path: path.to_path_buf(),
        members,
        compress_toggle,
        name_prefix,
        file_len,
    })
}

/// Cursor over the pre-read file-name block.
struct Names<'a> {
    block: &'a [u8],
    cursor: usize,
}

impl<'a> Names<'a> {
    fn new(block: &'a [u8]) -> Self {
        Self { block, cursor: 0 }
    }

    /// The next NUL-terminated name; a missing terminator or non-UTF-8 bytes
    /// are [`BsaError::NameTable`].
    fn next(&mut self) -> Result<&'a str, BsaError> {
        let start = self.cursor;
        let end = self.block[start..]
            .iter()
            .position(|&byte| byte == 0)
            .map(|offset| start + offset)
            .ok_or(BsaError::NameTable)?;
        self.cursor = end + 1;
        std::str::from_utf8(&self.block[start..end]).map_err(|_| BsaError::NameTable)
    }
}

/// Read a 1-byte-length-prefixed name (the folder-name encoding).
///
/// SSE archives count the trailing NUL inside the declared length, so one
/// present NUL is part of the entry and dropped from the name.
fn read_sized_name<R: std::io::Read>(
    reader: &mut R,
    what: &'static str,
) -> Result<String, BsaError> {
    let mut len = [0u8; 1];
    reader
        .read_exact(&mut len)
        .map_err(|_| BsaError::Truncated { what })?;
    let mut name = vec![0u8; len[0] as usize];
    reader
        .read_exact(&mut name)
        .map_err(|_| BsaError::Truncated { what })?;
    if name.last() == Some(&0) {
        name.pop();
    }
    String::from_utf8(name).map_err(|_| BsaError::Truncated { what })
}

/// Bound one member's data block against the file size at index time.
///
/// The stored length is the whole block — name prefix included when the
/// archive writes one — for an uncompressed member; a compressed member must
/// at least fit its name prefix and 4-byte size header. What happens inside a
/// compressed stream stays a read-time refusal.
fn bound_member(
    member: Member,
    file_len: u64,
    compress_toggle: bool,
    name_prefix: bool,
) -> Result<(), BsaError> {
    let start = u64::from(member.offset);
    if member.effective_compression(compress_toggle) {
        if start + u64::from(name_prefix) + 4 > file_len {
            return Err(BsaError::Truncated {
                what: "member data",
            });
        }
    } else if start + u64::from(member.stored_len()) > file_len {
        return Err(BsaError::Truncated {
            what: "member data",
        });
    }
    Ok(())
}

/// `\`→`/` and ASCII lowercase, applied to both sides of every lookup.
fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

impl BsaIndex {
    /// The archive this index was built from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of indexed members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the archive indexed no members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Every member key, normalized and sorted.
    pub fn members(&self) -> impl Iterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    /// Whether the archive holds `rel_path`, after normalization.
    pub fn contains(&self, rel_path: &str) -> bool {
        self.members.contains_key(&normalize(rel_path))
    }

    fn lookup(&self, normalized: &str) -> Option<&Member> {
        self.members.get(normalized)
    }

    /// The member's uncompressed size, when present.
    ///
    /// A compressed member's size lives in its payload header, so this peeks
    /// four bytes; an unreadable or truncated peek is `None`, never a guess.
    pub fn member_size(&self, rel_path: &str) -> Option<u64> {
        let key = normalize(rel_path);
        let member = self.lookup(&key)?;
        if !member.effective_compression(self.compress_toggle) {
            let stored = u64::from(member.stored_len());
            // A prefixed member's stored length includes its own path (one
            // length byte + the path, which normalizes to `key` without
            // changing length); the content size excludes it.
            return Some(if self.name_prefix {
                stored.saturating_sub(1 + key.len() as u64)
            } else {
                stored
            });
        }
        let mut file = std::fs::File::open(&self.path).ok()?;
        let mut offset = u64::from(member.offset);
        if self.name_prefix {
            let mut len = [0u8; 1];
            file.seek(std::io::SeekFrom::Start(offset)).ok()?;
            file.read_exact(&mut len).ok()?;
            offset += 1 + u64::from(len[0]);
        }
        let mut header = [0u8; 4];
        file.seek(std::io::SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut header).ok()?;
        Some(u64::from(u32::from_le_bytes(header)))
    }

    /// Extract one member's bytes.
    ///
    /// # Errors
    /// [`BsaError::MemberNotFound`] for an absent member; reads beyond the
    /// file and payloads that do not decode to their declared size are named
    /// refusals.
    pub fn read_member(&self, rel_path: &str) -> Result<Vec<u8>, BsaError> {
        let key = normalize(rel_path);
        let member = self
            .lookup(&key)
            .ok_or_else(|| BsaError::MemberNotFound(key.clone()))?;
        let io = |error: std::io::Error| BsaError::Io {
            path: self.path.clone(),
            message: error.to_string(),
        };
        let mut file = std::fs::File::open(&self.path).map_err(io)?;
        let mut offset = u64::from(member.offset);
        let mut stored = member.stored_len();
        if self.name_prefix {
            let mut len = [0u8; 1];
            file.seek(std::io::SeekFrom::Start(offset)).map_err(io)?;
            file.read_exact(&mut len).map_err(|_| BsaError::Truncated {
                what: "member name prefix",
            })?;
            let prefix = 1 + u64::from(len[0]);
            offset += prefix;
            stored = stored
                .checked_sub(u32::try_from(prefix).unwrap_or(u32::MAX))
                .ok_or(BsaError::Truncated {
                    what: "member name prefix",
                })?;
        }
        if offset + u64::from(stored) > self.file_len {
            return Err(BsaError::Truncated {
                what: "member data",
            });
        }
        let mut payload = vec![0u8; stored as usize];
        file.seek(std::io::SeekFrom::Start(offset)).map_err(io)?;
        file.read_exact(&mut payload)
            .map_err(|_| BsaError::Truncated {
                what: "member data",
            })?;
        if !member.effective_compression(self.compress_toggle) {
            return Ok(payload);
        }
        decompress(&key, &payload)
    }
}

/// Decode one compressed member: `u32` declared size + LZ4 frame, length
/// verified against the declaration.
fn decompress(member: &str, payload: &[u8]) -> Result<Vec<u8>, BsaError> {
    if payload.len() < 4 {
        return Err(BsaError::Decompress {
            member: member.to_string(),
            message: "payload is shorter than its size header".to_string(),
        });
    }
    let declared = u32::from_le_bytes(payload[..4].try_into().expect("slice"));
    if declared == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(declared as usize);
    lz4_flex::frame::FrameDecoder::new(&payload[4..])
        .read_to_end(&mut out)
        .map_err(|error| BsaError::Decompress {
            member: member.to_string(),
            message: error.to_string(),
        })?;
    if out.len() != declared as usize {
        return Err(BsaError::Decompress {
            member: member.to_string(),
            message: format!("declared {declared} bytes, decoded {}", out.len()),
        });
    }
    Ok(out)
}

/// Where a texture reference resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureSource {
    /// A loose file under one of the searched roots.
    Loose(PathBuf),
    /// A member of one indexed BSA.
    Archive {
        /// Archive holding the member.
        bsa: PathBuf,
        /// Normalized member key.
        member: String,
    },
}

/// Resolve one authored texture path: loose under `roots` first, then the
/// `indexes` in order; the first match wins and `None` means unresolved.
///
/// Both sides are normalized (`\`→`/`, ASCII lowercase), matching the
/// reference reader's lookup contract.
pub fn resolve_texture(
    path: &str,
    roots: &[PathBuf],
    indexes: &[BsaIndex],
) -> Option<TextureSource> {
    let normalized = normalize(path);
    if normalized.is_empty() {
        return None;
    }
    for root in roots {
        let candidate = root.join(&normalized);
        if candidate.is_file() {
            return Some(TextureSource::Loose(candidate));
        }
    }
    for index in indexes {
        if index.lookup(&normalized).is_some() {
            return Some(TextureSource::Archive {
                bsa: index.path().to_path_buf(),
                member: normalized,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests;
