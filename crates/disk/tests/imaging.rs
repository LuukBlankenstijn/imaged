use std::path::Path;

use imaged_disk::{Filesystem, clone_compressed};
use tokio::io::AsyncReadExt as _;

#[tokio::test]
async fn a_failed_partclone_fails_the_stream_instead_of_ending_it() {
    let dir = std::env::temp_dir().join(format!("imaged-disk-imaging-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("not-ext4.img");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(4 * 1024 * 1024)
        .unwrap();

    let mut compressed =
        clone_compressed(Filesystem::Ext4, &source, &dir.join("partclone.log")).unwrap();
    let error = compressed.read_to_end(&mut Vec::new()).await.unwrap_err();

    assert!(
        error.to_string().contains("partclone exited with"),
        "{error}"
    );
    std::fs::remove_dir_all(Path::new(&dir)).unwrap();
}
