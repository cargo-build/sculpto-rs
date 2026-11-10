//! Outfit Studio morph data files: named `.osd` containers and bare `.bsd` sets.
//!
//! Two different layouts carry one sparse-offset model.
//!
//! * `.osd` is a named container (`DiffData.cpp:136-183`): a 4-byte tag, a
//!   `u32` version, a `u32` block count, then per block a length-prefixed UTF-8
//!   name, a `u16` record count and 14-byte records (`u16` index, three `f32`
//!   deltas).
//! * `.bsd` is a bare record set (`DiffData.cpp:229-251`): no tag, no version
//!   and no name — a `u32` count, then 16-byte records (`u32` index, three `f32`
//!   deltas). The shape a bare set belongs to is named by the project `Data`
//!   reference, never by the file.
//!
//! The tag is `"OSD\0"_mci`, and `operator"" _mci` (`PlatformUtil.h:37-43`)
//! folds characters left to right, so the value is `0x4f534400`; written as a
//! little-endian `u32` that is the byte sequence `00 44 53 4f`, exposed here as
//! [`MAGIC`]. There is no literal ASCII `OSD` in the file.
//!
//! Both readers reproduce nifly's read-time snap, `Vector3::clampEpsilon`
//! (`Object3d.hpp:334-341`, `EPSILON = 0.0001`): a component with
//! `|value| < 0.0001` becomes exactly `0.0`. Writers keep authored values
//! unchanged, because the reference never snaps on save; the asymmetry is
//! therefore observable in one direction only and a parse/write/parse round
//! trip is stable.
//!
//! Deliberate differences from the reference, all on the data-safety side: a
//! bare record whose `u32` index exceeds `u16::MAX` is refused instead of being
//! truncated by `static_cast<uint16_t>` (`DiffData.cpp:247`); non-finite
//! offsets, trailing bytes, impossible counts, duplicate records and an
//! unreachable record count are refused instead of being read past. A record
//! index addresses a position in the shape that owns the data, so range
//! validation against a concrete mesh belongs to the bake plan, not here.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::error::{Error, ErrorKind, Result};

/// Container tag `0x4f534400` as its little-endian bytes, `00 44 53 4f`.
///
/// Only a named `.osd` container starts with these bytes; a bare `.bsd` starts
/// with its `u32` record count.
pub const MAGIC: [u8; 4] = *b"\x00DSO";

/// The only container version this crate reads and writes.
pub const SUPPORTED_VERSION: u32 = 1;

/// nifly's `EPSILON`: components below this magnitude read back as zero.
pub const RECORD_EPSILON: f32 = 0.0001;

/// Fixed `.osd` record width: `u16` index plus three `f32` deltas.
const OSD_RECORD_LEN: usize = 14;

/// Fixed `.bsd` record width: `u32` index plus three `f32` deltas.
const BSD_RECORD_LEN: usize = 16;

/// A decoded named `.osd` container.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphContainer {
    /// Container version; always [`SUPPORTED_VERSION`] after parsing.
    pub version: u32,
    /// Shape blocks in document order.
    pub blocks: Vec<MorphBlock>,
}

/// One shape's sparse vertex offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphBlock {
    /// Shape name the offsets deform, e.g. `HIMBO Body - Vanilla`.
    pub name: String,
    /// Offsets in document order.
    pub offsets: Vec<VertexOffset>,
}

/// One sparse vertex offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexOffset {
    /// Position in the source shape's vertex array.
    ///
    /// Both layouts share this `u16` model. The bare `.bsd` wire carries a
    /// `u32`; see [`parse_bare_bsd`] for the width check.
    pub index: u16,
    /// Delta applied at that vertex, in the shape's units.
    pub delta: [f32; 3],
}

/// A morph data file as read from disk: named blocks or one bare record set.
///
/// The distinction is the format, not a detail: a bare set stores no name, so
/// callers that need a block name take it from the project reference instead of
/// inventing one here.
#[derive(Debug, Clone, PartialEq)]
pub enum MorphData {
    /// `.osd`: a versioned container of named blocks.
    Named(MorphContainer),
    /// `.bsd`: a single bare record set with no stored name.
    Bare(Vec<VertexOffset>),
}

impl From<MorphContainer> for MorphData {
    fn from(container: MorphContainer) -> Self {
        Self::Named(container)
    }
}

