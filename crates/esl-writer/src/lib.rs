//! Pure-Rust writer for minimal Bethesda light-master (ESL) carrier plugins.
//!
//! Emits tiny TES4-only plugins for Skyrim SE/AE, Fallout 4, and Starfield. The headline use is a
//! carrier plugin: an empty light master whose only job is to make the game auto-load a same-named
//! BA2 archive (the companion to the `btdx` crate). Save [`carrier_plugin`] output next to the
//! archives as `<base>.esl` (Fallout 4 / Skyrim SE) or `<base>.esm` (Starfield), where `<base>`
//! matches the archive file-name stem. The [`Plugin`] builder additionally writes an author, a
//! description, and master dependencies.
//!
//! Beyond headers, the [`Plugin`] builder writes content records grouped into top-level [`Group`]s:
//! each [`Record`] carries a signature, a FormID, and raw fields, and the writer computes every size
//! and count. FormIDs are caller-owned; for a light plugin's own new records, Fallout 4 and Skyrim SE
//! encode them as `(master_count << 24) | object_index` with the object index in `0x000..=0xFFF`.

#![forbid(unsafe_code)]

mod error;

pub use error::WriteError;

const TES4: &[u8; 4] = b"TES4";
const HEDR: &[u8; 4] = b"HEDR";
const CNAM: &[u8; 4] = b"CNAM";
const SNAM: &[u8; 4] = b"SNAM";
const MAST: &[u8; 4] = b"MAST";
const DATA: &[u8; 4] = b"DATA";
const GRUP: &[u8; 4] = b"GRUP";
const XXXX: &[u8; 4] = b"XXXX";

const FLAG_MASTER: u32 = 0x0000_0001;
const FLAG_COMPRESSED: u32 = 0x0004_0000;
const RECORD_HEADER_LEN: usize = 24;

/// The target game, which selects the HEDR version, the light-master flag bit, and the form version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Game {
    /// Skyrim Special Edition / Anniversary Edition
    SkyrimSe,
    /// Fallout 4
    Fallout4,
    /// Starfield
    Starfield,
}

impl Game {
    /// The `HEDR` version float this game's plugins carry
    fn hedr_version(self) -> f32 {
        match self {
            Game::SkyrimSe => 1.71,
            Game::Fallout4 => 1.0,
            Game::Starfield => 0.96,
        }
    }

    /// The record-header light-master flag bit for this game
    fn light_flag(self) -> u32 {
        match self {
            Game::SkyrimSe | Game::Fallout4 => 0x0000_0200,
            Game::Starfield => 0x0000_0100,
        }
    }

    /// The record-header form version stamped on the TES4 record
    fn form_version(self) -> u16 {
        match self {
            Game::SkyrimSe => 44,
            Game::Fallout4 => 131,
            Game::Starfield => 552,
        }
    }

    /// Whether this game pairs each `MAST` entry with a `DATA` (u64) field
    fn masters_have_data(self) -> bool {
        !matches!(self, Game::Starfield)
    }
}

/// A minimal Bethesda plugin, serialized as a single TES4 header record
#[derive(Debug, Clone)]
pub struct Plugin {
    game: Game,
    is_master: bool,
    is_light: bool,
    author: Option<String>,
    description: Option<String>,
    masters: Vec<String>,
    next_object_id: u32,
    groups: Vec<Group>,
}

impl Plugin {
    /// Start a carrier plugin for `game`: a master light plugin with no author, masters, or records
    pub fn new(game: Game) -> Self {
        Self {
            game,
            is_master: true,
            is_light: true,
            author: None,
            description: None,
            masters: Vec::new(),
            next_object_id: 1,
            groups: Vec::new(),
        }
    }

    /// Set the plugin author written as `CNAM`
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Set the plugin description written as `SNAM`
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Append a master dependency written as `MAST`, in load order
    pub fn master(mut self, name: impl Into<String>) -> Self {
        self.masters.push(name.into());
        self
    }

