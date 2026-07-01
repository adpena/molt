# Molt Agent Contract

This root file is intentionally compact so Claude and other agents ingest the
real contract without drowning in duplicated policy text. The full
pre-compaction instruction body is preserved at `docs/agent/CLAUDE.full.md`,
and the Codex full body is preserved at `docs/agent/AGENTS.full.md`.

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
- Preserve user and parallel-agent work. Start from live `git status`; never
  revert, overwrite, reset, or clean unrelated changes.

## Ecosystem Compatibility

- Molt is an AOT Python compiler, not a project to reimplement NumPy, SciPy,
  pandas, tinygrad, or the Python ecosystem in Molt-owned Python.
- Ecosystem support flows through reusable primitives: package/import custody,
  source-recompiled extensions, CPython C-API/ABI symbols and type objects,
  typed strided storage, buffer protocol, ndarray/tensor dtype/shape/stride
  ownership, capsules, module state, extension object closure, native artifact
  staging, sidecar custody, tree-shaken reachability, and per-target packaging.
- Python is allowed as user source, thin import/API routing, diagnostics,
  generators, and test fixtures. It is forbidden as the implementation substrate
  for upstream package semantics, ndarray kernels, C-extension behavior, or hot
  numeric operations.
- Compile and link only the functions, objects, symbols, tables, native
  artifacts, runtime features, and package source proven reachable from the
  user's program. Missing ABI behavior must become a shared primitive or fail
  closed with a precise diagnostic.

## Structural Work Pattern

- Begin with one narrow named aperture into the real structure; the aperture
  bounds discovery but does not shrink the implementation scope.
- Once duplicate authority is exposed, rip through the coherent class: callers,
  generated facts, backend/frontend/tooling consumers, docs, tests, and proof
  lanes needed to delete the old path.
- Do not burn down one match arm, audit row, failing test, or file helper when
  the evidence shows a shared abstraction. Expand to the whole bug class inside
  the boundary.
- A small landing is valid only when it is a complete end-state subsystem cut
  with no adjacent same-kind duplicate lane left behind.

## DX, Queue, And Proof Discipline

- Use `uv run --active --project . --python 3.12 ...` for Python commands.
- Expensive or contention-heavy work must go through `tools/proof_queue.py`:
  Cargo builds, WASM/browser proofs, benchmark lanes, conformance shards,
  stress tests, and any test likely to contend for shared resources.
- Before queueing, run:
  `uv run --active --project . --python 3.12 python tools/proof_queue.py status`
- Submit queued work with a clear `--reason`, `--resource-family`,
  `--contention-key`, and `--scope`; cite queue run IDs/log paths as evidence.
- If a queue row stalls, inspect the queue log and memory-guard summary. Use
  `tools/proof_queue.py prune-stale` for stale rows; do not kill broad process
  families.
- Treat interactive agent stdin writes as stdin input only, not process control.
  Never send Ctrl-C (`\u0003`), SIGINT-like bytes, ESC/control sequences, or
  other interrupt payloads to stop a command. On Windows Codex Desktop this can
  crash the control plane with `code=3221225786` and
  `write_stdin failed: Unified exec process failed: process interrupt is not
  supported by this process backend` (`codex_core::tools::router`). Track as
  upstream `openai/codex#30847`; adjacent stale-stdin lifecycle issue:
  `openai/codex#18494`.
- If a command is too broad, noisy, or slow, do not try to salvage it with an
  interactive interrupt. Prefer bounded command timeouts, narrower selectors,
  pytest deselection, proof-queue custody, passive polling until natural exit,
  or exact live-proved Molt-owned PID cleanup with an incident record. Plan
  future long commands so they can finish, timeout, or be owned by the queue.
- Verification proves the changed invariant. Broad suites and benchmarks are
  for explicit compatibility, performance, release, or handoff claims.

## Crash And Process Custody

- Crash recovery constrains fanout, not ambition: one active structural arc, one
  bounded proof lane, no retry storms, no parallel proof fanout.
- If Codex crashes with the unsupported stdin-interrupt error, classify it as a
  Codex control-plane/backend capability failure, not Molt evidence. Preserve
  the exact error text/screenshot when available, restart from live `git
  status`, inspect queue/guard evidence, and continue with bounded commands. Do
  not retry the same interrupt path.
- Molt cleanup may target only live-proved Molt-owned build, test, bench,
  backend-daemon, runtime-child, or guard-owned workers.
- Never kill Codex, Claude, the Codex app, renderers, app-server helpers,
  node-repl, MCP/plugin helpers, shell hosts whose ancestry is Codex/Claude,
  Git pollers, ancestors, or ambiguous host-control-plane processes.

## Codebase Authority

- Live code and executable tests are the source of truth. Generated outputs are
  generated-only; update source data and generators instead of hand-editing.
- Update docs in the same arc when supported semantics, backend contracts,
  compiler architecture, compatibility claims, validation gates, or roadmap
  priority move.
- Prefer generated or shared tables over local scans. If a check is needed by
  CLI setup, diagnostics, validation, closure, and docs, route it through one
  authority.
- A wrapper is architecture only when it is the thinnest ABI entrypoint, import
  route, or diagnostic boundary over a real authority. A wrapper that preserves
  duplicate execution authority is a bug.

## High-Signal File Map

- TIR/op facts: `runtime/molt-ir/src/tir/`,
  `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py`
- Passes and representation facts: `runtime/molt-passes/src/tir/`,
  `runtime/molt-passes/src/representation_facts.rs`
- Backend lowering: `runtime/molt-backend-native/src/`,
  `runtime/molt-backend-wasm/src/`, `runtime/molt-backend-luau/src/`,
  `runtime/molt-backend-rust/src/`
- Runtime/C-API/ABI: `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
  `runtime/molt-runtime/src/intrinsics/generated.rs`,
  `runtime/molt-cpython-abi/`, `src/molt/_intrinsics.pyi`
- WASM ABI and runner: `runtime/molt-backend-wasm/src/`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`,
  `tools/gen_wasm_abi.py`, `wasm/loader_bridge.js`, `wasm/run_wasm.js`,
  `wasm/browser_embed.js`, `wasm/browser_host.js`
- Process/proof custody: `tools/proof_queue.py`, `tools/memory_guard.py`,
  `tools/harness_memory_guard.py`, `tools/process_sentinel.py`,
  `tools/guarded_exec.py`, `src/molt/backend_daemon_custody.py`
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
