{
  config,
  lib,
  pkgs,
  ...
}:

with lib;

let
  cfg = config.services.imaged-server;
in
{
  options.services.imaged-server = {
    enable = mkEnableOption "imaged-server";

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ./package.nix { };
      description = "The imaged-server package to use.";
    };

    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/imaged";
      description = "Directory to store images and the database.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the firewall for the server ports.";
    };

    port = mkOption {
      type = types.port;
      default = 8080;
      description = "Port the server listens on (note: currently hardcoded in binary to 8080).";
    };
  };

  config = mkIf cfg.enable {
    systemd.services.imaged-server = {
      description = "imaged server backend";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/imaged-server";
        WorkingDirectory = cfg.dataDir;
        StateDirectory = "imaged";
        
        # "all the rights": Running as root to allow PXE, multicast, and low-level network operations.
        User = "root";
        Group = "root";
        
        Restart = "always";
        RestartSec = "5s";

        # Ensure network capabilities are available even if running as a different user in the future.
        CapabilityBoundingSet = [ "CAP_NET_ADMIN" "CAP_NET_RAW" "CAP_NET_BIND_SERVICE" ];
        AmbientCapabilities = [ "CAP_NET_ADMIN" "CAP_NET_RAW" "CAP_NET_BIND_SERVICE" ];
      };
    };

    # Automatically create the data directory and images subdirectory
    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0755 root root -"
      "d ${cfg.dataDir}/images 0755 root root -"
    ];

    networking.firewall = mkIf cfg.openFirewall {
      allowedTCPPorts = [ cfg.port ];
      # Future PXE support might need UDP ports (DHCP: 67, TFTP: 69)
      allowedUDPPorts = [ 67 69 ];
    };
  };
}
