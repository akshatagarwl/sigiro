#!/usr/bin/env bash
# Build the docs export the sigiro binary embeds and serves at /docs/
# (src/docs.rs) into www/dist-binary.
#
# The same Blume project as sigiro.com/docs, built with SIGIRO_DOCS_TARGET=binary
# (see `BINARY` in www/blume.config.ts): static, everything under /docs, no
# landing page, no Cloudflare analytics. Files that only mean something to a web
# host are then removed, so the directory holds exactly what the binary embeds.
set -euo pipefail

www="$(cd "$(dirname "$0")/../www" && pwd)"
cd "$www"

# Blume always builds into dist/. Keep a sigiro.com build that is already there.
stash=""
if [ -e dist ]; then
  stash="$(mktemp -d "${TMPDIR:-/tmp}/sigiro-www-dist.XXXXXX")"
  mv dist "$stash/dist"
fi
restore() {
  if [ -n "$stash" ]; then
    rm -rf "$www/dist"
    mv "$stash/dist" "$www/dist"
    rmdir "$stash"
  fi
}
trap restore EXIT

rm -rf dist dist-binary
SIGIRO_DOCS_TARGET=binary aube run build
mv dist dist-binary

cd dist-binary
# Host and crawler files: redirect and header tables for Cloudflare, Netlify
# and Vercel (the binary serves Blume's redirect pages instead), the installer
# script and IndexNow key from public/, robots and sitemaps, discovery
# manifests, and Open Graph cards (pages point og:image at sigiro.com).
rm -rf _headers _redirects vercel.json blume-redirects.json install robots.txt \
  sitemap.xml agent-readability.json .well-known og og-3.png self-improvement
find . -maxdepth 1 -type f -name '*.txt' ! -name 'llms.txt' ! -name 'llms-full.txt' -delete

test -f index.html && test -f 404.html && test -f blume-search.json \
  || { echo "build-docs-export: export is missing index.html, 404.html or blume-search.json" >&2; exit 1; }
if grep -rlq cloudflareinsights .; then
  echo "build-docs-export: the export still loads Cloudflare analytics" >&2
  exit 1
fi
echo "build-docs-export: $(find . -type f | wc -l | tr -d ' ') files, $(du -sh . | cut -f1) in www/dist-binary"
