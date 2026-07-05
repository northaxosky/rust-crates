//! Byte-layout constants and low-level serialization primitives.

use crate::error::WriteError;
use crate::win1252::encode_win1252;

pub(crate) const TES4: &[u8; 4] = b"TES4";
pub(crate) const HEDR: &[u8; 4] = b"HEDR";
pub(crate) const CNAM: &[u8; 4] = b"CNAM";
pub(crate) const SNAM: &[u8; 4] = b"SNAM";
pub(crate) const MAST: &[u8; 4] = b"MAST";
pub(crate) const DATA: &[u8; 4] = b"DATA";
pub(crate) const GRUP: &[u8; 4] = b"GRUP";
pub(crate) const XXXX: &[u8; 4] = b"XXXX";

pub(crate) const FLAG_MASTER: u32 = 0x0000_0001;
pub(crate) const FLAG_COMPRESSED: u32 = 0x0004_0000;
pub(crate) const RECORD_HEADER_LEN: usize = 24;

/// Append a field: 4-byte signature, u16 payload length, then the payload
pub(crate) fn write_field(out: &mut Vec<u8>, sig: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(payload);
}

/// Append the 12-byte `HEDR` field: version float, record count, next object ID
pub(crate) fn write_hedr(out: &mut Vec<u8>, version: f32, num_records: u32, next_object_id: u32) {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&version.to_le_bytes());
    payload[4..8].copy_from_slice(&num_records.to_le_bytes());
    payload[8..12].copy_from_slice(&next_object_id.to_le_bytes());
    write_field(out, HEDR, &payload);
}

/// Append a NUL-terminated Windows-1252 zstring field, naming it for error context
pub(crate) fn write_string_field(
    out: &mut Vec<u8>,
    sig: &[u8; 4],
    field: &'static str,
    value: &str,
) -> Result<(), WriteError> {
    if value.contains('\0') {
        return Err(WriteError::InteriorNul { field });
    }
    let mut payload = encode_win1252(value, field)?;
    payload.push(0);
    if payload.len() > u16::MAX as usize {
        return Err(WriteError::StringTooLong {
            field,
            len: payload.len(),
        });
    }
    write_field(out, sig, &payload);
    Ok(())
}

/// Append the 24-byte record header preceding `data_size` bytes of payload
pub(crate) fn write_record_header(
    out: &mut Vec<u8>,
    sig: &[u8; 4],
    data_size: u32,
    flags: u32,
    form_id: u32,
    form_version: u16,
) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&form_id.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
    out.extend_from_slice(&0u16.to_le_bytes()); // version control info
    out.extend_from_slice(&form_version.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_field_layout() {
        let mut out = Vec::new();
        write_field(&mut out, b"EDID", b"hi");
        assert_eq!(&out[0..4], b"EDID");
        assert_eq!(u16::from_le_bytes([out[4], out[5]]), 2);
        assert_eq!(&out[6..8], b"hi");
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn write_hedr_layout() {
        let mut out = Vec::new();
        write_hedr(&mut out, 1.0, 3, 7);
        assert_eq!(&out[0..4], b"HEDR");
        assert_eq!(u16::from_le_bytes([out[4], out[5]]), 12);
        assert_eq!(f32::from_le_bytes([out[6], out[7], out[8], out[9]]), 1.0);
        assert_eq!(u32::from_le_bytes([out[10], out[11], out[12], out[13]]), 3);
        assert_eq!(u32::from_le_bytes([out[14], out[15], out[16], out[17]]), 7);
        assert_eq!(out.len(), 18);
    }

    #[test]
    fn write_record_header_layout() {
        let mut out = Vec::new();
        write_record_header(&mut out, b"KYWD", 0x11, 0x22, 0x33, 0x44);
        assert_eq!(&out[0..4], b"KYWD");
        assert_eq!(u32::from_le_bytes([out[4], out[5], out[6], out[7]]), 0x11);
        assert_eq!(u32::from_le_bytes([out[8], out[9], out[10], out[11]]), 0x22);
        assert_eq!(
            u32::from_le_bytes([out[12], out[13], out[14], out[15]]),
            0x33
        );
        assert_eq!(u16::from_le_bytes([out[16], out[17]]), 0); // timestamp
        assert_eq!(u16::from_le_bytes([out[18], out[19]]), 0); // version control info
        assert_eq!(u16::from_le_bytes([out[20], out[21]]), 0x44); // formVersion
        assert_eq!(u16::from_le_bytes([out[22], out[23]]), 0); // unknown
        assert_eq!(out.len(), 24);
    }

    #[test]
    fn write_string_field_is_nul_terminated() {
        let mut out = Vec::new();
        write_string_field(&mut out, b"CNAM", "author", "Kuz").unwrap();
        assert_eq!(&out[0..4], b"CNAM");
        assert_eq!(u16::from_le_bytes([out[4], out[5]]), 4);
        assert_eq!(&out[6..10], b"Kuz\0");
    }

    #[test]
    fn write_string_field_rejects_interior_nul() {
        let mut out = Vec::new();
        let err = write_string_field(&mut out, b"CNAM", "author", "a\0b").unwrap_err();
        assert!(matches!(err, WriteError::InteriorNul { field: "author" }));
    }

    #[test]
    fn write_string_field_rejects_overlong() {
        let mut out = Vec::new();
        let long = "a".repeat(70_000);
        let err = write_string_field(&mut out, b"CNAM", "author", &long).unwrap_err();
        assert!(matches!(
            err,
            WriteError::StringTooLong {
                field: "author",
                ..
            }
        ));
    }
}
