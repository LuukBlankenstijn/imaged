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
            cp ${udpcast}/bin/udp-receiver "$STAGING/bin/"
            cp ${pkgs.pkgsStatic.klibc}/lib/klibc/bin.static/ipconfig "$STAGING/bin/"
            cp ${partclone}/bin/partclone.extfs "$STAGING/bin/"
            cp ${partclone}/bin/partclone.vfat "$STAGING/bin/"
            ln -sfr "$STAGING/bin/partclone.extfs" "$STAGING/bin/partclone.ext4"
            ln -sf busybox "$STAGING/bin/sh"

            cat > "$STAGING/init" <<'EOF'
            #!/bin/sh
            /bin/busybox --install -s /bin
            /bin/busybox mount -t proc none /proc
            /bin/busybox mount -t sysfs none /sys
            /bin/busybox mount -t devtmpfs none /dev

            /bin/ipconfig -d all
            if [ -f /run/net-eth0.conf ]; then
                . /run/net-eth0.conf
                [ -n "$IPV4DNS0" ] && [ "$IPV4DNS0" != "0.0.0.0" ] && echo "nameserver $IPV4DNS0" > /etc/resolv.conf
                echo "$ROOTSERVER" > /run/imaging_server
            fi

            exec /bin/sh
            EOF
            chmod +x "$STAGING/init"

            OUTPUT="$(pwd)/initramfs.cpio.gz"
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
              -kernel vmlinuz \
              -initrd initramfs.cpio.gz \
              -append "console=tty0 console=ttyS0 earlyprintk=vga loglevel=8" \
              -nographic
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
        };
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            build-initramfs
            run-vm
          ];

          shellHook = ''
            cp -f ${pkgs.ipxe}/ipxe.efi ./tftp/ipxe.efi
            cp -f ${pkgs.ipxe}/undionly.kpxe ./tftp/undionly.kpxe
            cp -f ${kernel}/bzImage ./vmlinuz
          '';
        };
      }
    );
}
