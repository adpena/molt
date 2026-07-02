# Live Orchestration Board

The orchestrator (Claude, senior engineer) owns this board, lane assignments,
review, and the decision of what lands when. Codex agents: read this board at
the START of every arc and before every commit. If your planned work touches a
lane you don't own, stop and pick from "Delegated to Codex" instead.

Last updated: 2026-07-02 by the orchestrator.

## Frozen / in-surgery — DO NOT TOUCH

- **Import/bootstrap/module-state layer** (runtime module cache + import
  transaction in `runtime/molt-runtime/src/builtins/modules.rs`,
  `cpython_abi_hooks.rs` import/sys hooks, isolate import lowering in
  `src/molt/cli/backend_ir.py`, `sys.modules` machinery): the bedrock
  redesign is being implemented from
  `docs/design/foundation/69_import_bedrock_frozen_module_layer.md` (PR1 in
  flight). After it lands this layer is FROZEN under the design's invariant
  gauntlet — no incremental patches, ever.
- **`runtime/molt-backend-wasm/src/wasm/module_abi/**`**: reserved for the
  bedrock PR3 (registry-projected tables). Poll-table deforestation is done
  (`39c85586b`); do not reopen.
- **Witness lane inputs**: `tmp/pact_*` seal roots, the acceptance build
  dirs, `wasm/*.sha256` pins — orchestrator-owned.

## Delegation model (operator directive 2026-07-02)

- The orchestrator runs at most ONE subagent (currently: the native
  call-dispatch integrator). All other implementation lanes are Codex's.
- Token-efficient, agent-first tooling is a standing deliverable: every
  repeated multi-line invocation becomes a script with a one-line compact
  verdict (rc + stage + first error) and a log path for digging deeper.
  `tools/witness_cycle.sh [entry] [build|run|cycle]` is the pattern: one
  short command, one verdict line, full logs on disk.

## Delegated to Codex (pick up, in priority order)

1. **TACTICAL: native imported-module TypeError regression.** Any compiled
   program importing a sibling module fails at runtime with
   `TypeError: module name must be str` (main @ aec11e1eb; minimal repro:
   `main.py = "import guardmod"` + empty `guardmod.py`, native dev build,
   run the binary; entry-only programs are fine). Frontend lowering is
   verified sane (const_str name into the `__import__`-shaped call) — do
   NOT touch the frontend. Raise sites: `builtins/modules.rs` ~759-840,
   1959, 2124 — the runtime import entrypoint's NAME arg slot is not a str
   at runtime, suggesting an arg-contract/ABI mismatch from the
   import-custody rework (suspects b675ab9bc, d1014e24c, 8bda411df); also
   rule out a stale runtime staticlib in the shared CARGO_TARGET_DIR on E:
   (clean rebuild disambiguates). SCOPE BOUND: minimal fix at the arg
   contract ONLY — no restructuring; the bedrock PR1 replaces this layer
   and will rebase over you. Verify: minimal repro + `pytest
   tests/test_native_import_bootstrap_regressions.py -k
   "imported_module_dunder_getattr or try_guard"` all green.
2. **Dirty-tree ignore globs for keyed wasm pins** (in progress): sidecars
   moved to keyed pins `wasm/molt_runtime*.wasm.<64-hex>.sha256`
   (`runtime_wasm_validation.py` authority; reader
   `wasm_link.py::_read_runtime_integrity_pins`; writer deletes bare
   slots). Update `tools/dirty_tree_policy.py` DEFAULT_DIRTY_TREE_IGNORE_GLOBS
   and `tools/molt_dev.py` DEFAULT_IGNORE_GLOBS; check whether molt_dev
   should IMPORT the policy table instead of duplicating it (single
   authority). Extend `tests/test_molt_dev.py` fixtures to a keyed pin
   path. Do not touch proof_queue files. Verify: `pytest
   tests/test_molt_dev.py -q`.
3. **Memory-guard test-fake drift** — 5 failures in
   `tests/test_memory_guard_tool.py` on pristine HEAD guard files. Class 1
   (sampler-failure + interrupt-snapshot tests): fakes monkeypatch
   `terminate_watched_processes` returning None; the marker payload
   (`memory_guard_core/payloads.py::termination_report_payload`) hits
   AttributeError on None. Production never returns None — make the fakes
   return real GuardTerminationReport objects (preferred) rather than
   adding None-tolerance to the payload authority. Class 2 (two reexec
   tests): main() reexec now passes stdout=/stderr= kwargs the
   `fake_subprocess_run` doesn't accept — update fakes to the real call
   signature and assert on the new kwargs. No weakened assertions.
   Verify: `pytest tests/test_memory_guard_tool.py -q`.
4. **Crate extraction of `cpython_abi_hooks`** per
   `docs/design/foundation/70_molt_runtime_crate_extraction.md` — follow it
   exactly (pure move, precise pub widening, digest/no_mangle notes,
   per-crate gates). Acceptance: a one-line hook edit rebuilds in seconds,
   `cargo check -p molt-runtime` + backend green, gates added to CI.
   NOTE: coordinate with the bedrock PR1 landing — if modules.rs churn is
   active, do this lane last.
