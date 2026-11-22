# The NixOS module (`nixosModules.default`): runs `sigiro serve` as a hardened
# systemd service. Settings map onto the SIGIRO_* environment variables
# documented in src/config.rs; secrets go in `environmentFile`.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.sigiro;
  inherit (lib) mkOption types;
  port =
    default: description:
    mkOption {
      type = types.port;
      inherit default description;
    };
  ports = [
    cfg.port
    cfg.otlpGrpcPort
    cfg.otlpHttpPort
  ];
  # Ports below 1024 need CAP_NET_BIND_SERVICE, which a user namespace drops.
  lowPort = lib.any (p: p < 1024) ports;
in
{
  options.services.sigiro = {
    enable = lib.mkEnableOption "sigiro, a single-binary OpenTelemetry backend";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.sigiro;
      defaultText = lib.literalExpression "sigiro.packages.\${pkgs.stdenv.hostPlatform.system}.sigiro";
      description = "The sigiro package. The flake's default is a static binary from its binary cache.";
    };

    bind = mkOption {
      type = types.str;
      default = "127.0.0.1";
      example = "0.0.0.0";
      description = ''
        Address the HTTP, OTLP HTTP and OTLP gRPC listeners bind
        (`SIGIRO_BIND`). An open-mode server does not authenticate requests:
        bind beyond loopback only behind an authenticated proxy or an
        access-controlled network.
      '';
    };

    port = port 9999 "Port for the dashboard, HTTP API and `/health` (`SIGIRO_HTTP_PORT`).";
    otlpGrpcPort = port 4317 "Port for OTLP gRPC (`SIGIRO_GRPC_PORT`).";
    otlpHttpPort = port 4318 "Port for OTLP HTTP (`SIGIRO_OTLP_HTTP_PORT`).";

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the three listener ports in the firewall.";
    };

    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/sigiro";
      description = "Where sigiro stores its catalog and Parquet files (`SIGIRO_DATA_DIR`).";
    };

    user = mkOption {
      type = types.str;
      default = "sigiro";
      description = "User the service runs as. The module creates `sigiro` when it is the default.";
    };

    group = mkOption {
      type = types.str;
      default = "sigiro";
      description = "Group the service runs as. The module creates `sigiro` when it is the default.";
    };

    settings = mkOption {
      type = types.attrsOf (
        types.oneOf [
          types.str
          types.int
          types.path
        ]
      );
      default = { };
      example = {
        SIGIRO_RETENTION_DAYS = 30;
        SIGIRO_MAX_DISK_BYTES = 50000000000;
      };
      description = ''
        Extra environment variables for `sigiro serve`, such as
        `SIGIRO_RETENTION_DAYS`. Do not put secrets here: they would land in
        the world-readable Nix store. Use `environmentFile` instead.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/sigiro.env";
      description = ''
        File of `NAME=value` lines loaded into the service environment, for
        secrets such as `SIGIRO_ADMIN_TOKEN` or `SIGIRO_OTEL_AUTH_KEY`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # `sigiro` on the PATH for `sigiro status`, `sigiro sql` and the like.
    environment.systemPackages = [ cfg.package ];

    users.users = lib.mkIf (cfg.user == "sigiro") {
      sigiro = {
        isSystemUser = true;
        inherit (cfg) group;
        home = cfg.dataDir;
      };
    };
    users.groups = lib.mkIf (cfg.group == "sigiro") { sigiro = { }; };

    systemd.tmpfiles.settings.sigiro.${cfg.dataDir}.d = {
      inherit (cfg) user group;
      mode = "0750";
    };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall ports;

    systemd.services.sigiro = {
      description = "sigiro OpenTelemetry backend";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      environment = lib.mapAttrs (_: toString) cfg.settings // {
        SIGIRO_BIND = cfg.bind;
        SIGIRO_HTTP_PORT = toString cfg.port;
        SIGIRO_GRPC_PORT = toString cfg.otlpGrpcPort;
        SIGIRO_OTLP_HTTP_PORT = toString cfg.otlpHttpPort;
        SIGIRO_DATA_DIR = cfg.dataDir;
        # `sigiro healthcheck` and the client commands use this endpoint.
        SIGIRO_ENDPOINT = "http://127.0.0.1:${toString cfg.port}";
        HOME = cfg.dataDir;
      };
      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} serve";
        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        Restart = "on-failure";
        RestartSec = 5;
        LimitNOFILE = 65536;
        UMask = "0027";

        AmbientCapabilities = lib.mkIf lowPort [ "CAP_NET_BIND_SERVICE" ];
        CapabilityBoundingSet = if lowPort then [ "CAP_NET_BIND_SERVICE" ] else [ "" ];
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.dataDir ];
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        PrivateUsers = !lowPort;
        ProtectHostname = true;
        ProtectClock = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectProc = "invisible";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
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
  };
}
