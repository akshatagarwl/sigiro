# Evaluates the home-manager module on this platform without pulling in
# home-manager: a stub declares the few options it sets, and the check fails
# if the generated launchd agent or systemd user service is wrong.
{
  lib,
  pkgs,
  module,
}:
let
  # home-manager adds `lib.hm`; the module only uses `hm.dag.entryAfter`.
  hmLib = lib.extend (
    _: _: {
      hm.dag.entryAfter = after: data: { inherit after data; };
    }
  );
  anything = lib.mkOption { type = lib.types.attrsOf lib.types.anything; };
  stub = {
    options = {
      home.homeDirectory = lib.mkOption { type = lib.types.str; };
      home.packages = lib.mkOption { type = lib.types.listOf lib.types.package; };
      home.activation = anything;
      xdg.dataHome = lib.mkOption { type = lib.types.str; };
      launchd.agents = anything;
      systemd.user.services = anything;
    };
    config = {
      home.homeDirectory = "/home/alice";
      xdg.dataHome = "/home/alice/.local/share";
    };
  };
  dummy = pkgs.writeShellScriptBin "sigiro" "";
  eval = hmLib.evalModules {
    specialArgs = { inherit pkgs; };
    modules = [
      stub
      module
      {
        services.sigiro = {
          enable = true;
          package = dummy;
          port = 19999;
          duckdbMemoryLimit = "2GB";
          retentionDays = 7;
        };
      }
    ];
  };
  inherit (eval) config;
  env =
    if pkgs.stdenv.hostPlatform.isDarwin then
      config.launchd.agents.sigiro.config.EnvironmentVariables
    else
      lib.listToAttrs (
        map (
          s:
          let
            kv = lib.splitString "=" (lib.removePrefix "\"" (lib.removeSuffix "\"" s));
          in
          lib.nameValuePair (lib.head kv) (lib.concatStringsSep "=" (lib.tail kv))
        ) config.systemd.user.services.sigiro.Service.Environment
      );
  expect =
    name: actual: wanted:
    lib.assertMsg (actual == wanted) "${name}: ${builtins.toJSON actual}";
  service =
    if pkgs.stdenv.hostPlatform.isDarwin then
      assert expect "systemd services on macOS" (builtins.attrNames config.systemd.user.services) [ ];
      assert expect "waitForNixStore" config.launchd.agents.sigiro.waitForNixStore false;
      assert expect "ProgramArguments" config.launchd.agents.sigiro.config.ProgramArguments [
        (lib.getExe dummy)
        "serve"
      ];
      assert expect "StandardErrorPath" config.launchd.agents.sigiro.config.StandardErrorPath
        "/home/alice/Library/Logs/sigiro/sigiro.err.log";
      config.launchd.agents.sigiro
    else
      assert expect "launchd agents on Linux" (builtins.attrNames config.launchd.agents) [ ];
      assert expect "WantedBy" config.systemd.user.services.sigiro.Install.WantedBy [ "default.target" ];
      config.systemd.user.services.sigiro;
in
assert expect "SIGIRO_HTTP_PORT" env.SIGIRO_HTTP_PORT "19999";
assert expect "SIGIRO_RETENTION_DAYS" env.SIGIRO_RETENTION_DAYS "7";
assert expect "SIGIRO_DUCKDB_MEMORY_LIMIT" env.SIGIRO_DUCKDB_MEMORY_LIMIT "2GB";
assert expect "SIGIRO_DATA_DIR" env.SIGIRO_DATA_DIR (
  if pkgs.stdenv.hostPlatform.isDarwin then
    "/home/alice/Library/Application Support/sigiro"
  else
    "/home/alice/.local/share/sigiro"
);
pkgs.writeText "sigiro-home-manager-module.json" (builtins.toJSON service)
