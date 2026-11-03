//! Random-access little-endian readers over a byte slice.
//!
//! The format parsers read fixed-offset fields out of subrecord payloads,
//! `.lod` / `.uvd` tables and DDS headers: `data.get(off..off + N)?` then
//! `from_le_bytes`. That shape had been re-grown locally about sixteen times
//! across the workspace (#5596), with the DDS copy indexing directly and
//! panicking on a truncated header. These helpers are the one home: they
//! return `None` past the end, and the offset arithmetic is checked, so an
//! `offset` near `usize::MAX` is a miss rather than an overflow.
//!
//! For sequential reads use the cursor types (`SubReader`, `NifStream`);
//! this module is for "give me the `u32` at byte 0x1C".

/// The `N` bytes at `offset`, or `None` if the slice is shorter.
#[inline]
fn le_bytes_at<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    let end = offset.checked_add(N)?;
    data.get(offset..end)?.try_into().ok()
}

/// Little-endian `u16` at `offset`.
#[inline]
pub fn le_u16_at(data: &[u8], offset: usize) -> Option<u16> {
    le_bytes_at(data, offset).map(u16::from_le_bytes)
}

/// Little-endian `u32` at `offset`.
#[inline]
pub fn le_u32_at(data: &[u8], offset: usize) -> Option<u32> {
    le_bytes_at(data, offset).map(u32::from_le_bytes)
}

/// Little-endian `f32` at `offset`. Any bit pattern is returned, NaN and
/// infinities included; callers that need a finite value filter it.
#[inline]
pub fn le_f32_at(data: &[u8], offset: usize) -> Option<f32> {
    le_bytes_at(data, offset).map(f32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_at_an_offset() {
        let data = [0xAA, 0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(le_u16_at(&data, 1), Some(0x0201));
        assert_eq!(le_u32_at(&data, 1), Some(0x0403_0201));
        let f = 1.5f32.to_le_bytes();
        assert_eq!(le_f32_at(&[0, f[0], f[1], f[2], f[3]], 1), Some(1.5));
    }

    #[test]
    fn a_read_past_the_end_is_a_miss_not_a_panic() {
        let data = [0u8; 4];
        assert_eq!(le_u32_at(&data, 0), Some(0));
        assert_eq!(le_u32_at(&data, 1), None, "straddles the end");
        assert_eq!(le_u16_at(&data, 3), None);
        assert_eq!(le_u32_at(&[], 0), None);
        assert_eq!(le_f32_at(&data, 4), None);
    }

    #[test]
    fn an_offset_that_overflows_is_a_miss() {
        let data = [0u8; 8];
        assert_eq!(le_u32_at(&data, usize::MAX), None);
        assert_eq!(le_u16_at(&data, usize::MAX - 1), None);
    }

    #[test]
    fn non_finite_floats_are_returned_unfiltered() {
        let nan = f32::NAN.to_le_bytes();
        assert!(le_f32_at(&nan, 0).unwrap().is_nan());
    }
}
