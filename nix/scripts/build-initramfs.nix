{
  writeShellApplication,
  coreutils,
  findutils,
  cpio,
  gzip,
  initramfsStaging,
}:

# Dev flow: builds the initramfs from the locally cargo-built client at
# target/x86_64-unknown-linux-musl/release/imaged-client and writes it to
# ./assets, mirroring the pure `initramfs` derivation via the shared staging
# helper. Use `nix build .#initramfs` for a fully pure build.
writeShellApplication {
  name = "build-initramfs";
  runtimeInputs = [
    coreutils
    findutils
    cpio
    gzip
  ];
  text = ''
    STAGING=$(mktemp -d)
    trap 'rm -rf "$STAGING"' EXIT

    ${initramfsStaging "$(pwd)/target/x86_64-unknown-linux-musl/release/imaged-client"}

    OUTPUT="$(pwd)/assets/initramfs.cpio.gz"
    cd "$STAGING"
    find . | cpio -o -H newc 2>/dev/null | gzip > "$OUTPUT"
    echo "built: $(du -h "$OUTPUT" | cut -f1)"
  '';
}
