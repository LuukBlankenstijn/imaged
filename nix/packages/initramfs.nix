{
  runCommand,
  coreutils,
  findutils,
  cpio,
  gzip,
  imaged-client,
  initramfsStaging,
}:

# Pure-Nix initramfs: embeds the Nix-built imaged-client so the image can be
# produced by `nix build` alone, with no filesystem/dev-shell steps. $out is
# the gzipped cpio archive itself (a file, not a directory).
runCommand "imaged-initramfs.cpio.gz"
  {
    nativeBuildInputs = [
      coreutils
      findutils
      cpio
      gzip
    ];
  }
  ''
    STAGING=$(mktemp -d)
    ${initramfsStaging "${imaged-client}/bin/imaged-client"}
    cd "$STAGING"
    find . | cpio -o -H newc 2>/dev/null | gzip > "$out"
  ''
