//! Fallout 4 BA2 path hashing.
//!
//! The engine indexes archive contents by a key of three parts, so a writer must reproduce it
//! exactly: the file stem and its parent directory are each hashed with a table CRC-32 (init 0, no
//! final inversion), and the extension is packed into four little-endian bytes. Paths are first
//! normalized the way the engine and Archive2 normalize them: lowercase ASCII, `/` to `\`, with
//! leading and trailing separators stripped.

/// A file's BA2 lookup key: stem CRC, packed extension, and parent-directory CRC
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileHash {
    pub(crate) file: u32,
    pub(crate) extension: u32,
    pub(crate) directory: u32,
}

/// Normalize `path` in place into the bytes stored on disk and fed to the hash
fn normalize_path(path: &mut Vec<u8>) {
    for b in path.iter_mut() {
        *b = if *b == b'/' {
            b'\\'
        } else {
            b.to_ascii_lowercase()
        };
    }

    while path.last() == Some(&b'\\') {
        path.pop();
    }
    while path.first() == Some(&b'\\') {
        path.remove(0);
    }
}

/// Split a normalized path into its parent directory, file stem, and extension
fn split(path: &[u8]) -> (&[u8], &[u8], &[u8]) {
    let sep = path.iter().rposition(|&b| b == b'\\');
    let parent = match sep {
        Some(i) => &path[..i],
        None => &[],
    };
    let dot = path.iter().rposition(|&b| b == b'.');
    let extension = match dot {
        Some(i) => &path[i + 1..],
        None => &[],
    };
    let first = sep.map_or(0, |i| i + 1);
    let last = dot.unwrap_or(path.len());
    let stem = path.get(first..last).unwrap_or(&[]);
    (parent, stem, extension)
}

/// Table CRC-32 as the game computes it: polynomial `0xEDB88320`, init `0`, no final inversion
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in bytes {
        crc = (crc >> 8) ^ CRC_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize];
    }
    crc
}

/// Normalize `path` and return its on-disk bytes together with its BA2 lookup key
pub(crate) fn hash_file(path: &[u8]) -> (Vec<u8>, FileHash) {
    let mut name = path.to_vec();
    normalize_path(&mut name);
    let (parent, stem, extension) = split(&name);
    let mut packed_ext = 0u32;
    for (i, &b) in extension.iter().take(4).enumerate() {
        packed_ext |= u32::from(b) << (i * 8);
    }
    let hash = FileHash {
        file: crc32(stem),
        extension: packed_ext,
        directory: crc32(parent),
    };
    (name, hash)
}

