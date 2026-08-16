{
  writeShellApplication,
  coreutils,
  findutils,
  cpio,
  gzip,
  musl,
  rustToolchain,
  initramfsStaging,
}:

# Dev flow: cargo-builds the client from the current working tree and packs it
# into ./assets/initramfs.cpio.gz, mirroring the pure `initramfs` derivation via
# the shared staging helper. Use `nix build .#initramfs` for a fully pure build.
writeShellApplication {
  name = "build-initramfs";
  runtimeInputs = [
    coreutils
    findutils
    cpio
    gzip
    rustToolchain
  ];
  text = ''
    TARGET=x86_64-unknown-linux-musl
    CLIENT="$(pwd)/target/$TARGET/release/imaged-client"

    CARGO_BUILD_TARGET="$TARGET" \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C target-feature=+crt-static -L native=${musl.out}/lib" \
      cargo build --release --package imaged-client

    STAGING=$(mktemp -d)
    trap 'rm -rf "$STAGING"' EXIT

    ${initramfsStaging "$CLIENT"}

    OUTPUT="$(pwd)/assets/initramfs.cpio.gz"
    cd "$STAGING"
    find . | cpio -o -H newc 2>/dev/null | gzip > "$OUTPUT"
    echo "built: $(du -h "$OUTPUT" | cut -f1) from $CLIENT"
  '';
}
