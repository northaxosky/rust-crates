//! Streaming VCDIFF header, window, and instruction decoding

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use crate::cache::AddressCache;
use crate::code_table::{Entry, InstKind, Instruction, default_table};
use crate::error::{ByteRange, DecodeContext, DecodeError, IoOperation, SectionKind};
use crate::input::{DeltaInput, SliceCursor};
use crate::options::DecodeOptions;
use crate::secondary::{PreparedSection, SecondaryStates, XDELTA_LZMA_ID};
use crate::target::{FallibleMemoryTarget, TargetWriter};

const MAGIC: [u8; 3] = [0xD6, 0xC3, 0xC4];

const VCD_DECOMPRESS: u8 = 0x01;
const VCD_CODETABLE: u8 = 0x02;
const VCD_APPHEADER: u8 = 0x04;

const VCD_SOURCE: u8 = 0x01;
const VCD_TARGET: u8 = 0x02;
const VCD_ADLER32: u8 = 0x04;

const VCD_DATACOMP: u8 = 0x01;
const VCD_INSTCOMP: u8 = 0x02;
const VCD_ADDRCOMP: u8 = 0x04;

const MEMORY_TARGET_LIMIT: u64 = 1 << 30;

/// Decode a VCDIFF buffer into memory with a private 1 GiB target limit
pub fn decode(source: &[u8], delta: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut source = Cursor::new(source);
    let mut delta = Cursor::new(delta);
    let mut target = FallibleMemoryTarget::new();
    let options = DecodeOptions {
        max_target_size: MEMORY_TARGET_LIMIT,
        max_window_memory: MEMORY_TARGET_LIMIT,
        max_secondary_dictionary_size: 64 * 1024 * 1024,
    };
    decode_to(&mut source, &mut delta, &mut target, &options)?;
    Ok(target.into_inner())
}

/// Decode a VCDIFF stream into an empty readable and seekable target
pub fn decode_to<S, D, T>(
    source: &mut S,
    delta: &mut D,
    target: &mut T,
    options: &DecodeOptions,
) -> Result<(), DecodeError>
where
    S: Read + Seek,
    D: Read + Seek,
    T: Read + Write + Seek,
{
    let source_len = measure_source(source)?;
    let delta_len = measure_delta(delta)?;
    validate_target(target)?;

    let mut input = DeltaInput::new(delta, delta_len)?;
    let header = parse_header(&mut input)?;
    let table = default_table();
    let mut output = TargetWriter::new(target)?;
    let mut secondary = SecondaryStates::new(options.max_secondary_dictionary_size);
    let mut window = 0_u64;

    while !input.is_empty() {
        input.set_window(Some(window));
        decode_window(
            &mut input,
            source,
            source_len,
            &mut output,
            &mut secondary,
            &table,
            header,
            options,
            window,
        )?;
        window = window
            .checked_add(1)
            .ok_or_else(|| DecodeError::ArithmeticOverflow {
                context: input.context(),
            })?;
    }

    output.finish()
}

fn measure_source<S: Seek>(source: &mut S) -> Result<u64, DecodeError> {
    let len = source
        .seek(SeekFrom::End(0))
        .map_err(|source| DecodeError::SourceIo {
            operation: IoOperation::Length,
            range: None,
            window: None,
            source,
        })?;
    let actual = source
        .seek(SeekFrom::Start(0))
        .map_err(|source| DecodeError::SourceIo {
            operation: IoOperation::Seek,
            range: Some(ByteRange::new(0, 0)),
            window: None,
            source,
        })?;
    if actual != 0 {
        return Err(DecodeError::SourceIo {
            operation: IoOperation::Seek,
            range: Some(ByteRange::new(0, 0)),
            window: None,
            source: wrong_seek_error("source"),
        });
    }
    Ok(len)
}

fn measure_delta<D: Seek>(delta: &mut D) -> Result<u64, DecodeError> {
    let context = DecodeContext::new(0, None, None);
    let len = delta
        .seek(SeekFrom::End(0))
        .map_err(|source| DecodeError::DeltaIo {
            operation: IoOperation::Length,
            context,
            source,
        })?;
    let actual = delta
        .seek(SeekFrom::Start(0))
        .map_err(|source| DecodeError::DeltaIo {
            operation: IoOperation::Seek,
            context,
            source,
        })?;
    if actual != 0 {
        return Err(DecodeError::DeltaIo {
            operation: IoOperation::Seek,
            context,
            source: wrong_seek_error("delta"),
        });
    }
    Ok(len)
}

