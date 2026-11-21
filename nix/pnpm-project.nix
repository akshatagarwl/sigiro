{
  lib,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm_10,
  nodejs_24,
}:
{
  pname,
  version,
  root,
  hash,
}:
let
  pnpm = pnpm_10;
  depsSrc = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions (
      [
        (root + "/package.json")
        (root + "/pnpm-lock.yaml")
      ]
      ++ lib.optional (builtins.pathExists (root + "/pnpm-workspace.yaml")) (
        root + "/pnpm-workspace.yaml"
      )
      ++ lib.optional (builtins.pathExists (root + "/patches")) (root + "/patches")
    );
  };
in
{
  pnpmDeps = fetchPnpmDeps {
    inherit
      pname
      version
      pnpm
      hash
      ;
    src = depsSrc;
    fetcherVersion = 5;
  };

  nativeBuildInputs = [
    pnpm
    nodejs_24
    pnpmConfigHook
  ];
}
