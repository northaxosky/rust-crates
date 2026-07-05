//! Every reachable `WriteError` variant is triggered through the public API.

use esl_writer::{Game, Group, Plugin, Record, WriteError};

#[test]
fn encoding_error_on_non_cp1252() {
    let err = Plugin::new(Game::Fallout4)
        .author("\u{1F600}")
        .to_bytes()
        .unwrap_err();
    assert!(matches!(
        err,
        WriteError::Encoding {
            field: "author",
            ..
        }
    ));
}

#[test]
fn interior_nul_error() {
    let err = Plugin::new(Game::Fallout4)
        .description("a\0b")
        .to_bytes()
        .unwrap_err();
    assert!(matches!(
        err,
        WriteError::InteriorNul {
            field: "description"
        }
    ));
}

#[test]
fn string_too_long_error() {
    let err = Plugin::new(Game::Fallout4)
        .author("a".repeat(70_000))
        .to_bytes()
        .unwrap_err();
    assert!(matches!(
        err,
        WriteError::StringTooLong {
            field: "author",
            ..
        }
    ));
}

#[test]
fn record_type_mismatch_error() {
    let err = Plugin::new(Game::Fallout4)
        .group(Group::top(b"KYWD").record(Record::new(b"GLOB", 1)))
        .to_bytes()
        .unwrap_err();
    assert!(matches!(err, WriteError::RecordTypeMismatch { .. }));
}

#[test]
fn reserved_field_signature_error() {
    let err = Plugin::new(Game::Fallout4)
        .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"XXXX", b"x")))
        .to_bytes()
        .unwrap_err();
    assert!(matches!(err, WriteError::ReservedFieldSignature));
}

#[test]
fn compressed_record_error() {
    let err = Plugin::new(Game::Fallout4)
        .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).flags(0x0004_0000)))
        .to_bytes()
        .unwrap_err();
    assert!(matches!(
        err,
        WriteError::CompressedRecordUnsupported { .. }
    ));
}

#[test]
fn nested_group_required_error() {
    for label in [b"CELL", b"WRLD", b"DIAL"] {
        let err = Plugin::new(Game::Fallout4)
            .group(Group::top(label).record(Record::new(label, 1)))
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, WriteError::NestedGroupRequired { .. }));
    }
}

#[test]
fn a_full_valid_plugin_is_ok() {
    let result = Plugin::new(Game::Starfield)
        .author("author")
        .description("desc")
        .master("Starfield.esm")
        .group(
            Group::top(b"GLOB")
                .record(Record::new(b"GLOB", 1).field(b"FLTV", 1.0f32.to_le_bytes())),
        )
        .to_bytes();
    assert!(result.is_ok());
}
