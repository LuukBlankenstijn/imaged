{ writeShellApplication, qemu }:

writeShellApplication {
  name = "run-vm-pxe";
  runtimeInputs = [ qemu ];
  text = ''
    exec qemu-system-x86_64 \
      -cpu host \
      -enable-kvm \
      -m 1G \
      -drive file=storage.qcow2,format=qcow2,if=virtio \
      -nographic \
      -netdev tap,id=net0,ifname=tap0,script=no,downscript=no \
      -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56,bootindex=1
  '';
}
