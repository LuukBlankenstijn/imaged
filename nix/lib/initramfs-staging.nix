{
  pkgsStatic,
  udpcast,
  partclone,
}:

# Populates the initramfs staging tree at the directory named by the `$STAGING`
# shell variable, embedding `clientBin` as /bin/imaged-client. `clientBin` is
# spliced verbatim, so callers may pass either a Nix store path (pure build) or
# a runtime shell expression like "$(pwd)/target/.../imaged-client" (dev flow).
#
# This is the single source of truth for what goes into the initramfs; both the
# pure `initramfs` derivation and the `build-initramfs` dev script consume it.
clientBin: ''
  mkdir -p "$STAGING"/{bin,proc,sys,dev,tmp,etc,run}

  cp ${pkgsStatic.busybox}/bin/busybox "$STAGING/bin/busybox"
  cp ${pkgsStatic.libuuid}/bin/lsblk "$STAGING/bin/"
  cp ${pkgsStatic.gptfdisk}/bin/sgdisk "$STAGING/bin/"
  cp ${udpcast}/bin/udp-receiver "$STAGING/bin/"
  cp ${pkgsStatic.klibc}/lib/klibc/bin.static/ipconfig "$STAGING/bin/"
  cp ${pkgsStatic.tcpdump}/bin/tcpdump "$STAGING/bin/"
  cp ${partclone}/bin/partclone.extfs "$STAGING/bin/"
  cp ${partclone}/bin/partclone.vfat "$STAGING/bin/"
  cp "${clientBin}" "$STAGING/bin/imaged-client"
  ln -sfr "$STAGING/bin/partclone.extfs" "$STAGING/bin/partclone.ext4"
  ln -sf busybox "$STAGING/bin/sh"

  install -m 0755 ${./init.sh} "$STAGING/init"
''
