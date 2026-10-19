{
  imaged,
  disko,
}:
let
  nixpkgs = imaged.inputs.nixpkgs;
  inherit (nixpkgs) lib;

  host =
    name: modules:
    nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        imaged.nixosModules.image
        common
        { imaged.image.name = "e2e-${name}"; }
      ]
      ++ modules;
    };

  diskoRoot = partitions: {
    imports = [ disko.nixosModules.disko ];
    boot.loader.systemd-boot.enable = true;
    disko.devices.disk.main = {
      device = "/dev/disk/by-id/imaged-e2e-disk";
      type = "disk";
      content = {
        type = "gpt";
        inherit partitions;
      };
    };
  };

  rootPartition = {
    size = "100%";
    content = {
      type = "filesystem";
      format = "ext4";
      mountpoint = "/";
    };
  };

  common =
    {
      config,
      lib,
      pkgs,
      modulesPath,
      ...
    }:
    {
      imports = [ "${modulesPath}/profiles/qemu-guest.nix" ];

      system.stateVersion = "26.05";
      documentation.enable = false;
      environment.defaultPackages = [ ];
      boot.kernelParams = [ "console=ttyS0,115200" ];
      boot.loader.timeout = lib.mkForce 1;
      networking.useDHCP = false;

      fileSystems."/" = lib.mkDefault {
        device = "/dev/disk/by-uuid/00000000-0000-0000-0000-000000000000";
        fsType = "ext4";
      };

      users.users.app = {
        isSystemUser = true;
        group = "app";
      };
      users.groups.app = { };

      imaged.image.secrets = {
        app_token = {
          path = "/var/lib/app/token";
          owner = "app";
          group = "app";
          mode = "0440";
          label = "App token";
        };
        nested = {
          path = "/var/lib/nested-app/keys/deep/token";
          owner = "app";
          group = "app";
        };
        age_key = {
          path = "/var/lib/sops-nix/key.txt";
          perMachine = false;
          description = "Bootstrap key for sops-nix";
        };
      };

      systemd.services.imaged-e2e-report = {
        wantedBy = [ "multi-user.target" ];
        after = [
          "local-fs.target"
          "swap.target"
        ];
        path = [
          pkgs.coreutils
          pkgs.util-linux
        ];
        serviceConfig = {
          Type = "oneshot";
          StandardOutput = "tty";
          TTYPath = "/dev/ttyS0";
        };
        script = ''
          report() { echo "IMAGED-E2E $*"; }
          ${lib.concatMapStrings (secret: ''
            f=${secret.path}
            if [ -e "$f" ]; then
              owner=$(stat -c %U "$f")
              readable=$(runuser -u "$owner" -- cat "$f" >/dev/null 2>&1 && echo readable || echo unreadable)
              report "file $f $owner:$(stat -c %G "$f") $(stat -c %a "$f") $(sha256sum < "$f" | cut -d' ' -f1) $readable"
            else
              report "file $f missing"
            fi
          '') (lib.attrValues config.imaged.image.secrets)}
          report "root $(findmnt -bno SIZE /)"
          report "swap $(blkid -s UUID -o value -t TYPE=swap | head -1 || true) $(swapon --show=NAME --noheadings | wc -l)"
          report "efi $([ -d /sys/firmware/efi ] && echo yes || echo no)"
          report "DONE"
        '';
      };
    };
in
{
  sdboot = host "sdboot" [ { boot.loader.systemd-boot.enable = true; } ];

  grub-bios = host "grub-bios" [
    {
      boot.loader.grub = {
        enable = true;
        device = "/dev/disk/by-id/imaged-e2e-disk";
      };
      imaged.image.partitionTableType = "legacy+gpt";
    }
  ];

  invalid = {
    "does not survive a reboot" = host "volatile" [
      {
        boot.loader.systemd-boot.enable = true;
        imaged.image.secrets.age_key.path = lib.mkForce "/run/keys/age";
      }
    ];
    "is not declared in users.users" = host "owner" [
      {
        boot.loader.systemd-boot.enable = true;
        imaged.image.secrets.app_token.owner = lib.mkForce "nobody-here";
      }
    ];
    "creates users at boot" = host "userborn" [
      {
        boot.loader.systemd-boot.enable = true;
        services.userborn.enable = true;
      }
    ];
    "may only contain a-z, 0-9 and _" = host "name" [
      {
        boot.loader.systemd-boot.enable = true;
        imaged.image.secrets."Bad-Name".path = "/var/lib/bad";
      }
    ];
    "describe the disk with disko instead" = host "extra-fs" [
      {
        boot.loader.systemd-boot.enable = true;
        fileSystems."/srv" = {
          device = "/dev/disk/by-label/srv";
          fsType = "ext4";
        };
      }
    ];
    "not: data (btrfs)" = host "btrfs" [
      (diskoRoot {
        data = {
          size = "1G";
          content = {
            type = "filesystem";
            format = "btrfs";
            mountpoint = "/srv";
          };
        };
        root = rootPartition;
      })
    ];
    "must be the last partition" = host "root-first" [
      (diskoRoot {
        root = rootPartition // {
          size = "2G";
        };
        swap = {
          size = "100%";
          content.type = "swap";
        };
      })
    ];
    "hold one disk" = host "two-disks" [
      (diskoRoot { root = rootPartition; })
      {
        disko.devices.disk.second = {
          device = "/dev/disk/by-id/imaged-e2e-second";
          type = "disk";
          content = {
            type = "gpt";
            partitions.data = rootPartition // {
              content = rootPartition.content // {
                mountpoint = "/srv";
              };
            };
          };
        };
      }
    ];
  };

  grub-hybrid = host "grub-hybrid" [
    {
      boot.loader.grub = {
        enable = true;
        efiSupport = true;
        efiInstallAsRemovable = true;
        device = "/dev/vda";
      };
      imaged.image.partitionTableType = "hybrid";
    }
  ];

  disko = host "disko" [
    disko.nixosModules.disko
    {
      boot.loader.systemd-boot.enable = true;
      disko.devices.disk.main = {
        device = "/dev/disk/by-id/imaged-e2e-disk";
        type = "disk";
        imageSize = "4G";
        content = {
          type = "gpt";
          partitions = {
            ESP = {
              type = "EF00";
              size = "256M";
              content = {
                type = "filesystem";
                format = "vfat";
                mountpoint = "/boot";
                mountOptions = [ "umask=0077" ];
              };
            };
            swap = {
              size = "128M";
              content.type = "swap";
            };
            root = {
              size = "100%";
              content = {
                type = "filesystem";
                format = "ext4";
                mountpoint = "/";
              };
            };
          };
        };
      };
    }
  ];
}
