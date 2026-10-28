{
  config,
  lib,
  pkgs,
  options,
  ...
}:
let
  inherit (lib) mkOption types;
  cfg = config.imaged.image;
  host = config;
  disko = options ? disko && config.disko.devices.disk != { };
  partition = cfg.kind == "partition";
  efi = cfg.partitionTableType != "legacy+gpt";
  onDisk = device: lib.hasPrefix "/dev/" (toString device);

  secretOptions =
    { name, ... }:
    {
      options = {
        path = mkOption {
          type = types.str;
          description = "Absolute path the deploy writes the secret to.";
        };
        owner = mkOption {
          type = types.str;
          default = "root";
        };
        group = mkOption {
          type = types.str;
          default = "root";
        };
        mode = mkOption {
          type = types.strMatching "[0-7]{3,4}";
          default = "0400";
        };
        label = mkOption {
          type = types.str;
          default = name;
          description = "Label of the field in imaged's deploy dialog.";
        };
        description = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Help text shown under the field in imaged's deploy dialog.";
        };
        perMachine = mkOption {
          type = types.bool;
          default = true;
          description = "Whether every machine of a deploy gets its own value or all share one.";
        };
        required = mkOption {
          type = types.bool;
          default = true;
        };
      };
    };

  scriptName =
    imageConfig:
    "imaged-nixos-${builtins.substring 0 12 (builtins.baseNameOf imageConfig.system.build.toplevel)}";

  scriptInputs = lib.mapAttrsToList (name: secret: {
    inherit name;
    inherit (secret) label description required;
    kind = "secret";
    per_machine = secret.perMachine;
  }) cfg.secrets;

  writeSecret =
    name: secret:
    let
      variable = "IMAGED_INPUT_${lib.toUpper name}";
      path = lib.escapeShellArg secret.path;
    in
    ''
      if [ -n "''${${variable}:-}" ]; then
        (umask 022; mkdir -p "$(dirname ${path})")
        rm -f ${path}
        printf '%s' "''$${variable}" > ${path}
        chown ${lib.escapeShellArg "${secret.owner}:${secret.group}"} ${path}
        chmod ${secret.mode} ${path}
        log "wrote secret ${name} to "${path}
      else
        log "WARNING: secret ${name} has no value, "${path}" was not written"
      fi
    '';

  bootDevices =
    imageConfig:
    let
      inherit (imageConfig.boot.loader) grub limine;
    in
    lib.optionals grub.enable (lib.filter (device: device != "nodev") grub.devices)
    ++ lib.optional (
      (limine.enable or false) && limine.biosSupport && limine.biosDevice != "nodev"
    ) limine.biosDevice;

  efiLoader =
    imageConfig:
    let
      loader = imageConfig.boot.loader;
    in
    loader.systemd-boot.enable
    || (loader.grub.enable && loader.grub.efiSupport)
    || ((loader.limine.enable or false) && loader.limine.efiSupport)
    || (loader.refind.enable or false);

  linkBootDevice =
    device:
    let
      path = lib.escapeShellArg device;
    in
    ''
      if [ ! -e ${path} ]; then
        mkdir -p "$(dirname ${path})"
        ln -s "$target_disk" ${path}
        log "linked boot loader device "${path}" to $target_disk"
      elif [ "$(readlink -f ${path})" != "$target_disk" ]; then
        log "boot loader device "${path}" is $(readlink -f ${path}), not the target disk $target_disk"
        exit 1
      fi
    '';

  linkBootDeviceInDiskoBuilder =
    device:
    let
      path = lib.escapeShellArg device;
    in
    ''
      if [ ! -e ${path} ]; then
        mkdir -p "$(dirname ${path})"
        ln -s "$device" ${path}
      fi
    '';

  postInstall =
    imageConfig:
    let
      systemPath = imageConfig.system.path;
      esp = lib.escapeShellArg imageConfig.boot.loader.efi.efiSysMountPoint;
    in
    ''
      #!${pkgs.runtimeShell}
      set -euo pipefail
      export PATH=${systemPath}/bin:${systemPath}/sbin
      log() { echo "imaged-nixos: $*"; }

      umask 077
      ${lib.concatStrings (lib.mapAttrsToList writeSecret cfg.secrets)}
      root_source=$(findmnt -no SOURCE /)
      target_disk=/dev/$(lsblk -no PKNAME "$root_source")
      log "root $root_source on disk $target_disk, mounted: $(findmnt -rno TARGET | tr '\n' ' ')"
      mkdir -p /run
      mountpoint -q /run || mount -t tmpfs -o mode=0755 tmpfs /run
      ${lib.concatMapStrings linkBootDevice (bootDevices imageConfig)}
      ${lib.optionalString (efiLoader imageConfig) ''
        if ! mountpoint -q ${esp}; then
          log "the ESP "${esp}" is not mounted, so the EFI boot loader cannot be installed"
          exit 1
        fi
      ''}
      log "installing the bootloader"
      SYSTEMD_RELAX_ESP_CHECKS=1 NIXOS_INSTALL_BOOTLOADER=1 \
        ${imageConfig.system.build.installBootLoader} ${imageConfig.system.build.toplevel}
      log "bootloader installed"
    '';

  packArchive =
    imageConfig: raw: rootPartition:
    let
      name = scriptName imageConfig;
      toplevel = imageConfig.system.build.toplevel;
      script = pkgs.writeText "${name}.sh" (postInstall imageConfig);
      closure = pkgs.closureInfo { rootPaths = [ toplevel ]; };
      start = if partition then "0" else "$(partx -g -o START --nr ${toString rootPartition} ${raw})";
      source =
        if partition then
          "the partition image"
        else
          "partition ${toString rootPartition} of the disk image";
    in
    pkgs.runCommand imageConfig.image.fileName
      {
        nativeBuildInputs = [
          cfg.package
          pkgs.e2fsprogs
          pkgs.util-linux
        ];
        exportReferencesGraph = [
          "script"
          script
        ];
      }
      ''
        missing=$(grep '^/nix/store' script | grep -vxF ${script} | sort -u | comm -23 - <(sort ${closure}/store-paths))
        if [ -n "$missing" ]; then
          echo "the post-install script needs store paths the image does not ship:" >&2
          echo "$missing" >&2
          exit 1
        fi
        start=${start}
        debugfs -R 'rdump /nix/var/nix/profiles .' "${raw}?offset=$((start * 512))" || true
        installed=$(cd profiles && readlink "$(readlink system)" || true)
        if [ "$installed" != ${toplevel} ]; then
          echo "${source} installs the system '$installed', but the post-install script runs ${toplevel}" >&2
          exit 1
        fi
        mkdir -p $out scripts
        cp ${script} scripts/${name}.sh
        cp ${pkgs.writeText "inputs.json" (builtins.toJSON scriptInputs)} scripts/${name}.inputs.json
        imaged-pack ${raw} --name ${lib.escapeShellArg cfg.name} \
          --root-partition ${toString rootPartition} \
          --script scripts/${name}.sh -o "$out"/${lib.escapeShellArg imageConfig.image.fileName}
      '';

  volatile = [
    "/run"
    "/tmp"
    "/dev"
    "/proc"
    "/sys"
    "/nix/store"
  ];

  under = dir: path: path == dir || lib.hasPrefix "${lib.removeSuffix "/" dir}/" path;

  secretAssertions =
    imageConfig:
    let
      tmpfsMounts = lib.mapAttrsToList (_: fs: fs.mountPoint) (
        lib.filterAttrs (_: fs: fs.fsType == "tmpfs") imageConfig.fileSystems
      );
      etcTarget = path: lib.removePrefix "/etc/" path;
      bootUsers =
        imageConfig.services.userborn.enable or false || imageConfig.systemd.sysusers.enable or false;
    in
    lib.concatLists (
      lib.mapAttrsToList (name: secret: [
        {
          assertion = builtins.match "[a-z0-9_]+" name != null;
          message = "imaged.image.secrets.${name}: the name becomes a deploy input and may only contain a-z, 0-9 and _";
        }
        {
          assertion = lib.hasPrefix "/" secret.path;
          message = "imaged.image.secrets.${name}.path must be absolute, got `${secret.path}`";
        }
        {
          assertion = !lib.any (dir: under dir secret.path) (volatile ++ tmpfsMounts);
          message = "imaged.image.secrets.${name}.path `${secret.path}` is on a filesystem that does not survive a reboot, but imaged writes secrets once, during the deploy";
        }
        {
          assertion = !(lib.hasPrefix "/etc/" secret.path && imageConfig.environment.etc ? ${etcTarget secret.path});
          message = "imaged.image.secrets.${name}.path `${secret.path}` is also managed by environment.etc, which would replace it on activation";
        }
        {
          assertion = secret.owner == "root" || imageConfig.users.users ? ${secret.owner};
          message = "imaged.image.secrets.${name}.owner `${secret.owner}` is not declared in users.users; the deploy can only chown to users that exist in the image";
        }
        {
          assertion = secret.group == "root" || imageConfig.users.groups ? ${secret.group};
          message = "imaged.image.secrets.${name}.group `${secret.group}` is not declared in users.groups; the deploy can only chown to groups that exist in the image";
        }
        {
          assertion = !bootUsers || (secret.owner == "root" && secret.group == "root");
          message = "imaged.image.secrets.${name} is owned by ${secret.owner}:${secret.group}, but services.userborn or systemd.sysusers creates users at boot, so they do not exist yet when the deploy writes the secret";
        }
      ]) cfg.secrets
    );

  systemAssertions =
    imageConfig:
    let
      esp = imageConfig.boot.loader.efi.efiSysMountPoint;
    in
    [
      {
        assertion = !(imageConfig.system.etc.overlay.enable or false);
        message = "imaged cannot deploy system.etc.overlay: the deploy reads /etc/fstab from the root filesystem and writes secrets into it, but at boot the overlay hides that /etc";
      }
      {
        assertion =
          !efiLoader imageConfig
          || lib.any (fs: fs.mountPoint == esp) (lib.attrValues imageConfig.fileSystems);
        message = "the EFI boot loader installs into boot.loader.efi.efiSysMountPoint `${esp}`, but no fileSystems entry mounts the ESP there";
      }
    ];

  diskoPartitions =
    imageConfig:
    lib.sort (a: b: a.priority < b.priority) (
      lib.attrValues (
        (lib.head (lib.attrValues imageConfig.disko.devices.disk)).content.partitions or { }
      )
    );

  diskoAssertions =
    imageConfig:
    let
      disks = imageConfig.disko.devices.disk;
      disk = lib.head (lib.attrValues disks);
      table = disk.content;
      partitions = diskoPartitions imageConfig;
      supported =
        partition:
        partition.content == null
        || partition.content.type == "swap"
        || (
          partition.content.type == "filesystem"
          && lib.elem partition.content.format [
            "ext2"
            "ext3"
            "ext4"
            "vfat"
          ]
        );
      unsupported = lib.filter (p: !supported p) partitions;
      describe =
        p: "${p.name} (${if p.content == null then "none" else p.content.format or p.content.type})";
      root = lib.last partitions;
      otherDevices = lib.filter (kind: (imageConfig.disko.devices.${kind} or { }) != { }) [
        "lvm_vg"
        "mdadm"
        "zpool"
        "bcachefs_filesystems"
      ];
    in
    [
      {
        assertion = lib.length (lib.attrNames disks) == 1;
        message = "imaged images hold one disk, but disko.devices.disk declares ${toString (lib.length (lib.attrNames disks))}: ${lib.concatStringsSep ", " (lib.attrNames disks)}";
      }
      {
        assertion = otherDevices == [ ];
        message = "imaged cannot image disko's ${lib.concatStringsSep ", " otherDevices} devices";
      }
    ]
    ++ lib.optionals (lib.length (lib.attrNames disks) == 1) [
      {
        assertion = table.type or null == "gpt";
        message = "imaged images need a GPT disk, but disko disk `${disk.name}` has content type `${table.type or "none"}`";
      }
    ]
    ++ lib.optionals (lib.length (lib.attrNames disks) == 1 && table.type or null == "gpt") [
      {
        assertion = unsupported == [ ];
        message = "imaged can image ext2/3/4, vfat and swap partitions and partitions without content, not: ${lib.concatMapStringsSep ", " describe unsupported}";
      }
      {
        assertion = partitions != [ ] && root.content != null && root.content.mountpoint or null == "/";
        message = "the root partition must be the last partition of disko disk `${disk.name}` (size = \"100%\") so a deployed disk can grow it to fill a larger disk";
      }
    ];

  makeDiskImageAssertions =
    imageConfig:
    let
      imaged = [ "/" ] ++ lib.optional efi "/boot";
      extraMounts = lib.filter (fs: !lib.elem fs.mountPoint imaged && onDisk fs.device) (
        lib.attrValues imageConfig.fileSystems
      );
      swaps = lib.filter (swap: onDisk swap.device) imageConfig.swapDevices;
    in
    [
      {
        assertion = extraMounts == [ ];
        message = "without disko the imaged image has only ${lib.concatStringsSep " and " imaged} (partitionTableType = \"${cfg.partitionTableType}\"), but fileSystems also mounts ${
          lib.concatMapStringsSep ", " (fs: "${fs.mountPoint} from ${toString fs.device}") extraMounts
        }; describe the disk with disko instead";
      }
      {
        assertion = swaps == [ ];
        message = "without disko the imaged image has no swap partition, but swapDevices uses ${
          lib.concatMapStringsSep ", " (swap: toString swap.device) swaps
        }; describe the disk with disko instead or use a swap file";
      }
    ];

  partitionAssertions =
    imageConfig:
    let
      swaps = lib.filter (swap: onDisk swap.device) imageConfig.swapDevices;
    in
    [
      {
        assertion = !disko;
        message = "imaged.image.kind = \"partition\" packs only the root filesystem, but disko describes a whole disk; remove disko or use imaged.image.kind = \"disk\"";
      }
      {
        assertion = !imageConfig.boot.growPartition;
        message = "imaged.image.kind = \"partition\" deploys into an existing partition and never changes the partition table, but boot.growPartition would grow the root partition at boot";
      }
      {
        assertion = swaps == [ ];
        message = "the imaged partition image holds only the root filesystem, but swapDevices uses ${
          lib.concatMapStringsSep ", " (swap: toString swap.device) swaps
        }; use a swap file";
      }
    ];
