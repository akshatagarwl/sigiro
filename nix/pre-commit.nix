# The Git pre-commit hook (`.githooks/pre-commit` runs `nix run .#pre-commit`).
# Every tool comes from the flake: rustfmt from the pinned Rust toolchain,
# nixfmt and gitleaks from nixpkgs. It checks the staged files as they are in
# the working tree and never rewrites them; fix with `cargo fmt` or `nix fmt`.
{
  lib,
  writeShellApplication,
  git,
  gitleaks,
  nixfmt,
  toolchain,
}:
writeShellApplication {
  name = "pre-commit";
  runtimeInputs = [
    git
    gitleaks
    nixfmt
    toolchain
  ];
  text = ''
    cd "$(git rev-parse --show-toplevel)"
    status=0

    staged() {
      git diff --cached --name-only --diff-filter=ACMR -z -- "$@"
    }

    mapfile -d "" rust < <(staged '*.rs')
    if [ "''${#rust[@]}" -gt 0 ]; then
      rustfmt --check --edition 2024 -- "''${rust[@]}" || {
        echo "pre-commit: Rust files need formatting; run cargo fmt" >&2
        status=1
      }
    fi

    mapfile -d "" nixFiles < <(staged '*.nix')
    if [ "''${#nixFiles[@]}" -gt 0 ]; then
      nixfmt --check -- "''${nixFiles[@]}" || {
        echo "pre-commit: Nix files need formatting; run nix fmt" >&2
        status=1
      }
    fi

    gitleaks git --pre-commit --staged --config .gitleaks.toml --redact --no-banner . || {
      echo "pre-commit: gitleaks found a secret in the staged changes" >&2
      status=1
    }

    exit "$status"
  '';
  meta.description = "sigiro's pre-commit checks: rustfmt, nixfmt and gitleaks on staged files";
  meta.platforms = lib.platforms.all;
}
