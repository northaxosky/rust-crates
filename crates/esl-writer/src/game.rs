//! The target game and its per-game plugin constants.

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
    pub(crate) fn hedr_version(self) -> f32 {
        match self {
            Game::SkyrimSe => 1.71,
            Game::Fallout4 => 1.0,
            Game::Starfield => 0.96,
        }
    }

    /// The record-header light-master flag bit for this game
    pub(crate) fn light_flag(self) -> u32 {
        match self {
            Game::SkyrimSe | Game::Fallout4 => 0x0000_0200,
            Game::Starfield => 0x0000_0100,
        }
    }

    /// The record-header form version stamped on the TES4 record
    pub(crate) fn form_version(self) -> u16 {
        match self {
            Game::SkyrimSe => 44,
            Game::Fallout4 => 131,
            Game::Starfield => 552,
        }
    }

    /// Whether this game pairs each `MAST` entry with a `DATA` (u64) field
    pub(crate) fn masters_have_data(self) -> bool {
        !matches!(self, Game::Starfield)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_flag_is_per_game() {
        assert_eq!(Game::Fallout4.light_flag(), 0x200);
        assert_eq!(Game::SkyrimSe.light_flag(), 0x200);
        assert_eq!(Game::Starfield.light_flag(), 0x100);
    }

    #[test]
    fn form_version_is_per_game() {
        assert_eq!(Game::SkyrimSe.form_version(), 44);
        assert_eq!(Game::Fallout4.form_version(), 131);
        assert_eq!(Game::Starfield.form_version(), 552);
    }

    #[test]
    fn only_starfield_omits_master_data() {
        assert!(Game::SkyrimSe.masters_have_data());
        assert!(Game::Fallout4.masters_have_data());
        assert!(!Game::Starfield.masters_have_data());
    }
}