#[rustfmt::skip]
const CRC_TABLE: [u32; 256] = [
    0x00000000, 0x77073096, 0xEE0E612C, 0x990951BA, 0x076DC419, 0x706AF48F, 0xE963A535,
    0x9E6495A3, 0x0EDB8832, 0x79DCB8A4, 0xE0D5E91E, 0x97D2D988, 0x09B64C2B, 0x7EB17CBD,
    0xE7B82D07, 0x90BF1D91, 0x1DB71064, 0x6AB020F2, 0xF3B97148, 0x84BE41DE, 0x1ADAD47D,
    0x6DDDE4EB, 0xF4D4B551, 0x83D385C7, 0x136C9856, 0x646BA8C0, 0xFD62F97A, 0x8A65C9EC,
    0x14015C4F, 0x63066CD9, 0xFA0F3D63, 0x8D080DF5, 0x3B6E20C8, 0x4C69105E, 0xD56041E4,
    0xA2677172, 0x3C03E4D1, 0x4B04D447, 0xD20D85FD, 0xA50AB56B, 0x35B5A8FA, 0x42B2986C,
    0xDBBBC9D6, 0xACBCF940, 0x32D86CE3, 0x45DF5C75, 0xDCD60DCF, 0xABD13D59, 0x26D930AC,
    0x51DE003A, 0xC8D75180, 0xBFD06116, 0x21B4F4B5, 0x56B3C423, 0xCFBA9599, 0xB8BDA50F,
    0x2802B89E, 0x5F058808, 0xC60CD9B2, 0xB10BE924, 0x2F6F7C87, 0x58684C11, 0xC1611DAB,
    0xB6662D3D, 0x76DC4190, 0x01DB7106, 0x98D220BC, 0xEFD5102A, 0x71B18589, 0x06B6B51F,
    0x9FBFE4A5, 0xE8B8D433, 0x7807C9A2, 0x0F00F934, 0x9609A88E, 0xE10E9818, 0x7F6A0DBB,
    0x086D3D2D, 0x91646C97, 0xE6635C01, 0x6B6B51F4, 0x1C6C6162, 0x856530D8, 0xF262004E,
    0x6C0695ED, 0x1B01A57B, 0x8208F4C1, 0xF50FC457, 0x65B0D9C6, 0x12B7E950, 0x8BBEB8EA,
    0xFCB9887C, 0x62DD1DDF, 0x15DA2D49, 0x8CD37CF3, 0xFBD44C65, 0x4DB26158, 0x3AB551CE,
    0xA3BC0074, 0xD4BB30E2, 0x4ADFA541, 0x3DD895D7, 0xA4D1C46D, 0xD3D6F4FB, 0x4369E96A,
    0x346ED9FC, 0xAD678846, 0xDA60B8D0, 0x44042D73, 0x33031DE5, 0xAA0A4C5F, 0xDD0D7CC9,
    0x5005713C, 0x270241AA, 0xBE0B1010, 0xC90C2086, 0x5768B525, 0x206F85B3, 0xB966D409,
    0xCE61E49F, 0x5EDEF90E, 0x29D9C998, 0xB0D09822, 0xC7D7A8B4, 0x59B33D17, 0x2EB40D81,
    0xB7BD5C3B, 0xC0BA6CAD, 0xEDB88320, 0x9ABFB3B6, 0x03B6E20C, 0x74B1D29A, 0xEAD54739,
    0x9DD277AF, 0x04DB2615, 0x73DC1683, 0xE3630B12, 0x94643B84, 0x0D6D6A3E, 0x7A6A5AA8,
    0xE40ECF0B, 0x9309FF9D, 0x0A00AE27, 0x7D079EB1, 0xF00F9344, 0x8708A3D2, 0x1E01F268,
    0x6906C2FE, 0xF762575D, 0x806567CB, 0x196C3671, 0x6E6B06E7, 0xFED41B76, 0x89D32BE0,
    0x10DA7A5A, 0x67DD4ACC, 0xF9B9DF6F, 0x8EBEEFF9, 0x17B7BE43, 0x60B08ED5, 0xD6D6A3E8,
    0xA1D1937E, 0x38D8C2C4, 0x4FDFF252, 0xD1BB67F1, 0xA6BC5767, 0x3FB506DD, 0x48B2364B,
    0xD80D2BDA, 0xAF0A1B4C, 0x36034AF6, 0x41047A60, 0xDF60EFC3, 0xA867DF55, 0x316E8EEF,
    0x4669BE79, 0xCB61B38C, 0xBC66831A, 0x256FD2A0, 0x5268E236, 0xCC0C7795, 0xBB0B4703,
    0x220216B9, 0x5505262F, 0xC5BA3BBE, 0xB2BD0B28, 0x2BB45A92, 0x5CB36A04, 0xC2D7FFA7,
    0xB5D0CF31, 0x2CD99E8B, 0x5BDEAE1D, 0x9B64C2B0, 0xEC63F226, 0x756AA39C, 0x026D930A,
    0x9C0906A9, 0xEB0E363F, 0x72076785, 0x05005713, 0x95BF4A82, 0xE2B87A14, 0x7BB12BAE,
    0x0CB61B38, 0x92D28E9B, 0xE5D5BE0D, 0x7CDCEFB7, 0x0BDBDF21, 0x86D3D2D4, 0xF1D4E242,
    0x68DDB3F8, 0x1FDA836E, 0x81BE16CD, 0xF6B9265B, 0x6FB077E1, 0x18B74777, 0x88085AE6,
    0xFF0F6A70, 0x66063BCA, 0x11010B5C, 0x8F659EFF, 0xF862AE69, 0x616BFFD3, 0x166CCF45,
    0xA00AE278, 0xD70DD2EE, 0x4E048354, 0x3903B3C2, 0xA7672661, 0xD06016F7, 0x4969474D,
    0x3E6E77DB, 0xAED16A4A, 0xD9D65ADC, 0x40DF0B66, 0x37D83BF0, 0xA9BCAE53, 0xDEBB9EC5,
    0x47B2CF7F, 0x30B5FFE9, 0xBDBDF21C, 0xCABAC28A, 0x53B39330, 0x24B4A3A6, 0xBAD03605,
    0xCDD70693, 0x54DE5729, 0x23D967BF, 0xB3667A2E, 0xC4614AB8, 0x5D681B02, 0x2A6F2B94,
    0xB40BBE37, 0xC30C8EA1, 0x5A05DF1B, 0x2D02EF8D,
];

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn key(path: &[u8]) -> FileHash {
        hash_file(path).1
    }

    fn triple(file: u32, extension: u32, directory: u32) -> FileHash {
        FileHash {
            file,
            extension,
            directory,
        }
    }

    // Vectors lifted from real Fallout 4 archives; the non-ASCII path guards raw-byte hashing.
    #[test]
    fn matches_all_known_vectors() {
        #[rustfmt::skip]
        let vectors: &[(&[u8], u32, u32, u32)] = &[
            (b"Sound\\Voice\\Fallout4.esm\\RobotMrHandy\\Mar\xEDa_M.fuz", 0xC9FB26F9, 0x007A7566, 0x8A9C014E),
            (br"Strings\ccBGSFO4001-PipBoy(Black)_en.DLSTRINGS", 0x1985075C, 0x74736C64, 0x29F6B58B),
            (br"Textures\CreationClub\BGSFO4001\AnimObjects\PipBoy\PipBoy02(Black)_d.DDS", 0x69E1E82C, 0x00736464, 0x23157A84),
            (br"Materials\CreationClub\BGSFO4003\AnimObjects\PipBoy\PipBoyLabels01(Camo01).BGSM", 0x0785843B, 0x6D736762, 0x818374CC),
            (br"Textures\CreationClub\BGSFO4003\AnimObjects\PipBoy\PipBoy02(Camo01)_d.DDS", 0xF2D2F9A7, 0x00736464, 0xE9DB0C08),
            (br"Strings\ccBGSFO4004-PipBoy(Camo02)_esmx.DLSTRINGS", 0xC26B77C1, 0x74736C64, 0x29F6B58B),
            (br"Textures\CreationClub\BGSFO4004\AnimObjects\PipBoy\PipBoyLabels01(Camo02)_d.DDS", 0xB32EE4B0, 0x00736464, 0x089FAA9B),
            (br"Strings\ccBGSFO4006-PipBoy(Chrome)_es.STRINGS", 0xA94A4503, 0x69727473, 0x29F6B58B),
            (br"Textures\CreationClub\BGSFO4006\AnimObjects\PipBoy\PipBoy01(Chrome)_s.DDS", 0xE2D67EE2, 0x00736464, 0xC251DC17),
            (br"Meshes\CreationClub\BGSFO4016\Clothes\Prey\MorganSpaceSuit_M_First.nif", 0x212E5DAD, 0x0066696E, 0x741DAAC0),
            (br"Textures\CreationClub\BGSFO4016\Clothes\Prey\Morgan_Male_Body_s.DDS", 0x9C672F34, 0x00736464, 0x1D5F0EDF),
            (br"Strings\ccBGSFO4018-GaussRiflePrototype_ru.STRINGS", 0x5198717F, 0x69727473, 0x29F6B58B),
            (br"Textures\CreationClub\BGSFO4018\Weapons\GaussRiflePrototype\Barrel02_s.DDS", 0x2C98BAA2, 0x00736464, 0x8D59E9EA),
            (br"Strings\ccBGSFO4019-ChineseStealthArmor_esmx.DLSTRINGS", 0xDDF2A35F, 0x74736C64, 0x29F6B58B),
            (br"Textures\CreationClub\BGSFO4019\Armor\ChineseStealthArmor\ChineseStealthArmor01_d.DDS", 0x03C2AA10, 0x00736464, 0x71ED2818),
            (br"Materials\CreationClub\BGSFO4020\Actors\PowerArmor\T45helmet01(Black).BGSM", 0xF56D31C0, 0x6D736762, 0x28A143A5),
            (br"Textures\CreationClub\BGSFO4020\Actors\PowerArmor\T51\Black\T51Helmet01(Black)_d.DDS", 0x3192919D, 0x00736464, 0xA56D1E61),
            (br"Materials\CreationClub\BGSFO4038\Actors\PowerArmor\HorsePAHelmet.BGSM", 0xE90B72CC, 0x6D736762, 0x44676566),
            (br"Textures\CreationClub\BGSFO4038\Actors\PowerArmor\HorsePATorso_teal_d.DDS", 0x0A6251B3, 0x00736464, 0xC1AC59B4),
            (br"Strings\ccBGSFO4044-HellfirePowerArmor_en.DLSTRINGS", 0x3E5C1E5E, 0x74736C64, 0x29F6B58B),
            (br"Textures\CreationClub\BGSFO4044\Actors\PowerArmor\HellfirePAHelmet_Institute_d.DDS", 0x0F221EAF, 0x00736464, 0xC021EF40),
            (br"Meshes\Weapons\HandmadeShotgun\HandmadeShotgun_GlowSights.nif", 0x4E080CE2, 0x0066696E, 0xCCD47ECF),
            (br"Textures\Weapons\HandmadeShotgun\HandmadeShotgun_Barrels_GhoulSlayer_d.DDS", 0xBBFC484C, 0x00736464, 0xCEAE4154),
            (br"Materials\CreationClub\FSVFO4001\Clothes\MilitaryBackpack\BackpackPatch_NCR02.bgsm", 0x90EB78B9, 0x6D736762, 0xDA685DF4),
            (br"Textures\CreationClub\FSVFO4001\Clothes\MilitaryBackpack\Button_SunsetSars_d.DDS", 0xC25F8604, 0x00736464, 0xD1CE178D),
            (br"Materials\CreationClub\FSVFO4002\Furniture\MidCenturyModern01\BedSpread01.bgsm", 0xA5AAE799, 0x6D736762, 0xBECD0DEF),
            (br"Textures\CreationClub\FSVFO4002\Furniture\MidCenturyModern01\Bed01_n.DDS", 0x6A09686A, 0x00736464, 0xBA782808),
            (br"Sound\FX\DLC03\NPC\Gulper\NPC_Gulper_Foot_Walk_02.xwm", 0xFE001981, 0x006D7778, 0xE7FBD6C4),
            (br"Textures\Terrain\DLC03FarHarbor\DLC03FarHarbor.4.-69.41.DDS", 0x36BACD03, 0x00736464, 0x8184624D),
            (br"Sound\Voice\DLCCoast.esm\PlayerVoiceFemale01\00043FFC_1.fuz", 0x339EFB3F, 0x007A7566, 0x3A5289D4),
            (br"Meshes\PreCombined\DLCNukaWorld.esm\0000F616_17EAC297_OC.NIF", 0xD4AD97F7, 0x0066696E, 0x0787B7E9),
            (br"Textures\Terrain\NukaWorld\NukaWorld.4.-28.28_msn.DDS", 0x86C13103, 0x00736464, 0x26C08933),
            (br"Sound\Voice\DLCNukaWorld.esm\DLC04NPCMJohnCalebBradberton\00044D5E_1.fuz", 0x896E4419, 0x007A7566, 0xD6575CD6),
            (br"Meshes\SCOL\DLCRobot.esm\CM00007BD8.NIF", 0x103559EF, 0x0066696E, 0xF584B7C4),
            (br"Textures\DLC01\SetDressing\Rubble\Robottrashpilesnorust_s.DDS", 0xC7AF7106, 0x00736464, 0x5FD1A1B0),
            (br"Sound\Voice\DLCRobot.esm\DLC01RobotCompanionFemaleProcessed\00001460_1.fuz", 0x6D3D7DC7, 0x007A7566, 0xB2B47CAD),
            (br"Materials\DLC02\SetDressing\Workshop\NeonSignage\NeonLetterKit01-Orange-5.BGEM", 0x21D59551, 0x6D656762, 0x926F0C27),
            (br"Textures\DLC02\SetDressing\Workshop\Traps\DLC02_SpringTrap01_s.DDS", 0x02BE99A4, 0x00736464, 0xF03CA2DF),
            (br"Sound\FX\DLC05\PHY\BallTrack\PHY_Metal_BallTrack_SteelBall_Wood_H_03.xwm", 0x33AABE0C, 0x006D7778, 0x07AA294C),
            (br"Textures\DLC05\Effects\PaintBalls\ImpactDecalPaintSplatters01Red_d.DDS", 0x6327DF24, 0x00736464, 0xFB5FB431),
            (br"Meshes\SCOL\DLCworkshop03.esm\CM00001091.NIF", 0x2CAF6750, 0x0066696E, 0xABA83647),
            (br"Textures\DLC06\Interiors\Vault\DLC06VltSignWelcome88_01_d.DDS", 0x825BD732, 0x00736464, 0xAE76DDEF),
            (br"Sound\Voice\DLCworkshop03.esm\FemaleEvenToned\00005232_1.fuz", 0x4DB6EE2D, 0x007A7566, 0xDA9F7ABC),
            (br"Meshes\AnimTextData\DynamicIdleData\5693375383928345500.txt", 0x997FC17A, 0x00747874, 0xFD345C50),
            (br"Interface\Pipboy_StatsPage.swf", 0x2F26E4D0, 0x00667773, 0xD2FDF873),
            (br"Materials\Landscape\Grass\BeachGrass01.BGSM", 0xB023CE22, 0x6D736762, 0x941D851F),
            (br"Meshes\Actors\Character\FaceGenData\FaceGeom\Fallout4.esm\000B3EC7.NIF", 0x90C91640, 0x0066696E, 0x067FA81E),
            (br"Meshes\PreCombined\0000E069_7831AAC9_OC.NIF", 0x5F0B19DF, 0x0066696E, 0xE659D075),
            (br"scripts\MinRadiantOwnedBuildResourceScript.pex", 0xA2DAD4FD, 0x00786570, 0x40724840),
            (br"Meshes\debris\roundrock2_dirt.nif", 0x1E47A158, 0x0066696E, 0xF55EC6BA),
            (br"ShadersFX\Shaders011.fxp", 0x883415D8, 0x00707866, 0xDFAE3D0F),
            (br"Sound\FX\FX\Bullet\Impact\xxx\FX_Bullet_Impact_Dirt_04.xwm", 0xFFAD9A14, 0x006D7778, 0xCBA20EB7),
            (br"Textures\Effects\ColorBlackZeroAlphaUtility.DDS", 0xF912F225, 0x00736464, 0xEA3C9738),
            (br"Textures\interiors\Building\BldWindow01_s.DDS", 0x6ECA4F0C, 0x00736464, 0x5A3A7C7A),
            (br"Textures\Terrain\Commonwealth\Commonwealth.4.-8.12_msn.DDS", 0x55E37BD8, 0x00736464, 0x4409E1A9),
            (br"Textures\Clothes\Nat\Nats_Outfit_s.DDS", 0x692FFE7D, 0x00736464, 0x3F5BEDF1),
            (br"Textures\Interface\Newspaper\Newspaper_s.DDS", 0xFAC17C6C, 0x00736464, 0x58B9C5A4),
            (br"Textures\Actors\Character\FaceCustomization\Fallout4.esm\00110043_s.DDS", 0x09A155E6, 0x00736464, 0x9C7DFA7A),
            (br"Textures\Terrain\Commonwealth\Commonwealth.4.-48.-60.DDS", 0x182C2446, 0x00736464, 0x4409E1A9),
            (br"Textures\Terrain\Commonwealth\Commonwealth.4.-80.8_msn.DDS", 0xDA3234A4, 0x00736464, 0x4409E1A9),
            (br"Textures\Terrain\SanctuaryHillsWorld\SanctuaryHillsWorld.4.-36.40.DDS", 0xDD27070A, 0x00736464, 0x49AAA5E1),
            (br"Textures\Terrain\SanctuaryHillsWorld\SanctuaryHillsWorld.4.76.-24.DDS", 0x71560B31, 0x00736464, 0x49AAA5E1),
            (br"Sound\Voice\Fallout4.esm\NPCMTravisMiles\000A6032_1.fuz", 0x34402DE0, 0x007A7566, 0xF186D761),
        ];
        for &(path, file, extension, directory) in vectors {
            assert_eq!(
                key(path),
                triple(file, extension, directory),
                "path: {}",
                String::from_utf8_lossy(path)
            );
        }
    }

    proptest! {
        // Normalization makes hashing case- and separator-insensitive with leading/trailing slashes.
        #[test]
        fn hashing_ignores_case_and_separator(
            dir in "[a-zA-Z0-9_]{1,16}",
            stem in "[a-zA-Z0-9_]{1,16}",
            ext in "[a-zA-Z]{1,5}",
        ) {
            let a = format!("{dir}\\{stem}.{ext}");
            let b = format!("/{}/{}.{}\\", dir.to_uppercase(), stem.to_uppercase(), ext.to_uppercase());
            prop_assert_eq!(key(a.as_bytes()), key(b.as_bytes()));
        }
    }

    // Forward slashes and mixed case must normalize to the same key and on-disk name.
    #[test]
    fn normalizes_separators_and_case() {
        let (name, hash) = hash_file(b"Tilemap/tiles.png");
        assert_eq!(name, b"tilemap\\tiles.png");
        assert_eq!(hash, triple(0xDA3773A6, 0x00676E70, 0x0B0A447E));
        assert_eq!(key(b"MESHES/A.NIF"), key(b"meshes\\a.nif"));
    }

    // Leading and trailing separators are stripped before hashing.
    #[test]
    fn strips_edge_separators() {
        assert_eq!(hash_file(b"\\Meshes\\a.nif\\").0, b"meshes\\a.nif");
        assert_eq!(key(b"\\Meshes\\a.nif\\"), key(b"Meshes\\a.nif"));
    }

    // A root-level file has an empty parent (directory CRC 0); no dot means an empty extension.
    #[test]
    fn handles_missing_directory_and_extension() {
        assert_eq!(key(b"readme.txt").directory, 0);
        let no_ext = key(b"meshes\\thing");
        assert_eq!(no_ext.extension, 0);
        assert_eq!(no_ext.file, crc32(b"thing"));
        assert_eq!(no_ext.directory, crc32(b"meshes"));
    }

    // Only the last dot starts the extension, and it is truncated to four bytes.
    #[test]
    fn extension_uses_last_dot_and_four_bytes() {
        let multi = hash_file(b"a\\b.c.d");
        assert_eq!(multi.1.extension, u32::from(b'd'));
        assert_eq!(multi.1.file, crc32(b"b.c"));
        let long = hash_file(b"x\\y.abcdef");
        assert_eq!(long.1.extension, u32::from_le_bytes(*b"abcd"));
    }
}
