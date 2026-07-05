//! The plugin builder and the carrier-plugin convenience.

use crate::error::WriteError;
use crate::game::Game;
use crate::record::{Group, write_group};
use crate::write::{
    CNAM, DATA, FLAG_MASTER, MAST, RECORD_HEADER_LEN, SNAM, TES4, write_field, write_hedr,
    write_record_header, write_string_field,
};

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
            count = count.saturating_add(group.record_count());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;
    use crate::write::FLAG_COMPRESSED;
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

    /// Read a little-endian u32 at `off`
    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    /// Read a little-endian u16 at `off`
    fn u16_at(b: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([b[off], b[off + 1]])
    }

    #[test]
    fn fallout4_carrier_matches_golden() {
        assert_eq!(carrier_plugin(Game::Fallout4), FO4_CARRIER);
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
