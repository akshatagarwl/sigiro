# The docs' static export (www/dist-binary) and docs MCP snapshot
# (www/dist-mcp) the binary embeds (src/docs.rs, src/docs_mcp.rs), built by
# scripts/build-docs-export.sh. The API reference is rendered from `openapi`,
# the spec of the binary being built, instead of the live sigiro.com one, so
# the build is offline and the reference matches the binary.
{
  lib,
  stdenvNoCC,
  callPackage,
  gzip,
  version,
  # A directory holding openapi.json.
  openapi,
}:
let
  repo = ../.;
  www = ../www;
  project = callPackage ./pnpm-project.nix { } {
    pname = "sigiro-www";
    inherit version;
    root = www;
    hash = "sha256-OkbfaAymqGOHnBYLTCeI+cGQfuAeJ68oJQd1mnSGkYE=";
  };
in
stdenvNoCC.mkDerivation {
  pname = "sigiro-docs";
  inherit version;

  src = lib.fileset.toSource {
    root = repo;
    fileset = lib.fileset.unions [
      ../scripts/build-docs-export.sh
      (lib.fileset.difference www (
        lib.fileset.unions (
          map lib.fileset.maybeMissing [
            (www + "/node_modules")
            (www + "/.blume")
            (www + "/.blume-verify")
            (www + "/.alchemy")
            (www + "/.wrangler")
            (www + "/.source")
            (www + "/dist")
            (www + "/dist-binary")
            (www + "/dist-mcp")
          ]
        )
      ))
    ];
  };

  inherit (project) pnpmDeps;
  pnpmRoot = "www";
  nativeBuildInputs = project.nativeBuildInputs ++ [ gzip ];

  postPatch = ''
    patchShebangs scripts
  '';

  env = {
    SIGIRO_OPENAPI_SPEC = "${openapi}/openapi.json";
    ASTRO_TELEMETRY_DISABLED = "1";
  };

  buildPhase = ''
    runHook preBuild
    scripts/build-docs-export.sh
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out"
    cp -r www/dist-binary "$out/dist-binary"
    cp -r www/dist-mcp "$out/dist-mcp"
    runHook postInstall
  '';

  dontFixup = true;
}
