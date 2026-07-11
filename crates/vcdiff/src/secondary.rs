//! Persistent xdelta ID-2 section decoding

use std::io::{Read, Seek};

use xz4rust::{DICT_SIZE_MAX, DICT_SIZE_MIN, XzDecoder, XzError};

use crate::error::{DecodeContext, DecodeError, SecondaryError, SectionKind};
use crate::input::DeltaInput;

pub(crate) const XDELTA_LZMA_ID: u8 = 2;
const STAGING_SIZE: usize = 64 * 1024;

/// One decoded section and its encoded delta offset
pub(crate) struct PreparedSection {
    pub(crate) bytes: Vec<u8>,
    pub(crate) delta_offset: u64,
}

/// Three lazy persistent secondary streams and one fixed staging buffer
pub(crate) struct SecondaryStates {
    decoders: [Option<Box<XzDecoder<'static>>>; 3],
    staging: Vec<u8>,
    max_dictionary_size: u64,
}

impl SecondaryStates {
    /// Create empty persistent state with a caller-controlled dictionary cap
    pub(crate) fn new(max_dictionary_size: u64) -> Self {
        Self {
            decoders: [None, None, None],
            staging: Vec::new(),
            max_dictionary_size,
        }
    }

    /// Prepare one raw or ID-2-compressed section
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare<D: Read + Seek>(
        &mut self,
        input: &mut DeltaInput<'_, D>,
        encoded_len: u64,
        compressed: bool,
        kind: SectionKind,
        window: u64,
        active_memory: &mut u64,
        max_window_memory: u64,
    ) -> Result<PreparedSection, DecodeError> {
        input.set_section(Some(kind));
        let section_start = input.position();
        let context = DecodeContext::new(section_start, Some(window), Some(kind));
        if !compressed {
            let mut bytes =
                allocate_section(encoded_len, context, active_memory, max_window_memory)?;
            input.read_exact(&mut bytes)?;
            return Ok(PreparedSection {
                bytes,
                delta_offset: section_start,
            });
        }

        let section_end = section_start
            .checked_add(encoded_len)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        if section_end > input.len() {
            return Err(DecodeError::TruncatedDelta { context });
        }
        let decoded_size = read_bounded_varint(input, section_end, context)?;
        if decoded_size == 0 {
            return Err(DecodeError::InvalidSecondarySize {
                value: decoded_size,
                context,
            });
        }
        let fragment_start = input.position();
        let fragment_len = section_end
            .checked_sub(fragment_start)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        let mut output = allocate_section(decoded_size, context, active_memory, max_window_memory)?;

        self.ensure_staging(context)?;
        let first_fragment = self.decoders[kind.index()].is_none();
        self.ensure_decoder(kind, context)?;
        let Some(decoder) = self.decoders[kind.index()].as_deref_mut() else {
            return Err(malformed(context));
        };
        decode_fragment(
            decoder,
            &mut self.staging,
            input,
            section_end,
            fragment_len,
            decoded_size,
            &mut output,
            self.max_dictionary_size,
            first_fragment,
            context,
        )?;
        Ok(PreparedSection {
            bytes: output,
            delta_offset: section_start,
        })
    }

    fn ensure_staging(&mut self, context: DecodeContext) -> Result<(), DecodeError> {
        if self.staging.len() == STAGING_SIZE {
            return Ok(());
        }
        self.staging.try_reserve_exact(STAGING_SIZE).map_err(|_| {
            DecodeError::AllocationFailed {
                requested: STAGING_SIZE as u64,
                context,
            }
        })?;
        self.staging.resize(STAGING_SIZE, 0);
        Ok(())
    }

    fn ensure_decoder(
        &mut self,
        kind: SectionKind,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        if self.decoders[kind.index()].is_some() {
            return Ok(());
        }
        if self.max_dictionary_size < DICT_SIZE_MIN as u64 {
            return Err(DecodeError::SecondaryDictionaryLimit {
                required: DICT_SIZE_MIN as u64,
                limit: self.max_dictionary_size,
                context,
            });
        }
        let effective_limit = effective_dictionary_limit(self.max_dictionary_size);
        let max_dictionary_size =
            usize::try_from(effective_limit).map_err(|_| DecodeError::PlatformSizeLimit {
                value: effective_limit,
                context,
            })?;
        self.decoders[kind.index()] = Some(XzDecoder::in_heap_with_alloc_dict_size(
            DICT_SIZE_MIN,
            max_dictionary_size,
        ));
        Ok(())
    }

