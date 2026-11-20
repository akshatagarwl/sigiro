# The dashboard's static export (dashboard/dist/dashboard), which the binary
# embeds (src/dashboard.rs). Platform-independent output.
{
  lib,
  stdenvNoCC,
  callPackage,
  version,
}:
let
  root = ../dashboard;
  project = callPackage ./aube-project.nix { } {
    pname = "sigiro-dashboard";
    inherit version root;
    hash = "sha256-5d6n+Ix+deHKQLf8u6nxV1mgBgxlMCsNZ3gsx0HMlIg=";
  };

  src = lib.fileset.toSource {
    inherit root;
    # Everything but local build output and installs.
    fileset = lib.fileset.difference root (
      lib.fileset.unions (
        map lib.fileset.maybeMissing [
          (root + "/node_modules")
          (root + "/dist")
          (root + "/out")
          (root + "/.next")
        ]
      )
    );
  };
in
stdenvNoCC.mkDerivation {
  pname = "sigiro-dashboard";
  inherit version src;

  inherit (project) pnpmDeps;
  nativeBuildInputs = project.nativeBuildInputs;
  postPatch = project.translate;

  # No NODE_ENV=production here: it would make the install skip the
  # devDependencies the build needs. `next build` sets it itself.
  env = {
    NEXT_TELEMETRY_DISABLED = "1";
    # Reproducible, and still new whenever the source or lockfile changes: a
    # hash of the content-addressed source path (next.config.ts). Hashed
    # again so the store path itself never ends up in the binary.
    SIGIRO_DASHBOARD_BUILD_ID = builtins.substring 0 20 (
      builtins.hashString "sha256" (builtins.unsafeDiscardStringContext (baseNameOf src))
    );
  };

  buildPhase = ''
    runHook preBuild
    pnpm run build
    test -f dist/dashboard/index.html
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    cp -r dist/dashboard "$out"
    runHook postInstall
  '';

  dontFixup = true;
}
