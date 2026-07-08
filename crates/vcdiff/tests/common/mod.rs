//! A minimal VCDIFF encoder used to build deltas for decode tests.
//!
//! It emits a single window using only the size-from-stream opcodes (RUN=0, ADD=1, COPY mode 0 = 19),
//! so tests can construct arbitrary valid deltas without reproducing the merged code table. COPY
//! addresses are absolute indices into the combined window `U = source || target`.

#![allow(dead_code)]

/// A high-level instruction the builder lowers into VCDIFF sections
pub enum Op {
    /// Append these literal bytes
    Add(Vec<u8>),
    /// Append `byte` repeated `len` times
    Run(u8, u64),
    /// Copy `len` bytes from absolute address `addr` in `U = source || target`
    Copy(u64, u64),
}

/// Encode `value` as an RFC 3284 base-128 big-endian varint
pub fn varint(value: u64) -> Vec<u8> {
    let mut bytes = vec![(value & 0x7f) as u8];
    let mut v = value >> 7;
    while v > 0 {
        bytes.push(0x80 | (v & 0x7f) as u8);
        v >>= 7;
    }
    bytes.reverse();
    bytes
}

/// Build a one-window VCDIFF delta from `ops`, using a whole-source segment when `source_len` > 0
pub fn build(source_len: usize, ops: &[Op]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut inst = Vec::new();
    let mut addr = Vec::new();
    let mut target_size: u64 = 0;

    for op in ops {
        match op {
            Op::Add(bytes) => {
                data.extend_from_slice(bytes);
                inst.push(1);
                inst.extend(varint(bytes.len() as u64));
                target_size += bytes.len() as u64;
            }
            Op::Run(byte, len) => {
                data.push(*byte);
                inst.push(0);
                inst.extend(varint(*len));
                target_size += *len;
            }
            Op::Copy(address, len) => {
                inst.push(19);
                inst.extend(varint(*len));
                addr.extend(varint(*address));
                target_size += *len;
            }
        }
    }

    let use_source = source_len > 0;

    let mut enc = Vec::new();
    enc.extend(varint(target_size));
    enc.push(0);
    enc.extend(varint(data.len() as u64));
    enc.extend(varint(inst.len() as u64));
    enc.extend(varint(addr.len() as u64));
    enc.extend_from_slice(&data);
    enc.extend_from_slice(&inst);
    enc.extend_from_slice(&addr);

    let mut window = Vec::new();
    window.push(if use_source { 0x01 } else { 0x00 });
    if use_source {
        window.extend(varint(source_len as u64));
        window.extend(varint(0));
    }
    window.extend(varint(enc.len() as u64));
    window.extend_from_slice(&enc);

    let mut out = vec![0xD6, 0xC3, 0xC4, 0x00, 0x00];
    out.extend_from_slice(&window);
    out
}

/// Concatenate a header and several already-built single windows into one multi-window delta
pub fn join_windows(deltas: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0xD6, 0xC3, 0xC4, 0x00, 0x00];
    for d in deltas {
        out.extend_from_slice(&d[5..]);
    }
    out
}
