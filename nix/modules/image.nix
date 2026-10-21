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

  scriptName = "imaged-nixos";

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
        log "wrote secret ${name} to ${secret.path}"
      else
        log "WARNING: secret ${name} has no value, ${secret.path} was not written"
      fi
    '';

  postInstall =
    imageConfig:
    let
      grub = imageConfig.boot.loader.grub;
      grubDevices = lib.optionals grub.enable (lib.filter (device: device != "nodev") grub.devices);
      systemPath = imageConfig.system.path;
    in
    ''
      #!${pkgs.runtimeShell}
      set -euo pipefail
      export PATH=${systemPath}/bin:${systemPath}/sbin
      log() { echo "${scriptName}: $*"; }

      umask 077
      ${lib.concatStrings (lib.mapAttrsToList writeSecret cfg.secrets)}
      root_source=$(findmnt -no SOURCE /)
      target_disk=/dev/$(lsblk -no PKNAME "$root_source")
      log "root $root_source on disk $target_disk, mounted: $(findmnt -rno TARGET | tr '\n' ' ')"
      mkdir -p /run
      mountpoint -q /run || mount -t tmpfs -o mode=0755 tmpfs /run
      ${lib.concatMapStrings (device: ''
        if [ ! -e ${device} ]; then
          mkdir -p "$(dirname ${device})"
          ln -s "$target_disk" ${device}
          log "linked grub device ${device} to $target_disk"
        elif [ "$(readlink -f ${device})" != "$target_disk" ]; then
          log "grub device ${device} is $(readlink -f ${device}), not the target disk $target_disk"
          exit 1
        fi
      '') grubDevices}
      log "installing the bootloader"
      SYSTEMD_RELAX_ESP_CHECKS=1 NIXOS_INSTALL_BOOTLOADER=1 \
        ${imageConfig.system.build.installBootLoader} ${imageConfig.system.build.toplevel}
      log "bootloader installed"
    '';

  packArchive =
    imageConfig: raw:
    let
      script = pkgs.writeText "${scriptName}.sh" (postInstall imageConfig);
      closure = pkgs.closureInfo { rootPaths = [ imageConfig.system.build.toplevel ]; };
    in
    pkgs.runCommand imageConfig.image.fileName
      {
        nativeBuildInputs = [ cfg.package ];
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
        mkdir -p $out scripts
        cp ${script} scripts/${scriptName}.sh
        cp ${pkgs.writeText "inputs.json" (builtins.toJSON scriptInputs)} scripts/${scriptName}.inputs.json
        imaged-pack ${raw} --name ${lib.escapeShellArg cfg.name} \
          --script scripts/${scriptName}.sh -o $out/${imageConfig.image.fileName}
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

  diskoAssertions =
    imageConfig:
    let
      disks = imageConfig.disko.devices.disk;
      disk = lib.head (lib.attrValues disks);
      table = disk.content;
      partitions = lib.sort (a: b: a.priority < b.priority) (lib.attrValues table.partitions);
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
    imageConfig: efi:
    let
      imaged = [ "/" ] ++ lib.optional efi "/boot";
      onDisk = device: lib.hasPrefix "/dev/" (toString device);
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
      description = "Image name stored in the archive.";
    };
    partitionTableType = mkOption {
      type = types.enum [
        "efi"
        "hybrid"
        "legacy+gpt"
      ];
      default = "efi";
      description = ''
        Disk layout when the configuration does not use disko: an ESP (`efi`),
        an ESP plus a BIOS boot partition (`hybrid`) or only a BIOS boot
        partition (`legacy+gpt`), each followed by the ext4 root.
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

  config.image.modules.imaged =
    {
      config,
      lib,
      pkgs,
      modulesPath,
      ...
    }:
    let
      disko = options ? disko && config.disko.devices.disk != { };
      efi = cfg.partitionTableType != "legacy+gpt";
      raw =
        if disko then
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
    in
    {
      imports = [ "${modulesPath}/image/file-options.nix" ];
      config = lib.mkMerge [
        {
          assertions =
            secretAssertions config
            ++ (if disko then diskoAssertions config else makeDiskImageAssertions config efi);
          boot.growPartition = lib.mkDefault true;
          fileSystems."/".autoResize = lib.mkDefault true;
          image.extension = "imaged";
          image.baseName = lib.mkDefault "${cfg.name}";
          system.build.image = packArchive config raw;
        }
        (lib.mkIf (!disko) {
          fileSystems."/" = lib.mkForce {
            device = "/dev/disk/by-label/nixos";
            fsType = "ext4";
            autoResize = true;
          };
          fileSystems."/boot" = lib.mkIf efi (
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
    };
}
