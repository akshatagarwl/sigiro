#!/usr/bin/env bash
set -euo pipefail

repo=${1:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}
source_driver="$repo/deploy/nightly_release.py"
scripts_dir="${HOME}/.hermes/scripts"
installed_driver="$scripts_dir/sigiro-nightly-release.py"
wrapper="$scripts_dir/sigiro-nightly-release.sh"
test -f "$source_driver" || { printf '%s\n' "missing driver: $source_driver" >&2; exit 1; }
mkdir -p "$scripts_dir"
expected=$(shasum -a 256 "$source_driver" | cut -d ' ' -f 1)
driver_tmp=$(mktemp "$scripts_dir/.sigiro-nightly-release.py.XXXXXX")
wrapper_tmp=$(mktemp "$scripts_dir/.sigiro-nightly-release.sh.XXXXXX")
trap 'rm -f "$driver_tmp" "$wrapper_tmp"' EXIT
cp "$source_driver" "$driver_tmp"
chmod 755 "$driver_tmp"
cat >"$wrapper_tmp" <<EOF
#!/usr/bin/env bash
set -euo pipefail
driver="$installed_driver"
expected_sha256="$expected"
actual_sha256=\$(shasum -a 256 "\$driver" | cut -d ' ' -f 1)
test "\$actual_sha256" = "\$expected_sha256" || { printf '%s\\n' 'sigiro nightly driver hash mismatch' >&2; exit 1; }
export SIGIRO_RELEASE_ALLOW_WRITES=1
export DUCKDB_STATIC_CACHE="${HOME}/Library/Caches/sigiro/duckdb-static"
exec python3 "\$driver" "\$@"
EOF
chmod 755 "$wrapper_tmp"
test "$(shasum -a 256 "$driver_tmp" | cut -d ' ' -f 1)" = "$expected"
mv "$driver_tmp" "$installed_driver"
mv "$wrapper_tmp" "$wrapper"
printf 'installed %s and %s (sha256 %s)\n' "$installed_driver" "$wrapper" "$expected"

printf '%s\n' \
  'Required mbx managed-target policy:' \
  '  [target]' \
  '  max_age = "none"' \
  '  max_size = "none"' \
  'Migrate once with DUCKDB_STATIC_CACHE set:' \
  '  export DUCKDB_STATIC_CACHE="$HOME/Library/Caches/sigiro/duckdb-static"' \
  '  mbx clean && scripts/run-with-duckdb-static.sh mbx build --release --locked --bin sigiro'
