# Offline, reproducible installs of an aube project (dashboard/, www/) for Nix.
#
# aube has no Nix fetcher, and `aube fetch` is not a usable basis for one (it
# skips the host's own optional packages, and its store index records absolute
# paths). aube-lock.yaml is pnpm's lockfile v9 format, though, so the project is
# installed with pnpm through nixpkgs' fetchPnpmDeps / pnpmConfigHook, after a
# mechanical translation:
#
#   - aube-lock.yaml -> pnpm-lock.yaml (nix/aube-to-pnpm-lock.mjs: the patch
#     block's shape, and a repair for a dangling reference aube writes);
#   - package.json `aube.patchedDependencies` -> pnpm-workspace.yaml, where
#     pnpm 10 reads them;
#   - package.json `packageManager: aube@...` removed, so pnpm does not refuse.
#
# The lockfile stays the single source of truth: pnpm installs it with
# --frozen-lockfile and fails if the translation drifts from package.json.
{
  lib,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm_10,
  nodejs_24,
  jq,
  moreutils,
}:
{
  pname,
  version,
  # The project directory, e.g. ../dashboard.
  root,
  # fetchPnpmDeps output hash. Platform-independent: pnpm fetches every
  # platform's optional packages.
  hash,
}:
let
  # pnpm 10 reads lockfile v9, which is what aube writes.
  pnpm = pnpm_10;

  # Translate in place. Runs in the project root, in both the dependency fetch
  # and the build, so both see the same pnpm-lock.yaml.
  translate = ''
    node ${./aube-to-pnpm-lock.mjs} aube-lock.yaml package.json > pnpm-lock.yaml
    if jq -e '(.aube.patchedDependencies // {}) | length > 0' package.json > /dev/null; then
      jq -r '"patchedDependencies:", (.aube.patchedDependencies | to_entries[] | "  \(.key | tojson): \(.value | tojson)")' package.json > pnpm-workspace.yaml
    fi
    jq 'del(.packageManager)' package.json | sponge package.json
  '';

  # Only what decides the dependency set, so editing a page or a component
  # does not invalidate (and re-download) the dependencies.
  depsSrc = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions (
      [
        (root + "/package.json")
        (root + "/aube-lock.yaml")
      ]
      ++ lib.optional (builtins.pathExists (root + "/patches")) (root + "/patches")
    );
  };
in
{
  inherit pnpm;

  pnpmDeps = fetchPnpmDeps {
    inherit
      pname
      version
      pnpm
      hash
      ;
    src = depsSrc;
    fetcherVersion = 5;
    nativeBuildInputs = [ moreutils ];
    postPatch = translate;
  };

  nativeBuildInputs = [
    pnpm
    # The build scripts run on Node 24, the package.json `engines` floor.
    nodejs_24
    pnpmConfigHook
    jq
    moreutils
  ];

  # For the consuming derivation's postPatch (in its pnpmRoot).
  inherit translate;
}