fn validate_target<T: Seek>(target: &mut T) -> Result<(), DecodeError> {
    let len = target
        .seek(SeekFrom::End(0))
        .map_err(|source| DecodeError::TargetIo {
            operation: IoOperation::Length,
            range: None,
            window: None,
            source,
        })?;
    if len != 0 {
        return Err(DecodeError::TargetNotEmpty { len });
    }
    let actual = target
        .seek(SeekFrom::Start(0))
        .map_err(|source| DecodeError::TargetIo {
            operation: IoOperation::Seek,
            range: Some(ByteRange::new(0, 0)),
            window: None,
            source,
        })?;
    if actual != 0 {
        return Err(DecodeError::TargetIo {
            operation: IoOperation::Seek,
            range: Some(ByteRange::new(0, 0)),
            window: None,
            source: wrong_seek_error("target"),
        });
    }
    Ok(())
}

fn wrong_seek_error(stream: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{stream} seek returned wrong offset"),
    )
}

#[derive(Clone, Copy)]
struct Header {
    compressor_id: Option<u8>,
}

fn parse_header<D: Read + Seek>(input: &mut DeltaInput<'_, D>) -> Result<Header, DecodeError> {
    let magic_context = input.context();
    let mut magic = [0; 3];
    input.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(DecodeError::BadMagic {
            context: magic_context,
        });
    }

    let version_context = input.context();
    let version = input.read_u8()?;
    if version != 0 {
        return Err(DecodeError::UnsupportedVersion {
            version,
            context: version_context,
        });
    }

    let indicator_context = input.context();
    let indicator = input.read_u8()?;
    if indicator & !(VCD_DECOMPRESS | VCD_CODETABLE | VCD_APPHEADER) != 0 {
        return Err(DecodeError::InvalidHeaderIndicator {
            indicator,
            context: indicator_context,
        });
    }

    let compressor_id = if indicator & VCD_DECOMPRESS != 0 {
        let context = input.context();
        let id = input.read_u8()?;
        if id != XDELTA_LZMA_ID {
            return Err(DecodeError::UnsupportedSecondaryCompressor { id, context });
        }
        Some(id)
    } else {
        None
    };

    if indicator & VCD_CODETABLE != 0 {
        return Err(DecodeError::UnsupportedCodeTable {
            context: input.context(),
        });
    }
    if indicator & VCD_APPHEADER != 0 {
        let len = input.read_varint()?;
        input.skip(len)?;
    }
    Ok(Header { compressor_id })
}

#[derive(Clone, Copy)]
enum SegmentKind {
    Source,
    Target,
}

#[derive(Clone, Copy)]
struct Segment {
    kind: SegmentKind,
    start: u64,
    len: u64,
}

#[derive(Clone, Copy)]
struct WindowContext {
    index: u64,
    window_start: u64,
    segment: Option<Segment>,
    target_size: u64,
}

struct WindowSections {
    data: PreparedSection,
    instructions: PreparedSection,
    addresses: PreparedSection,
}

