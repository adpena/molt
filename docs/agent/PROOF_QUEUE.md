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

On this Windows workstation, expensive queue rows must refresh the canonical DX
environment before submission so APDataStore is the selected artifact and
toolchain authority:

```powershell
Invoke-Expression (uv run --active --project . --python 3.12 python tools\run_context_env.py --prefer-external-artifacts --dx --format powershell)
```

The healthy default is `MOLT_EXT_ROOT=D:\Molt`,
`CARGO_TARGET_DIR=D:\Molt\target\sessions\<MOLT_SESSION_ID>`, and
`MOLT_TARGET_ROOT=D:\Molt\target-root`, with `UV_LINK_MODE=copy` emitted for
APDataStore/exFAT unless an explicit operator value is present. Do not submit
rows with inherited `E:\molt-target`, `E:\Molt\target-root`, or the empty
legacy `D:\molt-target` default unless the operator explicitly set
`MOLT_PRESERVE_TARGET_ROOT=1` for that row. APDataStore is exFAT, so hard-link
fallbacks are cache-authority defects to diagnose, not a reason to reroute
proof lanes to legacy `E:` roots.

Do not use the queue as proof theater. Submit the narrow proof that covers the
changed contract, then return to structural work.

## Cargo Proof Lanes

Cargo proofs use the queue-native `cargo` subcommand. Do not submit raw
`cargo ...` through `exec`, the TOML DSL, shell backgrounding, or a Codex-held
interactive session. The cargo lane builds the canonical command envelope:
active uv with `--no-sync` for the internal runner,
`tools/guarded_exec.py --prefix MOLT_TEST_SUITE`, queue contention, memory
guard, timeout, logs, optional detached runner, and a Cargo contention key
inferred from `-p/--package` when one is present. A cargo row that spends its
proof budget rebuilding or syncing the Python project before Cargo starts is a
queue DX regression.

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

For `exec` and `cargo`, the `--` delimiter before the proof command is
mandatory. The queue rejects any positional token before that delimiter because
it means shell quoting likely broke a metadata value such as `--reason` or
`--note`; running anyway would silently drop scope, contention, notes, detach
mode, or timeout authority. `exec --help` and `cargo --help` are parser help
and do not require a delimiter; `--help` after `--` remains an argument to the
proof command.

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
already failed or gone stale. A blocked row is scheduling evidence, not a lost
proof log: the queue writes a small blocked log, keeps the DAG parent visible in
`evidence`, and reports the deterministic `proof-dependency-blocked` diagnostic.
`run`, `status`, `prune-stale`, `evidence`, `audit`, `diagnose`, `notebook`,
and new submissions reconcile impossible queued dependencies before reporting
or enforcing contention; do not launch a worker just to clear a dead dependency
row.

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
exact run ID, marks the row `dispatched`, and prints both the run ID and
`*.runner.log`. The runner then uses
`tools\proof_queue.py run --run-id RUN_ID`, so it cannot steal a different
queued row. `dispatched` is active queue custody: it consumes queue capacity and
prevents duplicate launch until the runner claims the row as `running` or
`prune-stale` reclaims an expired handoff. WASM resource families also preflight
the checked-in Rust toolchain contract and install/check required Rust targets
before Cargo starts.

Use the queue-size scheduler instead of launching several detached rows by hand:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py run `
  --detach `
  --queue-size 3
```

`--queue-size N` is the maximum number of concurrently `dispatched` or
`running` rows across all contention keys. The default is `1`; set
`MOLT_PROOF_QUEUE_SIZE=N` for a shell/session default. `run --detach` defaults
its launch limit to the queue size, while `--limit` remains a per-invocation cap.
The scheduler skips rows whose contention key is already active or already
selected in the same batch, so increasing queue size only admits independent
work.

The source checkout also exposes a shell-free convenience front door. Prefer
this for interactive use because it is the portable command surface:

```shell
molt queue run --detach --queue-size 3
```

`molt queue ...` forwards to `tools/proof_queue.py` using Python argv lists, not
a shell. It is the same command syntax on Windows, macOS, and Linux, and it must
not be replaced with PowerShell-specific launch wrappers, POSIX backgrounding,
or shell-quoted command reconstruction. Raw `uv run ... tools/proof_queue.py`
examples below remain source-checkout diagnostics and CI/bootstrap forms; they
are not a second queue authority.
Queue-owned pytest commands carry `MOLT_PROOF_QUEUE_*` custody plus a canonical
`MOLT_PYTEST_CURRENT_TEST_FILE` path so the pytest bootstrap can reuse the
outer queue memory guard instead of recursively rewrapping the test process on
Windows.

If a row was deliberately parked with `--queue-only`, launch that exact row
later through the same custody boundary:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py run --run-id RUN_ID --detach
```

