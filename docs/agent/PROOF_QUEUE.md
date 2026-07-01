# Proof Queue Agent Contract

`tools/proof_queue.py` is the custody boundary for expensive, contentious, or
long-running Molt proof work. It serializes lanes by contention key, records the
exact command and git snapshot, writes guarded logs, enforces proof DAG
dependencies, and projects each noted or linked run into a deterministic marimo
notebook for collaborative inspection.

## When To Use It

Use the queue for Cargo builds, WASM/browser proofs, benchmark lanes,
conformance shards, stress tests, and any command likely to contend for shared
build/runtime resources. Direct commands are still appropriate for cheap source
inspection, changed-file formatting, static checks, narrow unit tests, and
queue/bootstrap repair. For Rust, use `tools/dev.py fmt-check` or
`tools/check_rustfmt.py --changed`; write mode compares `rustfmt --emit stdout`
before touching files and keeps generated Rust under generator custody.

Before queueing, always inspect live custody:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py status
```

Do not use the queue as proof theater. Submit the narrow proof that covers the
changed contract, then return to structural work.

## Cargo Proof Lanes

Cargo proofs use the queue-native `cargo` subcommand. Do not submit raw
`cargo ...` through `exec`, the TOML DSL, shell backgrounding, or a Codex-held
interactive session. The cargo lane builds the canonical command envelope:
active uv, `tools/guarded_exec.py --prefix MOLT_TEST_SUITE`, queue contention,
memory guard, timeout, logs, optional detached runner, and a Cargo contention
key inferred from `-p/--package` when one is present.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py cargo `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Moved buffer descriptor authority beside TypedStridedStorage; proving C API and ABI layout stay aligned." `
  --timeout 900 `
  --detach `
  -- test -p molt-runtime buffer --lib -- --nocapture
```

Use `--contention-key` only when the inferred `cargo:<package>` or
`cargo:workspace` key is not precise enough for the shared artifact cache and
compile slot being protected. Use `cargo-template` to print the current command
shape instead of reconstructing it from memory:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py cargo-template
```

## Required Submission Shape

Every queued run needs a meaningful reason, resource family, contention key,
scope, and note. The note should say what changed or what is being tested or
explored and why.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py exec `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --resource-family python `
  --contention-key python:runtime-buffer-descriptor `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Moved buffer descriptor authority beside TypedStridedStorage; proving C API and ABI layout stay aligned." `
  --timeout 900 `
  -- uv run --active --project . --python 3.12 pytest tests/path.py -q
```

Use `--depends-on RUN_ID` when a proof is not valid until earlier evidence has
passed. Dependency edges are immutable, acyclic, and queue-enforced: a child
waits while parents are queued/running and becomes `blocked` if a parent has
already failed or gone stale.

Queue commands that invoke Python must use:

```powershell
uv run --active --project . --python 3.12 ...
```

Non-active `uv run` is rejected because it creates throwaway environments and
destroys proof latency.

## Detached Long Runs

Do not hand-roll background proof launchers with PowerShell `Start-Process`,
shell-specific quoting, or Codex interactive sessions. The queue owns detached
launch:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py exec `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --resource-family python `
  --contention-key python:runtime-buffer-descriptor `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Detached queue-owned runner for the focused buffer proof." `
  --timeout 900 `
  --detach `
  -- uv run --active --project . --python 3.12 pytest tests/path.py -q
```

Named lanes support the same mode:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance --detach
```

Detached submission creates a queued row, starts a queue-owned runner for that
exact run ID, and prints both the run ID and `*.runner.log`. The runner then
uses `tools\proof_queue.py run --run-id RUN_ID`, so it cannot steal a different
queued row. WASM resource families also preflight the checked-in Rust toolchain
contract and install/check required Rust targets before Cargo starts.

## Latency Discipline

Treat avoidable proof latency as a bug. Before spending a heavy slot, ask
whether the command is proving the changed invariant or merely paying for a cold
cache, a broad selector, or a stale generated file.

- Prefer exact test selectors for new invariants. A substring selector that
  misses the newly added test is false evidence; cite the precise test name or
  the precise queue run that covered it.
- Prefer a warmed canonical target/cache when it is already part of the DX
  authority and safe for the lane. If overriding `CARGO_TARGET_DIR` or another
  cache knob, record the reason in `--note`.
- For generators, use their timing mode when available and record the number.
  A generator check that rewrites identical files or reruns formatters on every
  output is a structural DX defect, not background noise.
- If a proof lane is already active, monitor it instead of stacking another
  Cargo/WASM proof unless the new command is independent and cheap.

## TOML DSL

For multi-run submissions, use a TOML file. `note` accepts one string and
`notes` accepts a list of strings.

```toml
[[proof]]
id = "pact-field-solve-candidate"
reason = "Run Pact field_solve candidate after import transaction authority change"
resource_family = "wasm-run"
contention_key = "wasm:pact-field-solve"
scope = ["collab/pact", "wasm/run_wasm.js"]
depends_on = ["previous-run-id-or-logical-id"]
note = "Testing whether relative import canonicalization moved the failure past import_transaction."
notes = ["Expect candidate_outputs.npz or a precise next ABI primitive failure."]
edge_kind = "derives_from"
edge_note = "Narrows the previous failure to the import transaction path."
command = [
  "uv", "run", "--active", "--project", ".", "--python", "3.12",
  "python", "tmp/pact_candidate_runner.py",
]
```

