//! Property-based round-trips: build an arbitrary plugin, serialize it, parse it back, and confirm
//! the decoded structure matches what was built.

mod common;

use common::parse;
use esl_writer::{Game, Group, Plugin, Record};
use proptest::prelude::*;

/// The HEDR version an independent oracle expects for each game
fn expected_hedr(game: Game) -> f32 {
    match game {
        Game::SkyrimSe => 1.71,
        Game::Fallout4 => 1.0,
        Game::Starfield => 0.96,
        _ => unreachable!(),
    }
}

/// The record form version an independent oracle expects for each game
fn expected_form_version(game: Game) -> u16 {
    match game {
        Game::SkyrimSe => 44,
        Game::Fallout4 => 131,
        Game::Starfield => 552,
        _ => unreachable!(),
    }
}

/// The light-master flag bit an independent oracle expects for each game
fn light_bit(game: Game) -> u32 {
    match game {
        Game::SkyrimSe | Game::Fallout4 => 0x200,
        Game::Starfield => 0x100,
        _ => unreachable!(),
    }
}

/// A field: its 4-byte signature and payload bytes
type FieldSpec = ([u8; 4], Vec<u8>);

/// A record: its FormID and fields
type RecordSpec = (u32, Vec<FieldSpec>);

/// A group: its label and the records that share it
type GroupSpec = ([u8; 4], Vec<RecordSpec>);

/// A 4-byte signature of uppercase letters
fn sig() -> impl Strategy<Value = [u8; 4]> {
    proptest::array::uniform4(b'A'..=b'Z')
}

/// A group label that is not one of the nesting-required record types
fn label() -> impl Strategy<Value = [u8; 4]> {
    sig().prop_filter("not a nesting-required label", |s| {
        s != b"CELL" && s != b"WRLD" && s != b"DIAL"
    })
}

/// A field signature (never the reserved `XXXX`) with a small payload
fn field() -> impl Strategy<Value = FieldSpec> {
    (sig(), prop::collection::vec(any::<u8>(), 0..40))
        .prop_filter("field sig is not XXXX", |(s, _)| s != b"XXXX")
}

/// A record body: a FormID and a handful of fields
fn record_body() -> impl Strategy<Value = RecordSpec> {
    (any::<u32>(), prop::collection::vec(field(), 0..5))
}

/// A group: a label plus records that all share it
fn group() -> impl Strategy<Value = GroupSpec> {
    (label(), prop::collection::vec(record_body(), 0..4))
}

/// One of the three supported games
fn game() -> impl Strategy<Value = Game> {
    prop_oneof![
        Just(Game::SkyrimSe),
        Just(Game::Fallout4),
        Just(Game::Starfield),
    ]
}

