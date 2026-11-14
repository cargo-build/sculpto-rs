//! SHA-256 of files and byte slices, lowercase hex.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{BlenderError, BlenderErrorKind, Result};

/// Digest one file without loading it into memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "digest_open",
            format!("{}: {error}", path.display()),
        )
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            BlenderError::new(
                BlenderErrorKind::Io,
                "digest_read",
                format!("{}: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Digest a byte slice.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}