impl MorphContainer {
    /// Decode a named `.osd` container from bytes.
    ///
    /// A bare `.bsd` has no tag and never parses here; read it through
    /// [`parse_bare_bsd`] or [`MorphData::from_file`].
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(bytes);
        let magic = cursor.take(4, "magic")?;
        if magic != MAGIC {
            return Err(Error::new(
                ErrorKind::Malformed,
                "magic",
                format!("expected `\\0DSO`, found {magic:02x?}"),
            ));
        }
        let version = cursor.u32("version")?;
        if version != SUPPORTED_VERSION {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "version",
                format!("version {version}; this crate reads version {SUPPORTED_VERSION}"),
            ));
        }
        let block_count = cursor.u32("block count")?;
        // A block needs at least one name byte and a two-byte count.
        if block_count as usize > cursor.remaining() / 3 {
            return Err(Error::new(
                ErrorKind::Malformed,
                "block count",
                format!("{block_count} blocks do not fit in {} bytes", bytes.len()),
            ));
        }

        let mut blocks = Vec::with_capacity(block_count as usize);
        for position in 0..block_count as usize {
            let path = format!("block[{position}]");
            let name_len = cursor.u8(&path)? as usize;
            let name_bytes = cursor.take(name_len, &path)?;
            let name = std::str::from_utf8(name_bytes)
                .map_err(|error| {
                    Error::new(
                        ErrorKind::Malformed,
                        &path,
                        format!("name is not UTF-8: {error}"),
                    )
                })?
                .to_string();
            if name.is_empty() {
                return Err(Error::new(
                    ErrorKind::Malformed,
                    &path,
                    "block name is empty",
                ));
            }
            let record_count = cursor.u16(&path)? as usize;
            let record_bytes = record_count
                .checked_mul(OSD_RECORD_LEN)
                .ok_or_else(|| Error::new(ErrorKind::Malformed, &path, "record count overflows"))?;
            if record_bytes > cursor.remaining() {
                return Err(Error::new(
                    ErrorKind::Malformed,
                    &path,
                    format!(
                        "{record_count} records need {record_bytes} bytes, {} left",
                        cursor.remaining()
                    ),
                ));
            }
            let mut offsets = Vec::with_capacity(record_count);
            for record in 0..record_count {
                let record_path = format!("{path}.record[{record}]");
                let index = cursor.u16(&record_path)?;
                let delta = cursor.delta(&record_path)?;
                offsets.push(VertexOffset { index, delta });
            }
            if blocks.iter().any(|block: &MorphBlock| block.name == name) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &path,
                    format!("duplicate block `{name}`"),
                ));
            }
            reject_duplicate_indices(&format!("{path}[{name}]"), &offsets)?;
            blocks.push(MorphBlock { name, offsets });
        }

        if cursor.remaining() != 0 {
            return Err(Error::new(
                ErrorKind::Malformed,
                "container",
                format!("{} trailing bytes after the last block", cursor.remaining()),
            ));
        }
        Ok(Self { version, blocks })
    }

    /// Decode a named `.osd` container from disk.
    pub fn parse_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|error| {
            Error::new(ErrorKind::Io, "container", error.to_string()).with_path(path)
        })?;
        Self::parse(&bytes).map_err(|error| error.with_path(path))
    }

    /// Encode a named `.osd` container.
    ///
    /// Byte-stable for one model: blocks and records keep document order, no
    /// delta is snapped, and the tag/version/count header is written from the
    /// model rather than guessed. What the reader cannot accept is refused
    /// here too, so a written file always reads back.
    pub fn to_osd_bytes(&self) -> Result<Vec<u8>> {
        if self.version != SUPPORTED_VERSION {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "version",
                format!(
                    "version {}; this crate writes version {SUPPORTED_VERSION}",
                    self.version
                ),
            ));
        }
        let block_count = u32::try_from(self.blocks.len()).map_err(|_| {
            Error::new(
                ErrorKind::InvalidValue,
                "block count",
                format!("{} blocks exceed the `u32` header count", self.blocks.len()),
            )
        })?;
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&block_count.to_le_bytes());

        let mut names = BTreeSet::new();
        for (position, block) in self.blocks.iter().enumerate() {
            let path = format!("block[{position}]");
            if block.name.is_empty() {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &path,
                    "block name is empty",
                ));
            }
            if !names.insert(block.name.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &path,
                    format!("duplicate block `{}`", block.name),
                ));
            }
            let name_len = u8::try_from(block.name.len()).map_err(|_| {
                Error::new(
                    ErrorKind::InvalidValue,
                    &path,
                    format!(
                        "name `{}` is {} bytes; the format carries at most {} name bytes",
                        block.name,
                        block.name.len(),
                        u8::MAX
                    ),
                )
            })?;
            let record_count = u16::try_from(block.offsets.len()).map_err(|_| {
                Error::new(
                    ErrorKind::InvalidValue,
                    &path,
                    format!(
                        "{} records exceed the `u16` per-block record count",
                        block.offsets.len()
                    ),
                )
            })?;
            validate_offsets(&path, &block.offsets)?;
            out.push(name_len);
            out.extend_from_slice(block.name.as_bytes());
            out.extend_from_slice(&record_count.to_le_bytes());
            for offset in &block.offsets {
                out.extend_from_slice(&offset.index.to_le_bytes());
                for value in offset.delta {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        Ok(out)
    }

    /// Number of offsets across every block.
    pub fn offset_count(&self) -> usize {
        self.blocks.iter().map(|block| block.offsets.len()).sum()
    }

    /// Highest vertex index any block addresses, if the container is not empty.
    pub fn max_index(&self) -> Option<u16> {
        self.blocks
            .iter()
            .flat_map(|block| block.offsets.iter())
            .map(|offset| offset.index)
            .max()
    }
}

