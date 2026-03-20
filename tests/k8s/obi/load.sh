#!/usr/bin/env bash
# Generate traffic against Sock Shop frontend (port-forwarded to localhost:8079).
# Usage: ./load.sh [duration_seconds]
set -euo pipefail

DURATION=${1:-60}
BASE=http://localhost:8079
END=$((SECONDS + DURATION))

echo "Generating Sock Shop traffic for ${DURATION}s ..."

while [ $SECONDS -lt $END ]; do
  curl -s "$BASE/" > /dev/null
  curl -s "$BASE/catalogue" > /dev/null
  curl -s "$BASE/catalogue/size?size=5" > /dev/null
  curl -s "$BASE/tags" > /dev/null
  curl -s "$BASE/cart" > /dev/null
  curl -s "$BASE/cards" > /dev/null
  curl -s "$BASE/addresses" > /dev/null
  curl -s "$BASE/login" -X POST -d '{}' -H 'Content-Type: application/json' > /dev/null 2>&1 || true
  sleep 1
done

echo "Done."
