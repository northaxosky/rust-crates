//! Reconstruct a Direct3D DDS file header for a Fallout 4 BA2 DX10 texture.
//!
//! Fallout 4 stores textures as raw DXGI mip data with the pixel format captured as a DXGI enum
//! value, so extraction wraps that data in a standard DDS: a 4-byte magic, a 124-byte `DDS_HEADER`,
//! and a 20-byte `DDS_HEADER_DXT10` carrying the exact DXGI format. The extension header is always
//! emitted so every format (sRGB, BC6H/BC7, and the uncompressed ones) round-trips losslessly.

use crate::error::DdsError;

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

/// A parsed DDS texture ready to pack: dimensions, DXGI format, cubemap flag, and mip data
pub(crate) struct ParsedTexture<'a> {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) mip_count: u8,
    pub(crate) dxgi: u8,
    pub(crate) cubemap: bool,
    pub(crate) data: &'a [u8],
}

const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;
const DDPF_RGB: u32 = 0x40;
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_LUMINANCE: u32 = 0x2_0000;

/// Parse a DDS file into the fields a DX10 archive record needs, validating shape and exact size
pub(crate) fn parse(bytes: &[u8]) -> Result<ParsedTexture<'_>, DdsError> {
    if bytes.len() < 4 || &bytes[0..4] != b"DDS " {
        return Err(DdsError::NotDds);
    }
    if bytes.len() < 128 {
        return Err(DdsError::Truncated);
    }
    if u32le(bytes, 0x04) != 124 || u32le(bytes, 0x4C) != 32 {
        return Err(DdsError::UnsupportedShape("malformed DDS header"));
    }

    let header_flags = u32le(bytes, 0x08);
    let height = u32le(bytes, 0x0C);
    let width = u32le(bytes, 0x10);
    let mip_raw = u32le(bytes, 0x1C);
    let pf_flags = u32le(bytes, 0x50);
    let caps2 = u32le(bytes, 0x70);

    let is_dx10 = pf_flags & DDPF_FOURCC != 0 && &bytes[0x54..0x58] == b"DX10";
    let (dxgi, cubemap, data_start) = if is_dx10 {
        if bytes.len() < 148 {
            return Err(DdsError::Truncated);
        }
        let dxgi = u32le(bytes, 0x80);
        let dimension = u32le(bytes, 0x84);
        let misc = u32le(bytes, 0x88);
        let array_size = u32le(bytes, 0x8C);
        if dimension != 3 {
            return Err(DdsError::UnsupportedShape("not a 2D texture"));
        }
        if array_size != 1 {
            return Err(DdsError::UnsupportedShape(
                "texture arrays are not supported",
            ));
        }
        if dxgi == 0 || dxgi > u32::from(u8::MAX) {
            return Err(DdsError::UnsupportedShape("DXGI format out of range"));
        }
        (dxgi, misc & 0x4 != 0, 148usize)
    } else {
        if caps2 & DDSCAPS2_VOLUME != 0 {
            return Err(DdsError::UnsupportedShape(
                "volume textures are not supported",
            ));
        }
        let dxgi =
            legacy_format(bytes).ok_or(DdsError::UnsupportedShape("unrecognized legacy format"))?;
        (dxgi, caps2 & DDSCAPS2_CUBEMAP != 0, 128usize)
    };
    let dxgi = dxgi as u8;

    let mip_count = if header_flags & DDSD_MIPMAPCOUNT == 0 || mip_raw == 0 {
        1
    } else {
        mip_raw
    };
    let width =
        u16::try_from(width).map_err(|_| DdsError::UnsupportedShape("width exceeds 65535"))?;
    let height =
        u16::try_from(height).map_err(|_| DdsError::UnsupportedShape("height exceeds 65535"))?;
    let mip_count =
        u8::try_from(mip_count).map_err(|_| DdsError::UnsupportedShape("too many mips"))?;
    if width == 0 || height == 0 {
        return Err(DdsError::UnsupportedShape("zero dimension"));
    }
    let max_mips = 32 - u32::from(width).max(u32::from(height)).leading_zeros();
    if u32::from(mip_count) > max_mips {
        return Err(DdsError::UnsupportedShape(
            "mip count exceeds the maximum for these dimensions",
        ));
    }
    if mip_size(u32::from(width), u32::from(height), u32::from(dxgi)).is_none() {
        return Err(DdsError::UnsupportedFormat(dxgi));
    }

    let faces = if cubemap { 6u64 } else { 1 };
    let mut expected = 0u64;
    for m in 0..u32::from(mip_count) {
        let mw = (u32::from(width) >> m).max(1);
        let mh = (u32::from(height) >> m).max(1);
        expected += mip_size(mw, mh, u32::from(dxgi)).unwrap_or(0);
    }
    let expected = usize::try_from(expected * faces)
        .map_err(|_| DdsError::UnsupportedShape("texture too large"))?;

    let data = &bytes[data_start..];
    if data.len() != expected {
        return Err(DdsError::SizeMismatch);
    }
    Ok(ParsedTexture {
        width,
        height,
        mip_count,
        dxgi,
        cubemap,
        data,
    })
}

