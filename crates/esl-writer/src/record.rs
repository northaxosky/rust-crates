//! Content records and top-level groups.

use crate::error::WriteError;
use crate::game::Game;
use crate::write::{FLAG_COMPRESSED, GRUP, XXXX, write_field, write_record_header};

/// A content record: a 4-byte signature, a FormID, flags, and an ordered list of fields
#[derive(Debug, Clone)]
pub struct Record {
    sig: [u8; 4],
    form_id: u32,
    flags: u32,
    form_version: Option<u16>,
    fields: Vec<([u8; 4], Vec<u8>)>,
}

impl Record {
    /// Start a record of type `sig` with FormID `form_id`
    pub fn new(sig: &[u8; 4], form_id: u32) -> Self {
        Self {
            sig: *sig,
            form_id,
            flags: 0,
            form_version: None,
            fields: Vec::new(),
        }
    }

    /// Set the record-header flags
    pub fn flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    /// Override the record form version, which otherwise defaults to the plugin game's
    pub fn form_version(mut self, version: u16) -> Self {
        self.form_version = Some(version);
        self
    }

    /// Append a field (subrecord) with signature `sig` and raw payload `data`
    pub fn field(mut self, sig: &[u8; 4], data: impl AsRef<[u8]>) -> Self {
        self.fields.push((*sig, data.as_ref().to_vec()));
        self
    }
}

/// A top-level group (GRUP) holding records of a single type
#[derive(Debug, Clone)]
pub struct Group {
    label: [u8; 4],
    records: Vec<Record>,
}

impl Group {
    /// Start a top-level group holding records of type `record_type`
    pub fn top(record_type: &[u8; 4]) -> Self {
        Self {
            label: *record_type,
            records: Vec::new(),
        }
    }

    /// Append a record to the group
    pub fn record(mut self, record: Record) -> Self {
        self.records.push(record);
        self
    }

    /// The number of records this group holds
    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }
}

/// Append a top-level GRUP: its 24-byte header with a backpatched size, then its records
pub(crate) fn write_group(out: &mut Vec<u8>, group: &Group, game: Game) -> Result<(), WriteError> {
    if is_nesting_required(&group.label) {
        return Err(WriteError::NestedGroupRequired { label: group.label });
    }
    let start = out.len();
    out.extend_from_slice(GRUP);
    let size_pos = out.len();
    out.extend_from_slice(&0u32.to_le_bytes()); // groupSize placeholder
    out.extend_from_slice(&group.label); // label = record type
    out.extend_from_slice(&0u32.to_le_bytes()); // groupType 0 = top-level
    out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
    out.extend_from_slice(&0u16.to_le_bytes()); // version control info
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    for record in &group.records {
        if record.sig != group.label {
            return Err(WriteError::RecordTypeMismatch {
                group: group.label,
                record: record.sig,
            });
        }
        write_record(out, record, game)?;
    }
    let group_size = u32::try_from(out.len() - start).map_err(|_| WriteError::GroupTooLong {
        len: out.len() - start,
    })?;
    out[size_pos..size_pos + 4].copy_from_slice(&group_size.to_le_bytes());
    Ok(())
}

/// Append a record: its 24-byte header with a computed dataSize, then its fields
fn write_record(out: &mut Vec<u8>, record: &Record, game: Game) -> Result<(), WriteError> {
    if record.flags & FLAG_COMPRESSED != 0 {
        return Err(WriteError::CompressedRecordUnsupported { record: record.sig });
    }
    let mut body = Vec::new();
    for (sig, data) in &record.fields {
        if sig == XXXX {
            return Err(WriteError::ReservedFieldSignature);
        }
        write_record_field(&mut body, sig, data)?;
    }
    let data_size =
        u32::try_from(body.len()).map_err(|_| WriteError::RecordTooLong { len: body.len() })?;
    let form_version = record.form_version.unwrap_or_else(|| game.form_version());
    write_record_header(
        out,
        &record.sig,
        data_size,
        record.flags,
        record.form_id,
        form_version,
    );
    out.extend_from_slice(&body);
    Ok(())
}

/// Append one field, using an `XXXX` overflow prefix when the payload exceeds the 16-bit size
fn write_record_field(out: &mut Vec<u8>, sig: &[u8; 4], data: &[u8]) -> Result<(), WriteError> {
    if data.len() > u16::MAX as usize {
        let real_size =
            u32::try_from(data.len()).map_err(|_| WriteError::RecordTooLong { len: data.len() })?;
        write_field(out, XXXX, &real_size.to_le_bytes());
        out.extend_from_slice(sig);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(data);
    } else {
        write_field(out, sig, data);
    }
    Ok(())
}

/// Whether a record type must be written with nested groups this crate cannot yet emit
fn is_nesting_required(label: &[u8; 4]) -> bool {
    matches!(label, b"CELL" | b"WRLD" | b"DIAL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_count_reflects_records() {
        let g = Group::top(b"KYWD")
            .record(Record::new(b"KYWD", 1))
            .record(Record::new(b"KYWD", 2));
        assert_eq!(g.record_count(), 2);
    }

    #[test]
    fn nesting_required_only_for_container_types() {
        assert!(is_nesting_required(b"CELL"));
        assert!(is_nesting_required(b"WRLD"));
        assert!(is_nesting_required(b"DIAL"));
        assert!(!is_nesting_required(b"KYWD"));
        assert!(!is_nesting_required(b"GLOB"));
    }

    #[test]
    fn write_group_backpatches_size() {
        let mut out = Vec::new();
        let g = Group::top(b"KYWD").record(Record::new(b"KYWD", 1));
        write_group(&mut out, &g, Game::Fallout4).unwrap();
        assert_eq!(&out[0..4], b"GRUP");
        let size = u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize;
        assert_eq!(size, out.len()); // groupSize includes the whole group
        assert_eq!(size, 24 + 24); // GRUP header plus one empty record
        assert_eq!(&out[8..12], b"KYWD");
    }

    #[test]
    fn write_record_computes_data_size() {
        let mut out = Vec::new();
        let r = Record::new(b"KYWD", 5).field(b"EDID", b"hi\0");
        write_record(&mut out, &r, Game::Starfield).unwrap();
        let data_size = u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize;
        assert_eq!(data_size, out.len() - 24);
        assert_eq!(data_size, 6 + 3); // EDID field header plus "hi\0"
        assert_eq!(u16::from_le_bytes([out[20], out[21]]), 552); // Starfield formVersion
    }

    #[test]
    fn write_record_field_uses_xxxx_over_u16() {
        let mut small = Vec::new();
        write_record_field(&mut small, b"DATA", &[0u8; 10]).unwrap();
        assert_eq!(&small[0..4], b"DATA");

        let mut big = Vec::new();
        let payload = vec![0u8; 70_000];
        write_record_field(&mut big, b"DATA", &payload).unwrap();
        assert_eq!(&big[0..4], b"XXXX");
        assert_eq!(u32::from_le_bytes([big[6], big[7], big[8], big[9]]), 70_000);
        assert_eq!(&big[10..14], b"DATA");
    }
}
