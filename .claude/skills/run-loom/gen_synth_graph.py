#!/usr/bin/env python3
"""
Generate a synthetic loom.graph.json for benchmarking.
Produces N intents and approximately E edges (RELATES_TO), plus
codefiles, IMPLEMENTS edges, and HIERARCHY edges to make the graph
realistic for smells/status/next calculations.

Usage:
    python3 gen_synth_graph.py <output_path> [N_intents] [E_edges]
Defaults: N=500, E=1000
"""
import json
import sys
import uuid
import random
import math

random.seed(42)

out_path = sys.argv[1] if len(sys.argv) > 1 else "synth.graph.json"
N = int(sys.argv[2]) if len(sys.argv) > 2 else 500
E = int(sys.argv[3]) if len(sys.argv) > 3 else 1000

TS = "2026-01-01T00:00:00.000000+00:00"

LEVELS = ["system", "component", "feature", "cross_cutting"]
DOMAINS = ["db", "cli", "sync", "output", "analysis", "unknown"]
STATUS_OPTS = ["proposed", "confirmed"]
INSPECT_OPTS = ["passing", "failing", "independent", "uninspected"]

# --- Nodes ---

# System intent (1)
system_id = str(uuid.uuid4())
intents = [{
    "id": system_id,
    "name": "synthetic system intent",
    "description": "top-level system intent for benchmarking purposes with many downstream features",
    "abstraction_level": "system",
    "aspect": "",
    "domain": "unknown",
    "lifecycle": "implemented",
    "status": "confirmed",
    "source_refs": "[]",
    "created_at": TS,
    "updated_at": TS,
}]

# Component intents (20)
comp_ids = []
for i in range(20):
    cid = str(uuid.uuid4())
    comp_ids.append(cid)
    intents.append({
        "id": cid,
        "name": f"component intent {i}",
        "description": f"component {i} handles a specific domain subsystem of the synthetic graph benchmark",
        "abstraction_level": "component",
        "aspect": "",
        "domain": random.choice(DOMAINS),
        "lifecycle": "implemented",
        "status": "confirmed",
        "source_refs": "[]",
        "created_at": TS,
        "updated_at": TS,
    })

# Feature intents (N - 1 - 20)
feat_count = N - 1 - 20
feat_ids = []
for i in range(feat_count):
    fid = str(uuid.uuid4())
    feat_ids.append(fid)
    intents.append({
        "id": fid,
        "name": f"feature {i} handler",
        "description": f"feature {i} processes requests and transforms data within the synthetic benchmark system",
        "abstraction_level": "feature",
        "aspect": "",
        "domain": random.choice(DOMAINS),
        "lifecycle": "implemented",
        "status": "confirmed" if random.random() > 0.3 else "proposed",
        "source_refs": "[]",
        "created_at": TS,
        "updated_at": TS,
    })

all_intent_ids = [system_id] + comp_ids + feat_ids

# CodeFiles (100 files)
CF_COUNT = 100
codefiles = []
cf_ids = []
for i in range(CF_COUNT):
    cfid = str(uuid.uuid4())
    cf_ids.append(cfid)
    codefiles.append({
        "id": cfid,
        "path": f"synth/src/module_{i // 10}/file_{i}.rs",
        "language": "rust",
        "last_modified": TS,
        "imports": "[]",
        "content_hash": f"{i:016x}",
    })

# --- Edges ---

# HIERARCHY: system -> each component, each component -> ~24 features
hierarchy = []
h_seen = set()

def add_hier(parent, child):
    k = (parent, child)
    if k not in h_seen:
        h_seen.add(k)
        hierarchy.append({
            "from": parent,
            "id": str(uuid.uuid4()),
            "to": child,
            "notes": "",
            "created_at": TS,
        })

for cid in comp_ids:
    add_hier(system_id, cid)