#[allow(clippy::too_many_arguments)]
fn decode_window<S, D, T>(
    input: &mut DeltaInput<'_, D>,
    source: &mut S,
    source_len: u64,
    output: &mut TargetWriter<'_, T>,
    secondary: &mut SecondaryStates,
    table: &[Entry; 256],
    header: Header,
    options: &DecodeOptions,
    window: u64,
) -> Result<(), DecodeError>
where
    S: Read + Seek,
    D: Read + Seek,
    T: Read + Write + Seek,
{
    input.set_section(None);
    let window_context = input.context();
    let indicator = input.read_u8()?;
    if indicator & !(VCD_SOURCE | VCD_TARGET | VCD_ADLER32) != 0
        || indicator & (VCD_SOURCE | VCD_TARGET) == (VCD_SOURCE | VCD_TARGET)
    {
        return Err(DecodeError::InvalidWindowIndicator {
            indicator,
            context: window_context,
        });
    }

    let segment = if indicator & (VCD_SOURCE | VCD_TARGET) != 0 {
        let context = input.context();
        let len = input.read_varint()?;
        let start = input.read_varint()?;
        let end = start
            .checked_add(len)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        let use_source = indicator & VCD_SOURCE != 0;
        let available = if use_source { source_len } else { output.len() };
        if end > available {
            return Err(DecodeError::SegmentOutOfBounds {
                range: ByteRange::new(start, len),
                available,
                context,
            });
        }
        Some(Segment {
            kind: if use_source {
                SegmentKind::Source
            } else {
                SegmentKind::Target
            },
            start,
            len,
        })
    } else {
        None
    };

    let encoding_len_context = input.context();
    let encoding_len = input.read_varint()?;
    let encoding_start = input.position();
    let encoding_end =
        encoding_start
            .checked_add(encoding_len)
            .ok_or(DecodeError::ArithmeticOverflow {
                context: encoding_len_context,
            })?;

    let target_context = input.context();
    let target_size = input.read_varint()?;
    let attempted_target =
        output
            .len()
            .checked_add(target_size)
            .ok_or(DecodeError::ArithmeticOverflow {
                context: target_context,
            })?;
    if attempted_target > options.max_target_size {
        return Err(DecodeError::TargetSizeLimit {
            attempted: attempted_target,
            limit: options.max_target_size,
            context: target_context,
        });
    }

    let delta_indicator_context = input.context();
    let delta_indicator = input.read_u8()?;
    if delta_indicator & !(VCD_DATACOMP | VCD_INSTCOMP | VCD_ADDRCOMP) != 0 {
        return Err(DecodeError::InvalidDeltaIndicator {
            indicator: delta_indicator,
            context: delta_indicator_context,
        });
    }
    validate_compressed_sections(
        delta_indicator,
        header.compressor_id,
        delta_indicator_context,
    )?;

    let data_len = input.read_varint()?;
    let instructions_len = input.read_varint()?;
    let addresses_len = input.read_varint()?;

    let checksum = if indicator & VCD_ADLER32 != 0 {
        let offset = input.position();
        Some((input.read_u32_be()?, offset))
    } else {
        None
    };

    let sections_start = input.position();
    let instructions_start =
        sections_start
            .checked_add(data_len)
            .ok_or(DecodeError::ArithmeticOverflow {
                context: input.context(),
            })?;
    let addresses_start = instructions_start.checked_add(instructions_len).ok_or(
        DecodeError::ArithmeticOverflow {
            context: input.context(),
        },
    )?;
    let sections_end =
        addresses_start
            .checked_add(addresses_len)
            .ok_or(DecodeError::ArithmeticOverflow {
                context: input.context(),
            })?;
    if sections_end != encoding_end {
        return Err(DecodeError::DeltaLengthMismatch {
            expected_end: encoding_end,
            actual_end: sections_end,
            context: encoding_len_context,
        });
    }
    if sections_end > input.len() {
        return Err(DecodeError::TruncatedDelta {
            context: DecodeContext::new(
                input.len(),
                Some(window),
                truncated_section(input.len(), instructions_start, addresses_start),
            ),
        });
    }

    let sections = read_sections(
        input,
        [data_len, instructions_len, addresses_len],
        delta_indicator,
        window,
        secondary,
        options.max_window_memory,
    )?;
    input.set_section(None);

    let context = WindowContext {
        index: window,
        window_start: output.len(),
        segment,
        target_size,
    };
    output.begin_window(window, window_context);
    run_instructions(context, table, &sections, source, output)?;

    let actual_size =
        output
            .len()
            .checked_sub(context.window_start)
            .ok_or(DecodeError::ArithmeticOverflow {
                context: input.context(),
            })?;
    if actual_size != target_size {
        return Err(DecodeError::TargetSizeMismatch {
            expected: target_size,
            actual: actual_size,
            context: input.context(),
        });
    }
    if let Some((expected, delta_offset)) = checksum {
        let actual = output.checksum();
        if actual != expected {
            return Err(DecodeError::ChecksumMismatch {
                window,
                expected,
                actual,
                delta_offset,
            });
        }
    }
    output.finish_window()
}

