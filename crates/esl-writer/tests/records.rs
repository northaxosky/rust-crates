//! Record and group edge cases, verified through the parser oracle.

mod common;

use common::parse;
use esl_writer::{Game, Group, Plugin, Record};

#[test]
fn field_at_u16_max_has_no_xxxx() {
    let bytes = Plugin::new(Game::Fallout4)
        .group(
            Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"DATA", vec![0u8; 65_535])),
        )
        .to_bytes()
        .unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"XXXX"));
    let p = parse(&bytes);
    let field = &p.groups[0].records[0].fields[0];
    assert_eq!(&field.sig, b"DATA");
    assert_eq!(field.data.len(), 65_535);
}

#[test]
fn field_over_u16_max_uses_xxxx() {
    let bytes = Plugin::new(Game::Fallout4)
        .group(
            Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"DATA", vec![7u8; 65_536])),
        )
        .to_bytes()
        .unwrap();
    assert!(bytes.windows(4).any(|w| w == b"XXXX"));
    let p = parse(&bytes);
    let field = &p.groups[0].records[0].fields[0];
    assert_eq!(&field.sig, b"DATA");
    assert_eq!(field.data.len(), 65_536);
    assert!(field.data.iter().all(|&b| b == 7));
}

#[test]
fn empty_group_counts_but_holds_no_records() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD"))
            .to_bytes()
            .unwrap(),
    );
    assert_eq!(p.num_records, 1);
    assert_eq!(p.groups.len(), 1);
    assert!(p.groups[0].records.is_empty());
    assert_eq!(&p.groups[0].label, b"KYWD");
}

#[test]
fn record_with_no_fields() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 7)))
            .to_bytes()
            .unwrap(),
    );
    let rec = &p.groups[0].records[0];
    assert_eq!(rec.form_id, 7);
    assert!(rec.fields.is_empty());
}

#[test]
fn zero_length_field() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).field(b"DATA", b"")))
            .to_bytes()
            .unwrap(),
    );
    let field = &p.groups[0].records[0].fields[0];
    assert_eq!(&field.sig, b"DATA");
    assert!(field.data.is_empty());
}

#[test]
fn many_records_and_groups() {
    let mut g1 = Group::top(b"KYWD");
    for i in 1..=10 {
        g1 = g1.record(Record::new(b"KYWD", i));
    }
    let mut g2 = Group::top(b"GLOB");
    for i in 100..=104 {
        g2 = g2.record(Record::new(b"GLOB", i).field(b"FLTV", 1.0f32.to_le_bytes()));
    }
    let p = parse(
        &Plugin::new(Game::Starfield)
            .group(g1)
            .group(g2)
            .to_bytes()
            .unwrap(),
    );
    assert_eq!(p.groups.len(), 2);
    assert_eq!(p.groups[0].records.len(), 10);
    assert_eq!(p.groups[1].records.len(), 5);
    assert_eq!(p.num_records, 2 + 10 + 5);
}

#[test]
fn multiple_fields_with_same_sig_are_preserved() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .group(
                Group::top(b"KYWD").record(
                    Record::new(b"KYWD", 1)
                        .field(b"CNAM", [1u8, 2, 3, 4])
                        .field(b"CNAM", [5u8, 6, 7, 8]),
                ),
            )
            .to_bytes()
            .unwrap(),
    );
    let fields = &p.groups[0].records[0].fields;
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].data, vec![1, 2, 3, 4]);
    assert_eq!(fields[1].data, vec![5, 6, 7, 8]);
}

