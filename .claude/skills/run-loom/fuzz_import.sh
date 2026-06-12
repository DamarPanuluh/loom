#!/usr/bin/env bash
# Hostile-import fuzz: a corrupted loom.graph.json must be rejected LOUDLY and
# leave NO partial graph behind (the two-phase import guarantee). Exits
# non-zero on the first survivor. Wired as the benchmark validation for the
# "hostile import rejected loudly" intent.
#
# Usage: fuzz_import.sh            (uses the repo's committed export as seed)
#        LOOM_BIN=/path/to/loom fuzz_import.sh
set -euo pipefail

L="${LOOM_BIN:-loom}"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SRC="$ROOT/loom.graph.json"
[ -f "$SRC" ] || { echo "FAIL: seed export $SRC missing"; exit 1; }

T="$(mktemp -d)" && cd "$T" || exit 1
trap 'cd /; rm -rf "$T"' EXIT

# A corrupted file must (a) fail the import, (b) leave zero intents — no
# partial graph, (c) leave the fresh graph fully usable.
expect_clean_fail() { # <file> <label>
  rm -rf .loom
  "$L" init . >/dev/null
  if "$L" import "$1" >/dev/null 2>&1; then
    echo "FAIL: $2 — import SUCCEEDED on corrupted input"; exit 1
  fi
  n="$("$L" intent list --json | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("total", len(d.get("intents", []))))')"
  if [ "$n" != 0 ]; then
    echo "FAIL: $2 — left $n intent(s) behind (partial import)"; exit 1
  fi
  "$L" status >/dev/null || { echo "FAIL: $2 — graph unusable afterwards"; exit 1; }
  echo "  ok: $2 rejected cleanly (0 nodes, graph usable)"
}

# 1. Truncated JSON (cut mid-structure).
head -c 2000 "$SRC" > trunc.json
expect_clean_fail trunc.json "truncated JSON"

# 2. Garbage bytes (not UTF-8, not JSON).
head -c 256 /dev/urandom > garbage.json
expect_clean_fail garbage.json "random bytes"

# 3. Wrong-typed field deep in the file: an intent's name becomes a number.
#    Valid JSON — only per-item validation can catch it; without the two-phase
#    import this used to leave every node before it already inserted.
python3 - "$SRC" <<'EOF'
import json, sys
g = json.load(open(sys.argv[1]))
g['nodes']['Intent'][-1]['name'] = 42
json.dump(g, open('wrongtype.json', 'w'))
EOF
expect_clean_fail wrongtype.json "wrong-typed field (late in file)"

# 4. Missing required field on a late edge.
python3 - "$SRC" <<'EOF'
import json, sys
g = json.load(open(sys.argv[1]))
del g['edges']['RELATES_TO'][-1]['inspection_status']
json.dump(g, open('missing.json', 'w'))
EOF
expect_clean_fail missing.json "missing required edge field"

# 5. Marker type confusion: loom_export as a string.
python3 - "$SRC" <<'EOF'
import json, sys
g = json.load(open(sys.argv[1]))
g['loom_export'] = "1"
json.dump(g, open('marker.json', 'w'))
EOF
expect_clean_fail marker.json "stringified loom_export marker"

# Control: the untouched export must still import (the gate rejects corruption,
# not imports).
rm -rf .loom && "$L" init . >/dev/null
"$L" import "$SRC" >/dev/null || { echo "FAIL: control import of the real export failed"; exit 1; }
echo "  ok: control — the genuine export imports"

echo "PASS — hostile imports rejected, no partial graphs."
