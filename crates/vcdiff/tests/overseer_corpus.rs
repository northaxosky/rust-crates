//! Ignored opt-in verification harness for private Overseer delta corpora

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crc32fast::Hasher as Crc32;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::{Builder, NamedTempFile};
use vcdiff_rs::{DecodeOptions, decode_to};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    case: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    source: PathBuf,
    delta: PathBuf,
    expected_target: PathBuf,
    expected_size: u64,
    expected_crc32: String,
    expected_sha256: String,
    #[serde(default = "default_compare_bytes")]
    compare_bytes: bool,
}

struct Fingerprint {
    size: u64,
    crc32: u32,
    sha256: String,
}

const fn default_compare_bytes() -> bool {
    true
}

fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn parse_crc32(value: &str) -> io::Result<u32> {
    let value = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_data(
            "expected_crc32 must contain eight hexadecimal digits",
        ));
    }
    u32::from_str_radix(value, 16).map_err(|_| invalid_data("expected_crc32 is invalid"))
}

fn parse_sha256(value: &str) -> io::Result<String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_data(
            "expected_sha256 must contain 64 hexadecimal digits",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn fingerprint(file: &mut File) -> io::Result<Fingerprint> {
    file.seek(SeekFrom::Start(0))?;
    let mut buffer = [0_u8; BUFFER_SIZE];
    let mut crc32 = Crc32::new();
    let mut sha256 = Sha256::new();
    let mut size = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        crc32.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| invalid_data("fingerprint size overflow"))?;
    }
    Ok(Fingerprint {
        size,
        crc32: crc32.finalize(),
        sha256: format!("{:x}", sha256.finalize()),
    })
}

fn compare_files(left: &mut File, right: &mut File, size: u64) -> io::Result<bool> {
    left.seek(SeekFrom::Start(0))?;
    right.seek(SeekFrom::Start(0))?;
    let mut left_buffer = [0_u8; BUFFER_SIZE];
    let mut right_buffer = [0_u8; BUFFER_SIZE];
    let mut remaining = size;
    while remaining != 0 {
        let count = usize::try_from(remaining.min(BUFFER_SIZE as u64))
            .map_err(|_| invalid_data("comparison size exceeds usize"))?;
        left.read_exact(&mut left_buffer[..count])?;
        right.read_exact(&mut right_buffer[..count])?;
        if left_buffer[..count] != right_buffer[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
}

fn create_output(expected: &Path, manifest_root: &Path) -> io::Result<NamedTempFile> {
    let adjacent = expected.parent().and_then(|parent| {
        Builder::new()
            .prefix(".vcdiff-rs-")
            .suffix(".tmp")
            .tempfile_in(parent)
            .ok()
    });
    if let Some(output) = adjacent {
        return Ok(output);
    }
    Builder::new()
        .prefix(".vcdiff-rs-")
        .suffix(".tmp")
        .tempfile_in(manifest_root)
}

fn verify_fingerprint(
    fingerprint: &Fingerprint,
    size: u64,
    crc32: u32,
    sha256: &str,
) -> io::Result<()> {
    if fingerprint.size != size {
        return Err(invalid_data(format!(
            "size mismatch: expected {size}, got {}",
            fingerprint.size
        )));
    }
    if fingerprint.crc32 != crc32 {
        return Err(invalid_data(format!(
            "CRC32 mismatch: expected {crc32:08x}, got {:08x}",
            fingerprint.crc32
        )));
    }
    if fingerprint.sha256 != sha256 {
        return Err(invalid_data(format!(
            "SHA-256 mismatch: expected {sha256}, got {}",
            fingerprint.sha256
        )));
    }
    Ok(())
}

#[test]
#[ignore = "requires VCDIFF_OVERSEER_CORPUS and external private data"]
fn overseer_corpus() -> Result<(), Box<dyn Error>> {
    let manifest_path = env::var_os("VCDIFF_OVERSEER_CORPUS")
        .map(PathBuf::from)
        .ok_or_else(|| invalid_data("VCDIFF_OVERSEER_CORPUS is not set"))?;
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| invalid_data("manifest path has no parent"))?;
    let manifest_text = std::fs::read_to_string(&manifest_path)?;
    let manifest: Manifest =
        toml::from_str(&manifest_text).map_err(|_| invalid_data("corpus manifest is invalid"))?;
    if manifest.case.is_empty() {
        return Err(invalid_data("corpus manifest has no cases").into());
    }

    for case in manifest.case {
        if case.name.trim().is_empty() {
            return Err(invalid_data("corpus case name is empty").into());
        }
        let expected_crc32 = parse_crc32(&case.expected_crc32)?;
        let expected_sha256 = parse_sha256(&case.expected_sha256)?;
        let source_path = resolve(manifest_root, &case.source);
        let delta_path = resolve(manifest_root, &case.delta);
        let expected_path = resolve(manifest_root, &case.expected_target);

        println!("verifying corpus case {}", case.name);
        let mut expected = File::open(&expected_path)?;
        let expected_fingerprint = fingerprint(&mut expected)?;
        verify_fingerprint(
            &expected_fingerprint,
            case.expected_size,
            expected_crc32,
            &expected_sha256,
        )?;

        let mut source = File::open(source_path)?;
        let mut delta = File::open(delta_path)?;
        let mut output = create_output(&expected_path, manifest_root)?;
        let mut options = DecodeOptions::default();
        options.max_target_size = case.expected_size;
        decode_to(&mut source, &mut delta, output.as_file_mut(), &options)?;

        let output_fingerprint = fingerprint(output.as_file_mut())?;
        verify_fingerprint(
            &output_fingerprint,
            case.expected_size,
            expected_crc32,
            &expected_sha256,
        )?;
        if case.compare_bytes
            && !compare_files(output.as_file_mut(), &mut expected, case.expected_size)?
        {
            return Err(invalid_data(format!("byte mismatch in case {}", case.name)).into());
        }
    }
    Ok(())
}
