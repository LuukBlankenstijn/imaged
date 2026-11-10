modulesInitrd: {
  modules = modulesInitrd {
    name = "imaged-modules";
    modules = [
      "(arch|block|drivers|io_uring|lib|mm|security|virt)/"
      "fs/(ext4|fat|btrfs|nls|jbd2|efivarfs)/"
      "net/packet/"
    ];
    excludedModules = [
      "arch/[^/]+/crypto/"
      "drivers/(media|iio|infiniband|staging|comedi|bluetooth|net/wireless|net/can)/"
      "drivers/gpu/drm/(amd|radeon|nouveau|i915|xe|panfrost|panthor|lima|etnaviv|v3d|msm)/"
      "drivers/(md|mtd|hwmon|isdn|accel|vfio|target|nfc|edac|watchdog|leds|w1|fpga|ntb)/"
      "drivers/(thunderbolt|firewire|parport|hte|siox|most|greybus|counter)/"
      "drivers/net/ethernet/(chelsio|qlogic|huawei|cavium|sfc|netronome|pensando|emulex|brocade)/"
      "drivers/net/ethernet/(mellanox/mlx4|mellanox/mlxsw|intel/ice|intel/idpf|marvell/octeontx2)/"
    ];
  };

  display-intel = modulesInitrd {
    name = "imaged-display-intel";
    modules = [ "drivers/gpu/drm/i915/i915\\.ko" ];
  };

  display-nvidia = modulesInitrd {
    name = "imaged-display-nvidia";
    modules = [ "drivers/gpu/drm/nouveau/nouveau\\.ko" ];
    extraCommands = ''
      gspFirmware=$(modinfo -F firmware "$MODULES/kernel/drivers/gpu/drm/nouveau/nouveau.ko" | awk -F/ '$3 == "gsp"')
      gspVersion=$(printf '%s\n' "$gspFirmware" | awk -F/ 'match($4, /^gsp-.*\.bin$/) { print substr($4, 5, RLENGTH - 8) }' | sort -V | tail -n 1)
      while read -r dir; do
        for source in "$FIRMWARE/$dir"/*-"$gspVersion".bin; do
          if [ -e "$source" ]; then
            stage_firmware "$dir/''${source##*/}"
          fi
        done
      done < <(printf '%s\n' "$gspFirmware" | awk -F/ '{ print $1 "/" $2 "/" $3 }' | sort -u)
    '';
  };
}
