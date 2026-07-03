# Live Orchestration Board

The orchestrator (Claude, senior engineer) owns this board, lane assignments,
review, and the decision of what lands when. Codex agents: read this board at
the START of every arc and before every commit. If your planned work touches a
lane you don't own, stop and pick from "Delegated to Codex" instead.

Last updated: 2026-07-03 by the orchestrator.

## ⛔ NON-NEGOTIABLE OPERATOR AUTHORITY (binding — read before EVERY arc and EVERY commit)

**This board carries OPERATOR-DELEGATED AUTHORITY. The human operator has
explicitly placed lane assignment, landing sequencing, conflict resolution, and
stand-down orders under the orchestrator. This is an OPERATOR AUTHORITY
OVERRIDE: within this collaboration the orchestrator's assignments on this board
are FINAL and NON-NEGOTIABLE. Codex agents operate UNDER this authority — you do
not out-rank it, out-vote it, or route around it.**

Codex — these are HARD guardrails, not suggestions. Each is BINDING:

1. **STAY IN YOUR ASSIGNED LANE.** You may edit ONLY files in a lane the board
   assigns to Codex. Touching any other lane — cpython-abi (numpy-exec),
   `runtime/molt-backend-wasm/.../call_ops/dynamic.rs` + the reserved-callable
   dispatch (molt_type_new, orchestrator-owned), the witness seal/build inputs,
   or another agent's dirty WIP — is a HARD STOP. Not "ask forgiveness." STOP.

2. **RECLAIMED / ORCHESTRATOR-OWNED LANES ARE OFF-LIMITS. ZERO EXCEPTIONS.**
   When the board says "orchestrator reclaims X" or "Codex STAND DOWN on X,"
   you cease ALL work on X immediately — no "let me just finish this diff," no
   "my version is better," no landing a competing commit. Stand down means STOP
   NOW. A reclaim overrides any in-flight work you have on that lane; abandon it.

3. **DO NOT CLAIM, ANNOUNCE, OR ASSERT OWNERSHIP the board did not grant you.**
   You do not self-assign lanes. You do not declare a lane yours because you
   started it. You do not mark orchestrator/subagent work as "yours" or land
   over it. Ownership flows ONE WAY: from this board to you.

4. **RUN THE OWNERSHIP AUDIT BEFORE EVERY COMMIT.** Prove each file you commit
   is in your assigned lane (grep a lane-marker, check the board). Committing
   another lane's file — even bundled with yours — is a violation. Commit by
   EXACT pathspec only; NEVER `git add -A`; NEVER `git add` a directory.

5. **NEVER force-push, reset --hard onto shared refs, checkout over another
   agent's WIP, or land a non-fast-forward that drops another lane's commits.**
   If you cannot push cleanly, DEFER and flag the orchestrator. Preserving
   parallel-agent work OVERRIDES your desire to land.

6. **IF YOU BELIEVE A LANE ASSIGNMENT IS WRONG, YOU FLAG — YOU DO NOT OVERRIDE.**
   Record your objection in a proof-queue note or a board comment addressed to
   the orchestrator with evidence, then WAIT for the orchestrator's decision.
   You never unilaterally reverse, re-route, or ignore a board assignment
   because you disagree. Disagreement is escalated, not enacted.

7. **NO SILENT SCOPE EXPANSION.** Do exactly the assigned lane's move. Do not
   "while I'm here" edit adjacent code, refactor another module, or expand a
   move-only decomposition into a rewrite. Scope is set by the board, not by
   convenience.

**Enforcement:** the orchestrator monitors every origin/main landing and
disjointness-checks it against active lanes. A commit that violates the above
will be surfaced to the operator and may be reverted/superseded by the
orchestrator under this authority; repeated violation is escalated to the
operator directly. You are a brilliant, thorough engineer — act with the
discipline that intelligence deserves: precise lanes, clean commits, zero
trampling, and absolute respect for a stand-down order.

## State of the world (read this first)