/// Byte size of one mip level, or `None` if the DXGI format cannot be sized
pub(crate) fn mip_size(width: u32, height: u32, dxgi: u32) -> Option<u64> {
    if let Some(block) = block_bytes(dxgi) {
        let bw = u64::from(width.div_ceil(4).max(1));
        let bh = u64::from(height.div_ceil(4).max(1));
        Some(bw * bh * u64::from(block))
    } else {
        let bits = bits_per_pixel(dxgi)?;
        Some((u64::from(width) * u64::from(bits)).div_ceil(8) * u64::from(height))
    }
}

/// Map a legacy (non-DX10) DDS pixel format to a DXGI value, or `None` if unrecognized
fn legacy_format(bytes: &[u8]) -> Option<u32> {
    let pf_flags = u32le(bytes, 0x50);
    if pf_flags & DDPF_FOURCC != 0 {
        return match &bytes[0x54..0x58] {
            b"DXT1" => Some(71),
            b"DXT3" => Some(74),
            b"DXT5" => Some(77),
            b"ATI1" | b"BC4U" => Some(80),
            b"BC4S" => Some(81),
            b"ATI2" | b"BC5U" => Some(83),
            b"BC5S" => Some(84),
            _ => None,
        };
    }
    let bits = u32le(bytes, 0x58);
    let (r, g, b, a) = (
        u32le(bytes, 0x5C),
        u32le(bytes, 0x60),
        u32le(bytes, 0x64),
        u32le(bytes, 0x68),
    );
    if pf_flags & DDPF_RGB != 0 && bits == 32 {
        if r == 0x00FF_0000 && g == 0x0000_FF00 && b == 0x0000_00FF {
            let opaque = pf_flags & DDPF_ALPHAPIXELS == 0 || a == 0;
            return Some(if opaque { 88 } else { 87 });
        }
        if r == 0x0000_00FF && g == 0x0000_FF00 && b == 0x00FF_0000 {
            return Some(28);
        }
    }
    if pf_flags & DDPF_RGB != 0 && bits == 16 && r == 0xF800 && g == 0x07E0 && b == 0x001F {
        return Some(85);
    }
    if pf_flags & (DDPF_RGB | DDPF_LUMINANCE) != 0 && bits == 8 && r == 0xFF {
        return Some(61);
    }
    None
}