    /// Set whether the per-game light-master flag is written
    pub fn light(mut self, yes: bool) -> Self {
        self.is_light = yes;
        self
    }

    /// Set whether the ESM master flag is written
    pub fn master_flag(mut self, yes: bool) -> Self {
        self.is_master = yes;
        self
    }

    /// Set the `HEDR` next-object-ID counter
    pub fn next_object_id(mut self, id: u32) -> Self {
        self.next_object_id = id;
        self
    }

    /// Append a top-level group of records
    pub fn group(mut self, group: Group) -> Self {
        self.groups.push(group);
        self
    }

    /// The record-header flags this plugin writes
    fn flags(&self) -> u32 {
        let mut flags = 0;
        if self.is_master {
            flags |= FLAG_MASTER;
        }
        if self.is_light {
            flags |= self.game.light_flag();
        }
        flags
    }

    /// The `HEDR` count of top-level groups plus records, excluding the TES4 record itself
    fn num_records(&self) -> Result<u32, WriteError> {
        let mut count = self.groups.len();
        for group in &self.groups {
            count = count.saturating_add(group.records.len());
        }
        u32::try_from(count).map_err(|_| WriteError::TooManyRecords { count })
    }

    /// Serialize the plugin to its on-disk bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, WriteError> {
        let mut fields = Vec::new();
        write_hedr(
            &mut fields,
            self.game.hedr_version(),
            self.num_records()?,
            self.next_object_id,
        );
        if let Some(author) = &self.author {
            write_string_field(&mut fields, CNAM, "author", author)?;
        }
        if let Some(description) = &self.description {
            write_string_field(&mut fields, SNAM, "description", description)?;
        }
        for master in &self.masters {
            write_string_field(&mut fields, MAST, "master", master)?;
            if self.game.masters_have_data() {
                write_field(&mut fields, DATA, &0u64.to_le_bytes());
            }
        }

        let data_size = u32::try_from(fields.len())
            .map_err(|_| WriteError::RecordTooLong { len: fields.len() })?;
        let mut out = Vec::with_capacity(RECORD_HEADER_LEN + fields.len());
        write_record_header(
            &mut out,
            TES4,
            data_size,
            self.flags(),
            0,
            self.game.form_version(),
        );
        out.extend_from_slice(&fields);
        for group in &self.groups {
            write_group(&mut out, group, self.game)?;
        }
        Ok(out)
    }
}

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
}

/// Build the minimal carrier plugin that makes `game` auto-load its same-named BA2 archives
pub fn carrier_plugin(game: Game) -> Vec<u8> {
    let mut fields = Vec::with_capacity(18);
    write_hedr(&mut fields, game.hedr_version(), 0, 1);
    let mut out = Vec::with_capacity(RECORD_HEADER_LEN + fields.len());
    let flags = FLAG_MASTER | game.light_flag();
    write_record_header(
        &mut out,
        TES4,
        fields.len() as u32,
        flags,
        0,
        game.form_version(),
    );
    out.extend_from_slice(&fields);
    out
}

/// Append a field: 4-byte signature, u16 payload length, then the payload
fn write_field(out: &mut Vec<u8>, sig: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(payload);
}

/// Append the 12-byte `HEDR` field: version float, record count, next object ID
fn write_hedr(out: &mut Vec<u8>, version: f32, num_records: u32, next_object_id: u32) {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&version.to_le_bytes());
    payload[4..8].copy_from_slice(&num_records.to_le_bytes());
    payload[8..12].copy_from_slice(&next_object_id.to_le_bytes());
    write_field(out, HEDR, &payload);
}

/// Append a NUL-terminated Windows-1252 zstring field, naming it for error context
fn write_string_field(
    out: &mut Vec<u8>,
    sig: &[u8; 4],
    field: &'static str,
    value: &str,
) -> Result<(), WriteError> {
    if value.contains('\0') {
        return Err(WriteError::InteriorNul { field });
    }
    let mut payload = encode_win1252(value, field)?;
    payload.push(0);
    if payload.len() > u16::MAX as usize {
        return Err(WriteError::StringTooLong {
            field,
            len: payload.len(),
        });
    }
    write_field(out, sig, &payload);
    Ok(())
}

