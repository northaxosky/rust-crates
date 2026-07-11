use std::error::Error;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use vcdiff_rs::{DecodeOptions, decode_to};

fn required_arg(args: &mut impl Iterator<Item = OsString>) -> io::Result<PathBuf> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing required argument"))
}

fn open_input(path: &Path, name: &str) -> io::Result<File> {
    File::open(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to open {name} {}: {error}", path.display()),
        )
    })
}

fn create_output(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to create output {}: {error}", path.display()),
            )
        })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let source_path = required_arg(&mut args)?;
    let delta_path = required_arg(&mut args)?;
    let output_path = required_arg(&mut args)?;
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: decode_file <source> <delta> <output>",
        )
        .into());
    }

    let mut source = open_input(&source_path, "source")?;
    let mut delta = open_input(&delta_path, "delta")?;
    let mut output = create_output(&output_path)?;
    decode_to(
        &mut source,
        &mut delta,
        &mut output,
        &DecodeOptions::default(),
    )?;
    Ok(())
}
