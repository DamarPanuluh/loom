#!/usr/bin/env bash
# bench.sh — benchmark loom hot commands on a synthetic graph of >=500 intents / >=1000 edges.
# Exits non-zero if any timed command exceeds LIMIT_SECS (default 2).
# Usage: bench.sh [N_intents] [E_edges] [LIMIT_SECS]
#
# SAFETY: always works in a throwaway temp dir. Never falls back to the repo dir.

set -euo pipefail

N=${1:-500}
E=${2:-1000}
LIMIT=${3:-2}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# Prefer an explicit binary, then the repo release build, then PATH.
if [ -n "${LOOM_BIN:-}" ]; then
    LOOM="$LOOM_BIN"
elif [ -x "$REPO_ROOT/target/release/loom" ]; then
    LOOM="$REPO_ROOT/target/release/loom"
elif [ -x "$REPO_ROOT/target/debug/loom" ]; then
    LOOM="$REPO_ROOT/target/debug/loom"
else
    LOOM="loom"
fi

# Strict throwaway dir — fail if mktemp fails, never cd back to cwd.
T=$(mktemp -d) || { echo "ERROR: mktemp -d failed" >&2; exit 1; }
trap 'rm -rf "$T"' EXIT
cd "$T" || { echo "ERROR: cannot cd to $T" >&2; exit 1; }

echo "=== loom scale benchmark: ${N} intents / ${E} edges (limit: ${LIMIT}s) ==="
echo "Temp dir: $T"

# Generate synthetic graph.
echo ""
echo "[1/6] Generating synthetic graph..."
python3 "$SCRIPT_DIR/gen_synth_graph.py" "$T/synth.graph.json" "$N" "$E"

# Import into a fresh loom graph.
echo ""
echo "[2/6] Importing into loom..."
"$LOOM" init . --name "bench-${N}i-${E}e" 2>&1
"$LOOM" import "$T/synth.graph.json" 2>&1
echo "Import done."

echo ""
echo "[3/6] Running benchmarks (wall-clock time)..."
echo ""

FAILED=0

bench_cmd() {
    local label="$1"
    shift
    local start end elapsed_s
    # Use python3 for portable sub-second timing (macOS date lacks %3N).
    start=$(python3 -c "import time; print(time.time())")
    "$@" > /dev/null 2>&1
    end=$(python3 -c "import time; print(time.time())")
    elapsed_s=$(python3 -c "print('%.3f' % ($end - $start))")
    local status="OK"
    if python3 -c "import sys; sys.exit(0 if ($elapsed_s < $LIMIT) else 1)"; then
        status="OK"
    else
        status="FAIL (>${LIMIT}s)"
        FAILED=1
    fi
    printf "  %-35s %ss  [%s]\n" "$label" "$elapsed_s" "$status"
}

bench_cmd "loom status"          "$LOOM" status
bench_cmd "loom next"            "$LOOM" next
bench_cmd "loom smells"          "$LOOM" smells
bench_cmd "loom next --all"      "$LOOM" next --all

echo ""
if [[ $FAILED -eq 0 ]]; then
    echo "PASS — all commands completed within ${LIMIT}s"
    exit 0
else
    echo "FAIL — one or more commands exceeded ${LIMIT}s"
    exit 1
fi
