{
  lib,
  writeShellApplication,
  coreutils,
  findutils,
  cpio,
  xz,
  rustToolchain,
  targets,
}:

# Dev flow: cargo-builds the agent from the current working tree and packs it
# into ./assets/<arch>/initramfs.cpio.xz, mirroring the pure `initramfs`
# derivations via the shared staging helper. Use `nix build .#initramfs` for a
# fully pure build.
let
  buildTarget =
    arch: target:
    let
      inherit (target.muslPkgs.stdenv) cc hostPlatform;
      rustTarget = hostPlatform.rust.rustcTarget;
      cargoEnv = hostPlatform.rust.cargoEnvVarTarget;
      ccEnv = builtins.replaceStrings [ "-" ] [ "_" ] rustTarget;
      toolPrefix = "${cc}/bin/${cc.targetPrefix}";
      agent = "$(pwd)/target/${rustTarget}/release/imaged-agent";
    in
    ''
      ${arch})
        CARGO_TARGET_${cargoEnv}_LINKER="${toolPrefix}cc" \
        CARGO_TARGET_${cargoEnv}_RUSTFLAGS="-C target-feature=+crt-static -L native=${target.muslPkgs.musl.out}/lib" \
        CC_${ccEnv}="${toolPrefix}cc" \
        AR_${ccEnv}="${toolPrefix}ar" \
          cargo build --release --package imaged-agent --target ${rustTarget}
        STAGING=$(mktemp -d)
        ${target.initramfsStaging agent}
        pack ${arch} "${agent}"
        ;;
    '';
in
writeShellApplication {
  name = "build-initramfs";
  runtimeInputs = [
    coreutils
    findutils
    cpio
    xz
    rustToolchain
  ];
  text = ''
    trap 'rm -rf "''${STAGING:-}"' EXIT

    pack() {
      OUTPUT="$(pwd)/assets/$1/initramfs.cpio.xz"
      mkdir -p "$(dirname "$OUTPUT")"
      (cd "$STAGING" && find . | cpio -o -H newc 2>/dev/null | xz --check=crc32 -9e > "$OUTPUT")
      rm -rf "$STAGING"
      echo "built: $(du -h "$OUTPUT" | cut -f1) from $2"
    }

    if [ "$#" -eq 0 ]; then
      set -- ${lib.concatStringsSep " " (lib.attrNames targets)}
    fi

    for arch in "$@"; do
      case "$arch" in
        ${lib.concatStrings (lib.mapAttrsToList buildTarget targets)}
        *)
          echo "unknown arch '$arch', expected one of: ${lib.concatStringsSep " " (lib.attrNames targets)}" >&2
          exit 1
          ;;
      esac
    done
  '';
}