/// Append the 24-byte record header preceding `data_size` bytes of payload
fn write_record_header(
    out: &mut Vec<u8>,
    sig: &[u8; 4],
    data_size: u32,
    flags: u32,
    form_id: u32,
    form_version: u16,
) {
    out.extend_from_slice(sig);
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&form_id.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
    out.extend_from_slice(&0u16.to_le_bytes()); // version control info
    out.extend_from_slice(&form_version.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // unknown
}

/// Append a top-level GRUP: its 24-byte header with a backpatched size, then its records
fn write_group(out: &mut Vec<u8>, group: &Group, game: Game) -> Result<(), WriteError> {
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

/// Encode `value` as Windows-1252, erroring on the first unrepresentable character
fn encode_win1252(value: &str, field: &'static str) -> Result<Vec<u8>, WriteError> {
    value
        .chars()
        .map(|ch| win1252_byte(ch).ok_or(WriteError::Encoding { field, ch }))
        .collect()
}

/// Map one char to its Windows-1252 byte, if representable
fn win1252_byte(ch: char) -> Option<u8> {
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
    use proptest::prelude::*;

    /// The exact 42-byte Fallout 4 carrier from the format spec
    #[rustfmt::skip]
    const FO4_CARRIER: [u8; 42] = [
        0x54, 0x45, 0x53, 0x34, // TES4
        0x12, 0x00, 0x00, 0x00, // dataSize = 18
        0x01, 0x02, 0x00, 0x00, // flags = ESM | Light = 0x201
        0x00, 0x00, 0x00, 0x00, // formID
        0x00, 0x00,             // timestamp
        0x00, 0x00,             // version control info
        0x83, 0x00,             // formVersion = 131
        0x00, 0x00,             // unknown
        0x48, 0x45, 0x44, 0x52, // HEDR
        0x0C, 0x00,             // field size = 12
        0x00, 0x00, 0x80, 0x3F, // version 1.0
        0x00, 0x00, 0x00, 0x00, // numRecords = 0
        0x01, 0x00, 0x00, 0x00, // nextObjectID = 1
    ];

    /// Read the little-endian u32 record-header flags
    fn flags_of(bytes: &[u8]) -> u32 {
        u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]])
    }

    /// Whether `sig` appears as a 4-byte field signature anywhere in the record body
    fn contains_field(bytes: &[u8], sig: &[u8; 4]) -> bool {
        bytes[RECORD_HEADER_LEN..].windows(4).any(|w| w == &sig[..])
    }

    #[test]
    fn fallout4_carrier_matches_golden() {
        assert_eq!(carrier_plugin(Game::Fallout4), FO4_CARRIER);
    }

    #[test]
    fn light_flag_is_per_game() {
        assert_eq!(Game::Fallout4.light_flag(), 0x200);
        assert_eq!(Game::SkyrimSe.light_flag(), 0x200);
        assert_eq!(Game::Starfield.light_flag(), 0x100);
    }

    #[test]
    fn carrier_is_master_and_light_per_game() {
        assert_eq!(
            flags_of(&carrier_plugin(Game::Fallout4)),
            FLAG_MASTER | 0x200
        );
        assert_eq!(
            flags_of(&carrier_plugin(Game::SkyrimSe)),
            FLAG_MASTER | 0x200
        );
        assert_eq!(
            flags_of(&carrier_plugin(Game::Starfield)),
            FLAG_MASTER | 0x100
        );
    }

    #[test]
    fn toggling_flags_clears_bits() {
        let plain = Plugin::new(Game::Fallout4)
            .light(false)
            .master_flag(false)
            .to_bytes()
            .unwrap();
        assert_eq!(flags_of(&plain), 0);
    }

    #[test]
    fn author_is_a_win1252_zstring() {
        let bytes = Plugin::new(Game::Fallout4)
            .author("Kuz")
            .to_bytes()
            .unwrap();
        assert_eq!(&bytes[42..46], b"CNAM");
        let size = u16::from_le_bytes([bytes[46], bytes[47]]) as usize;
        assert_eq!(size, 4);
        assert_eq!(&bytes[48..48 + size], b"Kuz\0");
    }

    #[test]
    fn non_win1252_char_is_rejected() {
        let err = Plugin::new(Game::Fallout4)
            .author("emoji \u{1F600}")
            .to_bytes()
            .unwrap_err();
        match err {
            WriteError::Encoding { field, ch } => {
                assert_eq!(field, "author");
                assert_eq!(ch, '\u{1F600}');
            }
            other => panic!("expected Encoding, got {other:?}"),
        }
    }

    #[test]
    fn interior_nul_is_rejected() {
        let err = Plugin::new(Game::Fallout4)
            .author("a\0b")
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, WriteError::InteriorNul { field: "author" }));
    }

    #[test]
    fn win1252_maps_specials_and_latin1() {
        assert_eq!(win1252_byte('\u{20AC}'), Some(0x80));
        assert_eq!(win1252_byte('\u{2014}'), Some(0x97));
        assert_eq!(win1252_byte('\u{00FC}'), Some(0xFC));
        assert_eq!(win1252_byte('\u{0080}'), None);
        assert_eq!(win1252_byte('\u{1F600}'), None);
    }

    #[test]
    fn overlong_string_is_rejected() {
        let long = "a".repeat(70_000);
        let err = Plugin::new(Game::Fallout4)
            .author(long)
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
    fn fallout4_master_has_data_starfield_does_not() {
        let fo4 = Plugin::new(Game::Fallout4)
            .master("Fallout4.esm")
            .to_bytes()
            .unwrap();
        assert!(contains_field(&fo4, DATA));
        let sf = Plugin::new(Game::Starfield)
            .master("Starfield.esm")
            .to_bytes()
            .unwrap();
        assert!(!contains_field(&sf, DATA));
    }

    /// Read a little-endian u32 at `off`
    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    /// Read a little-endian u16 at `off`
    fn u16_at(b: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([b[off], b[off + 1]])
    }

    #[test]
    fn single_record_group_serializes() {
        let bytes = Plugin::new(Game::SkyrimSe)
            .group(
                Group::top(b"GLOB").record(
                    Record::new(b"GLOB", 0x0100_0801)
                        .field(b"EDID", b"MyGlobal\0")
                        .field(b"FNAM", b"f")
                        .field(b"FLTV", 1.0f32.to_le_bytes()),
                ),
            )
            .to_bytes()
            .unwrap();
        assert_eq!(&bytes[0..4], b"TES4");
        assert_eq!(u32_at(&bytes, 34), 2); // numRecords = 1 group + 1 record
        assert_eq!(&bytes[42..46], b"GRUP");
        let group_size = u32_at(&bytes, 46) as usize;
        assert_eq!(&bytes[50..54], b"GLOB"); // label = record type
        assert_eq!(u32_at(&bytes, 54), 0); // groupType top-level
        assert_eq!(&bytes[66..70], b"GLOB"); // record signature
        let rec_data = u32_at(&bytes, 70) as usize;
        assert_eq!(u32_at(&bytes, 78), 0x0100_0801); // formID
        assert_eq!(u16_at(&bytes, 86), Game::SkyrimSe.form_version());
        assert_eq!(group_size, 24 + 24 + rec_data); // groupSize includes the GRUP header
        assert_eq!(bytes.len(), 42 + group_size);
    }

    #[test]
    fn hedr_counts_groups_and_records() {
        let one_group = Plugin::new(Game::Fallout4)
            .group(
                Group::top(b"KYWD")
                    .record(Record::new(b"KYWD", 1))
                    .record(Record::new(b"KYWD", 2)),
            )
            .to_bytes()
            .unwrap();
        assert_eq!(u32_at(&one_group, 34), 3); // 1 group + 2 records

        let two_groups = Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1)))
            .group(Group::top(b"GLOB").record(Record::new(b"GLOB", 2)))
            .to_bytes()
            .unwrap();
        assert_eq!(u32_at(&two_groups, 34), 4); // 2 groups + 2 records
    }

    #[test]
    fn record_type_mismatch_is_rejected() {
        let err = Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"GLOB", 1)))
            .to_bytes()
            .unwrap_err();
        match err {
            WriteError::RecordTypeMismatch { group, record } => {
                assert_eq!(&group, b"KYWD");
                assert_eq!(&record, b"GLOB");
            }
            other => panic!("expected RecordTypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn reserved_xxxx_field_is_rejected() {
        let err = Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"XXXX", b"x")))
            .to_bytes()
            .unwrap_err();
        assert!(matches!(err, WriteError::ReservedFieldSignature));
    }

    #[test]
    fn compressed_record_is_rejected() {
        let err = Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).flags(FLAG_COMPRESSED)))
            .to_bytes()
            .unwrap_err();
        assert!(matches!(
            err,
            WriteError::CompressedRecordUnsupported { .. }
        ));
    }

    #[test]
    fn nesting_required_group_is_rejected() {
        for label in [b"CELL", b"WRLD", b"DIAL"] {
            let err = Plugin::new(Game::Fallout4)
                .group(Group::top(label).record(Record::new(label, 1)))
                .to_bytes()
                .unwrap_err();
            assert!(matches!(err, WriteError::NestedGroupRequired { .. }));
        }
    }

    #[test]
    fn large_field_uses_xxxx_overflow() {
        let big = vec![0xABu8; 70_000];
        let bytes = Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"DATA", &big)))
            .to_bytes()
            .unwrap();
        assert_eq!(&bytes[90..94], b"XXXX"); // record body begins at 90
        assert_eq!(u16_at(&bytes, 94), 4);
        assert_eq!(u32_at(&bytes, 96), 70_000);
        assert_eq!(&bytes[100..104], b"DATA"); // real field, size 0
        assert_eq!(u16_at(&bytes, 104), 0);
        assert_eq!(&bytes[106..106 + 70_000], &big[..]);
    }

    #[test]
    fn record_form_version_defaults_and_overrides() {
        let default = Plugin::new(Game::Starfield)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1)))
            .to_bytes()
            .unwrap();
        assert_eq!(u16_at(&default, 86), Game::Starfield.form_version());
        let overridden = Plugin::new(Game::Starfield)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).form_version(999)))
            .to_bytes()
            .unwrap();
        assert_eq!(u16_at(&overridden, 86), 999);
    }

    proptest! {
        #[test]
        fn data_size_always_excludes_the_header(id in any::<u32>()) {
            for game in [Game::SkyrimSe, Game::Fallout4, Game::Starfield] {
                let bytes = Plugin::new(game).next_object_id(id).to_bytes().unwrap();
                let data_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
                prop_assert_eq!(data_size, bytes.len() - RECORD_HEADER_LEN);
            }
        }

        #[test]
        fn ascii_author_round_trips(author in "[ -~]{0,64}") {
            let bytes = Plugin::new(Game::Fallout4).author(author.clone()).to_bytes().unwrap();
            let size = u16::from_le_bytes([bytes[46], bytes[47]]) as usize;
            let recovered = &bytes[48..48 + size - 1];
            prop_assert_eq!(recovered, author.as_bytes());
        }

        #[test]
        fn record_field_round_trips(data in proptest::collection::vec(any::<u8>(), 0..300)) {
            let bytes = Plugin::new(Game::Fallout4)
                .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"DATA", &data)))
                .to_bytes()
                .unwrap();
            prop_assert_eq!(&bytes[90..94], b"DATA");
            let size = u16::from_le_bytes([bytes[94], bytes[95]]) as usize;
            prop_assert_eq!(size, data.len());
            prop_assert_eq!(&bytes[96..96 + size], data.as_slice());
        }
    }
}
