//! Stateless xdelta DJW section decoding

use std::fmt;
use std::io::{Read, Seek};

use crate::error::{DecodeContext, DecodeError, SecondaryError};
use crate::input::DeltaInput;

pub(crate) const XDELTA_DJW_ID: u8 = 1;

const BYTE_SYMBOLS: usize = 256;
const MAX_GROUPS: usize = 8;
const MAX_CODE_WIDTH: usize = 20;
const TOKEN_SYMBOLS: usize = 22;
const MAX_TOKEN_WIDTH: usize = 15;
const MAX_SELECTOR_WIDTH: usize = 7;
const SECTOR_STEP: usize = 5;
const INITIAL_LENGTH_ORDER: [u8; 21] = [
    0, 4, 5, 6, 7, 8, 9, 10, 3, 11, 2, 12, 13, 1, 14, 15, 16, 17, 18, 19, 20,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DjwFault {
    Truncated,
    InvalidField,
    InvalidTable,
    EmptyTable,
    InvalidCode,
    InvalidMtf,
    RunOverflow,
    RunPastEnd,
    InvalidSelector,
}

impl fmt::Display for DjwFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Truncated => "truncated DJW payload",
            Self::InvalidField => "invalid DJW field",
            Self::InvalidTable => "invalid DJW Huffman table",
            Self::EmptyTable => "empty DJW Huffman table",
            Self::InvalidCode => "invalid DJW Huffman code",
            Self::InvalidMtf => "invalid DJW move-to-front position",
            Self::RunOverflow => "DJW run overflow",
            Self::RunPastEnd => "DJW run exceeds output",
            Self::InvalidSelector => "invalid DJW selector",
        };
        formatter.write_str(message)
    }
}

#[derive(Clone, Copy)]
struct PrefixBook {
    counts: [u16; MAX_CODE_WIDTH + 1],
    first_codes: [u32; MAX_CODE_WIDTH + 1],
    starts: [u16; MAX_CODE_WIDTH + 1],
    ordered: [u8; BYTE_SYMBOLS],
    symbol_count: u16,
    min_width: u8,
    max_width: u8,
}

impl PrefixBook {
    const EMPTY: Self = Self {
        counts: [0; MAX_CODE_WIDTH + 1],
        first_codes: [0; MAX_CODE_WIDTH + 1],
        starts: [0; MAX_CODE_WIDTH + 1],
        ordered: [0; BYTE_SYMBOLS],
        symbol_count: 0,
        min_width: 0,
        max_width: 0,
    };

    fn build(lengths: &[u8], width_limit: usize) -> Result<Self, DjwFault> {
        if lengths.len() > BYTE_SYMBOLS || width_limit == 0 || width_limit > MAX_CODE_WIDTH {
            return Err(DjwFault::InvalidField);
        }

        let mut book = Self::EMPTY;
        for &width in lengths {
            let width = usize::from(width);
            if width > width_limit {
                return Err(DjwFault::InvalidTable);
            }
            if width == 0 {
                continue;
            }
            book.counts[width] = book.counts[width]
                .checked_add(1)
                .ok_or(DjwFault::InvalidTable)?;
        }

        let mut open_slots = 1_u32;
        for width in 1..=width_limit {
            open_slots <<= 1;
            let used = u32::from(book.counts[width]);
            if used > open_slots {
                return Err(DjwFault::InvalidTable);
            }
            open_slots -= used;
        }

        let mut next_code = 0_u32;
        let mut next_symbol = 0_u16;
        for width in 1..=width_limit {
            next_code = (next_code + u32::from(book.counts[width - 1])) << 1;
            book.first_codes[width] = next_code;
            book.starts[width] = next_symbol;
            next_symbol = next_symbol
                .checked_add(book.counts[width])
                .ok_or(DjwFault::InvalidTable)?;
            if book.counts[width] != 0 {
                if book.min_width == 0 {
                    book.min_width = width as u8;
                }
                book.max_width = width as u8;
            }
        }
        book.symbol_count = next_symbol;

        let mut slots = book.starts;
        for (symbol, &width) in lengths.iter().enumerate() {
            if width == 0 {
                continue;
            }
            let width = usize::from(width);
            let slot = usize::from(slots[width]);
            if slot >= usize::from(book.symbol_count) {
                return Err(DjwFault::InvalidTable);
            }
            book.ordered[slot] = u8::try_from(symbol).map_err(|_| DjwFault::InvalidField)?;
            slots[width] = slots[width].checked_add(1).ok_or(DjwFault::InvalidTable)?;
        }

        Ok(book)
    }

    fn lookup(&self, width: usize, code: u32) -> Option<u8> {
        let count = u32::from(*self.counts.get(width)?);
        let first = *self.first_codes.get(width)?;
        if count == 0 || code < first {
            return None;
        }
        let offset = code - first;
        if offset >= count {
            return None;
        }
        let ordinal = u32::from(self.starts[width]) + offset;
        if ordinal >= u32::from(self.symbol_count) {
            return None;
        }
        self.ordered.get(ordinal as usize).copied()
    }

