{
  config,
  lib,
  pkgs,
  utils,
  ...
}:

with lib;

let
  cfg = config.services.imaged.server;

  portOf = address: toInt (last (splitString ":" address));

  tcpAddresses = filter (address: address != null) [
    cfg.bindAddress
    cfg.webBindAddress
    cfg.proxyBindAddress
  ];
  tcpPorts = map portOf tcpAddresses;
  udpPorts = optional (cfg.tftpBindAddress != null) (portOf cfg.tftpBindAddress);
  bindsPrivilegedPort = any (port: port < 1024) (tcpPorts ++ udpPorts);

  configFile = (pkgs.formats.toml { }).generate "imaged.toml" (
    {
      log_level = cfg.logLevel;
      data_dir = cfg.dataDir;
      multicast_interface = cfg.multicastInterface;
      listen = {
        machine = cfg.bindAddress;
      }
      // optionalAttrs (cfg.webBindAddress != null) { dashboard = cfg.webBindAddress; }
      // optionalAttrs (cfg.tftpBindAddress != null) { tftp = cfg.tftpBindAddress; }
      // optionalAttrs (cfg.proxyBindAddress != null) { proxy = cfg.proxyBindAddress; };
    }
    // optionalAttrs (cfg.publicUrl != null) { public_url = cfg.publicUrl; }
    // optionalAttrs (cfg.scriptProxy != null) { script_proxy = cfg.scriptProxy; }
  );
in
{
  options.services.imaged.server = {
    enable = mkEnableOption "imaged-server";
    package = mkOption {
      type = types.package;
      description = "The imaged-server package to use.";
    };
    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/imaged";
      description = "Directory to store images and the database.";
    };
    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0:8080";
      description = "host:port the imaged-server PXE / agent HTTP API listens on.";
    };
    logLevel = mkOption {
      type = types.str;
      default = "info";
    };
    multicastInterface = mkOption {
      type = types.str;
      default = "lo";
      description = "Network interface used for multicast deploys.";
    };
    webBindAddress = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "host:port for the dashboard UI + server functions; defaults to bindAddress when null.";
    };
    tftpBindAddress = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "0.0.0.0:69";
      description = "host:port for the built-in TFTP server serving iPXE, kernels and initramfs; disabled when null.";
    };
    publicUrl = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "http://192.168.100.1:8080";
      description = "URL that iPXE scripts point booting machines at; the binary's default (http://localhost:8080) when null.";
    };
    proxyBindAddress = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "192.168.100.1:3128";
      description = ''
        host:port for the built-in HTTP proxy post-install scripts reach the internet through; disabled when null.
        Unless scriptProxy is set, scripts get it as http_proxy. Anyone who reaches the port can use it,
        so bind it to the netboot interface.
      '';
    };
    scriptProxy = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "http://192.168.100.1:3128";
      description = "Proxy URL handed to post-install scripts as http_proxy/https_proxy, overriding the built-in proxy; unset when null.";
    };
    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the HTTP, dashboard, proxy and TFTP ports on multicastInterface, the netboot network.";
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
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = utils.escapeSystemdExecArgs [
          (getExe cfg.package)
          "--config"
          configFile
        ];
        WorkingDirectory = cfg.dataDir;
        StateDirectory = "imaged";
        User = "imaged-server";
        Group = "imaged-server";
        Restart = "always";
        RestartSec = "5s";

        CapabilityBoundingSet = optional bindsPrivilegedPort "CAP_NET_BIND_SERVICE";
        AmbientCapabilities = optional bindsPrivilegedPort "CAP_NET_BIND_SERVICE";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.dataDir ];
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        ProtectProc = "invisible";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
          "AF_NETLINK"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
        ];
      };
    };

    networking.firewall.interfaces.${cfg.multicastInterface} = mkMerge [
      {
        allowedUDPPortRanges = [
          {
            from = 50000;
            to = 50127;
          }
        ];
      }
      (mkIf cfg.openFirewall {
        allowedTCPPorts = tcpPorts;
        allowedUDPPorts = udpPorts;
      })
    ];

    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0755 imaged-server imaged-server -"
      "d ${cfg.dataDir}/images 0755 imaged-server imaged-server -"
    ];
  };
}
