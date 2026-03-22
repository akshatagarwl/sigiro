#!/usr/bin/env bash
# Tiger Style pre-commit checks for sigiro
# Enforces safety-first coding: no silent error drops, proper error propagation,
# and structured logging.
set -euo pipefail

RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

ERRORS=0
WARNINGS=0

# Get staged .rs files (exclude deleted files)
STAGED_RS=$(git diff --cached --name-only --diff-filter=ACM -- '*.rs' | grep '^src/' || true)

if [ -z "$STAGED_RS" ]; then
    echo -e "${GREEN}No staged Rust src files — skipping Tiger Style checks${NC}"
    exit 0
fi

echo "=== Tiger Style Checks ==="
echo ""

# ---------------------------------------------------------------------------
# 1. Silent error drops: .ok() without chaining into error handling
# ---------------------------------------------------------------------------
check_pattern() {
    local pattern="$1"
    local description="$2"
    local severity="$3"  # ERROR or WARNING

    local matches
    matches=$(echo "$STAGED_RS" | xargs grep -n "$pattern" 2>/dev/null | grep -v 'TRADEOFF' | grep -v '#\[cfg(test)\]' | grep -v '#\[test\]' | grep -v '// .*'"$pattern" || true)

    if [ -n "$matches" ]; then
        if [ "$severity" = "ERROR" ]; then
            echo -e "${RED}ERROR: $description${NC}"
            ERRORS=$((ERRORS + 1))
        else
            echo -e "${YELLOW}WARNING: $description${NC}"
            WARNINGS=$((WARNINGS + 1))
        fi
        echo "$matches" | head -10
        echo ""
    fi
}

# ERROR-level checks (block commit)
check_pattern '\.unwrap_or_default()' \
    '.unwrap_or_default() masks errors with zero values — use .context(\"...\")?' \
    "ERROR"

check_pattern 'let _ = ' \
    'let _ = silently discards errors — handle with ? or match' \
    "ERROR"

check_pattern 'debug_assert!' \
    'debug_assert!() compiled out in production — use assert!() (Tiger Style: assertions in prod)' \
    "ERROR"

# WARNING-level checks (allow commit but flag)
check_pattern '\.unwrap()' \
    '.unwrap() panics on error — prefer .context(\"...\")? or .expect(\"invariant: ...\")' \
    "WARNING"

check_pattern 'eprintln!' \
    'eprintln!() — use tracing::error!() or tracing::warn!() instead' \
    "WARNING"

# ---------------------------------------------------------------------------
# 2. cargo fmt check
# ---------------------------------------------------------------------------
echo "--- cargo fmt --check ---"
if ! cargo fmt --check 2>/dev/null; then
    echo -e "${RED}ERROR: cargo fmt check failed — run 'cargo fmt' first${NC}"
    ERRORS=$((ERRORS + 1))
else
    echo -e "${GREEN}OK${NC}"
fi
echo ""

# ---------------------------------------------------------------------------
# 3. cargo clippy (standard -D warnings, Tiger Style thresholds in clippy.toml)
# ---------------------------------------------------------------------------
echo "--- cargo clippy ---"
if ! cargo clippy -- -D warnings 2>&1 | tail -5; then
    echo -e "${RED}ERROR: cargo clippy failed${NC}"
    ERRORS=$((ERRORS + 1))
else
    echo -e "${GREEN}OK${NC}"
fi
echo ""

# ---------------------------------------------------------------------------
# 4. Semgrep (if installed)
# ---------------------------------------------------------------------------
if command -v semgrep &>/dev/null && [ -f .semgrep/tiger-style.yaml ]; then
    echo "--- semgrep Tiger Style ---"
    SEMGREP_OUT=$(semgrep --config .semgrep/tiger-style.yaml --quiet --no-git-ignore $STAGED_RS 2>&1 || true)
    if [ -n "$SEMGREP_OUT" ]; then
        echo -e "${YELLOW}Semgrep findings:${NC}"
        echo "$SEMGREP_OUT" | head -30
        WARNINGS=$((WARNINGS + 1))
    else
        echo -e "${GREEN}OK${NC}"
    fi
    echo ""
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "=== Summary ==="
if [ $ERRORS -gt 0 ]; then
    echo -e "${RED}BLOCKED: $ERRORS error(s), $WARNINGS warning(s)${NC}"
    echo "Fix errors before committing. Add '// TRADEOFF: <reason>' comments to suppress false positives."
    exit 1
elif [ $WARNINGS -gt 0 ]; then
    echo -e "${YELLOW}PASSED with $WARNINGS warning(s)${NC}"
    echo "Consider fixing warnings. Use --no-verify to bypass if intentional."
    exit 0
else
    echo -e "${GREEN}All Tiger Style checks passed${NC}"
    exit 0
fi
