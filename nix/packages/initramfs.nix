{
  runCommand,
  coreutils,
  findutils,
  cpio,
  xz,
  imaged-client,
  initramfsStaging,
}:

# Pure-Nix initramfs: embeds the Nix-built imaged-client so the image can be
# produced by `nix build` alone, with no filesystem/dev-shell steps. $out is
# the xz-compressed cpio archive itself (a file, not a directory).
runCommand "imaged-initramfs.cpio.xz"
  {
    nativeBuildInputs = [
      coreutils
      findutils
      cpio
      xz
    ];
  }
  ''
    STAGING=$(mktemp -d)
    ${initramfsStaging "${imaged-client}/bin/imaged-client"}
    cd "$STAGING"
    find . | cpio -o -H newc 2>/dev/null | xz --check=crc32 -9e > "$out"
  ''
