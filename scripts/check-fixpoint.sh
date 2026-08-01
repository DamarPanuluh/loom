#!/usr/bin/env bash
# Verify the committed graph is a fixpoint: init + import + full dogfood
# sequence reproduces the committed loom.graph.json's SUBSTANCE.
#
# Usage: scripts/check-fixpoint.sh [workdir]
# Runs in a copy of the repo so the live store is never touched.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${1:-$(mktemp -d)}"
mkdir -p "$WORK"
cd "$WORK"
git -C "$ROOT" archive HEAD | tar -x
ln -sf "$ROOT/target" "$WORK/target"
export PATH="$WORK/target/debug:$PATH"

# Build once (the dogfood script's own cargo gates are skipped here — this
# checks the graph fixpoint, not the code gates).
cargo build --quiet

B="$WORK/target/debug/loom"
"$B" init . --name loom >/dev/null
"$B" import loom.graph.json >/dev/null

# Re-approve the committed proof commands (same as dogfood.sh).
python3 - <<'PY'
import json, subprocess, os
env = dict(os.environ)
n = 0; offset = 0
while True:
    out = subprocess.run(["loom","validation","list","--json","--offset",str(offset),"--limit","50"],
                         capture_output=True, text=True, env=env).stdout
    items = json.loads(out).get("items", [])
    if not items: break
    for v in items:
        b = v.get("body") or {}
        if b.get("command_trusted") is False and b.get("command"):
            subprocess.run(["loom","validation","update", v.get("name",""), "--command", b["command"]],
                           capture_output=True, text=True, env=env)
            n += 1
    offset += len(items)
    if len(items) < 50: break
print(f"re-approved {n} command(s)")
PY

"$B" sync >/dev/null
"$B" validation run --all >/dev/null
for spec in "$WORK"/journeys/*.yaml; do "$B" journey run "$spec" >/dev/null; done
"$B" sync >/dev/null
"$B" export >/dev/null

# Substance compare (strip volatile run fields), mirroring dogfood.sh.
python3 - "$ROOT/loom.graph.json" "$WORK/loom.graph.json" <<'PY'
import json, sys
VOLATILE = {"stdout_hash", "stdout_excerpt", "duration_ms", "ran_at"}
def norm(path):
    g = json.load(open(path))
    for row in g.get("evidence", []):
        row.pop("recorded_at", None)
        p = row.get("payload")
        if isinstance(p, dict) and p.get("kind") == "run":
            row["payload"] = {k: v for k, v in p.items() if k not in VOLATILE}
    return json.dumps(g, sort_keys=True)
same = norm(sys.argv[1]) == norm(sys.argv[2])
print("fixpoint:", "OK — fresh checkout reproduces the committed graph" if same else "BROKEN")
sys.exit(0 if same else 1)
PY
