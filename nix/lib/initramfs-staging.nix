{
  pkgsStatic,
  partclone,
}:

# Populates the initramfs staging tree at the directory named by the `$STAGING`
# shell variable, embedding `agentBin` as /bin/imaged-agent. `agentBin` is
# spliced verbatim, so callers may pass either a Nix store path (pure build) or
# a runtime shell expression like "$(pwd)/target/.../imaged-agent" (dev flow).
agentBin: ''
  mkdir -p "$STAGING"/{bin,proc,sys,dev,tmp,etc,run}

  cp ${pkgsStatic.busybox}/bin/busybox "$STAGING/bin/busybox"
  cp ${pkgsStatic.libuuid}/bin/lsblk "$STAGING/bin/"
  cp ${pkgsStatic.gptfdisk}/bin/sgdisk "$STAGING/bin/"
  cp ${pkgsStatic.klibc}/lib/klibc/bin.static/ipconfig "$STAGING/bin/"
  cp ${pkgsStatic.e2fsprogs.bin}/bin/{e2fsck,debugfs,resize2fs} "$STAGING/bin/"
  cp ${pkgsStatic.dosfstools}/bin/fsck.fat "$STAGING/bin/"
  cp ${pkgsStatic.btrfs-progs}/bin/btrfs "$STAGING/bin/"
  cp ${partclone}/bin/partclone.extfs "$STAGING/bin/"
  cp ${partclone}/bin/partclone.vfat "$STAGING/bin/"
  cp ${partclone}/bin/partclone.btrfs "$STAGING/bin/"
  cp "${agentBin}" "$STAGING/bin/imaged-agent"
  ln -sfr "$STAGING/bin/partclone.extfs" "$STAGING/bin/partclone.ext4"
  ln -sf busybox "$STAGING/bin/sh"
  printf '[options]\nbroken_system_clock = 1\n' > "$STAGING/etc/e2fsck.conf"

  install -m 0755 ${./init.sh} "$STAGING/init"
  install -m 0644 ${./mdev.conf} "$STAGING/etc/mdev.conf"
''
