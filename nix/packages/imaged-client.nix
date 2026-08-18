{ musl, craneLib }:

let
  # nixpkgs' musl cross links dynamically by default (ld-musl-x86_64.so.1);
  # force a fully static binary for the target only, so it runs in the initramfs
  # with no dynamic loader. Host proc-macros/build scripts stay dynamic gnu.
  #
  # rustc's self-contained musl sysroot ships libc.a and the crt objects but not
  # musl's empty libdl.a stub, so any crate carrying #[link(name = "dl")] (e.g.
  # libloading, reached via dioxus-core -> subsecond) fails to link. Point the
  # search path at musl's own lib dir, which provides the real stub.
  commonArgs = {
    pname = "imaged-client";
    version = "0.1.0";
    src = craneLib.cleanCargoSource ../..;
    strictDeps = true;
    cargoExtraArgs = "-p imaged-client";
    doCheck = false;

    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS =
      "-C target-feature=+crt-static -L native=${musl.out}/lib";
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  }
)
