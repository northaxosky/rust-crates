//! The VCDIFF file header, window loop, and instruction execution.

use crate::cache::AddressCache;
use crate::code_table::{Entry, InstKind, Instruction, default_table};
use crate::cursor::Cursor;
use crate::error::DecodeError;

const MAGIC: [u8; 3] = [0xD6, 0xC3, 0xC4];

const VCD_DECOMPRESS: u8 = 0x01;
const VCD_CODETABLE: u8 = 0x02;
const VCD_APPHEADER: u8 = 0x04;

const VCD_SOURCE: u8 = 0x01;
const VCD_TARGET: u8 = 0x02;
const VCD_ADLER32: u8 = 0x04;

/// The largest total output this decoder will produce, as a decompression-bomb guard
const MAX_OUTPUT: u64 = 1 << 30;

/// Decode `delta` applied to `source`, returning the reconstructed target
pub fn decode(source: &[u8], delta: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut cursor = Cursor::new(delta);
    parse_header(&mut cursor)?;
    let table = default_table();
    let mut out = Vec::new();
    while !cursor.is_empty() {
        decode_window(&mut cursor, source, &mut out, &table)?;
    }
    Ok(out)
}

/// Parse and validate the file header, rejecting unsupported features
fn parse_header(cursor: &mut Cursor<'_>) -> Result<(), DecodeError> {
    if cursor.read_slice(3)? != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = cursor.read_u8()?;
    if version != 0 {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    let indicator = cursor.read_u8()?;
    if indicator & !(VCD_DECOMPRESS | VCD_CODETABLE | VCD_APPHEADER) != 0 {
        return Err(DecodeError::InvalidHeaderIndicator(indicator));
    }
    if indicator & VCD_DECOMPRESS != 0 {
        return Err(DecodeError::UnsupportedSecondaryCompressor);
    }
    if indicator & VCD_CODETABLE != 0 {
        return Err(DecodeError::UnsupportedCodeTable);
    }
    if indicator & VCD_APPHEADER != 0 {
        let len = as_usize(cursor.read_varint()?)?;
        cursor.read_slice(len)?;
    }
    Ok(())
}

/// The immutable per-window context shared by the instruction helpers
#[derive(Clone, Copy)]
struct Ctx {
    window_start: usize,
    seg_from_source: bool,
    seg_pos: usize,
    seg_len: u64,
    target_size: usize,
}

/// Decode one window, appending its target bytes to `out`
fn decode_window(
    cursor: &mut Cursor<'_>,
    source: &[u8],
    out: &mut Vec<u8>,
    table: &[Entry; 256],
) -> Result<(), DecodeError> {
    let win_indicator = cursor.read_u8()?;
    if win_indicator & !(VCD_SOURCE | VCD_TARGET | VCD_ADLER32) != 0 {
        return Err(DecodeError::InvalidWindowIndicator(win_indicator));
    }
    let use_source = win_indicator & VCD_SOURCE != 0;
    let use_target = win_indicator & VCD_TARGET != 0;
    if use_source && use_target {
        return Err(DecodeError::InvalidWindowIndicator(win_indicator));
    }

    let (seg_pos, seg_len) = if use_source || use_target {
        let size = as_usize(cursor.read_varint()?)?;
        let pos = as_usize(cursor.read_varint()?)?;
        let end = pos.checked_add(size).ok_or(DecodeError::IntegerOverflow)?;
        let backing_len = if use_source { source.len() } else { out.len() };
        if end > backing_len {
            return Err(DecodeError::SegmentOutOfBounds);
        }
        (pos, size)
    } else {
        (0, 0)
    };

    let delta_len = as_usize(cursor.read_varint()?)?;
    let delta_start = cursor.position();

    let target_size_u64 = cursor.read_varint()?;
    let total = (out.len() as u64)
        .checked_add(target_size_u64)
        .ok_or(DecodeError::IntegerOverflow)?;
    if total > MAX_OUTPUT {
        return Err(DecodeError::SizeLimitExceeded);
    }
    let target_size = as_usize(target_size_u64)?;

    let delta_indicator = cursor.read_u8()?;
    if delta_indicator != 0 {
        return Err(DecodeError::InvalidDeltaIndicator(delta_indicator));
    }

    let data_len = as_usize(cursor.read_varint()?)?;
    let inst_len = as_usize(cursor.read_varint()?)?;
    let addr_len = as_usize(cursor.read_varint()?)?;

    let checksum = if win_indicator & VCD_ADLER32 != 0 {
        Some(cursor.read_u32_be()?)
    } else {
        None
    };

    let data = cursor.read_slice(data_len)?;
    let inst = cursor.read_slice(inst_len)?;
    let addr = cursor.read_slice(addr_len)?;

    if cursor.position() - delta_start != delta_len {
        return Err(DecodeError::DeltaLengthMismatch);
    }

    // reserve the window up front so growth cannot abort on a hostile size; failure is a clean error
    out.try_reserve(target_size)
        .map_err(|_| DecodeError::SizeLimitExceeded)?;

    let ctx = Ctx {
        window_start: out.len(),
        seg_from_source: use_source,
        seg_pos,
        seg_len: seg_len as u64,
        target_size,
    };
    run_instructions(ctx, table, inst, data, addr, source, out)?;

    if out.len() - ctx.window_start != target_size {
        return Err(DecodeError::TargetSizeMismatch);
    }
    if let Some(expected) = checksum {
        if adler32(&out[ctx.window_start..]) != expected {
            return Err(DecodeError::ChecksumMismatch);
        }
    }
    Ok(())
}

/// Run the instruction section, executing each opcode's one or two instructions
fn run_instructions(
    ctx: Ctx,
    table: &[Entry; 256],
    inst: &[u8],
    data: &[u8],
    addr: &[u8],
    source: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), DecodeError> {
    let mut inst_cursor = Cursor::new(inst);
    let mut data_cursor = Cursor::new(data);
    let mut addr_cursor = Cursor::new(addr);
    let mut cache = AddressCache::new();

    while !inst_cursor.is_empty() {
        let opcode = inst_cursor.read_u8()?;
        let entry = table[opcode as usize];
        let size1 = read_size(entry.first, &mut inst_cursor)?;
        let size2 = read_size(entry.second, &mut inst_cursor)?;
        execute(
            ctx,
            entry.first,
            size1,
            &mut data_cursor,
            &mut addr_cursor,
            &mut cache,
            source,
            out,
        )?;
        execute(
            ctx,
            entry.second,
            size2,
            &mut data_cursor,
            &mut addr_cursor,
            &mut cache,
            source,
            out,
        )?;
    }

    if !data_cursor.is_empty() || !addr_cursor.is_empty() {
        return Err(DecodeError::TrailingSectionData);
    }
    Ok(())
}

/// The size of a half-instruction: its table size, or a varint when the table size is 0
fn read_size(inst: Instruction, cursor: &mut Cursor<'_>) -> Result<u64, DecodeError> {
    if inst.kind == InstKind::NoOp {
        return Ok(0);
    }
    if inst.size == 0 {
        cursor.read_varint()
    } else {
        Ok(u64::from(inst.size))
    }
}

/// Execute one half-instruction against the output
#[allow(clippy::too_many_arguments)]
fn execute(
    ctx: Ctx,
    inst: Instruction,
    size: u64,
    data: &mut Cursor<'_>,
    addr: &mut Cursor<'_>,
    cache: &mut AddressCache,
    source: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), DecodeError> {
    match inst.kind {
        InstKind::NoOp => Ok(()),
        InstKind::Add => add(ctx, size, data, out),
        InstKind::Run => run(ctx, size, data, out),
        InstKind::Copy => copy(ctx, size, inst.mode, addr, cache, source, out),
    }
}

/// Append `size` bytes from the data section
fn add(ctx: Ctx, size: u64, data: &mut Cursor<'_>, out: &mut Vec<u8>) -> Result<(), DecodeError> {
    let n = as_usize(size)?;
    check_room(ctx, out, n)?;
    let bytes = data.read_slice(n)?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// Append one data byte repeated `size` times
fn run(ctx: Ctx, size: u64, data: &mut Cursor<'_>, out: &mut Vec<u8>) -> Result<(), DecodeError> {
    let n = as_usize(size)?;
    check_room(ctx, out, n)?;
    let byte = data.read_u8()?;
    out.resize(out.len() + n, byte);
    Ok(())
}

/// Append `size` bytes copied from `mode`'s address in the combined source-and-target window
#[allow(clippy::too_many_arguments)]
fn copy(
    ctx: Ctx,
    size: u64,
    mode: u8,
    addr: &mut Cursor<'_>,
    cache: &mut AddressCache,
    source: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), DecodeError> {
    let n = as_usize(size)?;
    check_room(ctx, out, n)?;
    let here = here_position(ctx, out);
    let address = cache.decode(mode, here, addr)?;
    if address >= here {
        return Err(DecodeError::AddressOutOfBounds);
    }
    if address < ctx.seg_len {
        let end = address
            .checked_add(size)
            .ok_or(DecodeError::IntegerOverflow)?;
        if end > ctx.seg_len {
            return Err(DecodeError::CopyCrossesSourceTarget);
        }
    }
    for i in 0..n {
        let u_index = address + i as u64;
        let byte = window_byte(ctx, source, out, u_index)?;
        out.push(byte);
    }
    Ok(())
}

/// The byte at `u_index` in the combined window `U = source_segment || produced_target`
fn window_byte(ctx: Ctx, source: &[u8], out: &[u8], u_index: u64) -> Result<u8, DecodeError> {
    if u_index < ctx.seg_len {
        let idx = ctx.seg_pos + as_usize(u_index)?;
        let backing = if ctx.seg_from_source { source } else { out };
        backing
            .get(idx)
            .copied()
            .ok_or(DecodeError::AddressOutOfBounds)
    } else {
        let offset = as_usize(u_index - ctx.seg_len)?;
        let idx = ctx.window_start + offset;
        out.get(idx).copied().ok_or(DecodeError::AddressOutOfBounds)
    }
}

/// The number of target bytes produced so far in the current window
fn produced(ctx: Ctx, out: &[u8]) -> usize {
    out.len() - ctx.window_start
}

/// The current output position in the combined window, used for address decoding
fn here_position(ctx: Ctx, out: &[u8]) -> u64 {
    ctx.seg_len + produced(ctx, out) as u64
}

/// Reject an instruction that would push the window past its declared target size
fn check_room(ctx: Ctx, out: &[u8], n: usize) -> Result<(), DecodeError> {
    let total = produced(ctx, out)
        .checked_add(n)
        .ok_or(DecodeError::IntegerOverflow)?;
    if total > ctx.target_size {
        return Err(DecodeError::TargetSizeMismatch);
    }
    Ok(())
}

/// Convert a decoded 64-bit size to a `usize`, erroring past the platform limit
fn as_usize(value: u64) -> Result<usize, DecodeError> {
    usize::try_from(value).map_err(|_| DecodeError::SizeLimitExceeded)
}

/// Compute the zlib Adler-32 checksum of `data`
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &byte in data {
        s1 = (s1 + u32::from(byte)) % MOD;
        s2 = (s2 + s1) % MOD;
    }
    (s2 << 16) | s1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_matches_known_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"abc"), 0x024D_0127);
    }
}