    fn read_symbol<D: Read + Seek>(
        &self,
        bits: &mut BitStream<'_, '_, D>,
    ) -> Result<u8, DecodeError> {
        if self.symbol_count == 0 {
            return Err(format_error(bits.context(), DjwFault::EmptyTable));
        }

        let mut code = 0_u32;
        for width in 1..=usize::from(self.max_width) {
            code = (code << 1) | u32::from(bits.read_bit()?);
            if width >= usize::from(self.min_width) {
                if let Some(symbol) = self.lookup(width, code) {
                    return Ok(symbol);
                }
            }
        }
        Err(format_error(bits.context(), DjwFault::InvalidCode))
    }

    #[cfg(test)]
    fn encoding(&self, symbol: u8) -> Option<(u32, u8)> {
        for width in 1..=usize::from(self.max_width) {
            let start = usize::from(self.starts[width]);
            let count = usize::from(self.counts[width]);
            for offset in 0..count {
                if self.ordered[start + offset] == symbol {
                    return Some((self.first_codes[width] + offset as u32, width as u8));
                }
            }
        }
        None
    }
}

struct BitStream<'input, 'reader, D> {
    input: &'input mut DeltaInput<'reader, D>,
    section_end: u64,
    current: u8,
    used: u8,
    total_bits: u64,
}

