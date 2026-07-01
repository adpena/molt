# Molt Agent Contract

This root file is intentionally compact so Codex always ingests the real
contract instead of truncating it under the project-doc byte budget. The full
pre-compaction instruction bodies are preserved at:

- `docs/agent/AGENTS.full.md`
- `docs/agent/CLAUDE.full.md`

For non-trivial architecture, compatibility, release, merge, or handoff work,
read the relevant full guide sections after this contract. If the compact and
full guides ever conflict, this compact contract controls until the full guide
is reconciled.

## Non-Negotiables

- Build the end state from the start: one authority per invariant, one storage
  home per value, one import transaction per module-state transition, one guard
  owner per process tree, one typed fact path through frontend, IR, optimizer,
  backend, runtime, tooling, and docs.
- No hacks, no shortcuts, no workarounds, no facades, no compatibility shims,
  no local-minimum patches, no TODO-as-plan, and no partial implementations
  committed as progress. If the abstraction is wrong, move the abstraction.
- No backward compatibility inside Molt internals. When a touched path has a
  legacy lane, delete it or structurally reconcile it in the same arc.
- Performance is part of correctness. Claimed support must be deterministic,
  portable, small, fast to start, and faster than CPython on the claimed
  benchmark/profile/target, with honest evidence.
- Verification proves the structural invariant; it must not become progress
  theater. Run the smallest high-signal proof that covers the changed contract,
  then return to structural work.
- Preserve user and parallel-agent work. Start from live `git status`; never
  revert, overwrite, reset, or clean unrelated changes. If a dirty path affects
  the task, work with it and keep signal.

## Ecosystem Compatibility

- Molt is an AOT Python compiler, not a project to reimplement NumPy, SciPy,
  pandas, tinygrad, or the Python ecosystem in Molt-owned Python.
- Ecosystem support must flow through reusable primitives: package/import
  custody, source-recompiled C/C++/Cython/Rust extensions, CPython C-API/ABI
  symbols and type objects, typed strided storage, buffer protocol,
  ndarray/tensor dtype/shape/stride ownership, capsules, module state,
  extension object closure, native artifact staging, sidecar custody,
  tree-shaken reachability, and per-target packaging.
- Python is allowed as user source, thin import/API routing, diagnostics,
  generators, and test fixtures. It is forbidden as the implementation substrate
  for upstream package semantics, ndarray kernels, C-extension behavior, or hot
  numeric operations.
- Compile and link only the functions, objects, symbols, tables, native
  artifacts, runtime features, and package source proven reachable from the
  user's program. Do not widen profiles or ship whole package images to hide
  missing closure analysis.
- Missing ABI behavior must become a shared primitive or fail closed with a
  precise diagnostic. No host-CPython fallback, monkeypatch, vendored fork, or
  package-specific crutch may masquerade as support.

## WASM And Pact Authority

- WASM ABI selectors, runtime callable signatures, import metadata, and reserved
  table slots are manifest/generated authority. Do not create side registries,
  inferred fallbacks, or loader-only selector truth.
- `known_modules` is import visibility. `direct_call_modules` is Python symbol
  link authority. Native callable exports must become executable ABI dispatch,
  not fake `module__function` Python calls.
- Root reserved table slots are runtime-owned. App code must not export,
  override, or infer ownership of those slots from table-ref names.
- Pact WASM witness acceptance is the real `field_solve.py` building and running
  through Molt WASM/browser, writing `candidate_outputs.npz`, then passing
  `collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz`.
  Forward-only smoke is not acceptance.

## Structural Work Pattern

- Begin with one narrow named aperture into the real structure: one invariant,
  command family, file cluster, authority surface, or failing execution path.
  The aperture bounds discovery; it is not the implementation scope.
- Once duplicate authority is exposed, rip through the coherent authority class:
  callers, generated facts, backend/frontend/tooling consumers, docs, tests,
  and proof lanes needed to delete the old path.
