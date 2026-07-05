{
  lib,
  rustPlatform,
  ipxe,
}:

rustPlatform.buildRustPackage {
  pname = "imaged-tftp";
  version = "0.1.0";

  src = ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
  };

  # tftp embeds the iPXE bootloaders at compile time via include_bytes! (see
  # crates/tftp/src/main.rs). Inject them from the Nix ipxe package so
  # `nix build .#imaged-tftp` works without the filesystem-based dev flow.
  postPatch = ''
    install -Dm0644 ${ipxe}/ipxe.efi assets/ipxe.efi
    install -Dm0644 ${ipxe}/undionly.kpxe assets/undionly.kpxe
  '';

  buildAndTestSubdir = "crates/tftp";

  meta = with lib; {
    description = "imaged tftp server serving ipxe and undionly files";
    homepage = "https://github.com/luuk/imaged";
    license = licenses.mit;
    maintainers = [ ];
    platforms = platforms.linux;
  };
}