    #[cfg(test)]
    fn is_initialized(&self, kind: SectionKind) -> bool {
        self.decoders[kind.index()].is_some()
    }
}

fn allocate_section(
    len: u64,
    context: DecodeContext,
    active_memory: &mut u64,
    max_window_memory: u64,
) -> Result<Vec<u8>, DecodeError> {
    let attempted = active_memory
        .checked_add(len)
        .ok_or(DecodeError::ArithmeticOverflow { context })?;
    if attempted > max_window_memory {
        return Err(DecodeError::WindowMemoryLimit {
            attempted,
            limit: max_window_memory,
            context,
        });
    }
    let len_usize = usize::try_from(len).map_err(|_| DecodeError::PlatformSizeLimit {
        value: len,
        context,
    })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len_usize)
        .map_err(|_| DecodeError::AllocationFailed {
            requested: len,
            context,
        })?;
    bytes.resize(len_usize, 0);
    *active_memory = attempted;
    Ok(bytes)
}

fn read_bounded_varint<D: Read + Seek>(
    input: &mut DeltaInput<'_, D>,
    section_end: u64,
    context: DecodeContext,
) -> Result<u64, DecodeError> {
    let mut value = 0_u64;
    for _ in 0..10 {
        if input.position() >= section_end {
            return Err(malformed(context));
        }
        let byte = input.read_u8()?;
        value = value
            .checked_mul(128)
            .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
            .ok_or(DecodeError::VarintOverflow { context })?;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DecodeError::VarintOverflow { context })
}

#[allow(clippy::too_many_arguments)]
fn decode_fragment<D: Read + Seek>(
    decoder: &mut XzDecoder<'_>,
    staging: &mut [u8],
    input: &mut DeltaInput<'_, D>,
    section_end: u64,
    fragment_len: u64,
    decoded_size: u64,
    output: &mut [u8],
    dictionary_limit: u64,
    first_fragment: bool,
    context: DecodeContext,
) -> Result<(), DecodeError> {
    let mut staging_start = 0_usize;
    let mut staging_end = 0_usize;
    let mut consumed = 0_u64;
    let mut produced = 0_usize;
    let mut header_checked = !first_fragment;

    loop {
        if staging_start == staging_end {
            staging_start = 0;
            staging_end = 0;
            if input.position() == section_end {
                break;
            }
            refill_staging(
                staging,
                &mut staging_start,
                &mut staging_end,
                input,
                section_end,
            )?;
        }
        if !header_checked && staging_end - staging_start >= 8 {
            if staging[staging_start + 7] != 0 {
                return Err(malformed(context));
            }
            header_checked = true;
        }

        let result = match decoder.decode(
            &staging[staging_start..staging_end],
            &mut output[produced..],
        ) {
            Ok(result) => result,
            Err(XzError::NeedsLargerInputBuffer) => {
                let buffered = staging_end - staging_start;
                if buffered == staging.len() || input.position() == section_end {
                    return Err(malformed(context));
                }
                refill_staging(
                    staging,
                    &mut staging_start,
                    &mut staging_end,
                    input,
                    section_end,
                )?;
                continue;
            }
            Err(XzError::DictionaryTooLarge(required)) => {
                return Err(dictionary_limit_error(required, dictionary_limit, context));
            }
            Err(source) => {
                return Err(DecodeError::SecondaryDecompression {
                    compressor_id: XDELTA_LZMA_ID,
                    context,
                    source: SecondaryError::new(source),
                });
            }
        };

        if result.is_end_of_stream() {
            return Err(malformed(context));
        }
        let input_consumed = result.input_consumed();
        let output_produced = result.output_produced();
        if input_consumed > staging_end - staging_start || output_produced > output.len() - produced
        {
            return Err(malformed(context));
        }
        if input_consumed == 0 && output_produced == 0 {
            return Err(malformed(context));
        }
        staging_start += input_consumed;
        produced += output_produced;
        consumed = consumed
            .checked_add(input_consumed as u64)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        if decoder.is_lzma2_chunk_boundary()
            && produced == output.len()
            && (staging_start != staging_end || input.position() != section_end)
        {
            return Err(DecodeError::SecondaryInputMismatch {
                expected: fragment_len,
                actual: consumed,
                context,
            });
        }
    }

    let produced_u64 = u64::try_from(produced).map_err(|_| DecodeError::PlatformSizeLimit {
        value: u64::MAX,
        context,
    })?;
    if produced_u64 != decoded_size {
        return Err(DecodeError::SecondarySizeMismatch {
            expected: decoded_size,
            actual: produced_u64,
            context,
        });
    }
    if consumed != fragment_len {
        return Err(DecodeError::SecondaryInputMismatch {
            expected: fragment_len,
            actual: consumed,
            context,
        });
    }
    if !decoder.is_lzma2_chunk_boundary() {
        return Err(malformed(context));
    }
    Ok(())
}

