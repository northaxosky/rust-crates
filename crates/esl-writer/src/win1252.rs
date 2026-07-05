//! Windows-1252 (cp1252) encoding for plugin zstrings.

use crate::error::WriteError;

/// Encode `value` as Windows-1252, erroring on the first unrepresentable character
pub(crate) fn encode_win1252(value: &str, field: &'static str) -> Result<Vec<u8>, WriteError> {
    value
        .chars()
        .map(|ch| win1252_byte(ch).ok_or(WriteError::Encoding { field, ch }))
        .collect()
}

/// Map one char to its Windows-1252 byte, if representable
pub(crate) fn win1252_byte(ch: char) -> Option<u8> {
    match ch {
        '\u{0000}'..='\u{007F}' => Some(ch as u8), // ASCII
        '\u{00A0}'..='\u{00FF}' => Some(ch as u8), // Latin-1 high range
        '\u{20AC}' => Some(0x80),                  // euro sign
        '\u{201A}' => Some(0x82),                  // single low quote
        '\u{0192}' => Some(0x83),                  // florin
        '\u{201E}' => Some(0x84),                  // double low quote
        '\u{2026}' => Some(0x85),                  // ellipsis
        '\u{2020}' => Some(0x86),                  // dagger
        '\u{2021}' => Some(0x87),                  // double dagger
        '\u{02C6}' => Some(0x88),                  // modifier circumflex
        '\u{2030}' => Some(0x89),                  // per mille
        '\u{0160}' => Some(0x8A),                  // S with caron
        '\u{2039}' => Some(0x8B),                  // single left angle quote
        '\u{0152}' => Some(0x8C),                  // OE ligature
        '\u{017D}' => Some(0x8E),                  // Z with caron
        '\u{2018}' => Some(0x91),                  // left single quote
        '\u{2019}' => Some(0x92),                  // right single quote
        '\u{201C}' => Some(0x93),                  // left double quote
        '\u{201D}' => Some(0x94),                  // right double quote
        '\u{2022}' => Some(0x95),                  // bullet
        '\u{2013}' => Some(0x96),                  // en dash
        '\u{2014}' => Some(0x97),                  // em dash
        '\u{02DC}' => Some(0x98),                  // small tilde
        '\u{2122}' => Some(0x99),                  // trademark
        '\u{0161}' => Some(0x9A),                  // s with caron
        '\u{203A}' => Some(0x9B),                  // single right angle quote
        '\u{0153}' => Some(0x9C),                  // oe ligature
        '\u{017E}' => Some(0x9E),                  // z with caron
        '\u{0178}' => Some(0x9F),                  // Y with diaeresis
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 27 code points cp1252 places in the 0x80-0x9F range, each with its byte
    const SPECIALS: &[(char, u8)] = &[
        ('\u{20AC}', 0x80),
        ('\u{201A}', 0x82),
        ('\u{0192}', 0x83),
        ('\u{201E}', 0x84),
        ('\u{2026}', 0x85),
        ('\u{2020}', 0x86),
        ('\u{2021}', 0x87),
        ('\u{02C6}', 0x88),
        ('\u{2030}', 0x89),
        ('\u{0160}', 0x8A),
        ('\u{2039}', 0x8B),
        ('\u{0152}', 0x8C),
        ('\u{017D}', 0x8E),
        ('\u{2018}', 0x91),
        ('\u{2019}', 0x92),
        ('\u{201C}', 0x93),
        ('\u{201D}', 0x94),
        ('\u{2022}', 0x95),
        ('\u{2013}', 0x96),
        ('\u{2014}', 0x97),
        ('\u{02DC}', 0x98),
        ('\u{2122}', 0x99),
        ('\u{0161}', 0x9A),
        ('\u{203A}', 0x9B),
        ('\u{0153}', 0x9C),
        ('\u{017E}', 0x9E),
        ('\u{0178}', 0x9F),
    ];

    #[test]
    fn all_specials_map_to_their_byte() {
        for &(ch, byte) in SPECIALS {
            assert_eq!(win1252_byte(ch), Some(byte), "char {ch:?}");
        }
    }

    #[test]
    fn ascii_and_latin1_are_identity() {
        for b in 0u8..=0x7F {
            assert_eq!(win1252_byte(b as char), Some(b));
        }
        for b in 0xA0u8..=0xFF {
            assert_eq!(win1252_byte(b as char), Some(b));
        }
    }

    #[test]
    fn c1_controls_and_out_of_range_are_unrepresentable() {
        for cp in 0x80u32..=0x9F {
            assert_eq!(win1252_byte(char::from_u32(cp).unwrap()), None);
        }
        assert_eq!(win1252_byte('\u{0100}'), None);
        assert_eq!(win1252_byte('\u{1F600}'), None);
        assert_eq!(win1252_byte('\u{4E2D}'), None);
    }

    #[test]
    fn every_byte_but_the_five_holes_has_one_preimage() {
        let mut seen = [false; 256];
        for b in (0u8..=0x7F).chain(0xA0u8..=0xFF) {
            seen[b as usize] = true;
        }
        for &(_, byte) in SPECIALS {
            assert!(!seen[byte as usize], "byte {byte:#04x} mapped twice");
            seen[byte as usize] = true;
        }
        for hole in [0x81u8, 0x8D, 0x8F, 0x90, 0x9D] {
            assert!(!seen[hole as usize], "byte {hole:#04x} should be undefined");
        }
        assert_eq!(seen.iter().filter(|&&s| s).count(), 256 - 5);
    }

    #[test]
    fn encode_succeeds_and_reports_the_offending_char() {
        assert_eq!(encode_win1252("A\u{20AC}", "f").unwrap(), vec![b'A', 0x80]);
        let err = encode_win1252("x\u{1F600}", "author").unwrap_err();
        assert!(matches!(
            err,
            WriteError::Encoding {
                field: "author",
                ch: '\u{1F600}'
            }
        ));
    }
}
