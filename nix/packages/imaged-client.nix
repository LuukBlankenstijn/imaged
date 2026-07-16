{ pkgsCross, rust-bin, ... }:

let
  # Host-runnable toolchain (runs on the gnu build host) carrying the musl std.
  toolchain = rust-bin.stable.latest.minimal.override {
    targets = [ "x86_64-unknown-linux-musl" ];
  };
  # Cross to musl so build scripts / proc-macros build for the gnu build host,
  # while the crate itself targets musl. buildRustPackage derives --target from
  # stdenv.hostPlatform, so a plain makeRustPlatform would (wrongly) build a
  # glibc-dynamic binary that needs a /nix/store loader absent in the initramfs.
  platform = pkgsCross.musl64.makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  };
in
platform.buildRustPackage {
  pname = "imaged-client";
  version = "0.1.0";

  src = ../..;
  cargoLock.lockFile = ../../Cargo.lock;

  buildAndTestSubdir = "crates/client";

  # nixpkgs' musl cross links dynamically by default (ld-musl-x86_64.so.1);
  # force a fully static binary for the target only, so it runs in the initramfs
  # with no dynamic loader. Host proc-macros/build scripts stay dynamic gnu.
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS = "-C target-feature=+crt-static";

  doCheck = false;
}
