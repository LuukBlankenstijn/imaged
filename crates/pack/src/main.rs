mod blkid;
mod cli;
mod extract;
mod pack;
mod scripts;
mod shrink;

use clap::Parser;
use imaged_model::setup_logging;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();
    setup_logging!("info", ["imaged-disk"]);
    pack::run(args).await
}