impl MorphData {
    /// Read a morph data file, choosing the layout by its `.bsd` suffix.
    ///
    /// The reference decides the same way (`SliderSet.cpp:376-380`): a slider
    /// data file is bare exactly when its name ends in `.bsd`. Nothing is
    /// inferred from the bytes, so a truncated `.osd` can never be reinterpreted
    /// as a bare record set. The suffix comparison ignores ASCII case, which the
    /// reference's exact compare does not; it only widens an edge case a
    /// case-insensitive filesystem already produces.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|error| {
            Error::new(ErrorKind::Io, "morph data", error.to_string()).with_path(path)
        })?;
        let bare = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bsd"));
        let data = if bare {
            parse_bare_bsd(&bytes).map(Self::Bare)
        } else {
            MorphContainer::parse(&bytes).map(Self::Named)
        };
        data.map_err(|error| error.with_path(path))
    }

    /// Encode into the layout this value came from.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Named(container) => container.to_osd_bytes(),
            Self::Bare(offsets) => to_bsd_bytes(offsets),
        }
    }

    /// Offsets of the block named `name`.
    ///
    /// A bare record set stores no name, so it answers for every authored name
    /// the project reference supplies; only a named container can miss.
    pub fn offsets(&self, name: &str) -> Option<&[VertexOffset]> {
        match self {
            Self::Named(container) => container
                .blocks
                .iter()
                .find(|block| block.name == name)
                .map(|block| block.offsets.as_slice()),
            Self::Bare(offsets) => Some(offsets.as_slice()),
        }
    }

    /// Number of offsets in the file.
    pub fn offset_count(&self) -> usize {
        match self {
            Self::Named(container) => container.offset_count(),
            Self::Bare(offsets) => offsets.len(),
        }
    }

    /// Highest vertex index the file addresses, if it is not empty.
    pub fn max_index(&self) -> Option<u16> {
        match self {
            Self::Named(container) => container.max_index(),
            Self::Bare(offsets) => offsets.iter().map(|offset| offset.index).max(),
        }
    }
}