5. **Proof-queue DX**: your existing diagnosis-rule lane remains yours.

## Additional working rule (incident 2026-07-02)

- Never revert or checkout files outside your lane, even transiently — an
  in-flight fix in `module_abi/imports.rs` was wiped by out-of-lane tooling
  and had to be re-applied. If a file you didn't edit shows up dirty, leave
  it alone; it is another lane's live WIP.

## Proof and cargo DX rules (binding — incident: 835s cold compile for one test)

- NEVER pay a cold crate compile for a single exact test. If your proof
  needs a compile, run the whole relevant test SHARD in that same compile
  (one compile, many tests). An exact `--lib <one_test>` proof is only
  acceptable against a warm target dir.
- Warm before you prove: session target dirs (target/sessions/<id>) start
  cold. Prefer the shared proof-family target dir the queue assigns per
  contention key; if you must use a fresh session dir, run a `cargo check
  -p <crate>` warmup FIRST while you do other work, then submit the proof.
- Set an explicit `--timeout` matched to warm-compile reality (a warm lib
  test proof is <120s; if your row is projected to exceed it because of a
  cold compile, cancel your plan and re-shape, don't wait it out).
- NEVER sit idle narrating a wait. Submit proofs with `--detach`, do other
  lane work (or end your arc), and read the row result when it closes. A
  turn that only tails a log is a wasted turn.
- Batch proof rows: if you have N tests to prove across one crate, that is
  ONE row, not N rows contending for the same contention key.
- Env for local iteration builds: `MOLT_MEMORY_GUARD_POLL_SEC=2.0` (the
  0.1s default guard sampling is for CI; locally it wastes a third of your
  wall time even after the caching fix).
- When a row's time is dominated by compile (log shows cargo compiling >60%
  of elapsed), file ONE queue note naming the crate and move on — do not
  re-diagnose build latency per row.

## Conduct standards (binding — you are brilliant; act like it)

- **Lane ownership is exclusive.** The native call-dispatch/trampoline
  defect currently has ONE integrator (orchestrator's agent, harvesting the
  E:/Molt/worktrees/native-import-typeerror-20260702 fix and the
  fc/modules.rs operand fix). If you were on it: your evidence is captured;
  STOP editing that lane and pick your next board item. Before opening any
  file, check this board for the lane owner; two engineers fixing one
  defect from two angles produces conflicts, not speed.
- **Evidence beats vigil.** A poll loop is not work. The budget is: at most
  ONE status read per 5 minutes on a row you own, ZERO on rows you don't.
  If you catch yourself writing "still running" twice in a row, you are
  idling — switch to a second deliverable or end the arc.
- **Diagnosis is time-boxed.** 15 minutes per fault to form a hypothesis
  with a bounded experiment; if the experiment needs a build, submit it
  detached and work on something else. Never re-run a failed shape
  unchanged ("doomed exact timeout" reruns).
- **No unbounded filesystem scans, ever.** You have the pytest log, the
  queue log, and the artifact manifest — derive exact paths. A recursive
  Get-ChildItem over E:\Molt\tmp is a firable offense in this codebase; it
  starves the builds everyone else is waiting on.
- **Process spelunking is capped at one snapshot.** One targeted
  Win32_Process query per incident to confirm liveness, then the queue owns
  it. Walking a guard chain five levels deep four separate times is
  self-harm.
- **Write down what you learned the moment you learn it** (queue note or
  worktree commit message). You are forgetful across compactions; the
  notes are your memory. A finding that lives only in your context is a
  finding the team loses.
- **Fix the tool when the tool wastes you twice.** The second time a queue
  row lies to you (stale status, zombie child-runner, prune gap), the
  defect IS the work: file it precisely on the board's queue-DX list with
  the row ID, don't route around it forever.
- **Side worktrees for runtime/backend edits** (as engineer A correctly
  did): the shared checkout's cargo state is everyone's build cache; your
  compile churn belongs in your own worktree until the arc is done.

## Working agreement (binding)

- Keep the shared tree compile-green: `cargo check` the crates you touched
  before any pause longer than a few minutes. Half-written states in the
  shared checkout break every other lane's builds.
- Regenerate generated files in the same edit as their consumers
  (`tools/gen_wasm_abi.py` etc.); never leave a consumer referencing a
  symbol its generated file lacks.
- Commit with pathspecs only (`git commit -- <files>`); never `git add -A`;
  never sweep another lane's dirty files.
- Land small and complete: one coherent arc per commit, replaced code
  deleted in the same commit, tests with teeth (proven to fail on
  violation).
- Run the gates you touched before landing; cite queue run IDs as evidence.
- Compatibility floor: CPython >= 3.12 parity with explicit VERSION GATING
  (semantic deltas across 3.12/3.13+ are gated variants keyed on the
  TargetPythonVersion authority, never silent single-version assumptions),
  and Windows/macOS/Linux support with explicit PLATFORM GATING for any
  divergent behavior — all within the verified subset, with honest-early
  fail-closed diagnostics outside it.