- Do not burn down one match arm, one audit row, one failing test, or one file
  helper when the evidence shows a shared abstraction. Expand to the whole bug
  class inside the boundary.
- A small landing is valid only when it is a complete end-state subsystem cut
  with no adjacent same-kind duplicate lane left behind.
- If the operator says "tiny slice", "rip it open", or rejects tiny chips, treat
  it as a binding scope override: narrow the aperture, deepen the structural
  rip, and stop defending comfort-sized work.

## DX, Queue, And Proof Discipline

- Use `uv run --active --project . --python 3.12 ...` for Python commands.
  Non-active `uv run` creates throwaway environments and is not acceptable.
- Queue contract and tutorial: `docs/agent/PROOF_QUEUE.md`. Read it before
  queueing or interpreting long-running proof evidence.
- Pact Kernel A acceptance must use the named queue lane
  `tools/proof_queue.py pact-witness-acceptance`. A row that only runs
  `python -m molt build ... field_solve.py` is build evidence, not acceptance;
  current acceptance is `tools/pact_witness_acceptance.py` producing
  `candidate_outputs.npz` and passing `check_parity.py`.
- Expensive or contention-heavy work must go through `tools/proof_queue.py`:
  Cargo builds, WASM/browser proofs, benchmark lanes, conformance shards,
  stress tests, and anything likely to contend for build/runtime resources.
- Before queueing, run:
  `uv run --active --project . --python 3.12 python tools/proof_queue.py status`
- Submit queued work with a clear `--reason`, `--resource-family`,
  `--contention-key`, `--scope`, and `--note` describing what changed or what is
  being tested/explored and why. Prefer TOML DSL or `exec` over ad hoc
  background processes. Cite queue run IDs/log/evidence paths as evidence.
- Queue rows record a git snapshot, append-only notes, append-only acyclic proof
  DAG edges, memory-guard summaries, and deterministic marimo notebook
  projections under `logs/proof_queue/notebooks/`. Append observations with
  `tools/proof_queue.py note RUN_ID --kind observation --note "..."`; do not
  edit/delete note history, rewrite DAG edges, or hand-edit generated
  notebooks. Use `--depends-on RUN_ID` for scheduling dependencies and
  `tools/proof_queue.py link CHILD --parent PARENT --kind reruns --note "..."`
  for post-submit lineage.
- If a queue row stalls, inspect the queue log and memory-guard summary. Use
  `tools/proof_queue.py prune-stale` for stale rows; do not kill broad process
  families.
- Treat `write_stdin` as stdin input only, not process control. Never send
  Ctrl-C (`\u0003`), SIGINT-like bytes, ESC/control sequences, or other
  interrupt payloads through it to stop a command. On Windows Codex Desktop this
  can crash the control plane with `code=3221225786` and
  `write_stdin failed: Unified exec process failed: process interrupt is not
  supported by this process backend` (`codex_core::tools::router`). Track as
  upstream `openai/codex#30847`; adjacent stale-stdin lifecycle issue:
  `openai/codex#18494`.
- If a command is too broad, noisy, or slow, do not try to salvage it with an
  interactive interrupt. Prefer bounded command timeouts, narrower selectors,
  pytest deselection, proof-queue custody, passive polling until natural exit,
  or exact live-proved Molt-owned PID cleanup with an incident record. Plan
  future long commands so they can finish, timeout, or be owned by the queue.
- Direct commands are acceptable for cheap formatting, static checks, narrow
  source inspection, and queue/bootstrap repair.
- Keep proof scoped to the claim. Broad regrtest, conformance, benchmark, and
  browser lanes are for explicit compatibility/performance/release claims or
  direct user request.

## Crash And Process Custody

- Crash recovery constrains fanout, not ambition: one active structural arc, one
  bounded proof lane, no retry storms, no parallel proof fanout.