fn validate_compressed_sections(
    indicator: u8,
    compressor_id: Option<u8>,
    context: DecodeContext,
) -> Result<(), DecodeError> {
    for (bit, section) in [
        (VCD_DATACOMP, SectionKind::Data),
        (VCD_INSTCOMP, SectionKind::Instructions),
        (VCD_ADDRCOMP, SectionKind::Addresses),
    ] {
        if indicator & bit != 0 {
            let context = DecodeContext::new(context.delta_offset, context.window, Some(section));
            if compressor_id.is_none() {
                return Err(DecodeError::CompressedSectionWithoutCompressor { section, context });
            }
        }
    }
    Ok(())
}

fn truncated_section(
    delta_len: u64,
    instructions_start: u64,
    addresses_start: u64,
) -> Option<SectionKind> {
    if delta_len < instructions_start {
        Some(SectionKind::Data)
    } else if delta_len < addresses_start {
        Some(SectionKind::Instructions)
    } else {
        Some(SectionKind::Addresses)
    }
}

fn read_sections<D: Read + Seek>(
    input: &mut DeltaInput<'_, D>,
    encoded_lengths: [u64; 3],
    indicator: u8,
    window: u64,
    secondary: &mut SecondaryStates,
    max_window_memory: u64,
) -> Result<WindowSections, DecodeError> {
    let mut active_memory = 0_u64;
    let data = secondary.prepare(
        input,
        encoded_lengths[SectionKind::Data.index()],
        indicator & VCD_DATACOMP != 0,
        SectionKind::Data,
        window,
        &mut active_memory,
        max_window_memory,
    )?;
    let instructions = secondary.prepare(
        input,
        encoded_lengths[SectionKind::Instructions.index()],
        indicator & VCD_INSTCOMP != 0,
        SectionKind::Instructions,
        window,
        &mut active_memory,
        max_window_memory,
    )?;
    let addresses = secondary.prepare(
        input,
        encoded_lengths[SectionKind::Addresses.index()],
        indicator & VCD_ADDRCOMP != 0,
        SectionKind::Addresses,
        window,
        &mut active_memory,
        max_window_memory,
    )?;
    Ok(WindowSections {
        data,
        instructions,
        addresses,
    })
}

fn run_instructions<S, T>(
    context: WindowContext,
    table: &[Entry; 256],
    sections: &WindowSections,
    source: &mut S,
    output: &mut TargetWriter<'_, T>,
) -> Result<(), DecodeError>
where
    S: Read + Seek,
    T: Read + Write + Seek,
{
    let mut instructions = SliceCursor::new(
        &sections.instructions.bytes,
        sections.instructions.delta_offset,
        context.index,
        SectionKind::Instructions,
    );
    let mut data = SliceCursor::new(
        &sections.data.bytes,
        sections.data.delta_offset,
        context.index,
        SectionKind::Data,
    );
    let mut addresses = SliceCursor::new(
        &sections.addresses.bytes,
        sections.addresses.delta_offset,
        context.index,
        SectionKind::Addresses,
    );
    let mut cache = AddressCache::new();

    while !instructions.is_empty() {
        let opcode = instructions.read_u8()?;
        let entry = table[opcode as usize];
        let size1 = read_size(entry.first, &mut instructions)?;
        let size2 = read_size(entry.second, &mut instructions)?;
        execute(
            context,
            entry.first,
            size1,
            &mut data,
            &mut addresses,
            &mut cache,
            source,
            output,
        )?;
        execute(
            context,
            entry.second,
            size2,
            &mut data,
            &mut addresses,
            &mut cache,
            source,
            output,
        )?;
    }

    if !data.is_empty() {
        return Err(DecodeError::TrailingSectionData {
            section: SectionKind::Data,
            remaining: data.remaining(),
            context: data.context(),
        });
    }
    if !addresses.is_empty() {
        return Err(DecodeError::TrailingSectionData {
            section: SectionKind::Addresses,
            remaining: addresses.remaining(),
            context: addresses.context(),
        });
    }
    Ok(())
}

fn read_size(instruction: Instruction, cursor: &mut SliceCursor<'_>) -> Result<u64, DecodeError> {
    if instruction.kind == InstKind::NoOp {
        return Ok(0);
    }
    if instruction.size == 0 {
        cursor.read_varint()
    } else {
        Ok(u64::from(instruction.size))
    }
}

