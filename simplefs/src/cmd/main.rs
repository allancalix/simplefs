use std::env::args_os;
use std::ffi::{OsStr};

use simplefs::{io::FileBlockEmulatorBuilder, SFS};
use std::fs::OpenOptions;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let device = OpenOptions::new()
        .create(false)
        .read(true)
        .write(true)
        // TODO(allancalix): Remove this hard-coded value.
        .open("/dev/fake_sda")?;
    let device = FileBlockEmulatorBuilder::from(device)
        .clear_medium(false)
        .with_block_size(64)
        .build()?;
    let sfs = SFS::from_block_storage(device)?;
    let mountpoint = args_os().nth(1).unwrap();
    let fuse_args: Vec<&OsStr> = ["-d", "-f", "-o", "allow_root"]
        .iter()
        .map(OsStr::new)
        .collect();
    fuse::mount(sfs, &mountpoint, &fuse_args)?;

    Ok(())
}
