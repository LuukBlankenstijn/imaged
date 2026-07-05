{
  config,
  lib,
  ...
}:

with lib;

let
  cfg = config.services.imaged.server;
in
{
  options.services.imaged.server = {
    enable = mkEnableOption "imaged-server";
    package = mkOption {
      type = types.package;
      description = "The imaged-server package to use.";
    };
    udpcast = mkOption {
      type = types.package;
      description = "Package providing udp-sender, used for multicast deploys.";
    };
    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/imaged";
      description = "Directory to store images and the database.";
    };
    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0:8080";
      description = "IP address the tftp server listens on.";
    };
    logLevel = mkOption {
      type = types.str;
      default = "info";
    };
  };

  config = mkIf cfg.enable {
    users.users.imaged-server = {
      isSystemUser = true;
      group = "imaged-server";
      home = cfg.dataDir;
    };
    users.groups.imaged-server = { };

    systemd.services.imaged-server = {
      description = "imaged server backend";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];
      path = [ cfg.udpcast ];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/imaged-server --bind-address ${cfg.bindAddress} --log-level ${cfg.logLevel}";
        WorkingDirectory = cfg.dataDir;
        StateDirectory = "imaged";
        User = "imaged-server";
        Group = "imaged-server";
        Restart = "always";
        RestartSec = "5s";
      };
    };

    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0755 imaged-server imaged-server -"
      "d ${cfg.dataDir}/images 0755 imaged-server imaged-server -"
    ];
  };
}
