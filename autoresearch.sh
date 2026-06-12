#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
export CARGO_NET_OFFLINE=true

# Build outside the timed region. The benchmark measures loom's runtime workload,
# not Rust compilation or dependency resolution.
cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked --offline --quiet

python3 - "$ROOT" <<'PY'
import json
import os
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

root = Path(sys.argv[1]).resolve()
bin_path = root / "target" / "release" / "loom"
bench_root = root / "target" / "autoresearch-bench"
work = bench_root / "repo"

runs = int(os.environ.get("AUTORESEARCH_RUNS", "3"))
if runs < 1:
    raise SystemExit("AUTORESEARCH_RUNS must be >= 1")

base_env = os.environ.copy()
base_env.pop("LOOM_GRAPH", None)
base_env.setdefault("LC_ALL", "C")
base_env.setdefault("TZ", "UTC")

ignore = shutil.ignore_patterns(
    ".git",
    ".loom",
    "target",
    ".DS_Store",
)

workload = [
    ["--json", "status"],
    ["--json", "next", "--all"],
    ["--json", "next", "--mode", "discovery", "--take", "50"],
    ["--json", "next", "--mode", "quality"],
    ["--json", "coverage"],
    ["--json", "detect"],
    ["--json", "smells", "--limit", "50"],
    ["--json", "hotspots", "--limit", "50"],
    ["--json", "find", "performance", "--limit", "10"],
    ["--json", "doctor"],
    ["--json", "report"],
    ["--json", "export", "-"],
]


def checked(args, cwd, *, capture=True):
    result = subprocess.run(
        args,
        cwd=cwd,
        env=base_env,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )
    if result.returncode != 0:
        cmd = " ".join(str(a) for a in args)
        if capture:
            sys.stderr.write(f"Command failed: {cmd}\n")
            sys.stderr.write(result.stdout)
            sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    return result.stdout if capture else ""


def prepare_fixture():
    if bench_root.exists():
        shutil.rmtree(bench_root)
    bench_root.mkdir(parents=True)
    shutil.copytree(root, work, ignore=ignore)
    checked([str(bin_path), "init", str(work), "--name", "bench-loom"], work)
    checked([str(bin_path), "--graph", str(work), "import", str(work / "loom.graph.json")], work)


def run_once():
    prepare_fixture()
    output_bytes = 0
    started = time.perf_counter_ns()
    for args in workload:
        out = checked([str(bin_path), "--graph", str(work), *args], work)
        output_bytes += len(out.encode("utf-8"))
        if args[-1] == "doctor":
            payload = json.loads(out)
            if payload.get("status") not in {"ok", "pass", "passed"} and payload.get("errors", 0) != 0:
                raise SystemExit(f"doctor reported a non-clean graph: {out}")
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    return elapsed_ms, output_bytes

samples = []
bytes_samples = []
for _ in range(runs):
    elapsed, output_bytes = run_once()
    samples.append(elapsed)
    bytes_samples.append(output_bytes)

# Quality gate outside the timed benchmark. This catches behavior regressions in
# the existing suite without allowing compile/test time to pollute the runtime metric.
checked(
    ["cargo", "test", "--manifest-path", str(root / "Cargo.toml"), "--locked", "--offline", "--quiet"],
    root,
)

median_ms = statistics.median(samples)
print(f"METRIC workload_ms={median_ms:.3f}")
print(f"METRIC min_ms={min(samples):.3f}")
print(f"METRIC max_ms={max(samples):.3f}")
print(f"METRIC runs={runs}")
print(f"METRIC commands_per_run={len(workload)}")
print(f"METRIC output_bytes={int(statistics.median(bytes_samples))}")
print("METRIC cargo_tests_passed=1")
PY
