//! The RFC 3284 default instruction code table.

/// The kind of a single delta instruction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstKind {
    /// No operation, the unused half of a single-instruction entry
    NoOp,
    /// Append `size` bytes taken from the data section
    Add,
    /// Append one data byte repeated `size` times
    Run,
    /// Append `size` bytes copied from `mode`'s address in the combined window
    Copy,
}

/// One half of a code-table entry: a kind, a size (0 means read from the stream), and a COPY mode
#[derive(Debug, Clone, Copy)]
pub(crate) struct Instruction {
    pub(crate) kind: InstKind,
    pub(crate) size: u8,
    pub(crate) mode: u8,
}

impl Instruction {
    /// The unused half of a single-instruction entry
    const fn noop() -> Self {
        Self {
            kind: InstKind::NoOp,
            size: 0,
            mode: 0,
        }
    }

    /// An instruction of `kind` with a table `size` and COPY `mode`
    const fn new(kind: InstKind, size: u8, mode: u8) -> Self {
        Self { kind, size, mode }
    }
}

/// A code-table entry: up to two instructions applied in order
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry {
    pub(crate) first: Instruction,
    pub(crate) second: Instruction,
}

/// The number of near-cache slots in the default table
pub(crate) const NEAR_COUNT: usize = 4;
/// The number of same-cache blocks in the default table
pub(crate) const SAME_COUNT: usize = 3;

/// Build the RFC 3284 section 5.6 default code table
pub(crate) fn default_table() -> [Entry; 256] {
    use InstKind::{Add, Copy, Run};
    let noop = Instruction::noop();
    let single = |inst: Instruction| Entry {
        first: inst,
        second: noop,
    };
    let mut table = [Entry {
        first: noop,
        second: noop,
    }; 256];

    // entry 0: RUN, size read from the stream
    table[0] = single(Instruction::new(Run, 0, 0));

    // entries 1..=18: ADD, entry 1 size from stream, entries 2..=18 sizes 1..=17
    table[1] = single(Instruction::new(Add, 0, 0));
    for size in 1u8..=17 {
        table[1 + size as usize] = single(Instruction::new(Add, size, 0));
    }

    // entries 19..=162: COPY, 9 modes, per mode base=19+16m: base size from stream, base+1..=15 sizes 4..=18
    for mode in 0u8..=8 {
        let base = 19 + 16 * mode as usize;
        table[base] = single(Instruction::new(Copy, 0, mode));
        for k in 1usize..=15 {
            table[base + k] = single(Instruction::new(Copy, (k + 3) as u8, mode));
        }
    }

    // entries 163..=246: ADD+COPY doubles
    // modes 0..=5: base=163+12m, ADD size 1..=4 (outer) x COPY size 4..=6 (inner)
    for mode in 0u8..=5 {
        let mut idx = 163 + 12 * mode as usize;
        for add_size in 1u8..=4 {
            for copy_size in 4u8..=6 {
                table[idx] = Entry {
                    first: Instruction::new(Add, add_size, 0),
                    second: Instruction::new(Copy, copy_size, mode),
                };
                idx += 1;
            }
        }
    }
    // modes 6..=8: base=235+4*(m-6), ADD size 1..=4 x COPY size 4
    for mode in 6u8..=8 {
        let base = 235 + 4 * (mode as usize - 6);
        for add_size in 1u8..=4 {
            table[base + (add_size as usize - 1)] = Entry {
                first: Instruction::new(Add, add_size, 0),
                second: Instruction::new(Copy, 4, mode),
            };
        }
    }

    // entries 247..=255: COPY(size 4, mode m) + ADD(size 1)
    for mode in 0u8..=8 {
        table[247 + mode as usize] = Entry {
            first: Instruction::new(Copy, 4, mode),
            second: Instruction::new(Add, 1, 0),
        };
    }

    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_entries_match_rfc_default_table() {
        let t = default_table();

        assert_eq!(t[0].first.kind, InstKind::Run);
        assert_eq!(t[0].first.size, 0);
        assert_eq!(t[0].second.kind, InstKind::NoOp);

        assert_eq!(t[1].first.kind, InstKind::Add);
        assert_eq!(t[1].first.size, 0);
        assert_eq!(t[2].first.size, 1);
        assert_eq!(t[18].first.size, 17);

        // COPY block: mode 0 base 19, sizes 0 then 4..18; mode 1 base 35
        assert_eq!(t[19].first.kind, InstKind::Copy);
        assert_eq!((t[19].first.size, t[19].first.mode), (0, 0));
        assert_eq!((t[20].first.size, t[20].first.mode), (4, 0));
        assert_eq!((t[34].first.size, t[34].first.mode), (18, 0));
        assert_eq!((t[35].first.size, t[35].first.mode), (0, 1));

        // ADD+COPY doubles
        assert_eq!(t[163].first.kind, InstKind::Add);
        assert_eq!(t[163].first.size, 1);
        assert_eq!(
            (t[163].second.kind, t[163].second.size, t[163].second.mode),
            (InstKind::Copy, 4, 0)
        );
        assert_eq!((t[164].second.size, t[164].second.mode), (5, 0));
        assert_eq!(
            (t[176].first.size, t[176].second.size, t[176].second.mode),
            (1, 5, 1)
        );
        assert_eq!(
            (t[235].first.size, t[235].second.size, t[235].second.mode),
            (1, 4, 6)
        );
        assert_eq!(
            (t[238].first.size, t[238].second.size, t[238].second.mode),
            (4, 4, 6)
        );

        // COPY+ADD doubles
        assert_eq!(
            (t[247].first.kind, t[247].first.size, t[247].first.mode),
            (InstKind::Copy, 4, 0)
        );
        assert_eq!((t[247].second.kind, t[247].second.size), (InstKind::Add, 1));
        assert_eq!((t[255].first.mode, t[255].second.size), (8, 1));
    }

    #[test]
    fn every_entry_is_initialized() {
        let t = default_table();
        // no entry past 0 should be a bare NoOp/NoOp gap
        for (i, e) in t.iter().enumerate() {
            assert_ne!(e.first.kind, InstKind::NoOp, "entry {i} first is NoOp");
        }
    }
}