#[test]
fn masters_order_and_data_pairing() {
    let fo4 = parse(
        &Plugin::new(Game::Fallout4)
            .master("A.esm")
            .master("B.esm")
            .to_bytes()
            .unwrap(),
    );
    let mast: Vec<_> = fo4
        .header_fields
        .iter()
        .filter(|f| &f.sig == b"MAST")
        .collect();
    let data: Vec<_> = fo4
        .header_fields
        .iter()
        .filter(|f| &f.sig == b"DATA")
        .collect();
    assert_eq!(mast.len(), 2);
    assert_eq!(data.len(), 2);
    assert_eq!(mast[0].data, b"A.esm\0");
    assert_eq!(mast[1].data, b"B.esm\0");

    let sf = parse(
        &Plugin::new(Game::Starfield)
            .master("A.esm")
            .to_bytes()
            .unwrap(),
    );
    assert_eq!(
        sf.header_fields
            .iter()
            .filter(|f| &f.sig == b"MAST")
            .count(),
        1
    );
    assert_eq!(
        sf.header_fields
            .iter()
            .filter(|f| &f.sig == b"DATA")
            .count(),
        0
    );
}

#[test]
fn author_and_description_are_zstrings() {
    let p = parse(
        &Plugin::new(Game::SkyrimSe)
            .author("me")
            .description("desc")
            .to_bytes()
            .unwrap(),
    );
    let cnam = p.header_fields.iter().find(|f| &f.sig == b"CNAM").unwrap();
    let snam = p.header_fields.iter().find(|f| &f.sig == b"SNAM").unwrap();
    assert_eq!(cnam.data, b"me\0");
    assert_eq!(snam.data, b"desc\0");
}

#[test]
fn carrier_equals_a_default_plugin() {
    for g in [Game::SkyrimSe, Game::Fallout4, Game::Starfield] {
        assert_eq!(
            esl_writer::carrier_plugin(g),
            Plugin::new(g).to_bytes().unwrap()
        );
    }
}

#[test]
fn next_object_id_is_preserved() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .next_object_id(0xDEAD_BEEF)
            .to_bytes()
            .unwrap(),
    );
    assert_eq!(p.next_object_id, 0xDEAD_BEEF);
}

#[test]
fn record_flags_are_preserved() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .group(Group::top(b"KYWD").record(Record::new(b"KYWD", 1).flags(0x0000_0400)))
            .to_bytes()
            .unwrap(),
    );
    assert_eq!(p.groups[0].records[0].flags, 0x0000_0400);
}

#[test]
fn flag_toggles_produce_expected_bits() {
    let cases = [
        (true, true, 0x201u32),
        (true, false, 0x001),
        (false, true, 0x200),
        (false, false, 0x000),
    ];
    for (master, light, want) in cases {
        let p = parse(
            &Plugin::new(Game::Fallout4)
                .master_flag(master)
                .light(light)
                .to_bytes()
                .unwrap(),
        );
        assert_eq!(p.flags, want);
    }
}

#[test]
fn kitchen_sink_round_trips() {
    let p = parse(
        &Plugin::new(Game::Fallout4)
            .author("me")
            .description("d")
            .master("A.esm")
            .master("B.esm")
            .next_object_id(0x800)
            .group(
                Group::top(b"GLOB").record(
                    Record::new(b"GLOB", 0x0100_0801)
                        .field(b"EDID", b"G1\0")
                        .field(b"FLTV", 1.5f32.to_le_bytes()),
                ),
            )
            .group(
                Group::top(b"KYWD")
                    .record(Record::new(b"KYWD", 0x0100_0802).field(b"EDID", b"K1\0"))
                    .record(Record::new(b"KYWD", 0x0100_0803)),
            )
            .to_bytes()
            .unwrap(),
    );
    assert_eq!(p.next_object_id, 0x800);
    assert_eq!(p.num_records, 2 + 3);
    assert_eq!(p.groups.len(), 2);
    assert_eq!(p.groups[0].records[0].fields.len(), 2);
    assert_eq!(p.groups[1].records.len(), 2);
    assert_eq!(
        p.header_fields.iter().filter(|f| &f.sig == b"MAST").count(),
        2
    );
    assert_eq!(
        p.header_fields.iter().filter(|f| &f.sig == b"DATA").count(),
        2
    );
}
