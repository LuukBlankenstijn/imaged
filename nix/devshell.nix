{
  mkShell,
  rustToolchain,
  kernel,
  udpcast,
  build-initramfs,
  run-vm,
  run-vm-pxe,
  setup-net,
  teardown-net,
  buf,
  protoc-gen-tonic,
  protoc-gen-prost,
  protoc-gen-es,
  protoc-gen-prost-crate,
  sqlx-cli,
  nodejs,
  pnpm,
  ipxe,
}:

mkShell {
  packages = [
    rustToolchain
    build-initramfs
    run-vm
    run-vm-pxe
    setup-net
    teardown-net
    udpcast
    buf
    protoc-gen-tonic
    protoc-gen-prost
    protoc-gen-es
    protoc-gen-prost-crate
    sqlx-cli
    nodejs
    pnpm
  ];

  env = {
    CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
    DATABASE_URL = "sqlite://imaged.db";
    MULTICAST_INTERFACE = "br-netboot";
    PUBLIC_BASE = "192.168.100.1:8080";
  };

  shellHook = ''
    cp -f ${ipxe}/ipxe.efi ./assets/ipxe.efi
    cp -f ${ipxe}/undionly.kpxe ./assets/undionly.kpxe
    cp -f ${kernel}/bzImage ./assets/vmlinuz
  '';
}
