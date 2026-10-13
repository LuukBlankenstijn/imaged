{
  description = "imaged: network boot imaging tool";

  nixConfig = {
    extra-substituters = [ "https://luukblankenstijn.cachix.org" ];
    extra-trusted-public-keys = [
      "luukblankenstijn.cachix.org-1:gRz/ypm8zdDizcdAuWD6UKLVBDeObfHsNDWoAka2WSw="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
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
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        inherit (pkgs) lib;

        mkTarget =
          {
            targetPkgs,
            muslPkgs,
            kernel,
            vmlinuz,
          }:
          let
            partclone = targetPkgs.callPackage ./nix/packages/partclone.nix { };
            imaged-client = muslPkgs.callPackage ./nix/packages/imaged-client.nix {
              craneLib = (crane.mkLib muslPkgs).overrideToolchain (
                pkgs.rust-bin.stable.latest.minimal.override {
                  targets = [ muslPkgs.stdenv.hostPlatform.rust.rustcTarget ];
                }
              );
            };
            initramfsStaging = import ./nix/lib/initramfs-staging.nix {
              inherit (targetPkgs) pkgsStatic;
              inherit (pkgs) gawk kmod xz;
              inherit partclone kernel;
            };
          in
          {
            inherit
              partclone
              imaged-client
              initramfsStaging
              muslPkgs
              vmlinuz
              ;
            inherit (targetPkgs) ipxe;
            initramfs = pkgs.callPackage ./nix/packages/initramfs.nix {
              inherit imaged-client initramfsStaging;
            };
          };

        targets = {
          x86_64 =
            let
              kernel = nixpkgs.legacyPackages.x86_64-linux.linuxPackages_latest.kernel;
            in
            mkTarget {
              targetPkgs = pkgs.pkgsCross.gnu64;
              muslPkgs = pkgs.pkgsCross.musl64;
              inherit kernel;
              vmlinuz = "${kernel}/bzImage";
            };
          arm64 =
            let
              kernel = nixpkgs.legacyPackages.aarch64-linux.linuxPackages_latest.kernel;
            in
            mkTarget {
              targetPkgs = pkgs.pkgsCross.aarch64-multiplatform;
              muslPkgs = pkgs.pkgsCross.aarch64-multiplatform-musl;
              inherit kernel;
              vmlinuz = pkgs.runCommand "vmlinuz-arm64" { } "gzip -9n < ${kernel}/Image > $out";
            };
        };

        kernelAssets = lib.mapAttrs' (
          arch: target: lib.nameValuePair "${arch}/vmlinuz" target.vmlinuz
        ) targets;
        initramfsAssets = lib.mapAttrs' (
          arch: target: lib.nameValuePair "${arch}/initramfs.cpio.xz" target.initramfs
        ) targets;
        kernels = pkgs.linkFarm "imaged-kernels" kernelAssets;
        bootAssets = pkgs.linkFarm "imaged-boot-assets" (kernelAssets // initramfsAssets);
        ipxeAssets = pkgs.linkFarm "imaged-ipxe-assets" {
          "ipxe.efi" = "${targets.x86_64.ipxe}/ipxe.efi";
          "undionly.kpxe" = "${targets.x86_64.ipxe}/undionly.kpxe";
          "ipxe-arm64.efi" = "${targets.arm64.ipxe}/ipxe.efi";
        };

        devRustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
          targets = [
            "x86_64-unknown-linux-musl"
            "aarch64-unknown-linux-musl"
            "wasm32-unknown-unknown"
          ];
        };

        buildRustToolchain = pkgs.rust-bin.stable.latest.minimal.override {
          targets = [ "wasm32-unknown-unknown" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain buildRustToolchain;

        hostPartclone = pkgs.callPackage ./nix/packages/partclone.nix { };
        imagingTools = pkgs.buildEnv {
          name = "imaged-imaging-tools";
          paths = [
            hostPartclone
            pkgs.gptfdisk
            pkgs.util-linux
            pkgs.e2fsprogs
          ];
          pathsToLink = [ "/bin" ];
        };
        diskBuildTools = pkgs.buildEnv {
          name = "imaged-disk-build-tools";
          paths = [
            imagingTools
            pkgs.dosfstools
            pkgs.mtools
          ];
          pathsToLink = [ "/bin" ];
        };

        wasm-bindgen-cli =
          let
            lock = builtins.fromTOML (builtins.readFile ./Cargo.lock);
            version =
              (pkgs.lib.findFirst (p: p.name == "wasm-bindgen")
                (throw "wasm-bindgen not found in Cargo.lock")
                lock.package).version;
          in
          pkgs."wasm-bindgen-cli_${builtins.replaceStrings [ "." ] [ "_" ] version}";

        build-initramfs = pkgs.callPackage ./nix/scripts/build-initramfs.nix {
          inherit targets;
          rustToolchain = devRustToolchain;
        };
        run-vm = pkgs.callPackage ./nix/scripts/run-vm.nix { };
        run-vm-pxe = pkgs.callPackage ./nix/scripts/run-vm-pxe.nix { };
        setup-net = pkgs.callPackage ./nix/scripts/setup-net.nix { };
        teardown-net = pkgs.callPackage ./nix/scripts/teardown-net.nix { };

        imaged-tests = pkgs.callPackage ./nix/packages/imaged-tests.nix {
          inherit craneLib bootAssets ipxeAssets;
          testTools = diskBuildTools;
        };

        devShell = pkgs.callPackage ./nix/devshell.nix {
          inherit
            kernels
            ipxeAssets
            build-initramfs
            run-vm
            run-vm-pxe
            setup-net
            teardown-net
            wasm-bindgen-cli
            ;
          rustToolchain = devRustToolchain;
          imagingTools = diskBuildTools;
        };
      in
      {
        checks = {
          agent-tests = imaged-tests.agent;
          native-tests = imaged-tests.native;
        };

        packages = {
          inherit (targets.x86_64)
            partclone
            imaged-client
            initramfs
            ;
          partclone-arm64 = targets.arm64.partclone;
          imaged-client-arm64 = targets.arm64.imaged-client;
          initramfs-arm64 = targets.arm64.initramfs;
          inherit
            build-initramfs
            run-vm
            run-vm-pxe
            setup-net
            teardown-net
            ;
          boot-assets = bootAssets;
          ipxe-assets = ipxeAssets;
          imaged-server = pkgs.callPackage ./nix/packages/imaged-server.nix {
            inherit
              bootAssets
              ipxeAssets
              wasm-bindgen-cli
              craneLib
              ;
          };
        };

        devShells.default = devShell;
      }
    )
    // {
      nixosModules.default =
        {
          pkgs,
          lib,
          ...
        }:
        {
          imports = [ ./nix/modules/server.nix ];
          nix.settings = {
            substituters = [ "https://luukblankenstijn.cachix.org" ];
            trusted-public-keys = [
              "luukblankenstijn.cachix.org-1:gRz/ypm8zdDizcdAuWD6UKLVBDeObfHsNDWoAka2WSw="
            ];
          };

          services.imaged.server.package =
            lib.mkDefault
              self.packages.${pkgs.stdenv.hostPlatform.system}.imaged-server;
        };
    };
}
