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
    root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
    mkdir -p "$root/assets"
    cp -rfL --no-preserve=mode ${ipxeAssets}/. ${kernels}/. "$root/assets/"
    export IMAGED_CONFIG="$root/dev/imaged.toml"
    ${tailwindcss_4}/bin/tailwindcss -i "$root/crates/web/input.css" -o "$root/crates/web/assets/tailwind.css" 2>/dev/null
    unset root
  '';
}
