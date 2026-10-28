{
  pkgsStatic,
  partclone,
  kernel,
  gawk,
  kmod,
  xz,
}:

# Populates the initramfs staging tree at the directory named by the `$STAGING`
# shell variable, embedding `agentBin` as /bin/imaged-agent. `agentBin` is
# spliced verbatim, so callers may pass either a Nix store path (pure build) or
# a runtime shell expression like "$(pwd)/target/.../imaged-agent" (dev flow).
#
# This is the single source of truth for what goes into the initramfs; both the
# pure `initramfs` derivation and the `build-initramfs` dev script consume it.
let
  version = kernel.modDirVersion;
  modules = "${kernel.modules}/lib/modules/${version}";
  excluded = builtins.concatStringsSep "|" [
    "drivers/(media|iio|infiniband|staging|comedi|bluetooth|net/wireless|net/can)/"
    "drivers/gpu/drm/(amd|radeon|nouveau|i915|xe|panfrost|panthor|lima|etnaviv|v3d|msm)/"
    "drivers/(md|mtd|hwmon|isdn|accel|vfio|target|nfc|edac|watchdog|leds|w1|fpga|ntb)/"
    "drivers/(thunderbolt|firewire|parport|hte|siox|most|greybus|counter)/"
    "drivers/net/ethernet/(chelsio|qlogic|huawei|cavium|sfc|netronome|pensando|emulex|brocade)/"
    "drivers/net/ethernet/(mellanox/mlx4|mellanox/mlxsw|intel/ice|intel/idpf|marvell/octeontx2)/"
    "sound/"
    "net/"
    "crypto/"
    "arch/[^/]+/crypto/"
    "kernel/"
    "samples/"
    "fs/"
  ];
  kept = "fs/(ext4|fat|nls|jbd2|efivarfs)/|fs/mbcache|net/packet/";
in
agentBin: ''
  mkdir -p "$STAGING"/{bin,proc,sys,dev,tmp,etc,run,lib/modules/${version}}

  cp ${pkgsStatic.busybox}/bin/busybox "$STAGING/bin/busybox"
  cp ${pkgsStatic.libuuid}/bin/lsblk "$STAGING/bin/"
  cp ${pkgsStatic.gptfdisk}/bin/sgdisk "$STAGING/bin/"
  cp ${pkgsStatic.klibc}/lib/klibc/bin.static/ipconfig "$STAGING/bin/"
  cp ${pkgsStatic.e2fsprogs.bin}/bin/{e2fsck,debugfs,resize2fs} "$STAGING/bin/"
  cp ${pkgsStatic.dosfstools}/bin/fsck.fat "$STAGING/bin/"
  cp ${partclone}/bin/partclone.extfs "$STAGING/bin/"
  cp ${partclone}/bin/partclone.vfat "$STAGING/bin/"
  cp "${agentBin}" "$STAGING/bin/imaged-agent"
  ln -sfr "$STAGING/bin/partclone.extfs" "$STAGING/bin/partclone.ext4"
  ln -sf busybox "$STAGING/bin/sh"
  printf '[options]\nbroken_system_clock = 1\n' > "$STAGING/etc/e2fsck.conf"

  cp --no-preserve=mode ${modules}/modules.{order,builtin,builtin.modinfo} "$STAGING/lib/modules/${version}/"
  ${gawk}/bin/awk -F': ?' \
    -v excluded='^kernel/(${excluded})' -v kept='^kernel/(${kept})' \
    '$1 !~ excluded || $1 ~ kept { print $1; n = split($2, deps, " "); for (i = 1; i <= n; i++) print deps[i] }' \
    ${modules}/modules.dep \
    | sort -u \
    | (cd ${modules} && xargs cp --parents --no-preserve=mode -t "$STAGING/lib/modules/${version}")
  chmod -R u+w "$STAGING/lib/modules"
  find "$STAGING/lib/modules" -name '*.ko.xz' -exec ${xz}/bin/xz -d {} +
  ${kmod}/bin/depmod -b "$STAGING" ${version}

  install -m 0755 ${./init.sh} "$STAGING/init"
  install -m 0644 ${./mdev.conf} "$STAGING/etc/mdev.conf"
''