feats_per_comp = max(1, feat_count // len(comp_ids))
for idx, fid in enumerate(feat_ids):
    parent = comp_ids[idx % len(comp_ids)]
    add_hier(parent, fid)

# IMPLEMENTS: each feature grounded to 1-2 codefiles, each component to 1 codefile
implements = []
def add_impl(intent_id, cf_id, locator=""):
    implements.append({
        "id": str(uuid.uuid4()),
        "from": intent_id,
        "to": cf_id,
        "locator": locator,
        "inspection_status": "passing",
        "criterion": "synthetic benchmark",
        "confidence": 0.8,
        "evidence": "",
        "last_inspected": TS,
        "inspected_by": "llm",
        "notes": "",
        "created_at": TS,
    })

for idx, cid in enumerate(comp_ids):
    add_impl(cid, cf_ids[idx % CF_COUNT])

for idx, fid in enumerate(feat_ids):
    add_impl(fid, cf_ids[idx % CF_COUNT])
    if random.random() > 0.6:
        add_impl(fid, cf_ids[(idx + 1) % CF_COUNT])

# RELATES_TO: generate E edges among the intent list
# Use a set to avoid duplicates; cap at C(N,2)
relates_to = []
rt_seen = set()
all_ids = all_intent_ids

# Use random pairs but ensure we get approximately E edges
attempts = 0
while len(relates_to) < E and attempts < E * 10:
    attempts += 1
    a, b = random.sample(all_ids, 2)
    k = (min(a, b), max(a, b))
    if k in rt_seen:
        continue
    rt_seen.add(k)
    status = random.choice(["passing", "passing", "passing", "uninspected", "independent"])
    relates_to.append({
        "id": str(uuid.uuid4()),
        "from": a,
        "to": b,
        "inspection_status": status,
        "criterion": "synthetic benchmark edge criterion text",
        "confidence": 0.9,
        "evidence": "",
        "last_inspected": TS if status != "uninspected" else "",
        "inspected_by": "llm" if status != "uninspected" else "",
        "priority_score": 0.0,
        "notes": "",
        "created_at": TS,
    })

# QualityRule (10 rules — ISO 5055 style)
rules = []
rule_ids = []
for i in range(10):
    rid = str(uuid.uuid4())
    rule_ids.append(rid)
    rules.append({
        "id": rid,
        "name": f"quality_rule_{i}",
        "description": f"Quality rule {i} for benchmark coverage",
        "detection_logic": f"Detect pattern {i} by inspecting relevant code constructs",
        "severity": "warning" if i % 2 == 0 else "error",
        "created_at": TS,
        "updated_at": TS,
    })

# GOVERNS: apply rules to a few component intents (not all — unmeasured_intents smell)
governs = []
for i, cid in enumerate(comp_ids[:5]):
    rid = rule_ids[i % len(rule_ids)]
    governs.append({
        "id": str(uuid.uuid4()),
        "from": rid,
        "to": cid,
        "inspection_status": "passing",
        "criterion": "benchmark criterion met",
        "confidence": 0.9,
        "evidence": "synthetic evidence",
        "last_inspected": TS,
        "inspected_by": "llm",
        "notes": "",
        "created_at": TS,
    })

# Validation (a few)
validations = []
validates_edges = []
for i in range(5):
    vid = str(uuid.uuid4())
    validations.append({
        "id": vid,
        "name": f"bench validation {i}",
        "description": f"synthetic validation {i}",
        "validation_type": "test",
        "command": f"cargo test bench_{i}",
        "last_run": TS,
        "last_result": "passed",
        "created_at": TS,
        "updated_at": TS,
    })
    target_intent = feat_ids[i % len(feat_ids)]
    validates_edges.append({
        "id": str(uuid.uuid4()),
        "from": vid,
        "to": target_intent,
        "inspection_status": "passing",
        "notes": "",
        "created_at": TS,
    })

graph = {
    "custody": "owned",
    "graph_id": str(uuid.uuid4()),
    "graph_name": f"synth-{N}i-{E}e",
    "loom_export": 1,
    "schema_version": "5",
    "nodes": {
        "CodeFile": codefiles,
        "Delegation": [],
        "Hypothesis": [],
        "Ignore": [],
        "Intent": intents,
        "Note": [],
        "Persona": [],
        "QualityRule": rules,
        "Validation": validations,
        "VocabTerm": [],
    },
    "edges": {
        "GOVERNS": governs,
        "HIERARCHY": hierarchy,
        "IMPLEMENTS": implements,
        "JOURNEYS": [],
        "RELATES_TO": relates_to,
        "SERVES": [],
        "TARGETS": [],
        "VALIDATES": validates_edges,
    },
}

with open(out_path, "w") as f:
    json.dump(graph, f, indent=2, sort_keys=True)

print(f"Generated: {len(intents)} intents, {len(relates_to)} RELATES_TO, "
      f"{len(hierarchy)} HIERARCHY, {len(implements)} IMPLEMENTS, "
      f"{len(codefiles)} CodeFiles, {len(rules)} rules")
