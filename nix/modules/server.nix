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

  credentialsDir = "/run/credentials/imaged-server.service";

  portOf = address: toInt (last (splitString ":" address));

  dashboardAddress =
    if elem cfg.webBindAddress [
      "machine"
      "off"
    ] then
      null
    else
      cfg.webBindAddress;

  netbootTcpPorts = map portOf (
    filter (address: address != null) [
      cfg.bindAddress
      cfg.proxyBindAddress
    ]
  );
  netbootUdpPorts = optional (cfg.tftpBindAddress != null) (portOf cfg.tftpBindAddress);
  dashboardPorts = optional (dashboardAddress != null) (portOf dashboardAddress);
  bindsPrivilegedPort = any (port: port < 1024) (netbootTcpPorts ++ netbootUdpPorts ++ dashboardPorts);

  configFile = (pkgs.formats.toml { }).generate "imaged.toml" (
    {
      log_level = cfg.logLevel;
      data_dir = cfg.dataDir;
      multicast_interface = cfg.multicastInterface;
      public_url = cfg.publicUrl;
      listen = {
        machine = cfg.bindAddress;
        dashboard = cfg.webBindAddress;
      }
      // optionalAttrs (cfg.tftpBindAddress != null) { tftp = cfg.tftpBindAddress; }
      // optionalAttrs (cfg.proxyBindAddress != null) { proxy = cfg.proxyBindAddress; };
      script_variables =
        cfg.scriptVariables
        // mapAttrs' (
          name: _: nameValuePair "${name}_file_contents" "${credentialsDir}/${name}"
        ) cfg.scriptVariableFiles;
    }
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
      description = "Directory to store images and the database; only the imaged-server user can read it.";
    };
    publicUrl = mkOption {
      type = types.str;
      example = "http://192.168.100.1:8080";
      description = "URL of bindAddress as booting machines reach it; iPXE scripts and agents use it.";
    };
    multicastInterface = mkOption {
      type = types.str;
      example = "eth1";
      description = "Network interface on the netboot network. Multicast deploys send on it, and its firewall gets the multicast ports and, with openFirewall, the machine, TFTP and proxy ports.";
    };
    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0:8080";
      description = "host:port the PXE boot files and agent API listen on.";
    };
    webBindAddress = mkOption {
      type = types.str;
      default = "127.0.0.1:8081";
      example = "machine";
      description = ''
        host:port for the dashboard and its /api/ui, which has no authentication.
        "machine" serves it on bindAddress instead and "off" disables it.
        openFirewall never opens it; put it on an admin-only network or behind an authenticating reverse proxy.
      '';
    };
    logLevel = mkOption {
      type = types.enum [
        "off"
        "error"
        "warn"
        "info"
        "debug"
        "trace"
      ];
      default = "info";
    };
    tftpBindAddress = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "0.0.0.0:69";
      description = "host:port for the built-in TFTP server serving iPXE, kernels and initramfs; disabled when null.";
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
    scriptVariables = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = {
        NTP_SERVER = "192.168.100.1";
      };
      description = "[script_variables] entries. They end up in the Nix store, so use scriptVariableFiles for secrets.";
    };
    scriptVariableFiles = mkOption {
      type = types.attrsOf (types.strMatching "/.+");
      default = { };
      example = {
        AGE_KEY = "/run/agenix/imaged-age-key";
      };
      description = ''
        [script_variables] entries read from files outside the Nix store. systemd passes each file
        to the service as a credential, so it may be readable by root only.
      '';
    };
    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the machine, TFTP and proxy ports on multicastInterface. The dashboard port is never opened.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = intersectLists (attrNames cfg.scriptVariables) (attrNames cfg.scriptVariableFiles) == [ ];
        message = "services.imaged.server: scriptVariables and scriptVariableFiles both set ${concatStringsSep ", " (intersectLists (attrNames cfg.scriptVariables) (attrNames cfg.scriptVariableFiles))}.";
      }
    ];

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
        StateDirectory = mkIf (hasPrefix "/var/lib/" cfg.dataDir) (removePrefix "/var/lib/" cfg.dataDir);
        StateDirectoryMode = "0700";
        User = "imaged-server";
        Group = "imaged-server";
        UMask = "0077";
        LoadCredential = mapAttrsToList (name: path: "${name}:${path}") cfg.scriptVariableFiles;
        Restart = "always";
        RestartSec = "5s";

        CapabilityBoundingSet = if bindsPrivilegedPort then [ "CAP_NET_BIND_SERVICE" ] else [ "" ];
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
            to = 50257;
          }
        ];
      }
      (mkIf cfg.openFirewall {
        allowedTCPPorts = netbootTcpPorts;
        allowedUDPPorts = netbootUdpPorts;
      })
    ];

    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0700 imaged-server imaged-server -"
      "d ${cfg.dataDir}/images 0700 imaged-server imaged-server -"
    ];
  };
}