impl<'input, 'reader, D: Read + Seek> BitStream<'input, 'reader, D> {
    fn new(input: &'input mut DeltaInput<'reader, D>, section_end: u64) -> Self {
        Self {
            input,
            section_end,
            current: 0,
            used: 8,
            total_bits: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u8, DecodeError> {
        if self.used == 8 {
            if self.input.position() >= self.section_end {
                return Err(format_error(self.context(), DjwFault::Truncated));
            }
            self.current = self.input.read_u8()?;
            self.used = 0;
        }
        let bit = (self.current >> self.used) & 1;
        self.used += 1;
        self.total_bits = self.total_bits.saturating_add(1);
        Ok(bit)
    }

    fn read_field(&mut self, width: u8) -> Result<u32, DecodeError> {
        if width == 0 {
            return Err(format_error(self.context(), DjwFault::InvalidField));
        }
        let mut value = 0_u32;
        for _ in 0..width {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    fn context(&self) -> DecodeContext {
        self.input.context()
    }
}

#[derive(Clone)]
struct MtfList {
    values: [u8; 21],
    len: usize,
}

impl MtfList {
    fn from_slice(values: &[u8]) -> Result<Self, DjwFault> {
        if values.is_empty() || values.len() > 21 {
            return Err(DjwFault::InvalidField);
        }
        let mut list = Self {
            values: [0; 21],
            len: values.len(),
        };
        list.values[..values.len()].copy_from_slice(values);
        Ok(list)
    }

    fn identity(len: usize) -> Result<Self, DjwFault> {
        if len == 0 || len > 21 {
            return Err(DjwFault::InvalidField);
        }
        let mut values = [0; 21];
        for (index, value) in values.iter_mut().take(len).enumerate() {
            *value = index as u8;
        }
        Ok(Self { values, len })
    }

    fn front(&self) -> u8 {
        self.values[0]
    }

    fn promote(&mut self, position: usize) -> Result<u8, DjwFault> {
        if position >= self.len {
            return Err(DjwFault::InvalidMtf);
        }
        let value = self.values[position];
        self.values.copy_within(..position, 1);
        self.values[0] = value;
        Ok(value)
    }
}

trait DecodeObserver {
    fn header(&mut self, _groups: usize, _sector_size: usize, _sectors: usize) {}
    fn implicit_zero(&mut self, _run_pending: bool, _move_pending: bool) {}
    fn finished(&mut self, _bits: u64) {}
}

struct NoObserver;

impl DecodeObserver for NoObserver {}

struct TokenMachine {
    mtf: MtfList,
    run_remaining: usize,
    run_exponent: u32,
    pending_move: Option<usize>,
}

impl TokenMachine {
    fn new(mtf: MtfList) -> Self {
        Self {
            mtf,
            run_remaining: 0,
            run_exponent: 0,
            pending_move: None,
        }
    }

    fn queue(&mut self, token: u8) -> Result<(), DjwFault> {
        if self.run_remaining != 0 || self.pending_move.is_some() {
            return Err(DjwFault::InvalidField);
        }
        if token <= 1 {
            let amount = usize::from(token + 1)
                .checked_shl(self.run_exponent)
                .ok_or(DjwFault::RunOverflow)?;
            self.run_remaining = self
                .run_remaining
                .checked_add(amount)
                .ok_or(DjwFault::RunOverflow)?;
            self.run_exponent = self
                .run_exponent
                .checked_add(1)
                .ok_or(DjwFault::RunOverflow)?;
        } else {
            self.pending_move = Some(usize::from(token - 1));
            self.run_exponent = 0;
        }
        Ok(())
    }

    fn advance<O: DecodeObserver>(
        &mut self,
        values: &mut [u8],
        index: usize,
        skip_stride: usize,
        observer: &mut O,
    ) -> Result<bool, DjwFault> {
        if skip_stride != 0 && index >= skip_stride && values[index - skip_stride] == 0 {
            observer.implicit_zero(self.run_remaining != 0, self.pending_move.is_some());
            values[index] = 0;
            return Ok(true);
        }
        if self.run_remaining != 0 {
            values[index] = self.mtf.front();
            self.run_remaining -= 1;
            return Ok(true);
        }
        if let Some(position) = self.pending_move {
            values[index] = self.mtf.promote(position)?;
            self.pending_move = None;
            return Ok(true);
        }
        Ok(false)
    }

    fn finish(&self) -> Result<(), DjwFault> {
        if self.run_remaining != 0 || self.pending_move.is_some() {
            return Err(DjwFault::RunPastEnd);
        }
        Ok(())
    }
}

fn expand_tokens<D: Read + Seek, O: DecodeObserver>(
    bits: &mut BitStream<'_, '_, D>,
    token_book: &PrefixBook,
    machine: &mut TokenMachine,
    values: &mut [u8],
    skip_stride: usize,
    observer: &mut O,
) -> Result<(), DecodeError> {
    let mut index = 0_usize;
    while index < values.len() {
        if machine
            .advance(values, index, skip_stride, observer)
            .map_err(|fault| format_error(bits.context(), fault))?
        {
            index += 1;
        } else {
            let token = token_book.read_symbol(bits)?;
            machine
                .queue(token)
                .map_err(|fault| format_error(bits.context(), fault))?;
        }
    }
    machine
        .finish()
        .map_err(|fault| format_error(bits.context(), fault))
}

fn allocate_selectors(
    count: u64,
    active_memory_after_output: u64,
    max_window_memory: u64,
    context: DecodeContext,
) -> Result<Vec<u8>, DecodeError> {
    let remaining_scratch = max_window_memory.saturating_sub(active_memory_after_output);
    if count > remaining_scratch {
        let attempted = active_memory_after_output
            .checked_add(count)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        return Err(DecodeError::WindowMemoryLimit {
            attempted,
            limit: max_window_memory,
            context,
        });
    }
    let count_usize = usize::try_from(count).map_err(|_| DecodeError::PlatformSizeLimit {
        value: count,
        context,
    })?;
    let mut selectors = Vec::new();
    selectors
        .try_reserve_exact(count_usize)
        .map_err(|_| DecodeError::AllocationFailed {
            requested: count,
            context,
        })?;
    selectors.resize(count_usize, 0);
    Ok(selectors)
}

fn decode_body<D: Read + Seek>(
    bits: &mut BitStream<'_, '_, D>,
    books: &[PrefixBook; MAX_GROUPS],
    group_count: usize,
    selectors: &[u8],
    sector_size: usize,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    for (sector, chunk) in output.chunks_mut(sector_size).enumerate() {
        let group = if group_count == 1 {
            0
        } else {
            usize::from(selectors[sector])
        };
        if group >= group_count {
            return Err(format_error(bits.context(), DjwFault::InvalidSelector));
        }
        for byte in chunk {
            *byte = books[group].read_symbol(bits)?;
        }
    }
    Ok(())
}

fn decode_observed<D: Read + Seek, O: DecodeObserver>(
    input: &mut DeltaInput<'_, D>,
    section_end: u64,
    output: &mut [u8],
    active_memory_after_output: u64,
    max_window_memory: u64,
    observer: &mut O,
) -> Result<(), DecodeError> {
    if output.is_empty() {
        return Err(format_error(input.context(), DjwFault::InvalidField));
    }

    let mut bits = BitStream::new(input, section_end);
    let group_count = bits.read_field(3)? as usize + 1;
    let sector_size = if group_count == 1 {
        output.len()
    } else {
        (bits.read_field(5)? as usize + 1) * SECTOR_STEP
    };
    let sector_count = output.len().div_ceil(sector_size);
    observer.header(group_count, sector_size, sector_count);

    let mut selectors = if group_count == 1 {
        Vec::new()
    } else {
        allocate_selectors(
            sector_count as u64,
            active_memory_after_output,
            max_window_memory,
            bits.context(),
        )?
    };

    let transmitted = bits.read_field(4)? as usize + 7;
    if transmitted > TOKEN_SYMBOLS {
        return Err(format_error(bits.context(), DjwFault::InvalidField));
    }
    let mut token_lengths = [0_u8; TOKEN_SYMBOLS];
    for width in token_lengths.iter_mut().take(transmitted) {
        *width = bits.read_field(4)? as u8;
    }
    let token_book = PrefixBook::build(&token_lengths, MAX_TOKEN_WIDTH)
        .map_err(|fault| format_error(bits.context(), fault))?;

    let mut all_lengths = [0_u8; MAX_GROUPS * BYTE_SYMBOLS];
    let length_count = group_count * BYTE_SYMBOLS;
    let length_mtf = MtfList::from_slice(&INITIAL_LENGTH_ORDER)
        .map_err(|fault| format_error(bits.context(), fault))?;
    let mut length_machine = TokenMachine::new(length_mtf);
    expand_tokens(
        &mut bits,
        &token_book,
        &mut length_machine,
        &mut all_lengths[..length_count],
        BYTE_SYMBOLS,
        observer,
    )?;

    let mut books = [PrefixBook::EMPTY; MAX_GROUPS];
    for (group, book) in books.iter_mut().enumerate().take(group_count) {
        let start = group * BYTE_SYMBOLS;
        *book = PrefixBook::build(&all_lengths[start..start + BYTE_SYMBOLS], MAX_CODE_WIDTH)
            .map_err(|fault| format_error(bits.context(), fault))?;
    }

    if group_count > 1 {
        let mut selector_lengths = [0_u8; MAX_GROUPS + 1];
        for width in selector_lengths.iter_mut().take(group_count + 1) {
            *width = bits.read_field(3)? as u8;
        }
        let selector_book =
            PrefixBook::build(&selector_lengths[..group_count + 1], MAX_SELECTOR_WIDTH)
                .map_err(|fault| format_error(bits.context(), fault))?;
        let selector_mtf =
            MtfList::identity(group_count).map_err(|fault| format_error(bits.context(), fault))?;
        let mut selector_machine = TokenMachine::new(selector_mtf);
        expand_tokens(
            &mut bits,
            &selector_book,
            &mut selector_machine,
            &mut selectors,
            0,
            observer,
        )?;
        if selectors
            .iter()
            .any(|&group| usize::from(group) >= group_count)
        {
            return Err(format_error(bits.context(), DjwFault::InvalidSelector));
        }
    }

    decode_body(
        &mut bits,
        &books,
        group_count,
        &selectors,
        sector_size,
        output,
    )?;
    observer.finished(bits.total_bits);
    Ok(())
}

pub(crate) fn decode_section<D: Read + Seek>(
    input: &mut DeltaInput<'_, D>,
    section_end: u64,
    output: &mut [u8],
    active_memory_after_output: u64,
    max_window_memory: u64,
) -> Result<(), DecodeError> {
    decode_observed(
        input,
        section_end,
        output,
        active_memory_after_output,
        max_window_memory,
        &mut NoObserver,
    )
}

fn format_error(context: DecodeContext, fault: DjwFault) -> DecodeError {
    DecodeError::SecondaryDecompression {
        compressor_id: XDELTA_DJW_ID,
        context,
        source: SecondaryError::djw(fault),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;
    use std::io::{self, Cursor, Read, Seek, SeekFrom};

    use proptest::prelude::*;

    use super::*;
    use crate::error::{IoOperation, SectionKind};

    const ONE_PAYLOAD: &[u8] =
        include_bytes!("../tests/fixtures/xdelta/djw-one-xdelta-3.2.0.payload.bin");
    const ONE_RAW: &[u8] = include_bytes!("../tests/fixtures/xdelta/djw-one-xdelta-3.2.0.raw.bin");
    const MULTI_PAYLOAD: &[u8] =
        include_bytes!("../tests/fixtures/xdelta/djw-multi-xdelta-3.2.0.payload.bin");
    const MULTI_RAW: &[u8] =
        include_bytes!("../tests/fixtures/xdelta/djw-multi-xdelta-3.2.0.raw.bin");

    #[derive(Debug, Default)]
    struct Trace {
        groups: usize,
        sector_size: usize,
        sectors: usize,
        implicit_with_run: usize,
        implicit_with_move: usize,
        bits: u64,
    }

    impl DecodeObserver for Trace {
        fn header(&mut self, groups: usize, sector_size: usize, sectors: usize) {
            self.groups = groups;
            self.sector_size = sector_size;
            self.sectors = sectors;
        }

        fn implicit_zero(&mut self, run_pending: bool, move_pending: bool) {
            self.implicit_with_run += usize::from(run_pending);
            self.implicit_with_move += usize::from(move_pending);
        }

        fn finished(&mut self, bits: u64) {
            self.bits = bits;
        }
    }

    #[derive(Default)]
    struct BitPacker {
        bytes: Vec<u8>,
        bit_count: usize,
    }

    impl BitPacker {
        fn bit(&mut self, value: u8) {
            let physical = self.bit_count % 8;
            if physical == 0 {
                self.bytes.push(0);
            }
            if value != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 1 << physical;
            }
            self.bit_count += 1;
        }

        fn field(&mut self, value: u32, width: u8) {
            for shift in (0..width).rev() {
                self.bit(((value >> shift) & 1) as u8);
            }
        }

        fn symbols(&mut self, book: &PrefixBook, symbols: &[u8]) {
            for &symbol in symbols {
                let (code, width) = book.encoding(symbol).unwrap();
                self.field(code, width);
            }
        }
    }

    fn anchor_decode(
        payload: &[u8],
        expected: &[u8],
        prior_resident: u64,
        max_window_memory: u64,
    ) -> Result<(Vec<u8>, Trace, u64), DecodeError> {
        let mut cursor = Cursor::new(payload);
        let mut input = DeltaInput::new(&mut cursor, payload.len() as u64)?;
        input.set_window(Some(3));
        input.set_section(Some(SectionKind::Data));
        let mut output = vec![0; expected.len()];
        let active_memory_after_output = prior_resident.checked_add(expected.len() as u64).unwrap();
        let mut trace = Trace::default();
        decode_observed(
            &mut input,
            payload.len() as u64,
            &mut output,
            active_memory_after_output,
            max_window_memory,
            &mut trace,
        )?;
        Ok((output, trace, input.position()))
    }

    fn decode_packed(
        book: &PrefixBook,
        bytes: &[u8],
        count: usize,
    ) -> Result<Vec<u8>, DecodeError> {
        let mut cursor = Cursor::new(bytes);
        let mut input = DeltaInput::new(&mut cursor, bytes.len() as u64)?;
        let mut bits = BitStream::new(&mut input, bytes.len() as u64);
        let mut output = Vec::with_capacity(count);
        for _ in 0..count {
            output.push(book.read_symbol(&mut bits)?);
        }
        Ok(output)
    }

    fn expand_sequence(
        token_count: usize,
        tokens: &[u8],
        machine: &mut TokenMachine,
        values: &mut [u8],
        skip_stride: usize,
        trace: &mut Trace,
    ) -> Result<(), DecodeError> {
        let width = token_count.ilog2() as u8;
        assert_eq!(1_usize << width, token_count);
        let lengths = vec![width; token_count];
        let book = PrefixBook::build(&lengths, MAX_TOKEN_WIDTH).unwrap();
        let mut packed = BitPacker::default();
        packed.symbols(&book, tokens);
        let mut cursor = Cursor::new(&packed.bytes);
        let mut input = DeltaInput::new(&mut cursor, packed.bytes.len() as u64).unwrap();
        let mut bits = BitStream::new(&mut input, packed.bytes.len() as u64);
        expand_tokens(&mut bits, &book, machine, values, skip_stride, trace)
    }

    #[test]
    fn bit_fields_use_lsb_physical_and_msb_logical_order() {
        let bytes = [0b0100_1101];
        let mut cursor = Cursor::new(bytes);
        let mut input = DeltaInput::new(&mut cursor, 1).unwrap();
        let mut bits = BitStream::new(&mut input, 1);
        assert_eq!(bits.read_field(3).unwrap(), 0b101);
        assert_eq!(bits.read_field(5).unwrap(), 0b10010);
        assert_eq!(input.position(), 1);
    }

    #[test]
    fn final_unused_bits_are_ignored_without_prefetching() {
        let (decoded, trace, consumed) = anchor_decode(ONE_PAYLOAD, ONE_RAW, 0, u64::MAX).unwrap();
        assert_eq!(decoded, ONE_RAW);
        assert_eq!(consumed, ONE_PAYLOAD.len() as u64);
        let used_in_last = (trace.bits % 8) as u8;
        assert_ne!(used_in_last, 0);

        let mut changed_padding = ONE_PAYLOAD.to_vec();
        let last = changed_padding.len() - 1;
        changed_padding[last] ^= 1 << used_in_last;
        let (decoded, _, consumed) = anchor_decode(&changed_padding, ONE_RAW, 0, u64::MAX).unwrap();
        assert_eq!(decoded, ONE_RAW);
        assert_eq!(consumed, ONE_PAYLOAD.len() as u64);

        let mut trailing = ONE_PAYLOAD.to_vec();
        trailing.push(0xa5);
        let (decoded, _, consumed) = anchor_decode(&trailing, ONE_RAW, 0, u64::MAX).unwrap();
        assert_eq!(decoded, ONE_RAW);
        assert_eq!(consumed, ONE_PAYLOAD.len() as u64);
        assert_eq!(trailing.len() as u64, consumed + 1);
    }

    #[test]
    fn canonical_order_is_length_then_symbol() {
        let book = PrefixBook::build(&[2, 3, 3, 2], 3).unwrap();
        assert_eq!(&book.ordered[..4], &[0, 3, 1, 2]);
        assert_eq!(book.encoding(0), Some((0, 2)));
        assert_eq!(book.encoding(3), Some((1, 2)));
        assert_eq!(book.encoding(1), Some((4, 3)));
        assert_eq!(book.encoding(2), Some((5, 3)));
    }

    #[test]
    fn canonical_validation_rejects_oversubscription_and_accepts_incomplete() {
        assert!(matches!(
            PrefixBook::build(&[1, 1, 1], 3),
            Err(DjwFault::InvalidTable)
        ));
        let incomplete = PrefixBook::build(&[1], 3).unwrap();
        assert_eq!(incomplete.lookup(1, 0), Some(0));
        assert_eq!(incomplete.lookup(1, 1), None);
        let empty = PrefixBook::build(&[0, 0], 3).unwrap();
        assert_eq!(empty.symbol_count, 0);

        let bytes = [0];
        let mut cursor = Cursor::new(bytes);
        let mut input = DeltaInput::new(&mut cursor, 1).unwrap();
        let mut bits = BitStream::new(&mut input, 1);
        assert!(empty.read_symbol(&mut bits).is_err());
    }

    #[test]
    fn canonical_offsets_are_strictly_bounded() {
        let book = PrefixBook::build(&[1, 2], 2).unwrap();
        assert_eq!(book.lookup(1, 0), Some(0));
        assert_eq!(book.lookup(1, 1), None);
        assert_eq!(book.lookup(2, 2), Some(1));
        assert_eq!(book.lookup(2, 3), None);
    }

    #[test]
    fn twenty_bit_codes_decode() {
        let book = PrefixBook::build(&[20, 20], 20).unwrap();
        let mut packed = BitPacker::default();
        packed.symbols(&book, &[0, 1]);
        assert_eq!(decode_packed(&book, &packed.bytes, 2).unwrap(), [0, 1]);
    }

    #[test]
    fn length_mtf_initialization_is_exact() {
        let list = MtfList::from_slice(&INITIAL_LENGTH_ORDER).unwrap();
        assert_eq!(list.len, 21);
        assert_eq!(list.values, INITIAL_LENGTH_ORDER);
    }

    #[test]
    fn run_tokens_accumulate_and_nonrun_resets_the_exponent() {
        let mut machine = TokenMachine::new(MtfList::identity(3).unwrap());
        let mut output = [0; 7];
        expand_sequence(
            4,
            &[0, 1, 2, 0],
            &mut machine,
            &mut output,
            0,
            &mut Trace::default(),
        )
        .unwrap();
        assert_eq!(output, [0, 0, 0, 0, 0, 1, 1]);
        assert_eq!(machine.run_exponent, 1);
    }

    #[test]
    fn runs_are_checked_for_overflow_and_output_overrun() {
        let mut overflow = TokenMachine::new(MtfList::identity(2).unwrap());
        overflow.run_exponent = usize::BITS;
        assert_eq!(overflow.queue(0), Err(DjwFault::RunOverflow));

        let mut overrun = TokenMachine::new(MtfList::identity(2).unwrap());
        let mut output = [0; 1];
        assert!(
            expand_sequence(2, &[1], &mut overrun, &mut output, 0, &mut Trace::default(),).is_err()
        );
    }

    #[test]
    fn nonzero_mtf_positions_move_to_front_and_are_bounded() {
        let mut list = MtfList::identity(4).unwrap();
        assert_eq!(list.promote(2).unwrap(), 2);
        assert_eq!(&list.values[..4], &[2, 0, 1, 3]);
        assert_eq!(list.promote(4), Err(DjwFault::InvalidMtf));
    }

    #[test]
    fn implicit_zero_precedes_and_pauses_pending_actions() {
        let mut machine = TokenMachine::new(MtfList::identity(3).unwrap());
        machine.run_remaining = 1;
        machine.pending_move = Some(1);
        let mut values = [0, 1, 1, 9, 9, 9];
        let mut trace = Trace::default();

        assert!(machine.advance(&mut values, 3, 3, &mut trace).unwrap());
        assert_eq!(values[3], 0);
        assert_eq!(machine.run_remaining, 1);
        assert_eq!(machine.pending_move, Some(1));
        assert_eq!(trace.implicit_with_run, 1);
        assert_eq!(trace.implicit_with_move, 1);

        assert!(machine.advance(&mut values, 4, 3, &mut trace).unwrap());
        assert_eq!(values[4], 0);
        assert_eq!(machine.run_remaining, 0);
        assert_eq!(machine.pending_move, Some(1));

        assert!(machine.advance(&mut values, 5, 3, &mut trace).unwrap());
        assert_eq!(values[5], 1);
        assert_eq!(machine.pending_move, None);
    }

    #[test]
    fn selector_tokens_and_last_partial_sector_decode() {
        let mut selector_machine = TokenMachine::new(MtfList::identity(3).unwrap());
        let mut selectors = [0; 4];
        expand_sequence(
            4,
            &[2, 0, 3, 0],
            &mut selector_machine,
            &mut selectors,
            0,
            &mut Trace::default(),
        )
        .unwrap();
        assert_eq!(selectors, [1, 1, 2, 2]);

        let mut lengths = [0_u8; BYTE_SYMBOLS];
        lengths[42] = 1;
        let mut books = [PrefixBook::EMPTY; MAX_GROUPS];
        books[0] = PrefixBook::build(&lengths, MAX_CODE_WIDTH).unwrap();
        books[1] = books[0];
        let bytes = [0];
        let mut cursor = Cursor::new(bytes);
        let mut input = DeltaInput::new(&mut cursor, 1).unwrap();
        let mut bits = BitStream::new(&mut input, 1);
        let mut output = [0; 3];
        decode_body(&mut bits, &books, 1, &[], 2, &mut output).unwrap();
        assert_eq!(output, [42; 3]);

        let mut cursor = Cursor::new(bytes);
        let mut input = DeltaInput::new(&mut cursor, 1).unwrap();
        let mut bits = BitStream::new(&mut input, 1);
        assert!(decode_body(&mut bits, &books, 2, &[2], 1, &mut output[..1]).is_err());
    }

    #[test]
    fn external_one_and_multi_table_payloads_decode_exactly() {
        let (one, one_trace, one_consumed) =
            anchor_decode(ONE_PAYLOAD, ONE_RAW, 0, u64::MAX).unwrap();
        assert_eq!(one, ONE_RAW);
        assert_eq!(one_trace.groups, 1);
        assert_eq!(one_trace.sectors, 1);
        assert_eq!(one_consumed, ONE_PAYLOAD.len() as u64);

        let (multi, multi_trace, multi_consumed) =
            anchor_decode(MULTI_PAYLOAD, MULTI_RAW, 0, u64::MAX).unwrap();
        assert_eq!(multi, MULTI_RAW);
        assert_eq!(multi_trace.groups, 2);
        assert_eq!(multi_trace.sector_size, 10);
        assert_eq!(multi_trace.sectors, 200);
        assert_eq!(
            multi_trace.sectors,
            MULTI_RAW.len().div_ceil(multi_trace.sector_size)
        );
        assert_eq!(multi_consumed, MULTI_PAYLOAD.len() as u64);
    }

    #[test]
    fn external_multi_table_trace_straddles_implicit_zero_with_a_run() {
        let (_, trace, _) = anchor_decode(MULTI_PAYLOAD, MULTI_RAW, 0, u64::MAX).unwrap();
        assert_eq!(trace.implicit_with_run, 48);
    }

    #[test]
    fn zero_truncation_and_malformed_tables_are_rejected() {
        let mut empty_cursor = Cursor::new([]);
        let mut empty_input = DeltaInput::new(&mut empty_cursor, 0).unwrap();
        assert!(decode_section(&mut empty_input, 0, &mut [], 0, 0).is_err());

        for bytes in [&[][..], &[0][..]] {
            let mut cursor = Cursor::new(bytes);
            let mut input = DeltaInput::new(&mut cursor, bytes.len() as u64).unwrap();
            assert!(decode_section(&mut input, bytes.len() as u64, &mut [0], 1, u64::MAX).is_err());
        }

        let mut oversubscribed = BitPacker::default();
        oversubscribed.field(0, 3);
        oversubscribed.field(0, 4);
        for width in [1, 1, 1, 0, 0, 0, 0] {
            oversubscribed.field(width, 4);
        }
        let mut cursor = Cursor::new(&oversubscribed.bytes);
        let mut input = DeltaInput::new(&mut cursor, oversubscribed.bytes.len() as u64).unwrap();
        assert!(
            decode_section(
                &mut input,
                oversubscribed.bytes.len() as u64,
                &mut [0],
                1,
                u64::MAX,
            )
            .is_err()
        );

        let mut empty_group = BitPacker::default();
        empty_group.field(0, 3);
        empty_group.field(0, 4);
        for width in [1, 1, 0, 0, 0, 0, 0] {
            empty_group.field(width, 4);
        }
        let token_book = PrefixBook::build(
            &[
                1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            15,
        )
        .unwrap();
        empty_group.symbols(&token_book, &[1, 0, 0, 0, 0, 0, 0, 0]);
        let mut cursor = Cursor::new(&empty_group.bytes);
        let mut input = DeltaInput::new(&mut cursor, empty_group.bytes.len() as u64).unwrap();
        assert!(
            decode_section(
                &mut input,
                empty_group.bytes.len() as u64,
                &mut [0],
                1,
                u64::MAX,
            )
            .is_err()
        );

        let truncated = &ONE_PAYLOAD[..ONE_PAYLOAD.len() - 1];
        let mut cursor = Cursor::new(truncated);
        let mut input = DeltaInput::new(&mut cursor, truncated.len() as u64).unwrap();
        assert!(
            decode_section(
                &mut input,
                truncated.len() as u64,
                &mut vec![0; ONE_RAW.len()],
                ONE_RAW.len() as u64,
                u64::MAX,
            )
            .is_err()
        );
    }

    #[test]
    fn selector_scratch_counts_prior_resident_memory() {
        let (_, trace, _) = anchor_decode(MULTI_PAYLOAD, MULTI_RAW, 0, u64::MAX).unwrap();
        let required = trace.sectors as u64;
        let prior_resident = 777_u64;
        let active_memory_after_output = prior_resident + MULTI_RAW.len() as u64;
        let max_window_memory = active_memory_after_output + required - 1;
        let error =
            anchor_decode(MULTI_PAYLOAD, MULTI_RAW, prior_resident, max_window_memory).unwrap_err();
        assert!(matches!(
            error,
            DecodeError::WindowMemoryLimit {
                attempted,
                limit,
                ..
            } if attempted == active_memory_after_output + required
                && limit == max_window_memory
        ));
        assert!(
            anchor_decode(
                MULTI_PAYLOAD,
                MULTI_RAW,
                prior_resident,
                active_memory_after_output + required,
            )
            .is_ok()
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn selector_allocation_failure_is_reported_directly() {
        let context = DecodeContext::new(4, Some(1), Some(SectionKind::Data));
        assert!(matches!(
            allocate_selectors(usize::MAX as u64, 0, u64::MAX, context),
            Err(DecodeError::AllocationFailed {
                requested,
                context: actual,
            }) if requested == usize::MAX as u64 && actual == context
        ));
    }

    #[derive(Debug)]
    struct ReadMarker(u32);

    impl fmt::Display for ReadMarker {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "DJW read marker {}", self.0)
        }
    }

    impl Error for ReadMarker {}

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other(ReadMarker(91)))
        }
    }

    impl Seek for FailingReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            match position {
                SeekFrom::Start(position) => Ok(position),
                SeekFrom::Current(0) => Ok(0),
                SeekFrom::End(0) => Ok(1),
                _ => Err(io::Error::other("unsupported test seek")),
            }
        }
    }