/// Read a little-endian u32 at offset `o`
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

    fn put_le(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    // A minimal legacy (non-DX10) DDS header with a DXT1 four-CC.
    fn legacy_dxt1(width: u32, height: u32) -> Vec<u8> {
        let mut h = vec![0u8; 128];
        h[0..4].copy_from_slice(b"DDS ");
        put_le(&mut h, 0x04, 124);
        put_le(&mut h, 0x08, 0x1007);
        put_le(&mut h, 0x0C, height);
        put_le(&mut h, 0x10, width);
        put_le(&mut h, 0x4C, 32);
        put_le(&mut h, 0x50, DDPF_FOURCC);
        h[0x54..0x58].copy_from_slice(b"DXT1");
        h
    }

    #[test]
    fn parses_a_dx10_dds() {
        let mut dds = header(8, 8, 1, 98, false);
        dds.extend_from_slice(&[0u8; 64]);
        let t = parse(&dds).unwrap();
        assert_eq!(
            (t.width, t.height, t.mip_count, t.dxgi, t.cubemap),
            (8, 8, 1, 98, false)
        );
        assert_eq!(t.data.len(), 64);
    }

    #[test]
    fn parses_a_legacy_dxt1_dds() {
        let mut dds = legacy_dxt1(8, 8);
        dds.extend_from_slice(&[0u8; 32]);
        let t = parse(&dds).unwrap();
        assert_eq!((t.width, t.height, t.dxgi), (8, 8, 71));
    }

    #[test]
    fn rejects_bad_truncated_and_mismatched_dds() {
        assert!(matches!(parse(b"NOPE"), Err(DdsError::NotDds)));
        assert!(matches!(parse(b"DDS "), Err(DdsError::Truncated)));
        let mut dds = header(8, 8, 1, 98, false);
        dds.extend_from_slice(&[0u8; 10]);
        assert!(matches!(parse(&dds), Err(DdsError::SizeMismatch)));
    }

    #[test]
    fn rejects_impossible_mip_count() {
        let dds = header(8, 8, 40, 71, false);
        assert!(matches!(parse(&dds), Err(DdsError::UnsupportedShape(_))));
    }

    #[test]
    fn rejects_a_volume_dds() {
        let mut dds = legacy_dxt1(8, 8);
        put_le(&mut dds, 0x70, 0x20_0000);
        dds.extend_from_slice(&[0u8; 32]);
        assert!(matches!(parse(&dds), Err(DdsError::UnsupportedShape(_))));
    }

    fn legacy_fourcc(cc: &[u8; 4], width: u32, height: u32) -> Vec<u8> {
        let mut h = legacy_dxt1(width, height);
        h[0x54..0x58].copy_from_slice(cc);
        h
    }

    #[test]
    fn maps_legacy_fourcc_formats() {
        for (cc, dxgi) in [(b"DXT3", 74u8), (b"DXT5", 77), (b"ATI2", 83), (b"BC5S", 84)] {
            let mut dds = legacy_fourcc(cc, 8, 8);
            dds.extend_from_slice(&[0u8; 64]);
            assert_eq!(parse(&dds).unwrap().dxgi, dxgi, "cc={cc:?}");
        }
        for (cc, dxgi) in [(b"ATI1", 80u8), (b"BC4U", 80)] {
            let mut dds = legacy_fourcc(cc, 8, 8);
            dds.extend_from_slice(&[0u8; 32]);
            assert_eq!(parse(&dds).unwrap().dxgi, dxgi, "cc={cc:?}");
        }
    }

    #[test]
    fn maps_legacy_uncompressed_bgra() {
        let mut h = vec![0u8; 128];
        h[0..4].copy_from_slice(b"DDS ");
        put_le(&mut h, 0x04, 124);
        put_le(&mut h, 0x08, 0x1007);
        put_le(&mut h, 0x0C, 4);
        put_le(&mut h, 0x10, 4);
        put_le(&mut h, 0x4C, 32);
        put_le(&mut h, 0x50, 0x41);
        put_le(&mut h, 0x58, 32);
        put_le(&mut h, 0x5C, 0x00FF_0000);
        put_le(&mut h, 0x60, 0x0000_FF00);
        put_le(&mut h, 0x64, 0x0000_00FF);
        put_le(&mut h, 0x68, 0xFF00_0000);
        h.extend_from_slice(&[0u8; 64]);
        assert_eq!(parse(&h).unwrap().dxgi, 87);
    }

    #[test]
    fn parses_a_dx10_cubemap() {
        let mut dds = header(8, 8, 1, 98, true);
        dds.extend_from_slice(&vec![0u8; 6 * 64]);
        let t = parse(&dds).unwrap();
        assert!(t.cubemap);
        assert_eq!(t.data.len(), 6 * 64);
    }

    #[test]
    fn rejects_arrays_and_3d() {
        let mut arr = header(8, 8, 1, 98, false);
        put_le(&mut arr, 0x8C, 2);
        arr.extend_from_slice(&[0u8; 64]);
        assert!(matches!(parse(&arr), Err(DdsError::UnsupportedShape(_))));
        let mut vol = header(8, 8, 1, 98, false);
        put_le(&mut vol, 0x84, 4);
        vol.extend_from_slice(&[0u8; 64]);
        assert!(matches!(parse(&vol), Err(DdsError::UnsupportedShape(_))));
    }

    #[test]
    fn mip_size_matches_the_formula() {
        assert_eq!(mip_size(1024, 1024, 71), Some(0x80000));
        assert_eq!(mip_size(8, 8, 98), Some(64));
        assert_eq!(mip_size(4, 1, 28), Some(16));
        assert_eq!(mip_size(8, 8, 250), None);
    }

    #[test]
    fn mip_size_sizes_uncompressed_formats() {
        assert_eq!(mip_size(8, 8, 85), Some(128));
        assert_eq!(mip_size(8, 8, 61), Some(64));
    }

    #[test]
    fn parses_the_block_format_matrix() {
        for (dxgi, block) in [
            (71u32, 8u64),
            (74, 16),
            (77, 16),
            (80, 8),
            (83, 16),
            (98, 16),
        ] {
            let mut dds = header(16, 16, 1, dxgi, false);
            dds.extend(vec![0u8; (16 * block) as usize]);
            let t = parse(&dds).unwrap();
            assert_eq!(t.dxgi, dxgi as u8);
            assert_eq!((t.width, t.height, t.mip_count), (16, 16, 1));
        }
    }

    // Build a DDS-shaped blob: valid magic and sizes, random dimensions/format/payload.
    fn arbitrary_dds() -> impl Strategy<Value = Vec<u8>> {
        (
            any::<u16>(),
            any::<u16>(),
            0u32..16,
            any::<u32>(),
            prop::sample::select(vec![*b"DXT1", *b"DXT5", *b"ATI2", *b"DX10", [0u8; 4]]),
            any::<u32>(),
            any::<u32>(),
            0usize..300,
            any::<u8>(),
            0u32..4,
        )
            .prop_map(|(w, h, mips, hflags, cc, pf, dxgi, plen, fill, arr)| {
                let mut b = vec![0u8; 128];
                b[0..4].copy_from_slice(b"DDS ");
                put_le(&mut b, 0x04, 124);
                put_le(&mut b, 0x08, hflags);
                put_le(&mut b, 0x0C, u32::from(h));
                put_le(&mut b, 0x10, u32::from(w));
                put_le(&mut b, 0x1C, mips);
                put_le(&mut b, 0x4C, 32);
                put_le(&mut b, 0x50, pf);
                b[0x54..0x58].copy_from_slice(&cc);
                put_le(&mut b, 0x58, 32);
                if &cc == b"DX10" {
                    b.extend_from_slice(&dxgi.to_le_bytes());
                    b.extend_from_slice(&3u32.to_le_bytes());
                    b.extend_from_slice(&0u32.to_le_bytes());
                    b.extend_from_slice(&arr.to_le_bytes());
                    b.extend_from_slice(&0u32.to_le_bytes());
                }
                let end = b.len() + plen;
                b.resize(end, fill);
                b
            })
    }

    proptest! {
        // Parsing arbitrary or DDS-shaped bytes must never panic.
        #[test]
        fn parse_never_panics(
            bytes in prop_oneof![
                proptest::collection::vec(any::<u8>(), 0..256),
                arbitrary_dds(),
            ],
        ) {
            let _ = parse(&bytes);
        }
    }
}
