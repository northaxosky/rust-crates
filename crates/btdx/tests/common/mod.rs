//! Shared helpers for the btdx integration tests.
//!
//! These build valid inputs and read fields using only the public crate API, so the integration
//! tests exercise btdx exactly as a downstream consumer would. The DDS builder re-derives the mip
//! chain size independently of the crate, giving the round-trip tests an independent size oracle.

/// Read a little-endian u16 at offset `o`
pub fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// Read a little-endian u32 at offset `o`
pub fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Read a little-endian u64 at offset `o`
pub fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Block byte size for the block-compressed DXGI formats the tests use
pub fn block_bytes(dxgi: u32) -> u32 {
    match dxgi {
        70..=72 | 79..=81 => 8,
        73..=78 | 82..=84 | 94..=99 => 16,
        other => panic!("test helper does not size DXGI format {other}"),
    }
}

/// Total byte size of a texture's mip chain for a block-compressed DXGI format
pub fn mip_chain_size(width: u32, height: u32, mips: u32, dxgi: u32, cubemap: bool) -> usize {
    let block = block_bytes(dxgi);
    let faces: u32 = if cubemap { 6 } else { 1 };
    let mut total = 0u32;
    for m in 0..mips.max(1) {
        let w = (width >> m).max(1);
        let h = (height >> m).max(1);
        total += w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * block;
    }
    (total * faces) as usize
}

/// Build a valid DX10-extended DDS for a block-compressed format, with deterministic pixel bytes
pub fn dx10_dds(width: u32, height: u32, mips: u32, dxgi: u32, cubemap: bool) -> Vec<u8> {
    let mips = mips.max(1);
    let block = block_bytes(dxgi);
    let linear = width.div_ceil(4).max(1) * height.div_ceil(4).max(1) * block;
    let flags: u32 = 0x1007 | 0x8_0000 | if mips > 1 { 0x2_0000 } else { 0 };
    let caps: u32 =
        0x1000 | if mips > 1 { 0x8 | 0x40_0000 } else { 0 } | if cubemap { 0x8 } else { 0 };
    let caps2: u32 = if cubemap { 0xFE00 } else { 0 };
    let misc: u32 = if cubemap { 0x4 } else { 0 };
    let mut words = [0u32; 37];
    words[0] = 0x2053_4444;
    words[1] = 124;
    words[2] = flags;
    words[3] = height;
    words[4] = width;
    words[5] = linear;
    words[7] = mips;
    words[19] = 32;
    words[20] = 0x4;
    words[21] = 0x3031_5844;
    words[27] = caps;
    words[28] = caps2;
    words[32] = dxgi;
    words[33] = 3;
    words[34] = misc;
    words[35] = 1;
    let mut out = Vec::with_capacity(148);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    let payload = mip_chain_size(width, height, mips, dxgi, cubemap);
    out.extend((0..payload).map(|i| (i * 7 + 1) as u8));
    out
}