Submit with:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py submit proof.toml
```

## Named Pact Witness Lanes

Use the named lane for Pact Kernel A acceptance. Do not queue ad hoc `molt
build` commands for this contract:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance
```

For the normal heavyweight lane, prefer:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance --detach
```

`pact-witness-acceptance` renders to `tools/pact_witness_acceptance.py`. That
script owns the full acceptance sequence: build `field_solve.py`, run the WASM
artifact from an isolated fixture directory, write
`tmp/pact_witness_acceptance_queue/runs/<attempt>/run/candidate_outputs.npz`,
then run `check_parity.py` against the checked Pact reference. The runner writes
`tmp/pact_witness_acceptance_queue/latest_attempt.txt` for quick navigation and
never deletes previous attempt directories, because Windows may keep linked
`.wat` or `.wasm` files open briefly after a failed run. A row whose command is
only `python -m molt build ... field_solve.py` is historical build evidence, not
Pact acceptance, and must be rerun through the named current spec after it exits.
If Node reports a static extension `Py_mod_exec` init failure, the runner emits
`run/static_extension_init_failure.json` with the matched staged manifest,
object-closure summary, source-derived capsule requirements, and source line
hints so agents do not hand-audit temp roots before reading the generated
dossier.

Before spending the heavy slot, inspect the rendered lane:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance --print-spec
```

Root selection is priority ordered, not directory-discovery ordered. The default
selector should prefer the canonical sealed witness roots
`tmp/pact_numpy_multiarray_sealed_for_witness` and
`tmp/pact_scipy_ndimage_sealed_for_witness_next`, followed by required
native sidecars and source roots. Older recovery roots may remain under `tmp/` as
fallback evidence, but they must not shadow the canonical roots. A staged root
may publish either a root `extension_manifest.json` or artifact-specific
`*.extension_manifest.json` sidecars; both forms are admitted by the queue
selector before the build path does deeper package-native validation.

## Append-Only Notes

Proof notes are append-only at the SQLite layer. Do not edit or delete notes.
If the understanding changes, append a new observation.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py note RUN_ID `
  --kind observation `
  --author codex `
  --note "R19 moved past PyInit and now traps at scipy.ndimage._nd_image isolate import."
```

Canonical note kinds are `submission`, `change`, `hypothesis`, `test`,
`observation`, `finding`, `decision`, `followup`, and `handoff`. The queue
enforces this vocabulary so status, evidence JSON, and notebook summaries stay
searchable across agents.

## Proof DAG

Proof edges are append-only at the SQLite layer and reject cycles. Use them to
make experimental lineage machine-readable instead of burying it in prose.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py link CHILD_RUN_ID `
  --parent PARENT_RUN_ID `
  --kind reruns `
  --author codex `
  --note "Replays the failed import path after the module-state fix."
```

Canonical edge kinds are `depends_on`, `derives_from`, `reruns`, `compares`,
and `supersedes`. `depends_on` is the scheduling edge; the others preserve
lineage and comparison intent for evidence review.

## Evidence And Notebooks

Each run records:

- command, cwd, status, return code, elapsed time
- resource family, contention key, scopes
- queue log and memory-guard summary paths
- git `HEAD`, dirty bit, and short status at submission
- append-only notes
- per-kind note counts
- append-only proof DAG parents/children, edge notes, and per-kind edge counts

Inspect machine-readable evidence with:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py evidence --run-id RUN_ID
```

Evidence includes deterministic `diagnostics` derived from queue metadata and
log tails. These are not guesses; they are first-party rules for recurring
proof failure classes such as queue policy rejection, static-linked
`Py_mod_exec` failure, unresolved native/WASM symbols, unsupported direct calls,
and Pact missing-output acceptance failures.
When the Pact runner emits `static_extension_init_failure.json`, the
static-link diagnostic includes that path in its `artifacts` list.

Use `diagnose` before manual log spelunking or hand-written status notes:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py diagnose RUN_ID
```

To preserve the finding for other agents, append the deterministic diagnosis as
an immutable note and regenerate the notebook projection:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py diagnose RUN_ID --append-note
```

`status` also prints the first diagnostic for recent failed rows. If a repeated
failure only shows `unclassified-failed-proof`, add a deterministic diagnosis
rule to `tools/proof_queue.py` before that pattern becomes tribal knowledge.
If the queue itself fails before launching a proof command, it must mark the row
terminal, write the failure log, release the contention key, and classify the
row as `queue-preexecution-failure`; that row is infrastructure evidence, not
product proof.

For runs with notes, the queue writes a deterministic marimo `.py` notebook under
`logs/proof_queue/notebooks/RUN_ID.py` by default. The notebook is a generated
projection of queue evidence and log tail, not the source of truth. Do not hand
edit it; regenerate it instead:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py notebook RUN_ID
```

Use `--notebooks-root` to redirect projections for local experiments. Generated
notebooks should normally stay untracked with the rest of `logs/`.

## Stall Recovery

If a queue row stalls, inspect the log and memory-guard summary first:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py evidence --run-id RUN_ID
```

Use `prune-stale` only for stale queue rows. Do not kill broad process families,
Codex, Claude, renderer helpers, node-repl, shell ancestors, or ambiguous host
control-plane processes.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py prune-stale
```

When citing proof, cite the run ID plus the log or evidence path. Treat
uncertain, stale, or dirty-run evidence as partial until the current tree proves
the claim.