in
{
  options.imaged.image = {
    package = mkOption {
      type = types.package;
      description = "The imaged-pack package that packs the disk image.";
    };
    name = mkOption {
      type = types.str;
      default = config.system.name;
      defaultText = lib.literalExpression "config.system.name";
      description = "Image name stored in the archive. The archive file is named after it, with characters that may not appear in store paths replaced by `-`.";
    };
    kind = mkOption {
      type = types.enum [
        "disk"
        "partition"
      ];
      default = "disk";
      description = ''
        What the image holds. `disk` is a whole disk with its partition
        table, laid out by disko or by `partitionTableType`. `partition` is
        only the ext4 root filesystem (label `nixos`): deploying it
        overwrites one existing partition of the host and leaves the
        partition table and the other partitions alone, so `/boot` and any
        other filesystems come from the configuration's own `fileSystems`.
        Partition images cannot use disko.
      '';
    };
    partitionTableType = mkOption {
      type = types.enum [
        "efi"
        "hybrid"
        "legacy+gpt"
      ];
      default = "efi";
      description = ''
        Disk layout of a `disk` image when the configuration does not use
        disko: an ESP (`efi`), an ESP plus a BIOS boot partition (`hybrid`)
        or only a BIOS boot partition (`legacy+gpt`), each followed by the
        ext4 root. Partition images ignore it.
      '';
    };
    additionalSpace = mkOption {
      type = types.str;
      default = "512M";
      description = "Free space added to the root filesystem when the configuration does not use disko.";
    };
    secrets = mkOption {
      type = types.attrsOf (types.submodule secretOptions);
      default = { };
      description = ''
        Files written during the deploy. Each becomes a required, masked
        `secret` input of the image's post-install script, named after the
        attribute, whose value is written to `path` with `owner`, `group` and
        `mode`.
      '';
    };
  };

  config = lib.mkMerge [
    {
      boot.growPartition = lib.mkDefault (!partition);
      fileSystems."/".autoResize = lib.mkDefault true;
      image.modules.imaged =
        {
          config,
          lib,
          pkgs,
          modulesPath,
          ...
        }:
        let
          partitionImage = import "${modulesPath}/../lib/make-disk-image.nix" {
            inherit lib config pkgs;
            name = "imaged-partition";
            baseName = "nixos";
            format = "raw";
            partitionTableType = "none";
            inherit (cfg) additionalSpace;
            installBootLoader = false;
            copyChannel = false;
          };
          activatedPartitionImage = pkgs.vmTools.runInLinuxVM (
            pkgs.runCommand "imaged-partition-activated"
              {
                preVM = ''
                  mkdir $out
                  cp --sparse=always ${partitionImage}/nixos.img $out/nixos.img
                  chmod u+w $out/nixos.img
                  diskImage=$out/nixos.img
                '';
                buildInputs = [
                  pkgs.util-linux
                  pkgs.e2fsprogs
                  pkgs.nixos-enter
                ];
                memSize = 1024;
              }
              ''
                mkdir /mnt
                mount /dev/vda /mnt
                nixos-enter --root /mnt -- true
                umount -R /mnt
                tune2fs -T now -c 0 -i 0 /dev/vda
                tune2fs -f -T 19700101 /dev/vda
              ''
          );
          raw =
            if partition then
              "${activatedPartitionImage}/nixos.img"
            else if disko then
              "${config.system.build.diskoImages}/${(lib.head (lib.attrValues config.disko.devices.disk)).imageName}.raw"
            else
              "${
                import "${modulesPath}/../lib/make-disk-image.nix" {
                  inherit lib config pkgs;
                  name = "imaged-disk";
                  baseName = "nixos";
                  format = "raw";
                  inherit (cfg) partitionTableType additionalSpace;
                  copyChannel = false;
                }
              }/nixos.img";
          rootPartition =
            if partition then
              1
            else if disko then
              (lib.findFirst (p: p.content.mountpoint or null == "/") null (diskoPartitions config))._index or 0
            else
              {
                efi = 2;
                hybrid = 3;
                "legacy+gpt" = 2;
              }
              .${cfg.partitionTableType};
        in
        {
          imports = [ "${modulesPath}/image/file-options.nix" ];
          config = lib.mkMerge [
            {
              assertions =
                systemAssertions config
                ++ secretAssertions config
                ++ (
                  if partition then
                    partitionAssertions config
                  else if disko then
                    diskoAssertions config
                  else
                    makeDiskImageAssertions config
                );
              image.extension = "imaged";
              image.baseName = lib.mkDefault (lib.strings.sanitizeDerivationName cfg.name);
              system.build.image = packArchive config raw rootPartition;
            }
            (lib.optionalAttrs disko {
              disko.imageBuilder.imageFormat = lib.mkForce "raw";
              boot.loader.grub.devices = lib.mkOverride 49 host.boot.loader.grub.devices;
              disko.devices.disk = lib.mapAttrs (_: _: {
                postCreateHook = lib.concatMapStrings linkBootDeviceInDiskoBuilder (bootDevices host);
              }) host.disko.devices.disk;
            })
          ];
        };
    }
    (lib.mkIf (!disko) {
      fileSystems."/" = lib.mkForce {
        device = "/dev/disk/by-label/nixos";
        fsType = "ext4";
        autoResize = true;
      };
      fileSystems."/boot" = lib.mkIf (efi && !partition) (
        lib.mkForce {
          device = "/dev/disk/by-label/ESP";
          fsType = "vfat";
          options = [
            "fmask=0077"
            "dmask=0077"
          ];
        }
      );
    })
  ];
}
