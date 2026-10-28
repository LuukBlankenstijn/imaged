pub mod manifest;
pub mod read;
pub mod write;

use imaged_model::script::ScriptInput;

pub use manifest::{Manifest, archive_file_name};
pub use read::{ArchiveReader, Unpacked};
pub use write::{Contents, PartitionSource, write_archive};

const CHUNK_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub name: String,
    pub body: String,
    pub inputs: Vec<ScriptInput>,
    pub root_partition: Option<i64>,
}
