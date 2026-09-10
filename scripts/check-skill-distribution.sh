#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
skill_name=debug-with-sigiro
canonical_skill="${CANONICAL_SKILL:-$repo_root/skills/skills/$skill_name/SKILL.md}"
skill_source="${SKILL_SOURCE:-sigiroai/skills}"
mode="${1:-remote}"

case "$mode" in
  local) skill_source="${SKILL_SOURCE:-$repo_root/skills}" ;;
  remote) ;;
  *) echo "usage: $0 [local|remote]" >&2; exit 2 ;;
esac

if [ ! -f "$canonical_skill" ]; then
  echo "canonical skill missing: $canonical_skill (is the skills submodule checked out?)" >&2
  exit 1
fi
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/sigiro-skill-distribution.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT

mkdir "$work_dir/install-bin"
(
  export SIGIRO_INSTALL_DIR="$work_dir/install-bin"
  curl -fsSL https://sigiro.com/install | sh
)
"$work_dir/install-bin/sigiro" --version

if [ "$mode" = remote ]; then
  public_skill_url="${PUBLIC_SKILL_URL:-https://raw.githubusercontent.com/sigiroai/skills/main/skills/$skill_name/SKILL.md}"
  curl -fsSL --retry 3 "$public_skill_url" -o "$work_dir/public-skill.md"
  cmp "$canonical_skill" "$work_dir/public-skill.md"
fi

mkdir "$work_dir/install"
(
  cd "$work_dir/install"
  DISABLE_TELEMETRY=1 npm_config_cache="$work_dir/npm-cache" \
    npx --yes skills add "$skill_source" --skill "$skill_name" --agent codex --copy --yes
)
cmp "$canonical_skill" "$work_dir/install/.agents/skills/$skill_name/SKILL.md"

(
  cd "$repo_root/www"
  bun install --frozen-lockfile
  bun run build
)
cmp "$canonical_skill" "$repo_root/www/dist/client/.well-known/agent-skills/$skill_name/SKILL.md"

if [ "$mode" = remote ]; then
  live_skill_url="${LIVE_SKILL_URL:-https://sigiro.com/.well-known/agent-skills/$skill_name/SKILL.md}"
  curl -fsSL --retry 3 "$live_skill_url" -o "$work_dir/live-skill.md"
  cmp "$canonical_skill" "$work_dir/live-skill.md"
fi

echo "skill distribution ($mode): ok"
