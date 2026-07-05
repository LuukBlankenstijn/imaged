{
  lib,
  rustPlatform,
  pkg-config,
  sqlite,
  initramfs,
}:

rustPlatform.buildRustPackage {
  pname = "imaged-server";
  version = "0.1.0";

  src = ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
  ];

  buildInputs = [
    sqlite
  ];

  # sqlx query!/query_as! macros verify SQL at compile time against the
  # checked-in crates/server/.sqlx offline cache instead of a live database.
  SQLX_OFFLINE = "true";

  # The server embeds vmlinuz and the initramfs at compile time via
  # include_bytes! (see crates/server/src/api/pxe.rs). The initramfs is built
  # by Nix and injected here; vmlinuz is read from the committed assets/vmlinuz
  # so building the server never triggers the (~10 min) kernel build. Refresh
  # that asset out-of-band with `nix build .#kernel` (the dev shell does this
  # on entry).
  postPatch = ''
    install -Dm0644 ${initramfs} assets/initramfs.cpio.gz
  '';

  buildAndTestSubdir = "crates/server";

  meta = with lib; {
    description = "imaged server backend";
    homepage = "https://github.com/luuk/imaged";
    license = licenses.mit;
    maintainers = [ ];
    platforms = platforms.linux;
  };
}
