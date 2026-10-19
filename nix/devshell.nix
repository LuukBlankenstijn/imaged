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
  };

  shellHook = ''
    mkdir -p assets
    cp -rfL --no-preserve=mode ${ipxeAssets}/. ${kernels}/. ./assets/
    export IMAGED_CONFIG="$PWD/dev/imaged.toml"
  '';
}
