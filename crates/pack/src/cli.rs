use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "Pack a raw GPT disk image, or a raw ext2/3/4 or vfat filesystem image, into an .imaged archive that imaged can import"
)]
pub struct Args {
    #[arg(
        value_name = "IMAGE",
        help = "Raw disk image with a GPT partition table, or a raw ext2/3/4 or vfat filesystem image packed as a single-partition image"
    )]
    pub image: PathBuf,
    #[arg(long, help = "Image name stored in the archive")]
    pub name: String,
    #[arg(
        long,
        value_name = "N",
        help = "Partition holding the operating system, 1 for a filesystem image [default: the only ext partition with /etc/os-release]"
    )]
    pub root_partition: Option<i64>,
    #[arg(
        long = "script",
        value_name = "PATH",
        help = "Post-install script <name>.sh to attach, inputs from <name>.inputs.json next to it; repeatable"
    )]
    pub scripts: Vec<PathBuf>,
    #[arg(
        long = "script-root",
        value_name = "NAME=N",
        value_parser = ScriptRoot::parse,
        help = "Run script <name> in partition N instead of the root partition; repeatable"
    )]
    pub script_roots: Vec<ScriptRoot>,
    #[arg(short, long, value_name = "OUT", help = "Archive file to write")]
    pub output: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ScriptRoot {
    pub name: String,
    pub partition: i64,
}

impl ScriptRoot {
    fn parse(value: &str) -> Result<Self, String> {
        let (name, partition) = value
            .split_once('=')
            .ok_or_else(|| format!("expected <name>=<N>, got `{value}`"))?;
        let partition = partition
            .parse()
            .map_err(|_| format!("`{partition}` is not a partition number"))?;
        Ok(Self {
            name: name.to_owned(),
            partition,
        })
    }
}
