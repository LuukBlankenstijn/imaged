{ makeRustPlatform, rust-bin, ... }:

let
  toolchain = rust-bin.stable.latest.minimal.override {
    targets = [ "x86_64-unknown-linux-musl" ];
  };
  platform = makeRustPlatform {
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
  CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";

  doCheck = false;
}
