use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "Pack a raw GPT disk image into an .imaged archive that imaged can import"
)]
pub struct Args {
    #[arg(help = "Raw disk image file with a GPT partition table")]
    pub disk: PathBuf,
    #[arg(long, help = "Image name stored in the archive")]
    pub name: String,
    #[arg(
        long,
        value_name = "N",
        help = "Partition holding the operating system [default: the only ext partition with /etc/os-release]"
    )]
    pub root_partition: Option<i64>,
    #[arg(
        long = "script",
        value_name = "PATH",
        help = "Post-install script <name>.sh to attach, inputs from <name>.inputs.json next to it; repeatable"
    )]
    pub scripts: Vec<PathBuf>,
    #[arg(short, long, value_name = "OUT", help = "Archive file to write")]
    pub output: PathBuf,
}
