{
  musl,
  mkShell,
  rustToolchain,
  kernels,
  ipxeAssets,
  build-initramfs,
  run-vm,
  run-vm-pxe,
  setup-net,
  teardown-net,
  sqlx-cli,
  dioxus-cli,
  tailwindcss_4,
  wasm-bindgen-cli,
  binaryen,
}:

mkShell {
  packages = [
    rustToolchain
    build-initramfs
    run-vm
    run-vm-pxe
    setup-net
    teardown-net
    sqlx-cli
    dioxus-cli
    tailwindcss_4
    wasm-bindgen-cli
    binaryen
  ];

  env = {
    CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
    DATABASE_URL = "sqlite://imaged.db";
    MULTICAST_INTERFACE = "br-netboot";
    PUBLIC_BASE = "192.168.100.1:8080";
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS = "-C target-feature=+crt-static -L native=${musl.out}/lib";
  };

  shellHook = ''
    cp -rfL --no-preserve=mode ${ipxeAssets}/. ${kernels}/. ./assets/
  '';
}