proptest! {
    #[test]
    fn arbitrary_plugin_round_trips(
        game in game(),
        author in prop::option::of("[ -~]{0,20}"),
        masters in prop::collection::vec("[A-Za-z0-9]{1,10}\\.esm", 0..3),
        groups in prop::collection::vec(group(), 0..4),
    ) {
        let mut plugin = Plugin::new(game);
        if let Some(a) = &author {
            plugin = plugin.author(a.clone());
        }
        for m in &masters {
            plugin = plugin.master(m.clone());
        }
        for (glabel, records) in &groups {
            let mut g = Group::top(glabel);
            for (form_id, fields) in records {
                let mut r = Record::new(glabel, *form_id);
                for (fsig, fdata) in fields {
                    r = r.field(fsig, fdata);
                }
                g = g.record(r);
            }
            plugin = plugin.group(g);
        }

        let bytes = plugin.to_bytes().unwrap();
        let parsed = parse(&bytes);

        prop_assert_eq!(parsed.hedr_version, expected_hedr(game));
        prop_assert_eq!(parsed.form_version, expected_form_version(game));

        let total_records: usize = groups.iter().map(|(_, r)| r.len()).sum();
        prop_assert_eq!(parsed.num_records as usize, groups.len() + total_records);

        let cnam = parsed.header_fields.iter().find(|f| &f.sig == b"CNAM");
        match &author {
            Some(a) => {
                let f = cnam.expect("CNAM present when an author is set");
                let mut want = a.clone().into_bytes();
                want.push(0);
                prop_assert_eq!(&f.data, &want);
            }
            None => prop_assert!(cnam.is_none()),
        }

        let mast: Vec<_> = parsed
            .header_fields
            .iter()
            .filter(|f| &f.sig == b"MAST")
            .collect();
        prop_assert_eq!(mast.len(), masters.len());
        for (mf, m) in mast.iter().zip(&masters) {
            let mut want = m.clone().into_bytes();
            want.push(0);
            prop_assert_eq!(&mf.data, &want);
        }
        let data_count = parsed
            .header_fields
            .iter()
            .filter(|f| &f.sig == b"DATA")
            .count();
        let expected_data = if matches!(game, Game::Starfield) {
            0
        } else {
            masters.len()
        };
        prop_assert_eq!(data_count, expected_data);

        prop_assert_eq!(parsed.groups.len(), groups.len());
        for (pg, (glabel, records)) in parsed.groups.iter().zip(&groups) {
            prop_assert_eq!(&pg.label, glabel);
            prop_assert_eq!(pg.group_type, 0);
            prop_assert_eq!(pg.records.len(), records.len());
            for (pr, (form_id, fields)) in pg.records.iter().zip(records) {
                prop_assert_eq!(&pr.sig, glabel);
                prop_assert_eq!(pr.form_id, *form_id);
                prop_assert_eq!(pr.form_version, expected_form_version(game));
                prop_assert_eq!(pr.fields.len(), fields.len());
                for (pf, (fsig, fdata)) in pr.fields.iter().zip(fields) {
                    prop_assert_eq!(&pf.sig, fsig);
                    prop_assert_eq!(&pf.data, fdata);
                }
            }
        }
    }

    #[test]
    fn header_round_trips(
        game in game(),
        master in any::<bool>(),
        light in any::<bool>(),
        author in prop::option::of("[ -~]{0,20}"),
        description in prop::option::of("[ -~]{0,20}"),
        masters in prop::collection::vec("[A-Za-z0-9]{1,10}\\.esm", 0..4),
        next_id in any::<u32>(),
    ) {
        let mut p = Plugin::new(game)
            .master_flag(master)
            .light(light)
            .next_object_id(next_id);
        if let Some(a) = &author {
            p = p.author(a.clone());
        }
        if let Some(d) = &description {
            p = p.description(d.clone());
        }
        for m in &masters {
            p = p.master(m.clone());
        }
        let parsed = parse(&p.to_bytes().unwrap());

        let mut want_flags = 0u32;
        if master {
            want_flags |= 0x1;
        }
        if light {
            want_flags |= light_bit(game);
        }
        prop_assert_eq!(parsed.flags, want_flags);
        prop_assert_eq!(parsed.next_object_id, next_id);
        prop_assert_eq!(parsed.num_records, 0);

        let snam = parsed.header_fields.iter().find(|f| &f.sig == b"SNAM");
        match &description {
            Some(d) => {
                let f = snam.expect("SNAM present when a description is set");
                let mut want = d.clone().into_bytes();
                want.push(0);
                prop_assert_eq!(&f.data, &want);
            }
            None => prop_assert!(snam.is_none()),
        }
    }
}

#[test]
fn carrier_round_trips_with_no_groups() {
    for g in [Game::SkyrimSe, Game::Fallout4, Game::Starfield] {
        let parsed = parse(&esl_writer::carrier_plugin(g));
        assert_eq!(parsed.num_records, 0);
        assert!(parsed.groups.is_empty());
        assert!(parsed.header_fields.is_empty());
        assert_eq!(parsed.hedr_version, expected_hedr(g));
    }
}

#[test]
fn a_known_plugin_round_trips() {
    let bytes = Plugin::new(Game::Fallout4)
        .author("muteptr")
        .master("Fallout4.esm")
        .group(
            Group::top(b"KYWD")
                .record(Record::new(b"KYWD", 0x0100_0801).field(b"EDID", b"MyKeyword\0"))
                .record(Record::new(b"KYWD", 0x0100_0802).field(b"EDID", b"Other\0")),
        )
        .to_bytes()
        .unwrap();
    let p = parse(&bytes);
    assert_eq!(p.num_records, 3);
    assert_eq!(p.groups.len(), 1);
    assert_eq!(&p.groups[0].label, b"KYWD");
    assert_eq!(p.groups[0].records.len(), 2);
    assert_eq!(p.groups[0].records[0].form_id, 0x0100_0801);
    assert_eq!(p.groups[0].records[1].fields[0].data, b"Other\0");
}
