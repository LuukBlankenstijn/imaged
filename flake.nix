{
  description = "imaged: network boot imaging tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        udpcast = pkgs.pkgsStatic.stdenv.mkDerivation {
          pname = "udpcast";
          version = "20211207";
          src = pkgs.fetchurl {
            url = "http://www.udpcast.linux.lu/download/udpcast-20211207.tar.gz";
            sha256 = "sha256-o86+56h+zxvKBkXxJb54+9ezeEak2oL+zvlrksxk0FA=";
          };
          postPatch = ''
            sed -i '1i #include <stddef.h>' receivedata.c
            sed -i '1i #include <stddef.h>' participants.c
          '';
          nativeBuildInputs = [
            pkgs.m4
            pkgs.perl
          ];
          # Force static linking for the binaries
          makeFlags = [ "LDFLAGS=-static" ];
          installPhase = ''
            mkdir -p $out/bin
            cp udp-receiver udp-sender $out/bin/
          '';
        };
        partclone =
          (pkgs.pkgsStatic.partclone.override {
            nilfs-utils = null;
          }).overrideAttrs
            (oldAttrs: {
              # 1. Clear out the hardcoded flags and use only what you need
              configureFlags = [
                "--enable-extfs"
                "--enable-fat"
                "--enable-ntfs"
                "--enable-pkg-config-static"
                "--disable-xfs"
                "--disable-btrfs"
                "--disable-f2fs"
                "--disable-nilfs2"
                "--disable-minix"
                "--disable-hfsp"
              ];

              NIX_LDFLAGS = "-lext2fs -lcom_err -lblkid -luuid";

              buildInputs = oldAttrs.buildInputs ++ [
                pkgs.pkgsStatic.e2fsprogs
                pkgs.pkgsStatic.libuuid
              ];

              nativeBuildInputs = (oldAttrs.nativeBuildInputs or [ ]) ++ [
                pkgs.pkg-config
              ];
            });
        kernel =
          let
            inherit (pkgs.linux_latest) src version;

            configfile = pkgs.stdenv.mkDerivation {
              name = "config";
              inherit src;
              nativeBuildInputs = [
                pkgs.perl
                pkgs.gnumake
                pkgs.stdenv.cc
                pkgs.bison
                pkgs.flex
              ];

              buildPhase = ''
                      patchShebangs scripts/config

                      make x86_64_defconfig

                      cat >> .config <<EOF
                CONFIG_IP_PNP=y
                CONFIG_IP_PNP_DHCP=y
                CONFIG_PACKET=y
                CONFIG_VIRTIO_NET=y
                CONFIG_VIRTIO_PCI=y
                CONFIG_E1000=y
                CONFIG_E1000E=y
                CONFIG_USB_RTL8152=y
                CONFIG_IKCONFIG=y
                CONFIG_IKCONFIG_PROC=y
                CONFIG_DEVTMPFS=y
                CONFIG_DEVTMPFS_MOUNT=y
                EOF

                      # Use the kernel script to ensure these are 'y' (Static)
                      ./scripts/config --enable CONFIG_IP_PNP_DHCP
                      ./scripts/config --enable CONFIG_VIRTIO_NET
                      ./scripts/config --enable CONFIG_E1000
                      ./scripts/config --enable CONFIG_E1000E

                      # Storage controllers built-in (initramfs has no modules)
                      # so real disks are detected on physical machines.
                      ./scripts/config --enable CONFIG_NVME_CORE
                      ./scripts/config --enable CONFIG_BLK_DEV_NVME
                      ./scripts/config --enable CONFIG_PCI_MSI
                      ./scripts/config --enable CONFIG_VMD
                      ./scripts/config --enable CONFIG_ATA
                      ./scripts/config --enable CONFIG_SATA_AHCI
                      ./scripts/config --enable CONFIG_ATA_PIIX
                      ./scripts/config --enable CONFIG_SCSI
                      ./scripts/config --enable CONFIG_BLK_DEV_SD
                      ./scripts/config --enable CONFIG_MMC
                      ./scripts/config --enable CONFIG_MMC_BLOCK
                      ./scripts/config --enable CONFIG_MMC_SDHCI
                      ./scripts/config --enable CONFIG_MMC_SDHCI_PCI
                      ./scripts/config --enable CONFIG_MMC_SDHCI_ACPI

                      # EFI framebuffer console so physical monitors show output.
                      ./scripts/config --enable CONFIG_SYSFB_SIMPLEFB
                      ./scripts/config --enable CONFIG_DRM
                      ./scripts/config --enable CONFIG_DRM_SIMPLEDRM
                      ./scripts/config --enable CONFIG_DRM_FBDEV_EMULATION
                      ./scripts/config --enable CONFIG_FB
                      ./scripts/config --enable CONFIG_FB_EFI
                      ./scripts/config --enable CONFIG_FRAMEBUFFER_CONSOLE

                      # Resolve dependencies
                      make olddefconfig
              '';

              installPhase = "cp .config $out";
            };
          in
          pkgs.linuxManualConfig {
            inherit src version configfile;
            allowImportFromDerivation = true;
          };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        build-initramfs = pkgs.writeShellApplication {
          name = "build-initramfs";
          runtimeInputs = [
            pkgs.coreutils
            pkgs.findutils
            pkgs.cpio
            pkgs.gzip
          ];
          text = ''
            STAGING=$(mktemp -d)
            trap 'rm -rf "$STAGING"' EXIT

            mkdir -p "$STAGING"/{bin,proc,sys,dev,tmp,etc,run}

            cp ${pkgs.pkgsStatic.busybox}/bin/busybox "$STAGING/bin/busybox"
            cp ${pkgs.pkgsStatic.libuuid}/bin/lsblk "$STAGING/bin/"
            cp ${pkgs.pkgsStatic.gptfdisk}/bin/sgdisk "$STAGING/bin/"
            cp ${udpcast}/bin/udp-receiver "$STAGING/bin/"
            cp ${pkgs.pkgsStatic.klibc}/lib/klibc/bin.static/ipconfig "$STAGING/bin/"
            cp ${pkgs.pkgsStatic.tcpdump}/bin/tcpdump "$STAGING/bin/"
            cp ${partclone}/bin/partclone.extfs "$STAGING/bin/"
            cp ${partclone}/bin/partclone.vfat "$STAGING/bin/"
            cp "$(pwd)/target/x86_64-unknown-linux-musl/release/imaged-client" "$STAGING/bin/"
            ln -sfr "$STAGING/bin/partclone.extfs" "$STAGING/bin/partclone.ext4"
            ln -sf busybox "$STAGING/bin/sh"

            cat > "$STAGING/init" <<'EOF'
            #!/bin/sh
            /bin/busybox --install -s /bin
            /bin/busybox mount -t proc none /proc
            /bin/busybox mount -t sysfs none /sys
            /bin/busybox mount -t devtmpfs none /dev

            echo "Running dhcp to get an ip"
            /bin/ipconfig -d all
            if [ -f /run/net-eth0.conf ]; then
                . /run/net-eth0.conf
                [ -n "$IPV4DNS0" ] && [ "$IPV4DNS0" != "0.0.0.0" ] && echo "nameserver $IPV4DNS0" > /etc/resolv.conf
                echo "$ROOTSERVER" > /run/imaging_server
            fi

            for arg in $(cat /proc/cmdline); do
                case "$arg" in
                    img_srv=*) SERVER_ADDR="''${arg#img_srv=}" ;;
                esac
            done

            if [ -z "''${SERVER_ADDR:-}" ]; then
                echo "ERROR: Kernel parameter 'img_srv' not found!"
                exec /bin/sh
            fi

            echo "Starting imaged-client at $SERVER_ADDR"
            /bin/imaged-client "$SERVER_ADDR"

            echo "imaged-client exited. Dropping to recovery shell."
            exec /bin/sh
            EOF
            chmod +x "$STAGING/init"

            OUTPUT="$(pwd)/boot/initramfs.cpio.gz"
            cd "$STAGING"
            find . | cpio -o -H newc 2>/dev/null | gzip > "$OUTPUT"
            echo "built: $(du -h "$OUTPUT" | cut -f1)"
          '';
        };

        run-vm = pkgs.writeShellApplication {
          name = "run-vm";
          runtimeInputs = [ pkgs.qemu ];
          text = ''
            exec qemu-system-x86_64 \
              -cpu host \
              -enable-kvm \
              -m 1G \
              -kernel boot/vmlinuz \
              -initrd boot/initramfs.cpio.gz \
              -drive file=storage.qcow2,format=qcow2,if=virtio \
              -append "console=tty0 console=ttyS0 earlyprintk=vga loglevel=8 img_srv=http://192.168.100.1:8080" \
              -nographic \
              -netdev tap,id=net0,ifname=tap0,script=no,downscript=no \
              -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
          '';
        };

        run-vm-pxe = pkgs.writeShellApplication {
          name = "run-vm-pxe";
          runtimeInputs = [ pkgs.qemu ];
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
        };

        run-server = pkgs.writeShellApplication {
          name = "run-server";
          runtimeInputs = [ udpcast ];
          text = ''
            cargo build --package imaged-server
            sudo setcap 'cap_net_bind_service=+ep' target/x86_64-unknown-linux-musl/debug/imaged-server
            target/x86_64-unknown-linux-musl/debug/imaged-server
          '';
        };

        setup-net = pkgs.writeShellApplication {
          name = "setup-net";
          runtimeInputs = [ pkgs.iproute2 ];
          text = ''
            bridge=br-netboot
            tap=tap0
            user=''${SUDO_USER:-$USER}

            if [ "$(id -u)" -ne 0 ]; then
              echo "must run as root: sudo setup-net" >&2
              exit 1
            fi

            if ! ip link show "$bridge" >/dev/null 2>&1; then
              echo "bridge $bridge not found; run 'docker compose up -d' first" >&2
              exit 1
            fi

            if ! ip link show "$tap" >/dev/null 2>&1; then
              ip tuntap add dev "$tap" mode tap user "$user"
              echo "created tap $tap owned by $user"
            fi

            current_master=$(ip -o link show "$tap" | sed -n 's/.* master \([^ ]*\).*/\1/p')
            if [ "$current_master" != "$bridge" ]; then
              ip link set "$tap" master "$bridge"
              echo "attached tap $tap to bridge $bridge"
            fi

            ip link set "$tap" up
          '';
        };

        teardown-net = pkgs.writeShellApplication {
          name = "teardown-net";
          runtimeInputs = [ pkgs.iproute2 ];
          text = ''
            tap=tap0

            if [ "$(id -u)" -ne 0 ]; then
              echo "must run as root: sudo teardown-net" >&2
              exit 1
            fi

            if ip link show "$tap" >/dev/null 2>&1; then
              ip link set "$tap" down || true
              ip tuntap del dev "$tap" mode tap
              echo "removed tap $tap"
            fi
          '';
        };
      in
      {
        packages = {
          inherit udpcast;
          inherit kernel;
          inherit partclone;
          inherit build-initramfs;
          inherit run-vm;
          inherit run-vm-pxe;
          inherit setup-net;
          inherit teardown-net;
          imaged-server = pkgs.callPackage ./nix/package.nix { };
        };
        devShells.default =
          with pkgs;
          pkgs.mkShell {
            packages = [
              rustToolchain
              build-initramfs
              run-vm
              run-vm-pxe
              run-server
              setup-net
              teardown-net
              udpcast
              buf
              protoc-gen-tonic
              protoc-gen-prost
              protoc-gen-es
              protoc-gen-prost-crate
              sqlx-cli

              nodejs
              pnpm
            ];

            env = {
              CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
              DATABASE_URL = "sqlite://imaged.db";
              MULTICAST_INTERFACE = "br-netboot";
              PUBLIC_BASE = "192.168.100.1:8080";
            };

            shellHook = ''
              cp -f ${pkgs.ipxe}/ipxe.efi ./tftp/ipxe.efi
              cp -f ${pkgs.ipxe}/undionly.kpxe ./tftp/undionly.kpxe
              cp -f ${kernel}/bzImage ./vmlinuz
            '';
          };
      }
    )
    // {
      nixosModules.default = import ./nix/module.nix;
    };
}
