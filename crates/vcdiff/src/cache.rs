//! The RFC 3284 SAME/NEAR address cache for COPY instructions.

use crate::code_table::{NEAR_COUNT, SAME_COUNT};
use crate::error::DecodeError;
use crate::input::SliceCursor;

/// The number of same-cache entries: one block of 256 per same slot
const SAME_SIZE: usize = SAME_COUNT * 256;

/// The address cache that resolves COPY addresses across the address modes
pub(crate) struct AddressCache {
    near: [u64; NEAR_COUNT],
    same: [u64; SAME_SIZE],
    next_near: usize,
}

impl AddressCache {
    /// A cache with every slot cleared, as required at the start of each window
    pub(crate) fn new() -> Self {
        Self {
            near: [0; NEAR_COUNT],
            same: [0; SAME_SIZE],
            next_near: 0,
        }
    }

    /// Decode a COPY address for `mode` at output position `here`, updating the cache
    pub(crate) fn decode(
        &mut self,
        mode: u8,
        here: u64,
        addr: &mut SliceCursor<'_>,
    ) -> Result<u64, DecodeError> {
        let same_start = 2 + NEAR_COUNT;
        let address = if mode == 0 {
            addr.read_varint()?
        } else if mode == 1 {
            let distance = addr.read_varint()?;
            if distance > here {
                return Err(DecodeError::AddressOutOfBounds {
                    address: distance,
                    here,
                    context: addr.context(),
                });
            }
            here - distance
        } else if (mode as usize) < same_start {
            let slot = mode as usize - 2;
            let offset = addr.read_varint()?;
            self.near[slot]
                .checked_add(offset)
                .ok_or_else(|| DecodeError::ArithmeticOverflow {
                    context: addr.context(),
                })?
        } else if (mode as usize) < same_start + SAME_COUNT {
            let block = mode as usize - same_start;
            let byte = addr.read_u8()? as usize;
            self.same[block * 256 + byte]
        } else {
            return Err(DecodeError::InvalidAddressMode {
                mode,
                context: addr.context(),
            });
        };
        self.update(address);
        Ok(address)
    }

    /// Record `address` in the near and same caches after a COPY
    fn update(&mut self, address: u64) {
        self.near[self.next_near] = address;
        self.next_near = (self.next_near + 1) % NEAR_COUNT;
        self.same[(address % SAME_SIZE as u64) as usize] = address;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SectionKind;

    fn addresses(bytes: &[u8]) -> SliceCursor<'_> {
        SliceCursor::new(bytes, 0, 0, SectionKind::Addresses)
    }

    #[test]
    fn resolves_every_mode_and_updates_the_cache() {
        let mut cache = AddressCache::new();

        // SELF: raw varint address
        assert_eq!(cache.decode(0, 100, &mut addresses(&[0x05])).unwrap(), 5);
        // NEAR[0] is now 5; add offset 3
        assert_eq!(cache.decode(2, 100, &mut addresses(&[0x03])).unwrap(), 8);
        // HERE: here - distance
        assert_eq!(cache.decode(1, 100, &mut addresses(&[0x0A])).unwrap(), 90);
        // SAME[0]: same[5] was set to 5 by the first COPY
        assert_eq!(cache.decode(6, 100, &mut addresses(&[0x05])).unwrap(), 5);
    }

    #[test]
    fn here_beyond_output_is_rejected() {
        let mut cache = AddressCache::new();
        assert!(matches!(
            cache.decode(1, 5, &mut addresses(&[0x0A])),
            Err(DecodeError::AddressOutOfBounds { .. })
        ));
    }

    #[test]
    fn mode_past_the_table_is_rejected() {
        let mut cache = AddressCache::new();
        assert!(matches!(
            cache.decode(9, 100, &mut addresses(&[0x00])),
            Err(DecodeError::InvalidAddressMode { mode: 9, .. })
        ));
    }
}
