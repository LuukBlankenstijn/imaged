{
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
  imagingTools,
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
    imagingTools
  ];

  env = {
    DATABASE_URL = "sqlite://imaged.db";
    MULTICAST_INTERFACE = "br-netboot";
    PUBLIC_BASE = "192.168.100.1:8080";
  };

  shellHook = ''
    mkdir -p assets
    cp -rfL --no-preserve=mode ${ipxeAssets}/. ${kernels}/. ./assets/
  '';
}
