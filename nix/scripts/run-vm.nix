{ writeShellApplication, qemu }:

writeShellApplication {
  name = "run-vm";
  runtimeInputs = [ qemu ];
  text = ''
    exec qemu-system-x86_64 \
      -cpu host \
      -enable-kvm \
      -m 1G \
      -kernel assets/vmlinuz \
      -initrd assets/initramfs.cpio.gz \
      -drive file=storage.qcow2,format=qcow2,if=virtio \
      -append "console=tty0 console=ttyS0 earlyprintk=vga loglevel=8 img_srv=http://192.168.100.1:8080" \
      -nographic \
      -netdev tap,id=net0,ifname=tap0,script=no,downscript=no \
      -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
  '';
}