- If Codex crashes with the unsupported `write_stdin` interrupt error, classify
  it as a Codex control-plane/backend capability failure, not Molt evidence.
  Preserve the exact error text/screenshot when available, restart from live
  `git status`, inspect queue/guard evidence, and continue with bounded
  commands. Do not retry the same interrupt path.
- Before risky commands, leave or rely on evidence paths under
  `tmp/memory_guard/active/`, `tmp/memory_guard/incidents/`,
  `logs/proof_queue/runs/`, and `logs/agents/codex_stall/`.
- Molt cleanup may target only live-proved Molt-owned build, test, bench,
  backend-daemon, runtime-child, or guard-owned workers.
- Never kill Codex, Claude, the Codex app, renderers, app-server helpers,
  node-repl, MCP/plugin helpers, shell hosts whose ancestry is Codex/Claude,
  Git pollers, ancestors, or ambiguous host-control-plane processes.
- A repo path, process name, stale PID, parent shell, or Codex ancestry is not
  process ownership. If identity is ambiguous, preserve evidence and fix
  custody instead of killing.

## Codebase Authority

- Live code and executable tests are the source of truth. Roadmap, status,
  design, matrix, and memory docs are routing aids until verified against the
  current tree.
- Generated outputs remain generated-only. Update the source data and generator,
  then regenerate; do not hand-edit generated semantic status.
- Update docs in the same arc when supported semantics, backend contracts,
  compiler architecture, compatibility claims, validation gates, or roadmap
  priority move.
- For compiler/runtime facts, prefer generated or shared tables over local
  scans. If a check is needed by CLI setup, diagnostics, validation, closure,
  and docs, route it through one authority.
- Wrapper-name trap: a wrapper is architecture only when it is the thinnest ABI
  entrypoint, import route, or diagnostic boundary over a real authority. A
  wrapper that preserves duplicate execution authority is a bug.

## High-Signal File Map

- TIR/op facts: `runtime/molt-ir/src/tir/`,
  `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py`
- Passes and representation facts: `runtime/molt-passes/src/tir/`,
  `runtime/molt-passes/src/representation_facts.rs`
- Backend-specific lowering: `runtime/molt-backend-native/src/`,
  `runtime/molt-backend-wasm/src/`, `runtime/molt-backend-luau/src/`,
  `runtime/molt-backend-rust/src/`
- Runtime intrinsic/C-API/ABI authority:
  `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
  `runtime/molt-runtime/src/intrinsics/generated.rs`,
  `runtime/molt-cpython-abi/`, `src/molt/_intrinsics.pyi`
- Frontend/lowering: `src/molt/frontend/`, `src/molt/frontend/lowering/`
- WASM ABI and runner surface: `runtime/molt-backend-wasm/src/`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`,
  `tools/gen_wasm_abi.py`, `wasm/loader_bridge.js`, `wasm/run_wasm.js`,
  `wasm/browser_embed.js`, `wasm/browser_host.js`
- Process/proof custody: `tools/proof_queue.py`, `tools/memory_guard.py`,
  `tools/harness_memory_guard.py`, `tools/process_sentinel.py`,
  `tools/guarded_exec.py`, `src/molt/backend_daemon_custody.py`
- GPU/tinygrad primitives: `runtime/molt-gpu/src/`
- Docs roots: `docs/CANONICALS.md`, `docs/INDEX.md`, `docs/spec/README.md`,
  `docs/spec/STATUS.md`, `ROADMAP.md`

## Git And Handoff

- Do not force-push, reset hard, checkout over local work, or delete branches
  unless explicitly instructed and safe after reviewing the diff.
- For pact-collab work, origin/main is the final source of truth. Landing
  requires hand-reviewed cherry-pick/merge with no signal loss, no trampling,
  no orphaned branch-only work, and updated handoff docs.
- Before declaring completion, prove the current tree against every explicit
  requirement, artifact, test, gate, performance claim, browser/WASM behavior,
  and handoff deliverable. Treat uncertain evidence as incomplete.
