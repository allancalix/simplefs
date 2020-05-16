use std::fs::OpenOptions;

use clap::{App, Arg, SubCommand};
use simplefs::{io::FileBlockEmulatorBuilder, SFS};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = App::new("sfs")
        .version("0.1.0")
        .about("A dead simple (maybe even foolish) file system.")
        .subcommand(
            SubCommand::with_name("fmt")
                .about("Formats block storage with a simplefs filesystem.")
                .arg(
                    Arg::with_name("PATH")
                        .help("Path the the block device to format the filesystem on.")
                        .required(true)
                        .index(1),
                )
                .arg_from_usage("-d, --debug 'Create a file to emulate block storage at path.'"),
        )
        .get_matches();

    if let Some(command) = matches.subcommand_matches("fmt") {
        let path = command.value_of("PATH").unwrap();

        let device = if command.occurrences_of("debug") > 0 {
            let device = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(path)?;
            FileBlockEmulatorBuilder::from(device)
                .clear_medium(true)
                .with_block_size(64)
                .build()?
        } else {
            let device = OpenOptions::new()
                .create(false)
                .read(true)
                .write(true)
                .open(path)?;
            FileBlockEmulatorBuilder::from(device)
                .clear_medium(false)
                .with_block_size(64)
                .build()?
        };
        SFS::create(device)?;

        return Ok(());
    }

    println!("{}", matches.usage());
    std::process::exit(2)
}
