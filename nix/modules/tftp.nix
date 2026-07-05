{
  config,
  lib,
  pkgs,
  ...
}:
with lib;
let
  cfg = config.services.imaged.tftp;
in
{
  options.services.imaged.tftp = {
    enable = mkEnableOption "imaged-tftp";
    package = mkOption {
      type = types.package;
      description = "The imaged-tftp package to use.";
    };
    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "IP address the tftp server listens on (note: currently hardcoded in binary to 0.0.0.0).";
    };
  };
  config = mkIf cfg.enable {
    users.users.imaged-tftp = {
      isSystemUser = true;
      group = "imaged-tftp";
    };
    users.groups.imaged-tftp = { };

    systemd.services.imaged-tftp = {
      description = "imaged tftp server";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/imaged-tftp";
        User = "imaged-tftp";
        Group = "imaged-tftp";
        Restart = "always";
        RestartSec = "5s";
        CapabilityBoundingSet = [ "CAP_NET_BIND_SERVICE" ];
        AmbientCapabilities = [ "CAP_NET_BIND_SERVICE" ];
      };
    };
  };
}
