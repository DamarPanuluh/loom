# Journey authoring and surface lint

Status: canonical for the Loom 0.32.0 Journey authoring/lint contract. This
document is intentionally limited to authored Journey surfaces and lint.
Broader release and resume guidance belongs to Phase 5.

Terminology follows [`terminology.md`](terminology.md). A Journey is authored
meaning; its surface manifest is the hash-bound, structured CLI projection that
makes that meaning executable.

The accepted projection should be a stable, production-owned black-box
consumer/administrative CLI over the same application, API, or service
boundary used by the public behavior. The CLI may be operator-only, but a
feature-gated proof binary, test fixture, mock-only path, or privileged shortcut
around production behavior is not an acceptable architectural substitute.

When the public CLI crosses a process or protocol boundary, keep the surface's
top-level `codefile`/`locator` as the one real public entrypoint. Declare
downstream handlers on the bound operation as optional `exercises` entries
(`id`, `codefile`, `locator`, `observed_by`). Those are not additional surface
owners; `journey compile` turns them into compiler-owned proof topology, and S3
still requires the referenced assertion to pass plus call-graph reach to a
realizing symbol. Assertion passage is machine evidence minted only by a local
`journey run` of compiler version 6 whose compiled proof is byte-identical to
the current accepted surface; imported, deserialized, or caller-authored
compiled proofs cannot earn that standing. Compiler-v5 proofs must be
recompiled and rerun.

When a compiler version bump changes the compiled proof shape, regenerated
surface manifests are not a drop-in: align each operation's argv (and any
argv tokens sourced from prior steps or inputs), its assertion and capture
pointers, its arguments, and the prior-output sources those arguments
reference. Human-gated Journeys additionally require structural edits — the
binding that names the gate step and the setup that materializes the gate's
inputs must be re-issued together, not replayed verbatim.

## Process exit is liveness, not an assertion

The runtime requires each operation's child process to exit with exactly the
operation's optional `expected_exit` integer. Omitting it (or setting `0`)
keeps the default rule: the child must exit 0. A structured-failure CLI — one
that always writes its single JSON envelope to stdout and then exits non-zero
to signal the rejection, for example a `gridctl` that exits 11 on a 401, 12
on a 404, and 2 on usage errors — proves that failure as an observed result
by declaring the exit code on the operation:

```json
{
  "id": "gridctl-auth-rejected",
  "summary": "Grid rejects an unauthorized request",
  "argv": ["gridctl", "request", "--token", "expired"],
  "expected_exit": 11,
  "output": {
    "format": "json",
    "assertions": [
      {"id": "not-ok", "pointer": "/ok", "equals": false},
      {"id": "kind", "pointer": "/error/kind", "equals": "authorization"},
      {"id": "code", "pointer": "/error/code", "equals": "auth_rejected"}
    ]
  }
}
```

`expected_exit` is process liveness only; it never relaxes the content
contract. Stdout must still be exactly one UTF-8 JSON value, and content
checks such as `/ok`, `/error.kind`, and `/error.code` remain JSON assertions
in `output.assertions` — exit codes never appear on assertions. A killed or
signaled process, a timeout, a different exit code, or a non-JSON envelope
still fails the run, and negative values are rejected at parse time. The
target CLI keeps its real exit-code contract; do not wrap it or flag it into
exiting 0 for the proof. Compiled proofs omit the field when it is `0`, so
surfaces that never set it compile byte-identically and existing proofs keep
deserializing unchanged.

## Authoring workflow

1. Author and register the `loom.journey/v1` artifact.
2. Use `loom journey surface <journey> --json` to inspect the current surface
   contract. The emitted `manifest_contract` already contains one operation and
   binding per authored step; replace only repository-specific CodeFile keys
   and locators, then author the adjacent surface manifest at
   `<journey-directory>/surfaces/<journey-id>.surface.json`.
3. Lint one manifest while iterating:

   ```sh
   loom journey lint checkout.happy --json
   ```

4. Lint every registered Journey before submitting a set of changes:

   ```sh
   loom journey lint --json
   ```

5. Accept only after blockers are gone:

   ```sh
   loom journey surface-accept checkout.happy \
     --manifest journeys/surfaces/checkout.happy.surface.json --json
   ```

The optional lint argument resolves a registered Journey by its accepted key.
Without it, Loom sorts registered Journeys by name and scans all of them. For
each Journey, lint loads the registered artifact, requires its semantic hash to
remain current, locates the adjacent surface manifest, and applies normal
surface-schema, binding, and setup-confinement validation before lint policy.
A missing or invalid manifest is a command error rather than a lint finding.
Lint is read-only.

## Acceptance policy in 0.30.0

Only `blocking` findings prevent acceptance. `advisory` findings do **not**
block `surface-accept` in 0.30.0. A lint report containing only advisories has
`status: "passed"`, and acceptance may proceed.

`surface-accept` parses and validates the supplied manifest, validates setup
paths against repository authority, then runs the same lint blocker policy
before beginning graph mutation. If a blocker exists, it fails with a message
beginning `surface lint blocked acceptance:` and leaves the graph unchanged.
The supplied manifest need not be at the conventional adjacent lint path, but
the policy applied to it is the same.

### Blockers

