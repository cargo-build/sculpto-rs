//! Outfit Studio morph containers (`.osd` / `.bsd`).
//!
//! A container holds one block per shape: a name and sparse per-vertex offsets.
//! Layout: 4-byte magic, `u32` version, `u32` block count, then per block a
//! length-prefixed name, a `u16` record count and fixed 14-byte records (`u16`
//! vertex index, three `f32` deltas). The walk must end exactly at EOF.
//!
//! A record index addresses a position in the shape that owns the container, so
//! range validation against a concrete mesh belongs to the bake plan, not to
//! this decoder.

use std::fs;
use std::path::Path;

use crate::error::{Error, ErrorKind, Result};

/// Container magic, the first four bytes of every `.osd`/`.bsd`.
pub const MAGIC: [u8; 4] = *b"\x00DSO";

/// The only container version this crate reads.
pub const SUPPORTED_VERSION: u32 = 1;

/// Fixed record width: `u16` index plus three `f32` deltas.
const RECORD_LEN: usize = 14;

/// A decoded `.osd`/`.bsd` container.
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

/// One sparse vertex offset inside a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexOffset {
    /// Position in the source shape's vertex array.
    pub index: u16,
    /// Delta applied at that vertex, in the shape's units.
    pub delta: [f32; 3],
}

impl MorphContainer {
    /// Decode a container from bytes.
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
                .checked_mul(RECORD_LEN)
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
                let index = cursor.u16(&format!("{path}.record[{record}]"))?;
                let x = cursor.f32(&path)?;
                let y = cursor.f32(&path)?;
                let z = cursor.f32(&path)?;
                offsets.push(VertexOffset {
                    index,
                    delta: [x, y, z],
                });
            }
            if blocks.iter().any(|block: &MorphBlock| block.name == name) {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    &path,
                    format!("duplicate block `{name}`"),
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            if let Some(duplicate) = offsets
                .iter()
                .map(|offset| offset.index)
                .find(|index| !seen.insert(*index))
            {
                return Err(Error::new(
                    ErrorKind::InvalidValue,
                    format!("{path}[{name}]"),
                    format!("vertex {duplicate} has more than one offset"),
                ));
            }
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

    /// Decode a container from disk.
    pub fn parse_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|error| {
            Error::new(ErrorKind::Io, "container", error.to_string()).with_path(path)
        })?;
        Self::parse(&bytes).map_err(|error| error.with_path(path))
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
}
