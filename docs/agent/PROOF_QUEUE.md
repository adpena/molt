# Proof Queue Agent Contract

`tools/proof_queue.py` is the custody boundary for expensive, contentious, or
long-running Molt proof work. It serializes lanes by contention key, records the
exact command and git snapshot, writes guarded logs, and projects each noted run
into a deterministic marimo notebook for collaborative inspection.

## When To Use It

Use the queue for Cargo builds, WASM/browser proofs, benchmark lanes,
conformance shards, stress tests, and any command likely to contend for shared
build/runtime resources. Direct commands are still appropriate for cheap source
inspection, formatting, static checks, narrow unit tests, and queue/bootstrap
repair.

Before queueing, always inspect live custody:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py status
```

Do not use the queue as proof theater. Submit the narrow proof that covers the
changed contract, then return to structural work.

## Required Submission Shape

Every queued run needs a meaningful reason, resource family, contention key,
scope, and note. The note should say what changed or what is being tested or
explored and why.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py exec `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --resource-family cargo `
  --contention-key cargo:molt-runtime-buffer `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Moved buffer descriptor authority beside TypedStridedStorage; proving C API and ABI layout stay aligned." `
  --timeout 900 `
  -- cargo test -p molt-runtime --lib buffer -- --nocapture
```

Queue commands that invoke Python must use:

```powershell
uv run --active --project . --python 3.12 ...
```

Non-active `uv run` is rejected because it creates throwaway environments and
destroys proof latency.

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
note = "Testing whether relative import canonicalization moved the failure past import_transaction."
notes = ["Expect candidate_outputs.npz or a precise next ABI primitive failure."]
command = [
  "uv", "run", "--active", "--project", ".", "--python", "3.12",
  "python", "tmp/pact_candidate_runner.py",
]
```

Submit with:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py submit proof.toml
```

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

## Evidence And Notebooks

Each run records:

- command, cwd, status, return code, elapsed time
- resource family, contention key, scopes
- queue log and memory-guard summary paths
- git `HEAD`, dirty bit, and short status at submission
- append-only notes
- per-kind note counts

Inspect machine-readable evidence with:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py evidence --run-id RUN_ID
```

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
