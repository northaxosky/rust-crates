//! A standalone plugin parser used as an independent oracle in integration tests.
//!
//! It re-reads the bytes esl-writer produces so tests can assert on decoded structure rather than
//! poking raw offsets, and it resolves the `XXXX` overflow prefix transparently.

#![allow(dead_code)]

/// Read a little-endian u16 at `off`
pub fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// Read a little-endian u32 at `off`
pub fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Read a little-endian f32 at `off`
pub fn f32le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// A parsed field: its 4-byte signature and payload bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub sig: [u8; 4],
    pub data: Vec<u8>,
}

/// A parsed record: header values plus its subrecord fields
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub sig: [u8; 4],
    pub form_id: u32,
    pub flags: u32,
    pub form_version: u16,
    pub fields: Vec<Field>,
}

/// A parsed top-level group
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub label: [u8; 4],
    pub group_type: u32,
    pub records: Vec<Record>,
}

/// A parsed plugin: the TES4 header plus its top-level groups
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub flags: u32,
    pub form_version: u16,
    pub hedr_version: f32,
    pub num_records: u32,
    pub next_object_id: u32,
    pub header_fields: Vec<Field>,
    pub groups: Vec<Group>,
}

/// Parse a TES4 record followed by top-level GRUPs into structured form
pub fn parse(bytes: &[u8]) -> Plugin {
    assert_eq!(&bytes[0..4], b"TES4", "must start with TES4");
    let data_size = u32le(bytes, 4) as usize;
    let flags = u32le(bytes, 8);
    let form_version = u16le(bytes, 20);
    let body = &bytes[24..24 + data_size];

    let all = parse_fields(body);
    assert_eq!(&all[0].sig, b"HEDR", "first field must be HEDR");
    let hedr = &all[0].data;
    assert_eq!(hedr.len(), 12, "HEDR must be 12 bytes");
    let hedr_version = f32le(hedr, 0);
    let num_records = u32le(hedr, 4);
    let next_object_id = u32le(hedr, 8);
    let header_fields = all[1..].to_vec();

    let mut groups = Vec::new();
    let mut off = 24 + data_size;
    while off < bytes.len() {
        assert_eq!(&bytes[off..off + 4], b"GRUP", "expected a GRUP");
        let group_size = u32le(bytes, off + 4) as usize;
        let label = [
            bytes[off + 8],
            bytes[off + 9],
            bytes[off + 10],
            bytes[off + 11],
        ];
        let group_type = u32le(bytes, off + 12);
        let records = parse_records(&bytes[off + 24..off + group_size]);
        groups.push(Group {
            label,
            group_type,
            records,
        });
        off += group_size;
    }
    assert_eq!(off, bytes.len(), "trailing bytes after the last group");

    Plugin {
        flags,
        form_version,
        hedr_version,
        num_records,
        next_object_id,
        header_fields,
        groups,
    }
}

/// Parse a run of records from a group body
fn parse_records(body: &[u8]) -> Vec<Record> {
    let mut records = Vec::new();
    let mut off = 0;
    while off < body.len() {
        let sig = [body[off], body[off + 1], body[off + 2], body[off + 3]];
        let data_size = u32le(body, off + 4) as usize;
        let flags = u32le(body, off + 8);
        let form_id = u32le(body, off + 12);
        let form_version = u16le(body, off + 20);
        let rec_body = &body[off + 24..off + 24 + data_size];
        records.push(Record {
            sig,
            form_id,
            flags,
            form_version,
            fields: parse_fields(rec_body),
        });
        off += 24 + data_size;
    }
    records
}

/// Parse a run of fields, resolving any `XXXX` overflow prefix into the following field
fn parse_fields(body: &[u8]) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut off = 0;
    while off < body.len() {
        let sig = [body[off], body[off + 1], body[off + 2], body[off + 3]];
        let size = u16le(body, off + 4) as usize;
        if &sig == b"XXXX" {
            let real = u32le(body, off + 6) as usize;
            let real_sig = [
                body[off + 10],
                body[off + 11],
                body[off + 12],
                body[off + 13],
            ];
            let start = off + 16;
            fields.push(Field {
                sig: real_sig,
                data: body[start..start + real].to_vec(),
            });
            off = start + real;
        } else {
            let start = off + 6;
            fields.push(Field {
                sig,
                data: body[start..start + size].to_vec(),
            });
            off = start + size;
        }
    }
    fields
}