#[allow(clippy::too_many_arguments)]
fn execute<S, T>(
    context: WindowContext,
    instruction: Instruction,
    size: u64,
    data: &mut SliceCursor<'_>,
    addresses: &mut SliceCursor<'_>,
    cache: &mut AddressCache,
    source: &mut S,
    output: &mut TargetWriter<'_, T>,
) -> Result<(), DecodeError>
where
    S: Read + Seek,
    T: Read + Write + Seek,
{
    match instruction.kind {
        InstKind::NoOp => Ok(()),
        InstKind::Add => add(context, size, data, output),
        InstKind::Run => run(context, size, data, output),
        InstKind::Copy => copy(
            context,
            size,
            instruction.mode,
            addresses,
            cache,
            source,
            output,
        ),
    }
}

fn add<T: Read + Write + Seek>(
    window: WindowContext,
    size: u64,
    data: &mut SliceCursor<'_>,
    output: &mut TargetWriter<'_, T>,
) -> Result<(), DecodeError> {
    let context = data.context();
    check_room(window, output, size, context)?;
    let bytes = data.read_slice(size)?;
    output.emit(bytes, context)
}

fn run<T: Read + Write + Seek>(
    window: WindowContext,
    size: u64,
    data: &mut SliceCursor<'_>,
    output: &mut TargetWriter<'_, T>,
) -> Result<(), DecodeError> {
    let context = data.context();
    check_room(window, output, size, context)?;
    let byte = data.read_u8()?;
    output.emit_run(byte, size, context)
}

#[allow(clippy::too_many_arguments)]
fn copy<S, T>(
    window: WindowContext,
    size: u64,
    mode: u8,
    addresses: &mut SliceCursor<'_>,
    cache: &mut AddressCache,
    source: &mut S,
    output: &mut TargetWriter<'_, T>,
) -> Result<(), DecodeError>
where
    S: Read + Seek,
    T: Read + Write + Seek,
{
    let context = addresses.context();
    check_room(window, output, size, context)?;
    let segment_len = window.segment.map_or(0, |segment| segment.len);
    let produced = output
        .len()
        .checked_sub(window.window_start)
        .ok_or(DecodeError::ArithmeticOverflow { context })?;
    let here = segment_len
        .checked_add(produced)
        .ok_or(DecodeError::ArithmeticOverflow { context })?;
    let address = cache.decode(mode, here, addresses)?;
    if address >= here {
        return Err(DecodeError::AddressOutOfBounds {
            address,
            here,
            context,
        });
    }

    if address < segment_len {
        let end = address
            .checked_add(size)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        if end > segment_len {
            return Err(DecodeError::CopyCrossesSourceTarget {
                address,
                size,
                boundary: segment_len,
                context,
            });
        }
        let Some(segment) = window.segment else {
            return Err(DecodeError::AddressOutOfBounds {
                address,
                here,
                context,
            });
        };
        let start = segment
            .start
            .checked_add(address)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        return match segment.kind {
            SegmentKind::Source => output.copy_from_source(source, start, size, context),
            SegmentKind::Target => output.copy_from_target(start, size, context),
        };
    }

    let window_offset = address - segment_len;
    let start = window
        .window_start
        .checked_add(window_offset)
        .ok_or(DecodeError::ArithmeticOverflow { context })?;
    let distance = output
        .len()
        .checked_sub(start)
        .ok_or(DecodeError::AddressOutOfBounds {
            address,
            here,
            context,
        })?;
    if distance == 0 {
        return Err(DecodeError::AddressOutOfBounds {
            address,
            here,
            context,
        });
    }
    output.copy_from_current(start, size, distance, context)
}

fn check_room<T: Read + Write + Seek>(
    window: WindowContext,
    output: &TargetWriter<'_, T>,
    size: u64,
    context: DecodeContext,
) -> Result<(), DecodeError> {
    let produced = output
        .len()
        .checked_sub(window.window_start)
        .ok_or(DecodeError::ArithmeticOverflow { context })?;
    let attempted = produced
        .checked_add(size)
        .ok_or(DecodeError::ArithmeticOverflow { context })?;
    if attempted > window.target_size {
        return Err(DecodeError::TargetSizeMismatch {
            expected: window.target_size,
            actual: attempted,
            context,
        });
    }
    Ok(())
}
