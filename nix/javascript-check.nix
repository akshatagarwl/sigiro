# The JavaScript/TypeScript gate: oxfmt, oxlint (type-aware for auth/) and
# the dashboard's typecheck, run offline from the committed pnpm lockfiles.
# Each package's dependencies are a fixed-output fetch (nix/pnpm-project.nix);
# after changing a lockfile, set its hash below to "" and copy the `got:` hash.
{
  lib,
  stdenvNoCC,
  callPackage,
  version,
  dashboardDeps,
  wwwDeps,
}:
let
  repo = ../.;
  project =
    name: root: hash:
    callPackage ./pnpm-project.nix { } {
      pname = "sigiro-${name}";
      inherit version root hash;
    };
  rootProject = project "root" repo "sha256-FSSI64ZkK1+5NvaIZichUsIWQHV1E1r1jRumeiBvNFM=";
  auth = project "auth" ../auth "sha256-6J4j7CGkkDwIX+0hm8X+Gd91yUezsTfMQSOix16oA2A=";

  # Everything oxfmt and oxlint read, without installs or build output.
  ignored = lib.fileset.unions (
    lib.concatMap
      (
        dir:
        map (sub: lib.fileset.maybeMissing (dir + "/${sub}")) [
          "node_modules"
          "dist"
          "dist-binary"
          "dist-mcp"
          ".astro"
          ".blume"
          ".next"
          ".wrangler"
          ".alchemy"
          "out"
        ]
      )
      [
        repo
        ../auth
        ../cloud
        ../dashboard
        ../www
      ]
  );
  src = lib.fileset.toSource {
    root = repo;
    fileset = lib.fileset.difference (lib.fileset.unions [
      ../package.json
      ../pnpm-lock.yaml
      ../oxfmt.config.ts
      ../oxlint.config.ts
      ../tools
      ../scripts
      ../tests
      ../auth
      ../cloud
      ../dashboard
      ../www
    ]) ignored;
  };
in
stdenvNoCC.mkDerivation {
  pname = "sigiro-javascript-check";
  inherit version src;

  nativeBuildInputs = rootProject.nativeBuildInputs;
  # Installed by hand below: one store and node_modules per package.
  dontPnpmConfigure = true;

  configurePhase = ''
    runHook preConfigure
    install() {
      pnpmDeps=$1 pnpmRoot=$2 pnpmConfigHook
    }
    install ${rootProject.pnpmDeps} .
    install ${auth.pnpmDeps} auth
    install ${dashboardDeps} dashboard
    install ${wwwDeps} www
    # The root `prepare` script, which the offline install skips: it patches
    # oxlint with Effect's language-service rules.
    pnpm run prepare
    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild
    pnpm run fmt:check
    pnpm run lint
    pnpm -C dashboard run typecheck
    runHook postBuild
  '';

  installPhase = "touch $out";
  dontFixup = true;
}