Do not reconstruct the original command, start a shell background process, or
submit a duplicate row unless the first row is terminal and the new row records
the rerun edge.

## Latency Discipline

Treat avoidable proof latency as a bug. Before spending a heavy slot, ask
whether the command is proving the changed invariant or merely paying for a cold
cache, a broad selector, or a stale generated file.

- Prefer exact test selectors for new invariants. A substring selector that
  misses the newly added test is false evidence; cite the precise test name or
  the precise queue run that covered it.
- Never pay a cold Cargo compile for one exact test. Use the queue-native
  `cargo` lane, batch the relevant crate shard into the same compile, and use
  `--allow-warm-single-test` only after a warmup has already made the target dir
  hot.
- Prefer a warmed canonical target/cache when it is already part of the DX
  authority and safe for the lane. If overriding `CARGO_TARGET_DIR` or another
  cache knob, record the reason in `--note`.
- Queue-owned proof runs default `MOLT_MEMORY_GUARD_POLL_SEC` to `2.0` for
  local iteration and pass that value through to `memory_guard.py`; set an
  explicit queue `--env MOLT_MEMORY_GUARD_POLL_SEC=...` override only when a
  proof genuinely needs a tighter poll. The queue validates that override as a
  positive finite number at submission time; inherited shell environment is not
  proof-row authority.
- Submit long or compile-heavy proof rows with `--detach`, then keep working or
  end the arc. Do not spend a turn tailing a queued log.
- When historical warning rows make audit output noisy, use
  `tools\proof_queue.py audit --errors-only` for human triage. This hides
  warning rows only from the terminal text; JSON/output payloads and the audit
  exit status still preserve real errors.
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
lineage and comparison intent for evidence review. Because queue databases are
worktree-local, non-scheduling lineage edges may name a parent run from another
worktree; `depends_on` parents must exist in the local queue so scheduling can
fail closed.

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
Pact missing-output acceptance failures, Rust compiler errors, pytest assertion
failures, external native artifact custody refusals, reachable native support
modules without source/artifact custody, reachability-driven stdlib profile
refusals, generated WASM ABI/link-import surface gaps, dependency-blocked rows,
Molt runtime invalid-object-header aborts, quiet running pytest rows with missing
current-test custody markers, non-final memory-guard summaries on terminal
rows, and memory-guard orphan cleanup.
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
`audit` also reports `audit-weak-proof-metadata` for rows that fell back to
generic resource/contention authority, have no scopes, or carry suspicious
reasons from broken shell quoting. Treat those rows as weak evidence and rerun
with the delimiter-guarded shape before citing them.
For active pytest rows, `status` prints `pytest_current=<nodeid> phase=<phase>`
when the memory-guard summary has a live marker. If the marker file is still
missing while the queue log is quiet, `diagnose` must classify the row as
`running-pytest-current-test-missing`; treat that as pre-test or collection
opacity. If the evidence includes `last_pytest_progress=...`, pytest has
started and the defect is current-test custody opacity after progress, not
startup opacity; inspect the pytest guard plugin/env wiring once, then rerun
with a focused selector only if the row does not finish. When the evidence also
names
`child_process=windows_memory_guard_child_runner`, the visible child is the
Windows child-limit runner; inspect the descendant uv/cache/startup command
once, then rerun with a focused selector instead of interrupting through Codex
stdin.
If a terminal row still has only a `running` or `child_running` memory-guard
summary with no summary return code, it must classify as
`memory-guard-summary-incomplete`; treat that row as queue-custody incomplete
evidence and rerun or fix the guard final-summary lifecycle. The diagnostic
evidence must include the row status/return code, elapsed time when known,
configured guard timeout when present, child guard identity, recorded summary
time, last log age, and last non-empty log line so the next agent can decide
from `audit`/`diagnose` output without manual tailing first. This diagnostic
dominates product-looking log matches: an incomplete guard summary must appear
first in `diagnose` output and must suppress frontier failure promotion for
that row.
If the queue itself fails before launching a proof command, it must mark the row
terminal, write the failure log, release the contention key, and classify the
row as `queue-preexecution-failure`; that row is infrastructure evidence, not
product proof.

