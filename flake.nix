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

        udpcast = pkgs.callPackage ./nix/packages/udpcast.nix { };
        partclone = pkgs.callPackage ./nix/packages/partclone.nix { };
        kernel = pkgs.callPackage ./nix/packages/kernel.nix { };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        imaged-client = pkgs.callPackage ./nix/packages/imaged-client.nix { };

        # Shared initramfs contents, consumed by both the pure `initramfs`
        # derivation and the `build-initramfs` dev script.
        initramfsStaging = import ./nix/lib/initramfs-staging.nix {
          inherit (pkgs) pkgsStatic;
          inherit udpcast partclone;
        };
        initramfs = pkgs.callPackage ./nix/packages/initramfs.nix {
          inherit imaged-client initramfsStaging;
        };

        build-initramfs = pkgs.callPackage ./nix/scripts/build-initramfs.nix {
          inherit initramfsStaging;
        };
        run-vm = pkgs.callPackage ./nix/scripts/run-vm.nix { };
        run-vm-pxe = pkgs.callPackage ./nix/scripts/run-vm-pxe.nix { };
        setup-net = pkgs.callPackage ./nix/scripts/setup-net.nix { };
        teardown-net = pkgs.callPackage ./nix/scripts/teardown-net.nix { };
      in
      {
        packages = {
          inherit
            udpcast
            kernel
            partclone
            imaged-client
            initramfs
            build-initramfs
            run-vm
            run-vm-pxe
            setup-net
            teardown-net
            ;
          imaged-server = pkgs.callPackage ./nix/packages/imaged-server.nix {
            inherit initramfs;
          };
          imaged-tftp = pkgs.callPackage ./nix/packages/imaged-tftp.nix { };
          imaged-dashboard = pkgs.callPackage ./nix/packages/imaged-dashboard.nix { };
        };

        devShells.default = pkgs.callPackage ./nix/devshell.nix {
          inherit
            rustToolchain
            kernel
            udpcast
            build-initramfs
            run-vm
            run-vm-pxe
            setup-net
            teardown-net
            ;
        };
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
          imports = [
            ./nix/modules/server.nix
            ./nix/modules/tftp.nix
          ];
          # Default the service packages to the ones this flake builds (with the
          # rust-overlay pkgs), so consumers get working defaults without having
          # to wire up rust-overlay or the initramfs chain themselves.
          services.imaged = {
            server.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.imaged-server;
            server.udpcast = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.udpcast;
            server.frontend = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.imaged-dashboard;
            tftp.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.imaged-tftp;
          };
        };
    };
}