    #[test]
    fn delta_io_retains_its_original_source() {
        let mut reader = FailingReader;
        let mut input = DeltaInput::new(&mut reader, 1).unwrap();
        let error = decode_section(&mut input, 1, &mut [0], 1, u64::MAX).unwrap_err();
        match error {
            DecodeError::DeltaIo {
                operation: IoOperation::Read,
                source,
                ..
            } => {
                let marker = source
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<ReadMarker>())
                    .unwrap();
                assert_eq!(marker.0, 91);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn every_anchor_truncation_is_rejected_without_panicking() {
        for (payload, output_len) in [
            (ONE_PAYLOAD, ONE_RAW.len()),
            (MULTI_PAYLOAD, MULTI_RAW.len()),
        ] {
            for end in 0..payload.len() {
                let mut cursor = Cursor::new(&payload[..end]);
                let mut input = DeltaInput::new(&mut cursor, end as u64).unwrap();
                let mut output = vec![0; output_len];
                assert!(
                    decode_section(
                        &mut input,
                        end as u64,
                        &mut output,
                        output_len as u64,
                        u64::MAX,
                    )
                    .is_err()
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn canonical_tables_round_trip_small_symbol_streams(
            width in 1_u8..=4,
            choices in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            let symbol_count = 1_usize << width;
            let lengths = vec![width; symbol_count];
            let book = PrefixBook::build(&lengths, MAX_CODE_WIDTH).unwrap();
            let symbols: Vec<u8> = choices
                .into_iter()
                .map(|value| value % symbol_count as u8)
                .collect();
            let mut packed = BitPacker::default();
            packed.symbols(&book, &symbols);
            prop_assert_eq!(decode_packed(&book, &packed.bytes, symbols.len()).unwrap(), symbols);
        }

        #[test]
        fn arbitrary_bounded_payloads_never_panic_or_resize_output(
            payload in prop::collection::vec(any::<u8>(), 0..64),
            output_len in 0_usize..64,
        ) {
            let mut cursor = Cursor::new(&payload);
            let mut input = DeltaInput::new(&mut cursor, payload.len() as u64).unwrap();
            let mut output = vec![0; output_len];
            let _result = decode_section(
                &mut input,
                payload.len() as u64,
                &mut output,
                output_len as u64,
                u64::MAX,
            );
            prop_assert_eq!(output.len(), output_len);
        }

        #[test]
        fn bounded_anchor_bit_mutations_never_panic(
            multi in any::<bool>(),
            bit_seed in any::<usize>(),
        ) {
            let (payload, output_len) = if multi {
                (MULTI_PAYLOAD, MULTI_RAW.len())
            } else {
                (ONE_PAYLOAD, ONE_RAW.len())
            };
            let mut changed = payload.to_vec();
            let bit = bit_seed % (changed.len() * 8);
            changed[bit / 8] ^= 1 << (bit % 8);
            let mut cursor = Cursor::new(&changed);
            let mut input = DeltaInput::new(&mut cursor, changed.len() as u64).unwrap();
            let mut output = vec![0; output_len];
            let _result = decode_section(
                &mut input,
                changed.len() as u64,
                &mut output,
                output_len as u64,
                u64::MAX,
            );
            prop_assert_eq!(output.len(), output_len);
        }
    }
}