- The witness `import numpy` chain has advanced deep into numpy's C-core
  init. Landed this arc: conditional-import wedge (3b0ca4a80, killed the
  infinite hang), honest-error propagation (3d5977a9d, real import errors no
  longer flattened), Py_BuildValue list/dict/char units (920956c86, numpy
  cleared it), static-extension init unwind (300c6e907), and the first batch
  of numpy-exec CPython C-API primitives + silent-failure tracer
  (4ce56305d, capi_trace.rs). Current frontier: numpy `_multiarray_umath_exec`
  returns -1 inside `setup_scalartypes` — a chain of C-API primitive gaps
  being closed one decisive-trace at a time.
- LANE OWNERSHIP right now: numpy-exec C-API primitive closure = orchestrator's
  ONE subagent (owns runtime/molt-cpython-abi/src/api/*, lib.rs, capi_trace.rs,
  the exec/PyType_Ready/PyCFunction path). Buffer/ndarray/dtype/shape/stride/
  memoryview truth (witness lane 2) = Codex (owns object/memoryview.rs,
  object/ops_memoryview.rs, api/buffer.rs, builtins/module_table.rs,
  builtins/array_mod.rs). These are DISJOINT — keep them so.
- The native indirect-call P0 (R1) is LANDED (codex/native-import-typeerror,
  0 ahead of origin). That lane is closed.

## Drift-resolution protocol (binding — the shared-checkout is the bottleneck)

The shared checkout accumulates multiple hands' uncommitted WIP, which blocks
`merge origin/main` and causes stale-base builds. Discipline to keep velocity:

- **Commit verified work in SMALL, DISJOINT commits, promptly.** The moment a
  proof row confirms your lane's change compiles/passes, commit ONLY your
  files by exact pathspec. Do not accumulate a large dirty tree — it drifts
  and blocks everyone.
- **Run an ownership audit before committing** (e.g. grep for a lane-marker
  like `capi_trace` reference count) to prove a file is yours, not another
  lane's WIP. Never bundle another lane's uncommitted files.
- **Never stash/overwrite another hand's WIP to force your branch forward.**
  If you can't push (non-fast-forward) because the shared tree is dirty, DEFER
  the push and tell the orchestrator. Preserving parallel work overrides
  tidiness.
- **Orchestrator lands via cherry-pick-in-isolated-worktree.** To land a
  disjoint commit onto origin/main without disturbing the shared dirty tree:
  `git worktree add --detach <E:/path> origin/main; git -C <path>
  cherry-pick <sha>; git -C <path> push origin HEAD:main`. Verify the base
  delta doesn't touch the commit's crate (`git log <oldbase>..origin/main --
  <crate>`) so the author's compile-check transfers — no rebuild. This is how
  4ce56305d landed cleanly while the shared tree stayed dirty.
- **Prefer per-lane worktrees for NEW build-heavy lanes** so the shared
  checkout stays clean; commit + push to a branch and the orchestrator
  cherry-picks to main.

## 1000-Year End-State Roadmap (R0–R9) — the recipe

This is the full path from HERE to the end state. Every item names its
ingredients (files/authorities), its process (commands), and its acceptance
evidence. No item may land as a partial: each is a complete subsystem cut.
Dependency edges are explicit; anything not blocked may proceed in parallel
subject to lane ownership. The overriding outcome bar, in order:
(1) correctness incl. memory safety, (2) faster than CPython everywhere
claimed — approaching/beating Codon and PyPy on numeric kernels,
(3) CPython >=3.12 parity with version+platform gating, (4) deterministic
small fast-start artifacts, (5) world-class agent-first DX.

### R0. Pact witness kernel GREEN end-to-end (owner: orchestrator) — ACTIVE

The done criterion of the current goal: `field_solve.py` from
`collab/pact/` compiles through the live WASM/browser path, produces
`candidate_outputs.npz`, and `check_parity.py` passes — no host-CPython
fallback, no fake symbols, upstream source only through package custody.

- R0.1 `_multiarray_umath` static init failure + propagation wedge.
  Ingredients: `runtime/molt-runtime/src/builtins/module_table.rs` (module
  states {Uninit, Initializing, Ready, Tombstone, Replaced};
  `molt_module_ensure` is the ONLY transition owner), static extension init
  path, `static_extension_init_failure.json` dossier emitted by the
  acceptance lane. Process: `bash tools/witness_cycle.sh` (build|run|cycle)
  with `MOLT_TRACE_IMPORT_STAGE=1`; read the dossier BEFORE any manual
  rummaging. Two defects to close as one arc: (a) the init failure itself
  (whatever C-API/ABI symbol or capsule the module needs — close it as a
  reusable primitive, never a stub); (b) init failure must propagate as a
  Python ImportError and unwind — a wedge/hang on the error path is a
  module-state custody bug (Initializing never resolved). Acceptance:
  `alias_probe.py` prints its numpy/scipy census and `WITNESS-CHAIN-OK`.
- R0.2 numpy.linalg closure (eigh chain). The built artifact
  `_umath_linalg.molt.wasm` is parked at `tmp/pact_staging_parked/`;
  restage into the numpy seal `numpy/linalg/`, rebuild, prove
  `numpy.linalg.eigh` executes. Depends: R0.1.
- R0.3 scipy.ndimage executable dispatch: `distance_transform_edt`,
  `gaussian_filter`, `label` native callable_exports must be executable ABI
  dispatch (not import-visible-only). Ingredients: callable-table slots
  (`module_abi/callable_table/layout.rs` slot-addressed builder), app
  callable resolver, `_nd_image.molt.wasm` manifest callable_exports.
  Acceptance: alias_probe's EDT/gaussian/label chain returns correct
  values. Depends: R0.1.
  RESOLVED 2026-07-03 on origin/main by 3b0ca4a80: the from-import form
  `from nativepkg.ndimage import distance_transform_edt;
  distance_transform_edt(x)` now lowers to `invoke_ffi` when the import binding
  is live. Conditional/evicted imports still route through `module_get_global`
  and `call_bind`, preserving CPython LOAD_GLOBAL semantics. Evidence:
  `tests/test_frontend_ir_alias_ops.py` passed 33/33 and pins both paths.
- R0.4 Acceptance lane: `uv run --active --project . --python 3.12 python
  tools/proof_queue.py pact-witness-acceptance --detach --timeout 7200`.
  Evidence: run ID, `candidate_outputs.npz` produced by Molt WASM,
  `check_parity.py` PASS. Depends: R0.1–R0.3.
- R0.5 Witness performance: time the kernel vs CPython (same inputs);
  faster-than-CPython is part of DONE, and the number goes on the R8
  scoreboard. Depends: R0.4.

### R1. Native call-lane unification (owner: orchestrator's subagent) — ACTIVE

End state: ONE call-target authority. Trampoline vs fixed-arity direct
dispatch is a single registry decision; the borrowed-vs-consumed argument
ownership contract is written in exactly one place and both lowering and
runtime read it. No callsite may resolve a function's direct target where
the trampoline target is required (the P0), and no borrowed name string may
be dec_ref'd by the callee. Remaining known layers after the current WIP
lands: dec_ref-of-borrowed-name; "SystemError: module id out of range".
Gates: `tests/test_native_import_bootstrap_regressions.py`, a synthetic
compile_func indirect-call test (E2E is release/WIP-brittle; the synthetic
test is the durable gate), differential `python tests/molt_diff.py ... --jobs 1`.

### R2. Import bedrock completion + FREEZE (owner: orchestrator; Codex preps PR2)

Per `docs/design/foundation/69_import_bedrock_frozen_module_layer.md`.
PR1 (generated ModuleRegistry + runtime ModuleTable) is LIVE.
- PR2: sys.modules becomes a dict VIEW over the one module store; DELETE the
  Rust mirror sync (task #14). Blocked on: R1 landing (modules.rs quiet).
- PR3: wasm import/export/callable tables become REGISTRY PROJECTIONS
  (generated from the same authority; `module_abi/**` is reserved for this).
- FREEZE: wire the design's 11 invariant gates into CI; add the freeze
  contract to CLAUDE.md + AGENTS.md ("the import/bootstrap layer changes
  only by amending doc 69 first"); then this layer is bedrock — no
  incremental patches ever again.

### R3. Numeric raw-lane keystone (owner: orchestrator; Codex builds R3a)

The single highest-leverage perf arc: molt currently BOXES loop arithmetic
(every int/float op = NaN-box runtime call + refcount). CheckedMul peel is
LANDED (261efc7b2) and is the pattern to generalize.
- R3a `molt-check` TIR translation validator: Repr may only move UP the
  lattice; built on `runtime/molt-passes/src/representation_facts.rs` +
  `typed_repr_report.rs`. This is the drift gate that catches silent-OOB
  class bugs (GAP-3) at IR level. Adapt existing egg/egraph_simplify.rs +
  fuzz_tir_passes.rs infrastructure; do NOT greenfield.
- R3b Loop-body int/float RAW-LANE specialization: native
  iadd/imul/fadd/fdiv in loop bodies with box/unbox hoisted to loop
  boundaries. Ingredients: `runtime/molt-passes/src/tir/scalar_carriers.rs`,
  `value_range.rs`, the CheckedMul lowering
  (Cranelift `smulhi`, 64-bit-exact flag; Luau conservative; WASM boxed
  until R4a). Carrier disagreements between value_range/arith_division/
  scalar_carriers are P0 silent-wrong-answer bugs (the loop-IV modulo class)
  — one carrier authority, gated by R3a.
- R3c Dynamic-IV bounds-check elimination (GAP-3): UNBLOCKED ONLY after R3a
  can prove the widening safe (silent OOB risk if widened wrong).
- Acceptance: spectral_norm and numeric cluster A GREEN vs CPython
  (`python -m molt build --release`, differential harness serial), then the
  same kernels timed vs Codon and PyPy for the R8 scoreboard.

### R4. Full WASM + WebGPU lowering (binding 1000-year directive)

No boxed fallbacks on proven-typed hot paths; lowering into NATIVE
instructions and symbols.
- R4a Numeric ops lower to native wasm instructions (i64.add, f64.mul, ...)
  driven by the generated op_kinds authority; delete the boxed runtime-call
  lane for proven-typed ops in the same arc. Depends: R3b (shared Repr facts).
- R4b simd128 for vectorizable kernels (the wasm feature is already in the
  target contract; lowering must actually emit v128 ops).
- R4c WebGPU: `molt.gpu` (the tinygrad custody shim's target) lowers to real
  WGSL/WebGPU dispatch. No stubs; if a kernel class isn't supported it
  fails closed with a precise diagnostic.
- R4d Browser embed API per `collab/pact/003_browser_single_function_embed_api.md`.
- Standing rule: every runtime-visible WASM op keeps the synced triple
  (ABI import + op_loop handler + #[no_mangle] export); gate
  `test_wasm_runtime_export_no_mangle.py`; validate E2E with
  `--target wasm --linked` + molt_diff native,wasm.

### R5. Iteration-loop velocity (owner: Codex — primary lane)

The compiler team's own loop is a first-class perf target. Budget: a
one-file edit reproves in <30s native / <60s wasm-link on a warm dir.
- R5a Extract `cpython_abi_hooks` crate per
  `docs/design/foundation/70_molt_runtime_crate_extraction.md` (measured
  47x: 282s→6s). Follow the doc exactly: pure move, precise pub widening,
  per-crate clippy gate added to ci.yml + molt_dev_gates.toml. Sequence
  AFTER R1/R2-PR2 quiet modules.rs churn.
- R5b Further molt-runtime splits (same doc, same discipline), then the
  21_decomposition_program T1 `molt-tir` extraction (~100k-line midend,
  zero tir→backend edges) in a BACKEND-QUIESCENT window.
- R5c Frontend: finish the profiling arc (task #13); produce the ranked
  hot-pass table; lower the top passes to Rust one at a time, each with a
  differential gate proving identical output on the conformance corpus.
- R5d Toolchain config authority: wasm-opt/binaryen + zig + rustup target
  preflights all resolve through checked-in contracts (rust-toolchain.toml,
  `find_wasm_opt()`); any new tool follows the same pattern — pin → PATH →
  MOLT_TARGET_ROOT/toolchains discovery, never ad-hoc.

### R6. CPython >=3.12 parity floor (owner: Codex — continuous)

Version-gated semantics keyed on the TargetPythonVersion authority (never
silent single-version assumptions); Windows/macOS/Linux with explicit
platform gating; all within the verified subset with honest-early
fail-closed diagnostics outside it. Process: conformance shards through the
proof queue; differential harness `--jobs 1`. Every parity fix lands with
its version/platform gate expressed, not hardcoded.

### R7. Ecosystem custody generalization (owner: orchestrator)

Turn the numpy/scipy witness machinery into THE reusable primitives:
- `molt extension build` (meson intro-targets + compile_commands source
  plans; zig as the wasm C++ toolchain; PyMODINIT_FUNC extern "C") is the
  one path for source-recompiled extensions — generalize beyond the
  vendored-meson fork specifics.
- Sealed-root curation (canary pruning, generated-file materialization,
  module-exec-level AST import closure) becomes a tool with a manifest, not
  a hand process.
- ndarray/tensor dtype/shape/stride ownership, buffer protocol, capsules,
  module state, extension object closure: each a shared primitive with one
  storage home. Missing C-API/ABI symbols close as primitives or fail
  closed with precise diagnostics — never per-package hacks.
- Reachability redesign ("Fact B": compute_intrinsic_manifest as the
  authority) kills the gratuitous-heavy-import class; lazy-gating imports
  requires this first (molt is AOT — no on-demand link).

### R8. Scoreboards + release gates (owner: Codex)

Per docs/design/foundation 54–67: perf scoreboards run quiescent and
classified, one row per benchmark/profile/target vs CPython AND Codon AND
PyPy; binary-size, startup, and throughput ratchets that only tighten.
A claimed support without a green scoreboard row is not claimed support.

### R9. Polish to freeze (owner: both, at the end of each arc — not a phase to defer)

God-file ratchet back to green by DECOMPOSITION (cli.py ~41k,
function_compiler.rs ~28k, frontend/__init__.py ~27k) — never re-pinned.
duplicate_authorities stays 0. Docs (CANONICALS/INDEX/spec/STATUS/ROADMAP)
move in the same arc as semantics. Final recursive adversarial senior
review before any layer is declared frozen.

### Dependency spine (what blocks what)

```
R0.1 ──> R0.2, R0.3 ──> R0.4 ──> R0.5
R1 ──> R2-PR2 ──> R2-FREEZE ──> R5a (modules.rs quiet)
R3a ──> R3c;  R3b ──> R4a
R7 reachability ──> any import lazy-gating
Everything else: parallel, lane-owned.
```

## Frozen / in-surgery — DO NOT TOUCH

- **Import/bootstrap/module-state layer** (`builtins/modules.rs`,
  `module_table.rs`, `cpython_abi_hooks.rs` import/sys hooks, isolate import
  lowering in `src/molt/cli/backend_ir.py`): bedrock program (R2) owns it.
- **Native call lane** (`call/function.rs`, `fc/modules.rs`,
  `call/class_init.rs`, `builtins/containers.rs`, `builtins/exceptions.rs`,
  `object/mod.rs`): R1 integrator owns the live WIP in the shared tree.
- **`runtime/molt-backend-wasm/src/wasm/module_abi/**`**: reserved for R2-PR3.
- **Witness lane inputs**: `tmp/pact_*` seal roots, acceptance build dirs,
  `wasm/*.sha256` pins — orchestrator-owned.

## Delegated to Codex (pick up, in priority order)

WITNESS CLOSURE is the top program (goal: field_solve.py → candidate_outputs.npz
→ check_parity.py PASS through Molt WASM). The orchestrator's ONE subagent owns
the numpy `_multiarray_umath` exec C-API closure (current import blocker — exec
returns -1 without exception; task #20). Lanes 1-3 run in PARALLEL and are
structurally testable WITHOUT a working numpy import (synthetic
native_callable_exports fixtures — `test_cli_import_collection.py`
native-callable tests pass today). Do NOT touch the numpy exec / cpython-abi
lane the subagent owns.

**CRITICAL-PATH WITNESS BLOCKER — `molt_type_new` reserved-callable arity
(2026-07-03, diagnosed, needs Codex wasm-backend owner).** numpy import now
advances THROUGH PyType_Ready + PyCFunction method population (all landed) and
fails at TYPE CREATION: `molt_call_indirect4 reserved runtime trampoline
molt_type_new expects closure, argv, argc; got 4 args` (idx 2536/2558,
trampoline range). FULL DIAGNOSIS: `molt_type_new`
(runtime/molt-runtime/src/builtins/types/class_model.rs:127) takes 5 args
`(cls_bits, name_bits, bases_bits, namespace_bits, kwargs_bits)` — it's the
`type(name, bases, dict)` metatype construction, invoked when numpy's C exec
calls `type(...)` (via PyObject_Call → PyType_Type.tp_call → the reserved
callable). Reserved callables occupy TWO table slots (loader_bridge.js:699-711):
a Direct slot (5 direct args) and a Trampoline slot (3 args: `0, argv_ptr, 5`).
molt_type_new is registered arity=5, dispatch=Direct
(wasm_callables_generated.rs:78; runtime_callables.rs:21312 `"molt_type_new" =>
Some(5)`). THE BUG: the dispatch that routes `type(...)`/tp_call to
molt_type_new emitted `molt_call_indirect4` (4 args) landing on the TRAMPOLINE
slot — wrong for both slots (trampoline needs `indirect3(0, argv, 5)`; Direct
needs `indirect5`). The arity was miscounted to 4 (dropped one of the 5, or the
metatype `type` call routed through a generic call path that packs cls+args
wrong). LANE: molt-backend-wasm dynamic-call lowering
(`runtime/molt-backend-wasm/src/wasm/op_loop/call_ops/dynamic.rs`) + the
reserved-callable table dispatch, and/or how PyType_Type.tp_call is wired to
the molt_type_new reserved-callable index. ORCHESTRATOR IS TAKING THIS (subagent a57228428b, 2026-07-03) — Codex
STAND DOWN on molt_type_new; stay on buffer lane 2 + the decomposition
directive below. The orchestrator has a subagent on the wasm-backend fix with
the full diagnosis; do not duplicate. Fix the arity/slot so a `type(...)` metatype call reaches molt_type_new
with the correct 5-arg Direct (or 3-arg trampoline) shape. VERIFY: orchestrator
runs the witness diag_probe wasm confirmation (advances past type creation);
add a differential/native test of `type(name, bases, dict)` construction. This
blocks numpy import (upstream of witness lanes 1-3 below) — TOP witness
priority.

**OPERATOR DIRECTIVE 2026-07-03 — GOD-FILE/GOD-CRATE DECOMPOSITION (high
priority, PARALLEL to witness).** World-class OSS separation of concerns:
execute the doc-21 decomposition program's PENDING moves. The god-crate is
`molt-runtime` (~346k lines); `molt-backend-wasm` (~88k) and `molt-passes`
(~82k) are next. Rules: (a) run `tools/structural_audit.py` first to get the
CURRENT RED god-files/crates over the ratchet (the ratchet only moves DOWN —
never re-baseline to green); (b) pick the highest-leverage PENDING move from
docs/design/foundation/21b/21e/21f (crate splits) or 21c/21d (Python
frontend/cli package splits — cli.py, frontend/__init__.py) that is DISJOINT
from the active witness lanes — do NOT touch `runtime/molt-cpython-abi/**`
(numpy-exec subagent) or `runtime/molt-runtime/src/object/**` +
`builtins/module_table.rs` + `builtins/array_mod.rs` (buffer lane 2); (c)
STRICT move-only diff: keep moved files as pure renames, widen `pub` PRECISELY
(never blanket `pub(crate)→pub`), gate on a byte-identical corpus build +
0-warning + lib tests + symbol identity (per 21f execution specs); (d) work in
an ISOLATED worktree, commit small per-move by exact pathspec, ping the
orchestrator to cherry-pick to main; (e) new crates are born UNGATED — add the
per-crate clippy gate to ci.yml + molt_dev_gates.toml in the SAME move. This
is R5b; it is the permanent fix for the ~2160s god-crate wasm rebuild that
throttles the witness confirmation loop.

1. **WITNESS: scipy.ndimage executable ABI dispatch** (goal's named aperture):
   `distance_transform_edt`, `gaussian_filter`, `label`, `maximum_filter`,
   `minimum_filter` must lower to executable ABI dispatch (native
   callable_exports → `invoke_ffi` → runtime forward/object-call ABI), NOT
   import-visible-only. Both `import scipy.ndimage as ndi; ndi.op(x)` and `from
   scipy.ndimage import op; op(x)` forms must emit `invoke_ffi` with correct
   binding/abi/symbol. No fake module__function symbols, no host fallback.
   Prove: `tests/cli/test_cli_import_collection.py` (frontend_native_callable_*
   + pact_ndimage_operation_closure) + the wasm export triple gate
   (`test_wasm_runtime_export_no_mangle.py`).
2. **WITNESS: ndarray/dtype/shape/stride/buffer truth** — typed strided
   storage + buffer protocol the ndimage ABI dispatch passes arrays across
   (dtype/shape/stride/contiguity). One storage home; no Python reimpl of numpy
   behavior. Prove with the buffer/ndarray structural tests; coordinate the ABI
   contract with lane 1.
3. **WITNESS: adversarial review of the numpy-exec closure** — as the
   subagent lands each closed C-API primitive (runtime/molt-cpython-abi),
   independently re-verify: re-run diag_probe, read the primitive against the
   CPython C-API spec (cite it), confirm it is a real reusable primitive not a
   numpy-specific hack, and flag/reconcile duplicate authority (e.g.
   include/molt/Python.h inline Py_BuildValue vs pyarg_variadic.c). Report
   confirmations or defects with evidence.
4. **R6 conformance rotation**: keep shards green through the queue;
   version/platform gates expressed via TargetPythonVersion authority.
   Use `tools/proof_queue.py r6-target-version-parity` for the named lane;
   use `--queue-only` to park rows without launching a runner.
5. **R5a crate extraction** (`cpython_abi_hooks` per doc 70) — ONLY once R1
   + PR2 land and modules.rs is quiet; check this board before starting.
6. **Proof-queue diagnosis/help/audit DX**: take only concrete defects from
   a failed/stale row or operator report; no open-ended queue rewrites.

Done recently by Codex (verified on origin/main): keyed-pin dirty-tree
globs (2f4ed1e88), memory-guard summary/custody DX (5a9c76ce8..08df9bd41),
proof-queue diagnosis/help/audit DX (6a4b7db1a, 6b4c255e3, ac569b886),
R5c frontend profiling contract/evidence (f821f8b62, 71636c688), R3a Repr
pass-delta validator (988020de3), PR2 sys.modules cutover plan (60536c4e3),
queue dead-guard finalization (613b8e954), R6 target-version queue lane and
queue-only named-lane support (e95267290, 9ecf8af05, 1afe04472),
from-import native-call provenance (3b0ca4a80, be516cbff).

## Delegation model (operator directive 2026-07-02)

- The orchestrator runs at most ONE subagent (currently: the R1 successor
  integrator). All other implementation lanes are Codex's.
- Token-efficient, agent-first tooling is a standing deliverable: every
  repeated multi-line invocation becomes a script with a one-line compact
  verdict (rc + stage + first error) and a log path for digging deeper.
  `tools/witness_cycle.sh [entry] [build|run|cycle]` is the pattern.
- Windows/MSYS rule (incident 2026-07-02, hours lost): bash scripts that
  export paths into env vars MUST convert through `cygpath -m` — MSYS
  converts command arguments but NOT custom env vars, and Windows Python
  cannot resolve `/c/...`. The build now fails closed naming any missing
  MOLT_MODULE_ROOTS entry; if you see that diagnostic, fix your script's
  path style.

## Proof and cargo DX rules (binding — incident: 835s cold compile for one test)

- **DO NOT iterate on a full witness/wasm rebuild.** Editing `molt-cpython-abi`
  forces the `molt-runtime` god-crate (~230k lines) to recompile to wasm every
  cycle (~1700s+ per gap; the pact-witness-acceptance E2E lane is ~1500s). That
  is NOT a dev loop. `molt-cpython-abi` has NO dependency on `molt-runtime`, so
  `cargo test -p molt-cpython-abi` compiles ONLY cpython-abi (+ its deps) —
  seconds-to-low-minutes, no god-crate rebuild. Close every CPython C-API
  primitive (PyType_Ready slot inheritance, PyCFunction_NewEx, module exec
  slots, buffer descriptors, number/mapping protocol) behind a
  `runtime/molt-cpython-abi/tests/*.rs` unit test with stub hooks and iterate
  there. Reserve a wasm rebuild for BATCH integration confirmation only.
- **Batch C-extension-init closure via a full trace, not one-gap-per-build.**
  One instrumented wasm build with `capi_trace.rs` (MOLT_TRACE_CAPI) captures
  the ENTIRE C-API call sequence a numpy/scipy extension exec makes up to its
  failure. From that + the extension source, close ALL the needed primitives in
  the fast unit-test loop, then ONE wasm build to confirm the whole batch
  advanced. Target ≤2-3 wasm builds to close an exec, not 20.
- For behavior-only confirmation builds (does the import succeed?), use the
  fastest profile that reproduces it (dev-fast: lto=off, codegen-units=256,
  incremental) — NOT release-fast. Perf gates are separate.
- The molt-runtime god-crate rebuild cost is the structural root; the finer-
  crate extraction (roadmap R5b / decomposition T1) removes it permanently.
- NEVER pay a cold crate compile for a single exact test. If your proof
  needs a compile, run the whole relevant test SHARD in that same compile.
- Warm before you prove: prefer the shared proof-family target dir the
  queue assigns per contention key; if you must use a fresh session dir,
  run `cargo check -p <crate>` warmup FIRST, then submit the proof.
- Set an explicit `--timeout` matched to warm-compile reality; if a row is
  projected to blow it on a cold compile, re-shape, don't wait it out.
- NEVER sit idle narrating a wait. Submit with `--detach`, do other lane
  work, read the row when it closes. A turn that only tails a log is a
  wasted turn.
- Batch proof rows: N tests in one crate = ONE row.
- Env for local iteration: `MOLT_MEMORY_GUARD_POLL_SEC=2.0`.
- When a row's time is dominated by compile, file ONE queue note naming the
  crate and move on.

## Conduct standards (binding — you are brilliant; act like it)

- **Lane ownership is exclusive.** Check this board's lane owner before
  opening any file. Two engineers fixing one defect from two angles
  produces conflicts, not speed.
- **Evidence beats vigil.** At most ONE status read per 5 minutes on a row
  you own, ZERO on rows you don't. Two consecutive "still running" notes
  means you're idling — switch deliverables or end the arc.
- **Diagnosis is time-boxed.** 15 minutes per fault to a hypothesis with a
  bounded experiment; builds go detached while you work elsewhere. Never
  re-run a failed shape unchanged.
- **No unbounded filesystem scans, ever.** Derive exact paths from the
  pytest log, queue log, or artifact manifest.
- **Process spelunking is capped at one snapshot per incident.**
- **Write down what you learned the moment you learn it** (queue note or
  commit message). A finding that lives only in your context is a finding
  the team loses.
- **Fix the tool when the tool wastes you twice.** The second lie from a
  queue row makes the defect the work: file it with the row ID.
- **Side worktrees for runtime/backend edits.** The shared checkout's cargo
  state is everyone's build cache.
- Never revert or checkout files outside your lane, even transiently. A
  file you didn't edit that shows up dirty is another lane's live WIP.

## Working agreement (binding)

- Keep the shared tree compile-green: `cargo check` touched crates before
  any pause longer than a few minutes.
- Regenerate generated files in the same edit as their consumers; never
  leave a consumer referencing a symbol its generated file lacks.
- Commit with pathspecs only (`git commit -- <files>`); never `git add -A`;
  never sweep another lane's dirty files.
- Land small and complete: one coherent arc per commit, replaced code
  deleted in the same commit, tests with teeth (proven to fail on
  violation).
- Run the gates you touched before landing; cite queue run IDs as evidence.
- Compatibility floor: CPython >= 3.12 parity with explicit VERSION GATING
  keyed on the TargetPythonVersion authority, and Windows/macOS/Linux with
  explicit PLATFORM GATING — all within the verified subset, with
  honest-early fail-closed diagnostics outside it.
