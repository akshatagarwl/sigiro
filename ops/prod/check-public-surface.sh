#!/usr/bin/env bash
# Assert every public surface sigiro.com serves, not just the one you changed.
#
# This exists because a cutover was declared green on /api/auth/jwks alone while
# every interactive sign-in page returned 404, and because a retired auth origin
# kept serving a second, independently keyed JWKS after it was "removed".
# Checking one endpoint proves one endpoint.
set -euo pipefail

fail=0
check() {
  local url=$1 want=$2 got
  got=$(curl -s -m 20 -o /dev/null -w '%{http_code}' "$url" || echo 000)
  if [[ $got == "$want" ]]; then
    printf '  ok   %-52s %s\n' "$url" "$got"
  else
    printf '  FAIL %-52s %s (want %s)\n' "$url" "$got" "$want"
    fail=1
  fi
}

echo "public surface"
check https://sigiro.com/health 200
check https://sigiro.com/api/auth/jwks 200
check https://sigiro.com/sign-in 200
check https://sigiro.com/sign-up 200
check https://sigiro.com/consent 200
check https://sigiro.com/device 200
check https://sigiro.com/device/capabilities 200
# /auth-ui.css and /auth-ui.js are deliberately gone: the auth pages are Astro
# pages in the website now, so they wear Blume's stylesheet and their script is
# bundled. A 200 here would mean the retired hand-rolled UI is still being
# served alongside the real one.
check https://sigiro.com/auth-ui.css 404
check https://sigiro.com/auth-ui.js 404
check https://api-origin.sigiro.com/health 200

echo "retired surfaces must NOT resolve"
# `host` exits 0 on NODATA as well as on a real answer, so count records
# instead of trusting the exit code.
if [[ -n $(dig +short auth-origin.sigiro.com @1.1.1.1) ]]; then
  printf '  FAIL %-52s still resolves\n' auth-origin.sigiro.com
  fail=1
else
  printf '  ok   %-52s no A record\n' auth-origin.sigiro.com
fi

echo "auth must reject bad credentials"
otp=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -X POST \
  https://sigiro.com/api/auth/sign-in/email-otp \
  -H 'content-type: application/json' -H 'origin: https://sigiro.com' \
  -d '{"email":"smoke@sigiro.invalid","otp":"123456"}' || echo 000)
if [[ $otp == 400 ]]; then
  printf '  ok   %-52s %s\n' 'test OTP rejected in production' "$otp"
else
  printf '  FAIL %-52s %s (want 400)\n' 'test OTP MUST be rejected' "$otp"
  fail=1
fi

xorigin=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -X POST \
  https://sigiro.com/api/auth/sign-in/email-otp \
  -H 'content-type: application/json' -H 'origin: https://evil.example' \
  -d '{"email":"smoke@sigiro.invalid","otp":"123456"}' || echo 000)
if [[ $xorigin == 403 ]]; then
  printf '  ok   %-52s %s\n' 'cross-origin rejected' "$xorigin"
else
  printf '  FAIL %-52s %s (want 403)\n' 'cross-origin MUST be rejected' "$xorigin"
  fail=1
fi

exit $fail
