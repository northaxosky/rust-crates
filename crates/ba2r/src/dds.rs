//! Reconstruct a Direct3D DDS file header for a Fallout 4 BA2 DX10 texture.
//!
//! Fallout 4 stores textures as raw DXGI mip data with the pixel format captured as a DXGI enum
//! value, so extraction wraps that data in a standard DDS: a 4-byte magic, a 124-byte `DDS_HEADER`,
//! and a 20-byte `DDS_HEADER_DXT10` carrying the exact DXGI format. The extension header is always
//! emitted so every format (sRGB, BC6H/BC7, and the uncompressed ones) round-trips losslessly.

const DDS_MAGIC: u32 = 0x2053_4444;
const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PITCH: u32 = 0x8;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSD_LINEARSIZE: u32 = 0x8_0000;
const DDPF_FOURCC: u32 = 0x4;
const FOURCC_DX10: u32 = 0x3031_5844;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;
const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0xFE00;
const DIMENSION_TEXTURE2D: u32 = 3;
const RESOURCE_MISC_TEXTURECUBE: u32 = 0x4;

/// Build the 148-byte DDS header (magic, `DDS_HEADER`, `DDS_HEADER_DXT10`) for a BA2 texture.
pub(crate) fn header(width: u32, height: u32, mips: u32, dxgi: u32, cubemap: bool) -> Vec<u8> {
    let mips = mips.max(1);
    let (pitch_or_linear, size_flag) = pitch_or_linear_size(width, height, dxgi);

    let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT | size_flag;
    if mips > 1 {
        flags |= DDSD_MIPMAPCOUNT;
    }

    let mut caps = DDSCAPS_TEXTURE;
    if mips > 1 {
        caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
    }
    if cubemap {
        caps |= DDSCAPS_COMPLEX;
    }
    let caps2 = if cubemap {
        DDSCAPS2_CUBEMAP_ALLFACES
    } else {
        0
    };
    let misc_flag = if cubemap {
        RESOURCE_MISC_TEXTURECUBE
    } else {
        0
    };

    let mut out = Vec::with_capacity(148);
    put_u32(&mut out, DDS_MAGIC);
    put_u32(&mut out, 124);
    put_u32(&mut out, flags);
    put_u32(&mut out, height);
    put_u32(&mut out, width);
    put_u32(&mut out, pitch_or_linear);
    put_u32(&mut out, 0);
    put_u32(&mut out, mips);
    for _ in 0..11 {
        put_u32(&mut out, 0);
    }
    put_u32(&mut out, 32);
    put_u32(&mut out, DDPF_FOURCC);
    put_u32(&mut out, FOURCC_DX10);
    for _ in 0..5 {
        put_u32(&mut out, 0);
    }
    put_u32(&mut out, caps);
    put_u32(&mut out, caps2);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, dxgi);
    put_u32(&mut out, DIMENSION_TEXTURE2D);
    put_u32(&mut out, misc_flag);
    put_u32(&mut out, 1);
    put_u32(&mut out, 0);
    debug_assert_eq!(out.len(), 148);
    out
}

/// Append a little-endian u32 to `out`.
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Compute `dwPitchOrLinearSize` and its header flag, or `(0, 0)` for an unclassified format.
fn pitch_or_linear_size(width: u32, height: u32, dxgi: u32) -> (u32, u32) {
    if let Some(block) = block_bytes(dxgi) {
        let blocks_w = u64::from(width.div_ceil(4).max(1));
        let blocks_h = u64::from(height.div_ceil(4).max(1));
        if let Ok(size) = u32::try_from(blocks_w * blocks_h * u64::from(block)) {
            return (size, DDSD_LINEARSIZE);
        }
    } else if let Some(bits) = bits_per_pixel(dxgi) {
        if let Ok(pitch) = u32::try_from((u64::from(width) * u64::from(bits)).div_ceil(8)) {
            return (pitch, DDSD_PITCH);
        }
    }
    (0, 0)
}

/// Bytes per 4x4 block for block-compressed DXGI formats, or `None` otherwise.
fn block_bytes(dxgi: u32) -> Option<u32> {
    match dxgi {
        70..=72 | 79..=81 => Some(8),
        73..=78 | 82..=84 | 94..=99 => Some(16),
        _ => None,
    }
}

/// Bits per pixel for the uncompressed DXGI formats Fallout 4 ships, or `None` if unknown.
fn bits_per_pixel(dxgi: u32) -> Option<u32> {
    match dxgi {
        23..=38 | 87..=93 => Some(32),
        48..=59 | 85 | 86 => Some(16),
        60..=65 => Some(8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32le(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    // The worked example from the DDS reconstruction spec: 1024x1024 BC7_UNORM_SRGB, 11 mips.
    #[test]
    fn builds_the_expected_bc7_header() {
        let h = header(1024, 1024, 11, 99, false);
        assert_eq!(h.len(), 148);
        assert_eq!(&h[0..4], b"DDS ");
        assert_eq!(u32le(&h, 0x04), 124);
        assert_eq!(u32le(&h, 0x08), 0x000A_1007);
        assert_eq!(u32le(&h, 0x0C), 1024);
        assert_eq!(u32le(&h, 0x10), 1024);
        assert_eq!(u32le(&h, 0x14), 0x0010_0000);
        assert_eq!(u32le(&h, 0x1C), 11);
        assert_eq!(u32le(&h, 0x4C), 32);
        assert_eq!(u32le(&h, 0x50), DDPF_FOURCC);
        assert_eq!(&h[0x54..0x58], b"DX10");
        assert_eq!(u32le(&h, 0x6C), 0x0040_1008);
        assert_eq!(u32le(&h, 0x70), 0);
        assert_eq!(u32le(&h, 0x80), 99);
        assert_eq!(u32le(&h, 0x84), 3);
        assert_eq!(u32le(&h, 0x88), 0);
        assert_eq!(u32le(&h, 0x8C), 1);
    }

    #[test]
    fn marks_a_cubemap() {
        let h = header(16, 16, 1, 98, true);
        assert_eq!(u32le(&h, 0x70), DDSCAPS2_CUBEMAP_ALLFACES);
        assert_eq!(u32le(&h, 0x88), RESOURCE_MISC_TEXTURECUBE);
        assert_eq!(u32le(&h, 0x6C) & DDSCAPS_COMPLEX, DDSCAPS_COMPLEX);
    }

    #[test]
    fn uses_pitch_for_uncompressed_and_falls_back_for_unknown() {
        let h = header(8, 1, 1, 28, false);
        assert_eq!(u32le(&h, 0x14), 32);
        assert_eq!(u32le(&h, 0x08) & DDSD_PITCH, DDSD_PITCH);
        let u = header(8, 8, 1, 250, false);
        assert_eq!(u32le(&u, 0x14), 0);
        assert_eq!(u32le(&u, 0x08) & (DDSD_PITCH | DDSD_LINEARSIZE), 0);
    }
}
