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

## Delegated to Codex (pick up, in priority order)

1. **Crate extraction of `cpython_abi_hooks`** per
   `docs/design/foundation/70_molt_runtime_crate_extraction.md` — follow it
   exactly (pure move, precise pub widening, digest/no_mangle notes,
   per-crate gates). Acceptance: a one-line hook edit rebuilds in seconds,
   `cargo check -p molt-runtime` + backend green, gates added to CI.
2. **Dirty-tree ignore globs for keyed wasm pins**: `tools/dirty_tree_policy.py`
   + `tools/molt_dev.py` still name the retired bare
   `wasm/molt_runtime*.wasm.sha256`; add `wasm/molt_runtime*.wasm.*.sha256`.
3. **Memory-guard test-fake drift**: 4 failures in
   `tests/test_memory_guard_tool.py` + 2 in `tests/test_harness_memory_guard.py`
   where fakes drifted from termination-report/re-exec plumbing. Fix the
   fakes to the real contract; no weakened assertions.
4. **Proof-queue DX**: your existing diagnosis-rule lane remains yours.

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
