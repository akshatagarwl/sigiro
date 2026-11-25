# The home-manager module (`homeManagerModules.default`): runs `sigiro serve`
# as a per-user service, a launchd agent on macOS and a systemd user service
# on Linux. Settings map onto the SIGIRO_* environment variables documented in
# src/config.rs. Option names follow nix/nixos-module.nix.
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
  inherit (pkgs.stdenv.hostPlatform) isDarwin;
  port =
    default: description:
    mkOption {
      type = types.port;
      inherit default description;
    };

  environment =
    lib.mapAttrs (_: toString) cfg.settings
    // {
      SIGIRO_BIND = cfg.bind;
      SIGIRO_HTTP_PORT = toString cfg.port;
      SIGIRO_GRPC_PORT = toString cfg.otlpGrpcPort;
      SIGIRO_OTLP_HTTP_PORT = toString cfg.otlpHttpPort;
      SIGIRO_DATA_DIR = cfg.dataDir;
      # `sigiro healthcheck` and the client commands use this endpoint.
      SIGIRO_ENDPOINT = "http://127.0.0.1:${toString cfg.port}";
      # Output goes to log files or the journal, not a terminal.
      NO_COLOR = "1";
    }
    // lib.optionalAttrs (cfg.duckdbMemoryLimit != null) {
      SIGIRO_DUCKDB_MEMORY_LIMIT = cfg.duckdbMemoryLimit;
    }
    // lib.optionalAttrs (cfg.retentionDays != null) {
      SIGIRO_RETENTION_DAYS = toString cfg.retentionDays;
    };

  command = [
    (lib.getExe cfg.package)
    "serve"
  ];
in
{
  options.services.sigiro = {
    enable = lib.mkEnableOption "sigiro, a single-binary OpenTelemetry backend, as a user service";

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

    dataDir = mkOption {
      type = types.str;
      default =
        if isDarwin then
          "${config.home.homeDirectory}/Library/Application Support/sigiro"
        else
          "${config.xdg.dataHome}/sigiro";
      defaultText = lib.literalMD ''
        `~/Library/Application Support/sigiro` on macOS,
        `$XDG_DATA_HOME/sigiro` on Linux (sigiro's own default)
      '';
      description = "Where sigiro stores its catalog and Parquet files (`SIGIRO_DATA_DIR`).";
    };

    logDir = mkOption {
      type = types.str;
      default = "${config.home.homeDirectory}/Library/Logs/sigiro";
      defaultText = lib.literalExpression ''"''${config.home.homeDirectory}/Library/Logs/sigiro"'';
      description = ''
        macOS only: directory for `sigiro.log` (stdout) and `sigiro.err.log`
        (stderr). On Linux the systemd journal keeps the output.
      '';
    };

    duckdbMemoryLimit = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "2GB";
      description = "DuckDB memory limit (`SIGIRO_DUCKDB_MEMORY_LIMIT`). `null` keeps sigiro's default.";
    };

    retentionDays = mkOption {
      type = types.nullOr types.ints.positive;
      default = null;
      example = 30;
      description = "Days of telemetry to keep (`SIGIRO_RETENTION_DAYS`). `null` keeps sigiro's default of 7.";
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
        SIGIRO_MAX_DISK_BYTES = 50000000000;
      };
      description = ''
        Extra environment variables for `sigiro serve`. Do not put secrets
        here: they would land in the world-readable Nix store.
      '';
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        # `sigiro` on the PATH for `sigiro status`, `sigiro sql` and the like.
        home.packages = [ cfg.package ];

        # Neither launchd nor a user unit creates these directories.
        home.activation.sigiroDirs = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
          run mkdir -p ${lib.escapeShellArgs ([ cfg.dataDir ] ++ lib.optional isDarwin cfg.logDir)}
        '';
      }

      (lib.mkIf isDarwin {
        launchd.agents.sigiro = {
          enable = true;
          # Start through a launcher named "sigiro", so macOS's background-item
          # notification and Login Items show "sigiro", not "sh". If the Nix
          # store is not mounted yet at login, KeepAlive retries.
          waitForNixStore = false;
          config = {
            ProgramArguments = command;
            EnvironmentVariables = environment;
            RunAtLoad = true;
            KeepAlive = true;
            # Back off after a crash instead of restarting in a tight loop.
            ThrottleInterval = 30;
            ProcessType = "Background";
            StandardOutPath = "${cfg.logDir}/sigiro.log";
            StandardErrorPath = "${cfg.logDir}/sigiro.err.log";
          };
        };
      })

      (lib.mkIf (!isDarwin) {
        systemd.user.services.sigiro = {
          Unit.Description = "sigiro OpenTelemetry backend";
          Service = {
            ExecStart = lib.escapeShellArgs command;
            # Quoted, so a data directory with spaces stays one assignment.
            Environment = lib.mapAttrsToList (n: v: ''"${n}=${v}"'') environment;
            WorkingDirectory = cfg.dataDir;
            Restart = "on-failure";
            RestartSec = 5;
            LimitNOFILE = 65536;
          };
          Install.WantedBy = [ "default.target" ];
        };
      })
    ]
  );
}