fn effective_dictionary_limit(configured_limit: u64) -> u64 {
    configured_limit
        .min(isize::MAX as u64)
        .min(DICT_SIZE_MAX as u64)
}

fn dictionary_limit_error(
    required: u64,
    configured_limit: u64,
    context: DecodeContext,
) -> DecodeError {
    if required > configured_limit {
        return DecodeError::SecondaryDictionaryLimit {
            required,
            limit: configured_limit,
            context,
        };
    }
    if required > isize::MAX as u64 {
        return DecodeError::PlatformSizeLimit {
            value: required,
            context,
        };
    }
    DecodeError::SecondaryDecompression {
        compressor_id: XDELTA_LZMA_ID,
        context,
        source: SecondaryError::new(XzError::DictionaryTooLarge(required)),
    }
}

fn refill_staging<D: Read + Seek>(
    staging: &mut [u8],
    start: &mut usize,
    end: &mut usize,
    input: &mut DeltaInput<'_, D>,
    section_end: u64,
) -> Result<(), DecodeError> {
    if *start != 0 {
        staging.copy_within(*start..*end, 0);
        *end -= *start;
        *start = 0;
    }
    let remaining = section_end.checked_sub(input.position()).ok_or_else(|| {
        DecodeError::ArithmeticOverflow {
            context: input.context(),
        }
    })?;
    let space = staging.len() - *end;
    let count_u64 = remaining.min(space as u64);
    let count = usize::try_from(count_u64).map_err(|_| DecodeError::PlatformSizeLimit {
        value: count_u64,
        context: input.context(),
    })?;
    if count == 0 {
        return Err(malformed(input.context()));
    }
    input.read_exact(&mut staging[*end..*end + count])?;
    *end += count;
    Ok(())
}