/// Decode a bare `.bsd` record set: `u32` count, then `u32`-index records.
///
/// The wire index is 32-bit and this crate's shared offset model is 16-bit, so
/// an index above [`u16::MAX`] is refused by name instead of being truncated the
/// way the reference does.
pub fn parse_bare_bsd(bytes: &[u8]) -> Result<Vec<VertexOffset>> {
    let mut cursor = Cursor::new(bytes);
    let record_count = cursor.u32("record count")? as usize;
    let record_bytes = record_count.checked_mul(BSD_RECORD_LEN).ok_or_else(|| {
        Error::new(
            ErrorKind::Malformed,
            "record count",
            "record count overflows",
        )
    })?;
    if record_bytes > cursor.remaining() {
        return Err(Error::new(
            ErrorKind::Malformed,
            "record count",
            format!(
                "{record_count} records need {record_bytes} bytes, {} left",
                cursor.remaining()
            ),
        ));
    }

    let mut offsets = Vec::with_capacity(record_count);
    for record in 0..record_count {
        let path = format!("record[{record}]");
        let wire = cursor.u32(&path)?;
        let index = u16::try_from(wire).map_err(|_| {
            Error::new(
                ErrorKind::InvalidValue,
                &path,
                format!(
                    "index {wire} exceeds the {} index width of the shared offset model; native truncates it, Sculpto refuses it",
                    u16::MAX
                ),
            )
        })?;
        let delta = cursor.delta(&path)?;
        offsets.push(VertexOffset { index, delta });
    }

    if cursor.remaining() != 0 {
        return Err(Error::new(
            ErrorKind::Malformed,
            "record set",
            format!(
                "{} trailing bytes after the last record",
                cursor.remaining()
            ),
        ));
    }
    reject_duplicate_indices("record set", &offsets)?;
    Ok(offsets)
}

/// Encode a bare `.bsd` record set: `u32` count, then `u32`-index records.
///
/// The caller supplies the record order; a `.bsd` file has no name, so nothing
/// here can invent one. Written indices are the `u16` model widened, and no
/// delta is snapped.
pub fn to_bsd_bytes(offsets: &[VertexOffset]) -> Result<Vec<u8>> {
    let record_count = u32::try_from(offsets.len()).map_err(|_| {
        Error::new(
            ErrorKind::InvalidValue,
            "record count",
            format!("{} records exceed the `u32` header count", offsets.len()),
        )
    })?;
    validate_offsets("record", offsets)?;
    let mut out = Vec::with_capacity(4 + offsets.len() * BSD_RECORD_LEN);
    out.extend_from_slice(&record_count.to_le_bytes());
    for offset in offsets {
        out.extend_from_slice(&u32::from(offset.index).to_le_bytes());
        for value in offset.delta {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(out)
}

/// Reject what both readers reject so a written file always reads back.
fn validate_offsets(what: &str, offsets: &[VertexOffset]) -> Result<()> {
    for offset in offsets {
        if let Some(value) = offset.delta.iter().find(|value| !value.is_finite()) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                what,
                format!(
                    "vertex {} has the non-finite offset component {value}",
                    offset.index
                ),
            ));
        }
    }
    reject_duplicate_indices(what, offsets)
}

fn reject_duplicate_indices(what: &str, offsets: &[VertexOffset]) -> Result<()> {
    let mut seen = BTreeSet::new();
    if let Some(duplicate) = offsets
        .iter()
        .map(|offset| offset.index)
        .find(|index| !seen.insert(*index))
    {
        return Err(Error::new(
            ErrorKind::InvalidValue,
            what,
            format!("vertex {duplicate} has more than one offset"),
        ));
    }
    Ok(())
}

/// Apply nifly's `clampEpsilon` to one read component.
fn snap_epsilon(value: f32) -> f32 {
    if value.abs() < RECORD_EPSILON {
        0.0
    } else {
        value
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }

    fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(len)
            .ok_or_else(|| Error::new(ErrorKind::Malformed, what, "length overflows"))?;
        let slice = self.data.get(self.position..end).ok_or_else(|| {
            Error::new(
                ErrorKind::Malformed,
                what,
                format!("needs {len} bytes, {} left", self.remaining()),
            )
        })?;
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self, what: &str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &str) -> Result<u16> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self, what: &str) -> Result<u32> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self, what: &str) -> Result<f32> {
        let bytes = self.take(4, what)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read, validate and snap one delta triple.
    fn delta(&mut self, what: &str) -> Result<[f32; 3]> {
        let raw = [self.f32(what)?, self.f32(what)?, self.f32(what)?];
        if let Some(value) = raw.iter().find(|value| !value.is_finite()) {
            return Err(Error::new(
                ErrorKind::InvalidValue,
                what,
                format!("offset component {value} is not finite"),
            ));
        }
        Ok(raw.map(snap_epsilon))
    }
}
