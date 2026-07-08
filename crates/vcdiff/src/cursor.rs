//! A bounds-checked byte cursor with RFC 3284 variable-length integer decoding.

use crate::error::DecodeError;

/// A forward cursor over a byte slice
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Wrap `bytes` in a cursor positioned at the start
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Whether every byte has been read
    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    /// The current absolute offset
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// Read a single byte, or error at end of input
    pub(crate) fn read_u8(&mut self) -> Result<u8, DecodeError> {
        let byte = *self.bytes.get(self.pos).ok_or(DecodeError::UnexpectedEof)?;
        self.pos += 1;
        Ok(byte)
    }

    /// Read a big-endian 4-byte integer
    pub(crate) fn read_u32_be(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.read_slice(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Borrow the next `len` bytes and advance past them
    pub(crate) fn read_slice(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DecodeError::IntegerOverflow)?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(DecodeError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    /// Decode an RFC 3284 base-128 big-endian variable-length integer
    pub(crate) fn read_varint(&mut self) -> Result<u64, DecodeError> {
        let mut value: u64 = 0;
        // a 64-bit value needs at most 10 base-128 bytes
        for _ in 0..10 {
            let byte = self.read_u8()?;
            if value > (u64::MAX >> 7) {
                return Err(DecodeError::IntegerOverflow);
            }
            value = (value << 7) | u64::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError::IntegerOverflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bytes_and_slices() {
        let mut c = Cursor::new(&[1, 2, 3, 4]);
        assert_eq!(c.read_u8().unwrap(), 1);
        assert_eq!(c.read_slice(2).unwrap(), &[2, 3]);
        assert_eq!(c.position(), 3);
        assert!(!c.is_empty());
        assert_eq!(c.read_u8().unwrap(), 4);
        assert!(c.is_empty());
    }

    #[test]
    fn read_past_end_errors() {
        let mut c = Cursor::new(&[1]);
        assert!(matches!(c.read_slice(2), Err(DecodeError::UnexpectedEof)));
        let mut c = Cursor::new(&[]);
        assert!(matches!(c.read_u8(), Err(DecodeError::UnexpectedEof)));
    }

    #[test]
    fn read_u32_be_is_big_endian() {
        let mut c = Cursor::new(&[0x00, 0x01, 0x02, 0x03]);
        assert_eq!(c.read_u32_be().unwrap(), 0x0001_0203);
    }

    #[test]
    fn varint_decodes_single_and_multi_byte() {
        assert_eq!(Cursor::new(&[0x00]).read_varint().unwrap(), 0);
        assert_eq!(Cursor::new(&[0x7F]).read_varint().unwrap(), 127);
        assert_eq!(Cursor::new(&[0x80, 0x00]).read_varint().unwrap(), 0);
        // RFC 3284 section 2 example: 123456789
        assert_eq!(
            Cursor::new(&[0xBA, 0xEF, 0x9A, 0x15])
                .read_varint()
                .unwrap(),
            123_456_789
        );
    }

    #[test]
    fn varint_overflow_is_rejected() {
        assert!(matches!(
            Cursor::new(&[0x80; 10]).read_varint(),
            Err(DecodeError::IntegerOverflow)
        ));
    }
}