| Rule | What is blocked | Durable alternative |
|---|---|---|
| `graph-local-identity` | An **undeclared exact 32-hex identity** used as a whole argv token, anywhere recursively inside an assertion's `equals` JSON value, or as an exact JSON-pointer segment. Embedded text such as `id=<32-hex>` is not matched. | Prefer a stable name or a typed value captured from a prior step. An exact node or edge ID already declared in the current repository graph is permitted: import preserves declared identities. |
| `stale-temporal-expected-hash` | On the first temporal use of each `setup.before_steps` path, `expected_hash` does not equal the current content fingerprint of that registered repository file. Later transitions of the same path are not compared with the original repository content again. | Re-read the current registered file and update the first transition's 16-character content fingerprint. Keep later transitions chained through authored temporal state rather than pinning them to repository start state. |

The identity rule is deliberately exact. It does not ban all identifiers, nor
does it reject exact graph IDs that the current repository declares. It rejects
otherwise graph-local constants that cannot survive in a repository where that
identity has no declaration.

### Advisories through 0.30.x

These remain advisories until 0.31 and therefore do not change report status or
acceptance when no blocker is present:

| Rule | Brittle form | Recommended durable form |
|---|---|---|
| `exact-census-pin` | `equals` pins a number, array, or object where the pointer's final segment is `count`, `counts`, `total`, `totals`, `census`, or ends in `_count`/`_total` (for example `/entry_count`). | Assert an invariant, a bounded relationship, or the presence/shape of the behavior under test instead of the repository's exact census. |
| `positional-census-pointer` | A JSON pointer contains a numeric segment such as `/findings/0/kind`. | Select by stable identity or expose a keyed result rather than depending on collection order. |
| `not-equals-empty` | An assertion uses `not_equals: ""`. | Use an explicit `exists`, `type`, or semantic value assertion. |
| `real-clock-minute-bucket` | One operation contains both an `adjudications` batch and an adjacent structured `loom audit --json` invocation, matching the known judgment-burst fixture whose audit grouping depends on the host clock's current minute. Prose or either signal alone is not matched. | Inject or control time and assert against deterministic clock-controlled evidence. |

Treat advisories as migration guidance, not as failed acceptance. Authors should
prefer the alternatives now so manifests remain durable when policy tightens in
0.31.

Current findings use these exact messages (consume `rule` and `severity` for
automation; messages remain explanatory):

- `graph-local-identity`: `replace the undeclared 32-hex identity in argv with
  a repository-declared identity, stable name, or captured value` for argv, or
  `replace the undeclared 32-hex identity with a repository-declared identity,
  stable name, or captured value` for an assertion.
- `stale-temporal-expected-hash`: `update setup path '<path>' expected_hash to
  the current repository content fingerprint`.
- `exact-census-pin`: `assert an invariant or bounded relationship instead of
  an exact whole-graph count or total`.
- `positional-census-pointer`: `select census data by stable identity instead
  of a numeric JSON-pointer position`.
- `not-equals-empty`: `use an explicit existence, type, or semantic assertion
  instead of not_equals empty string`.
- `real-clock-minute-bucket`: `replace the real-clock
  judgment-burst/minute-bucket fixture with deterministic clock-controlled
  evidence`.

## JSON report contract

`loom journey lint [<journey>] --json` writes one report with schema
`loom.journey-lint/v1`:

```json
{
  "schema": "loom.journey-lint/v1",
  "status": "passed",
  "scanned": 1,
  "blocking": 0,
  "advisory": 1,
  "findings": [
    {
      "rule": "positional-census-pointer",
      "severity": "advisory",
      "journey_id": "checkout.happy",
      "manifest_path": "journeys/surfaces/checkout.happy.surface.json",
      "operation": "show-cart",
      "assertion": "first-item",
      "message": "select census data by stable identity instead of a numeric JSON-pointer position"
    }
  ]
}
```

- `status` is `blocked` when `blocking > 0`; otherwise it is `passed`, including
  when `advisory > 0`.
- `scanned` counts successfully linted Journey surface manifests, not findings.
- `blocking` and `advisory` are flat finding counts; their sum equals the length
  of `findings`.
- Every finding includes `rule`, `severity`, `journey_id`, `manifest_path`, and
  `message`. `operation` and `assertion` appear only when the rule has those
  locations.
- Findings are deterministic. Loom sorts the combined targeted or all-project
  report by the finding contract fields (rule, severity, Journey/path, optional
  operation/assertion, then message), rather than discovery order.

The command prints the JSON report even when blockers exist, then exits
unsuccessfully with `Journey lint found N blocking finding(s)`. Consumers should
parse the report fields rather than scrape human messages. A passed report exits
successfully, including an advisory-only report.

## Durable authoring checklist

- Keep operations as structured argv and pass dynamic identities through
  authored inputs or captures from prior steps.
- Prefer stable names and keyed JSON objects over graph-local constants and
  positional arrays.
- Assert behavior and invariants, not whole-repository population totals.
- Use explicit existence, type, or semantic comparisons.
- Make time an injectable fixture whenever minute boundaries affect truth.
- Before lint or acceptance, refresh the first `expected_hash` for every
  temporally transitioned registered file from current repository content.
- Run targeted lint during editing and all-project lint before acceptance or
  review.
