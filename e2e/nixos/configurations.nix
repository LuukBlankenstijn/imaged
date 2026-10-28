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
        { imaged.image.name = "e2e ${name}"; }
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

  diskoDeploy = partitions: {
    imports = [ disko.nixosModules.disko ];
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
          root = rootPartition;
        }
        // partitions;
      };
    };
  };

  hardwareTest =
    { lib, ... }:
    {
      boot.kernelParams = lib.mkAfter [ "console=tty0" ];
      boot.initrd.systemd.emergencyAccess = true;
      systemd.services.imaged-e2e-report.serviceConfig.TTYPath = lib.mkForce "/dev/tty1";
      users.users.root.initialPassword = "imaged";
      networking.hostName = "imaged-hardware";
      networking.useDHCP = lib.mkForce true;
      services.openssh = {
        enable = true;
        settings.PermitRootLogin = "yes";
        settings.PasswordAuthentication = true;
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

  partition = host "partition" [
    {
      boot.loader.systemd-boot.enable = true;
      fileSystems."/boot" = {
        device = "/dev/disk/by-label/ESP";
        fsType = "vfat";
        options = [ "umask=0077" ];
      };
      imaged.image.kind = "partition";
    }
  ];

  partition-grub-bios = host "partition-grub-bios" [
    {
      boot.loader.grub = {
        enable = true;
        device = "/dev/disk/by-id/imaged-e2e-disk";
      };
      imaged.image.kind = "partition";
    }
  ];

  hardware = host "hardware" [
    hardwareTest
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

  hardware-sdboot = host "hardware-sdboot" [
    hardwareTest
    {
      boot.loader.systemd-boot.enable = true;
      boot.loader.efi.canTouchEfiVariables = true;
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
    "has only / (partitionTableType" = host "legacy-boot" [
      {
        boot.loader.grub = {
          enable = true;
          device = "/dev/disk/by-id/imaged-e2e-disk";
        };
        imaged.image.partitionTableType = "legacy+gpt";
        fileSystems."/boot" = {
          device = "/dev/disk/by-uuid/1234-ABCD";
          fsType = "vfat";
        };
      }
    ];
    "has no swap partition" = host "swap-device" [
      {
        boot.loader.systemd-boot.enable = true;
        swapDevices = [ { device = "/dev/disk/by-uuid/5d1f7a3e-2b4c-4e8f-9a61-0c3b7d2e9f14"; } ];
      }
    ];
    "imaged cannot deploy system.etc.overlay" = host "etc-overlay" [
      {
        boot.loader.systemd-boot.enable = true;
        boot.initrd.systemd.enable = true;
        system.etc.overlay.enable = true;
      }
    ];
    "no fileSystems entry mounts the ESP" = host "sdboot-without-esp" [
      {
        boot.loader.systemd-boot.enable = true;
        imaged.image.partitionTableType = "legacy+gpt";
      }
    ];
    "must be the last partition of disko disk `main`" = host "no-partitions" [
      (diskoRoot { })
      {
        fileSystems."/" = {
          device = "/dev/disk/by-label/elsewhere";
          fsType = "ext4";
        };
      }
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
    "packs only the root filesystem, but disko describes a whole disk" = host "partition-disko" [
      (diskoRoot { root = rootPartition; })
      { imaged.image.kind = "partition"; }
    ];
    "but boot.growPartition would grow the root partition" = host "partition-grow" [
      {
        boot.loader.grub = {
          enable = true;
          device = "/dev/disk/by-id/imaged-e2e-disk";
        };
        boot.growPartition = true;
        imaged.image.kind = "partition";
      }
    ];
    "partition image holds only the root filesystem" = host "partition-swap" [
      {
        boot.loader.grub = {
          enable = true;
          device = "/dev/disk/by-id/imaged-e2e-disk";
        };
        swapDevices = [ { device = "/dev/disk/by-label/swap"; } ];
        imaged.image.kind = "partition";
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
    (diskoDeploy { })
    { boot.loader.systemd-boot.enable = true; }
  ];

  disko-grub = host "disko-grub" [
    (diskoDeploy {
      bios = {
        size = "1M";
        type = "EF02";
      };
    })
    {
      boot.loader.grub = {
        enable = true;
        efiSupport = true;
        efiInstallAsRemovable = true;
      };
    }
  ];
}