fn malformed(context: DecodeContext) -> DecodeError {
    DecodeError::MalformedSecondarySection {
        compressor_id: XDELTA_LZMA_ID,
        context,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io::Cursor;

    use xz4rust::XzNextBlockResult;

    use super::*;

    const DELTA: &[u8] = include_bytes!("../tests/fixtures/xdelta/xdelta-3.2.0-lzma.vcdiff");
    const TARGET: &[u8] = include_bytes!("../tests/fixtures/xdelta/target.bin");
    const MAX_DICTIONARY: u64 = 64 * 1024 * 1024;

    struct FixtureWindow<'a> {
        payload: &'a [u8],
        fragment: &'a [u8],
        decoded_size: u64,
        expected: &'a [u8],
    }

    struct FixtureCursor<'a> {
        bytes: &'a [u8],
        pos: usize,
    }

    impl<'a> FixtureCursor<'a> {
        const fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, pos: 0 }
        }

        fn byte(&mut self) -> u8 {
            let byte = self.bytes[self.pos];
            self.pos += 1;
            byte
        }

        fn take(&mut self, len: usize) -> &'a [u8] {
            let start = self.pos;
            self.pos += len;
            &self.bytes[start..self.pos]
        }

        fn varint(&mut self) -> u64 {
            let mut value = 0_u64;
            loop {
                let byte = self.byte();
                value = (value << 7) | u64::from(byte & 0x7f);
                if byte & 0x80 == 0 {
                    return value;
                }
            }
        }
    }

    fn fixture_windows() -> Vec<FixtureWindow<'static>> {
        let mut cursor = FixtureCursor::new(DELTA);
        assert_eq!(cursor.take(3), [0xD6, 0xC3, 0xC4]);
        assert_eq!(cursor.byte(), 0);
        assert_eq!(cursor.byte(), 1);
        assert_eq!(cursor.byte(), XDELTA_LZMA_ID);
        let mut target_start = 0_usize;
        let mut windows = Vec::new();
        while cursor.pos < DELTA.len() {
            let window_indicator = cursor.byte();
            assert_eq!(window_indicator & 0x03, 0);
            let encoding_len = cursor.varint() as usize;
            let encoding_start = cursor.pos;
            let target_size = cursor.varint() as usize;
            assert_eq!(cursor.byte(), 1);
            let data_len = cursor.varint() as usize;
            let instructions_len = cursor.varint() as usize;
            let addresses_len = cursor.varint() as usize;
            assert_ne!(window_indicator & 0x04, 0);
            cursor.take(4);
            let payload = cursor.take(data_len);
            cursor.take(instructions_len);
            cursor.take(addresses_len);
            assert_eq!(cursor.pos, encoding_start + encoding_len);

            let mut payload_cursor = FixtureCursor::new(payload);
            let decoded_size = payload_cursor.varint();
            let fragment = &payload[payload_cursor.pos..];
            let target_end = target_start + target_size;
            windows.push(FixtureWindow {
                payload,
                fragment,
                decoded_size,
                expected: &TARGET[target_start..target_end],
            });
            target_start = target_end;
        }
        assert_eq!(target_start, TARGET.len());
        assert_eq!(windows.len(), 6);
        windows
    }

    fn prepare_payload(
        states: &mut SecondaryStates,
        kind: SectionKind,
        payload: &[u8],
        compressed: bool,
        window: u64,
    ) -> Result<Vec<u8>, DecodeError> {
        let mut stream = Cursor::new(payload);
        let mut input = DeltaInput::new(&mut stream, payload.len() as u64)?;
        input.set_window(Some(window));
        let mut active_memory = 0;
        states
            .prepare(
                &mut input,
                payload.len() as u64,
                compressed,
                kind,
                window,
                &mut active_memory,
                u64::MAX,
            )
            .map(|section| section.bytes)
    }

    fn seeded_states(kind: SectionKind, count: usize) -> SecondaryStates {
        let windows = fixture_windows();
        let mut states = SecondaryStates::new(MAX_DICTIONARY);
        for (index, window) in windows.iter().take(count).enumerate() {
            let decoded =
                prepare_payload(&mut states, kind, window.payload, true, index as u64).unwrap();
            assert_eq!(decoded, window.expected);
        }
        states
    }

    fn seeded_decoder(count: usize) -> Box<XzDecoder<'static>> {
        let windows = fixture_windows();
        let mut decoder =
            XzDecoder::in_heap_with_alloc_dict_size(DICT_SIZE_MIN, MAX_DICTIONARY as usize);
        for window in windows.iter().take(count) {
            let mut output = vec![0; window.decoded_size as usize];
            let result = decoder.decode(window.fragment, &mut output).unwrap();
            assert!(matches!(result, XzNextBlockResult::NeedMoreData(_, _)));
            assert_eq!(result.input_consumed(), window.fragment.len());
            assert_eq!(result.output_produced(), output.len());
            assert!(decoder.is_lzma2_chunk_boundary());
            assert_eq!(output, window.expected);
        }
        decoder
    }

    fn varint(value: u64) -> Vec<u8> {
        let mut bytes = vec![(value & 0x7f) as u8];
        let mut remaining = value >> 7;
        while remaining != 0 {
            bytes.push(0x80 | (remaining & 0x7f) as u8);
            remaining >>= 7;
        }
        bytes.reverse();
        bytes
    }

    fn payload_with_size(decoded_size: u64, fragment: &[u8]) -> Vec<u8> {
        let mut payload = varint(decoded_size);
        payload.extend_from_slice(fragment);
        payload
    }

    #[test]
    fn later_fragments_require_persistent_state() {
        let windows = fixture_windows();
        let mut persistent = seeded_states(SectionKind::Data, 2);
        let decoded = prepare_payload(
            &mut persistent,
            SectionKind::Data,
            windows[2].payload,
            true,
            2,
        )
        .unwrap();
        assert_eq!(decoded, windows[2].expected);

        let mut fresh = SecondaryStates::new(MAX_DICTIONARY);
        assert!(
            prepare_payload(&mut fresh, SectionKind::Data, windows[2].payload, true, 2).is_err()
        );
    }

    #[test]
    fn each_section_kind_has_an_independent_persistent_stream() {
        let windows = fixture_windows();
        let mut states = SecondaryStates::new(MAX_DICTIONARY);
        for (window_index, window) in windows.iter().enumerate() {
            for kind in [
                SectionKind::Data,
                SectionKind::Instructions,
                SectionKind::Addresses,
            ] {
                let decoded =
                    prepare_payload(&mut states, kind, window.payload, true, window_index as u64)
                        .unwrap();
                assert_eq!(decoded, window.expected);
            }
        }
        assert!(states.is_initialized(SectionKind::Data));
        assert!(states.is_initialized(SectionKind::Instructions));
        assert!(states.is_initialized(SectionKind::Addresses));
    }

    #[test]
    fn raw_sections_do_not_initialize_or_advance_secondary_state() {
        let windows = fixture_windows();
        let mut states = SecondaryStates::new(MAX_DICTIONARY);
        assert_eq!(
            prepare_payload(&mut states, SectionKind::Data, b"raw", false, 0).unwrap(),
            b"raw"
        );
        assert!(!states.is_initialized(SectionKind::Data));

        assert_eq!(
            prepare_payload(&mut states, SectionKind::Data, windows[0].payload, true, 0).unwrap(),
            windows[0].expected
        );
        assert_eq!(
            prepare_payload(&mut states, SectionKind::Data, b"still raw", false, 1).unwrap(),
            b"still raw"
        );
        assert_eq!(
            prepare_payload(&mut states, SectionKind::Data, windows[1].payload, true, 1).unwrap(),
            windows[1].expected
        );
    }

    #[test]
    fn xz_decoder_accepts_reviewed_chunk_shapes() {
        let windows = fixture_windows();
        for (input_chunk, output_chunk) in [(256, 16_384), (257, 1_024), (1_024, 257)] {
            let mut decoder =
                XzDecoder::in_heap_with_alloc_dict_size(DICT_SIZE_MIN, MAX_DICTIONARY as usize);
            for window in &windows {
                let mut output = vec![0; window.decoded_size as usize];
                let mut input_position = 0;
                let mut output_position = 0;
                while input_position < window.fragment.len() || output_position < output.len() {
                    let input_end = (input_position + input_chunk).min(window.fragment.len());
                    let output_end = (output_position + output_chunk).min(output.len());
                    let result = decoder
                        .decode(
                            &window.fragment[input_position..input_end],
                            &mut output[output_position..output_end],
                        )
                        .unwrap();
                    assert!(!result.is_end_of_stream());
                    assert!(result.made_progress());
                    input_position += result.input_consumed();
                    output_position += result.output_produced();
                }
                assert_eq!(input_position, window.fragment.len());
                assert_eq!(output_position, output.len());
                assert!(decoder.is_lzma2_chunk_boundary());
                assert_eq!(output, window.expected);
            }
        }
    }

    #[test]
    fn malformed_fragments_and_sizes_are_rejected() {
        let windows = fixture_windows();
        let window = &windows[2];

        let truncated = &window.payload[..window.payload.len() - 1];
        let mut states = seeded_states(SectionKind::Data, 2);
        assert!(prepare_payload(&mut states, SectionKind::Data, truncated, true, 2).is_err());

        let mut appended = window.payload.to_vec();
        appended.push(0);
        let mut states = seeded_states(SectionKind::Data, 2);
        assert!(matches!(
            prepare_payload(&mut states, SectionKind::Data, &appended, true, 2),
            Err(DecodeError::MalformedSecondarySection { .. })
        ));

        let mut mutated = window.payload.to_vec();
        let mutation_offset = mutated.len() / 2;
        mutated[mutation_offset] ^= 1;
        let mut states = seeded_states(SectionKind::Data, 2);
        let error = prepare_payload(&mut states, SectionKind::Data, &mutated, true, 2).unwrap_err();
        assert!(matches!(
            error,
            DecodeError::SecondaryDecompression {
                compressor_id: XDELTA_LZMA_ID,
                context,
                ..
            } if context.window == Some(2) && context.section == Some(SectionKind::Data)
        ));
        assert!(error.source().is_some());

        let smaller = payload_with_size(window.decoded_size - 1, window.fragment);
        let mut states = seeded_states(SectionKind::Data, 2);
        assert!(matches!(
            prepare_payload(&mut states, SectionKind::Data, &smaller, true, 2),
            Err(DecodeError::MalformedSecondarySection { .. })
        ));

        let larger = payload_with_size(window.decoded_size + 1, window.fragment);
        let mut states = seeded_states(SectionKind::Data, 2);
        assert!(matches!(
            prepare_payload(&mut states, SectionKind::Data, &larger, true, 2),
            Err(DecodeError::SecondarySizeMismatch {
                expected,
                actual,
                ..
            }) if expected == window.decoded_size + 1 && actual == window.decoded_size
        ));
    }

    #[test]
    fn chunk_boundary_rejects_cases_that_public_counters_accept() {
        let windows = fixture_windows();
        let window = &windows[2];

        let mut appended = window.fragment.to_vec();
        appended.push(0);
        let mut decoder = seeded_decoder(2);
        let mut output = vec![0; window.decoded_size as usize];
        let result = decoder.decode(&appended, &mut output).unwrap();
        assert!(matches!(result, XzNextBlockResult::NeedMoreData(_, _)));
        assert_eq!(result.input_consumed(), appended.len());
        assert_eq!(result.output_produced(), output.len());
        assert!(!decoder.is_lzma2_chunk_boundary());

        let mut decoder = seeded_decoder(2);
        let mut output = vec![0; window.decoded_size as usize - 1];
        let result = decoder.decode(window.fragment, &mut output).unwrap();
        assert!(matches!(result, XzNextBlockResult::NeedMoreData(_, _)));
        assert_eq!(result.input_consumed(), window.fragment.len());
        assert_eq!(result.output_produced(), output.len());
        assert!(!decoder.is_lzma2_chunk_boundary());
    }

    #[test]
    fn zero_decoded_size_is_rejected_before_state_initialization() {
        let mut states = SecondaryStates::new(MAX_DICTIONARY);
        assert!(matches!(
            prepare_payload(&mut states, SectionKind::Data, &[0], true, 0),
            Err(DecodeError::InvalidSecondarySize { value: 0, .. })
        ));
        assert!(!states.is_initialized(SectionKind::Data));
    }

    #[test]
    fn effective_dictionary_cap_clamps_unlimited_policy() {
        let expected = (isize::MAX as u64).min(DICT_SIZE_MAX as u64);
        let effective = effective_dictionary_limit(u64::MAX);
        assert_eq!(effective, expected);
        assert!(usize::try_from(effective).is_ok());
    }

    #[test]
    fn dictionary_capacity_overflow_is_a_platform_error() {
        let required = isize::MAX as u64 + 1;
        let context = DecodeContext::new(17, Some(2), Some(SectionKind::Data));
        assert!(matches!(
            dictionary_limit_error(required, u64::MAX, context),
            DecodeError::PlatformSizeLimit {
                value,
                context: actual,
            } if value == required && actual == context
        ));
    }

    #[test]
    fn dictionary_policy_below_codec_minimum_is_rejected() {
        let windows = fixture_windows();
        let configured_limit = DICT_SIZE_MIN as u64 - 1;
        let mut states = SecondaryStates::new(configured_limit);
        assert!(matches!(
            prepare_payload(
                &mut states,
                SectionKind::Data,
                windows[0].payload,
                true,
                0,
            ),
            Err(DecodeError::SecondaryDictionaryLimit {
                required,
                limit,
                ..
            }) if required == DICT_SIZE_MIN as u64 && limit == configured_limit
        ));
        assert!(!states.is_initialized(SectionKind::Data));
    }

    #[test]
    fn id2_requires_lzma_check_none() {
        let windows = fixture_windows();
        let window = &windows[0];
        let mut payload = window.payload.to_vec();
        let fragment_start = payload.len() - window.fragment.len();
        payload[fragment_start + 7] = 1;
        let mut states = SecondaryStates::new(MAX_DICTIONARY);
        assert!(matches!(
            prepare_payload(&mut states, SectionKind::Data, &payload, true, 0),
            Err(DecodeError::MalformedSecondarySection { .. })
        ));
    }
}