Use `audit` for recursive queue health review before starting another long
proof tranche:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py audit
```

`audit` walks active and recent rows, diagnostics, append-only notes, DAG edges,
guard liveness, log freshness, and notebook projections. A classified product
failure is allowed to remain evidence. Queue debt is not: missing logs, queue
pre-execution failures, policy rejections, unclassified failures, dead running
guards, duplicate active contention keys, stale active logs, missing proof
notes, and missing notebook projections are surfaced as explicit audit issues.
By default the command exits non-zero for errors and reports warnings without
failing; add `--strict` when warnings should fail the pass. Human output prints
diagnostic and issue counts first, then a `frontier:` block for the latest
non-superseded classified product failures, then queue-debt issues. Rerun or
supersede edges retire older frontier failures from that block once a child row
exists, so audit points agents at the current boundary instead of replaying
stale failures. Default audit also treats superseded terminal rows as
archaeology and omits their old queue-debt issues from exit status and human
triage; the human summary prints `archaeology: superseded_terminal=N` and the
JSON payload exposes `superseded_archaeology_runs` when rows are retired this
way. Use `--all` when you intentionally want complete historical debt. Active
pytest rows that have gone quiet before writing a current-test marker surface as
`audit-running-pytest-current-test-missing` warnings, so collection/startup
opacity does not masquerade as a healthy queue. The issue wall is capped by
default; use `--max-issues 0`, `--json`, or `--output` for the full
machine-readable handoff.

For runs with notes, the queue writes a deterministic marimo `.py` notebook under
`logs/proof_queue/notebooks/RUN_ID.py` by default. The notebook is a generated
projection of queue evidence and log tail, not the source of truth. Do not hand
edit it; regenerate it instead:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py notebook RUN_ID
```

Use `--notebooks-root` to redirect projections for local experiments. Generated
notebooks should normally stay untracked with the rest of `logs/`.

Notebook projection is observability, not launch authority. If projection fails
during submission, run completion, `note`, `link`, or `diagnose --append-note`,
the queue must preserve the row or mutation, append a nonfatal infrastructure
failure to the run log, classify the row as `queue-infra-warning`, and continue.
Only the explicit `notebook RUN_ID` command treats notebook generation as the
requested artifact and fails directly when it cannot write that projection.

## Stall Recovery

If a queue row stalls, inspect the log and memory-guard summary first:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py evidence --run-id RUN_ID
```

Use `prune-stale --run-id RUN_ID` for a stale row you own. The unscoped
`prune-stale` form is intentionally broad; reserve it for queue-wide cleanup
after checking active ownership. Do not kill broad process families, Codex,
Claude, renderer helpers, node-repl, shell ancestors, or ambiguous host
control-plane processes. Each pruned row prints the deterministic diagnosis
that justified pruning, compact diagnostic evidence, and the memory-guard
summary and queue log paths; treat that line as the handoff breadcrumb instead
of rerunning broad status loops.
If the queue guard process is still alive but the nested memory-guard child in
the running summary is dead and the log is stale, `prune-stale` must still mark
the row stale with `running-proof-child-missing`; do not keep that row active
waiting for a child that custody already proved gone.
Active queue-owned runners apply the same stale-running diagnostics while they
wait, but only `running-proof-child-missing` is a self-terminalizing runner
signal. `running-proof-launch-summary-stale` means the guard summary has not
yet advanced far enough to prove nested custody; the runner must keep waiting
for the guard process it launched. Use `prune-stale --run-id RUN_ID` after an
ownership check when a launch-summary-only row truly needs manual cleanup.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py prune-stale --run-id RUN_ID
uv run --active --project . --python 3.12 python tools\proof_queue.py prune-stale
```

When citing proof, cite the run ID plus the log or evidence path. Treat
uncertain, stale, or dirty-run evidence as partial until the current tree proves
the claim.
