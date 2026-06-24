<!-- Foundation design 45. Supervisor-authored from the #77 diagnosis + recovery
agent's lifecycle proof (baton: memory/project_exception_loop_leak_baton.md) +
council directive (2026-06-08). Executable design, not a survey. HEAD-anchored
at origin/main 0e233db5f. -->

# ExceptionRegion ownership — the exception-lifetime substrate

## 0. Why this exists (the discovery, not a benchmark patch)

`bench_exception_heavy` measured 0.68× warm vs CPython, cycle-attributed (#76,
quiet, 100% in-binary) to **inc_ref/dec_ref ~22%** + GIL ~11% + exception-stack
bookkeeping ~12%, with a coupled **~70 MiB / 30-iteration leak**. The #77 attack
proved this is not a peephole: exception ownership is **region-scoped**, and
molt's RC model reasons mostly in value / global-last-use space. The single-
global-last-use model **structurally cannot place** the correct release of
handler-owned exception state. So `exception_heavy` is a *missing abstraction*,
not a slow path — and the fix retires a whole class (the leak, the churn, LLVM
exception-CFG fragility, drop placement on handler edges, finalizer/unwind
ordering).

## 1. Problem: the leak has TWO independently-owned components

Per raised-and-immediately-caught exception in a loop (diagnosed op-by-op via
`MOLT_DUMP_FINAL_FUNC_IR` + `MOLT_TRACE_EXC_RC`):

- **Component A — CreationRef** (`exception_new*` result): per-iteration-dead,
  SSA last-use = the `raise`. **Value-tracking-expressible** — a #46-style
  per-iteration-temp analysis releases it (prototype: rc 2→1, preserved at
  `memory/recovery/excfix_wip/function_compiler_excregion_wip.patch`).
- **Component B — MatchRef** (`exception_last_pending` result): was the
  remaining native leak class in the original diagnosis. Its SSA last-use is
  the re-raise in the **no-match ELSE branch that never executes** on the
  caught path; on the matched path it is only *borrowed*. The correct release
  point is **handler-region exit (`exception_pop`)** — CPython's implicit clear
  of the caught exception — a **per-PATH exception-CFG liveness fact** the
  single-global-last-use model cannot express. Native Cranelift now has a
  targeted Phase-1 slice that pairs owned handler MatchRefs from
  `exception_last*`, `exception_active`, `exception_current`, and
  `exceptiongroup_*` with the reachable path-depth closing `exception_pop`.

Original pre-fix net per caught exception: 3 inc / 2 dec → rc=2 leaked (with the
Component-A prototype alone: 3 inc / 3 dec but the lone `dec` sits in the dead
ELSE branch → rc=1, still leaked). The current native slice closes that specific
split-cleanup MatchRef pairing bug, but it is not yet the backend-neutral
`ExceptionRegion` authority.

## 2. Falsified vs supported (binding)

- **FALSIFIED:** "exception objects are missing from value-tracking
  registration." They ARE tracked (generic per-op tail registration,
  `function_compiler.rs:~24992`).
- **SUPPORTED:** the issue is **release-boundary PLACEMENT**, not registration.
  CreationRef releases at the raise; MatchRef must release at handler-region
  exit. No UAF exists in the prototype: every exception-STATE SLOT
  (`global_last_exception`, the task slot, `ACTIVE_EXCEPTION_STACK`) holds its
  **own independent inc'd reference**, and `sys.exc_info()` / `sys.exception()`
  lower to `molt_exception_active` reading the *slot*, not the SSA temp — so
  releasing an exception SSA temp at its last use can never dangle a slot.

## 3. The CPython semantic contract (Python-visible lifetime rules — not impl details)

- `sys.exception()` returns the caught exception **only while a handler is
  executing**, `None` otherwise; the stored active exception is **reset on
  leaving the handler** (Python language ref, §try; sys docs).
- `except E as e` is cleared at the end of the except clause — effectively
  `finally: del e` — *specifically* because exceptions with tracebacks form
  cycles with stack frames and keep locals alive.
- A traceback attached to an exception keeps **frame + local** state alive;
  `__traceback__` / `__context__` / `__cause__` / `__suppress_context__` are
  reachable while the exception is.
- `finally` can save / discard / re-raise the active exception across nonlocal
  exits (return/break/continue/raise).

These are the placement obligations the abstraction must honor exactly.

## 4. IR model: ExceptionRegion / HandlerState

Exception ownership is **a property of a region state machine, not of one SSA
value.** Introduce a per-`try` `HandlerState` with explicit lifetime points:
`entry → match → bind → (body) → pop | reraise | finally-save/restore/discard`.
A `HandlerState` is the owner of the active-exception roots for its region; its
boundary (`pop`/`reraise`/transfer) is where release/transfer obligations fire.

## 5. Ownership model (who owns each root, released/transferred where)

- **CreationRef** — owned by the `raise` site; released or **transferred** into
  the pending/HandlerState at the raise boundary (a propagating exception
  transfers; a caught one hands to HandlerState).
- **MatchRef** — owned by the HandlerState; released at **region exit** unless
  transferred to user-visible storage (stored binding, `__context__`/`__cause__`
  of an escaping exception, a returned value).
- **BindingRef** — `except E as e`: the local `e` owns/references the handler
  exception; **cleared at handler exit** (`del e`) unless the value escaped/was
  stored.
- **Traceback / context / cause** — owned by the exception object and/or explicit
  traceback roots; follow the exception's ownership boundary (released with it
  unless reachable via a stored ref).

## 6. Placement rules (every exit edge)

normal handler fallthrough · break/continue/return from a handler · exception
raised **inside** a handler (the new exception's region nests; the outer match
becomes `__context__`) · `raise` (re-raise → transfer back to propagating) ·
`raise X from inner` (transfer inner to `__cause__`) · `finally` (save before /
restore-or-discard after the protected region) · nested handlers (LIFO region
stack). **Invariant:** every handler-owned exception root is `pop`'d,
transferred, or re-raised **exactly once on every exit path**.

## 7. Event model (name the semantic events; not all become public TIR opcodes day 1)

```
ExceptionPush(exc)          pending exception roots exc (raise / propagate)
ExceptionMatch(exc, h)      HandlerState h acquires the match ref
ExceptionBind(name, exc)    local binding per `except as` rules
ExceptionPop(h)             leave handler: restore prior sys.exception, release match ref
ExceptionReraise(h)         transfer handler exception back to propagating state
ExceptionClearBinding(name) `except E as e` cleanup at handler exit
ExceptionFinallySave(exc) / ExceptionFinallyRestore(exc) / ExceptionFinallyDiscard(exc)
```
Once named, the compiler/runtime places DecRef / transfer obligations correctly
at each. This is the same generated-fact discipline as the op-semantics ladder
(#70/#72/#73/#74) — exception ownership becomes a *region* fact on the #58
ownership-boundary lattice (region-lifetime facts; `InteriorBorrowKeepAlive`
#73 and `ConditionalValidOnlyOnEdge` #74 are the path-sensitive siblings that
prove the lattice can carry exactly this kind of boundary).

## 8. Minimal implementation — phased (prove the model before the edge cases)

The current Phase-1 implementation slice has one shared fact authority and two
consumption paths:

- CreationRef temporaries from `exception_new*` are released at the raise
  boundary when the creation reference is per-iteration-dead. This release is
  path-local per `Raise` op: mutually exclusive raise edges that share one SSA
  exception object each materialize their own post-raise `DecRef`, rather than a
  function-global "first raise wins" release.
- Handler MatchRef temporaries from `exception_last*`, `exception_active`,
  `exception_current`, and `exceptiongroup_*` are selected by TIR
  `ExceptionRegions` path-depth facts and bound to the reachable
  handler-region `exception_pop`.
- Shared TIR drop insertion consumes CreationRefs selected by
  `ownership_lattice_min::exception_creation_ref_values` at the `raise`
  boundary and MatchRefs immediately after the owning `exception_pop`, before
  the conservative handler-CFG drop-pass bail, by materializing ordinary TIR
  `DecRef` ops.
- Native Cranelift is now activated on the same TIR DropInsertion path, and the
  old native-only CreationRef lifetime carve-out plus `exception_pop`
  side-emission path are deleted.
- Runtime module attribute lookup preserves CPython-shaped module
  `__getattr__` exception behavior: direct `module.attr` and two-argument
  `getattr(module, name)` propagate a raised `AttributeError`, while
  `getattr(module, name, default)` consumes only `AttributeError` and returns
  the default.

Backend-neutral TIR now has `ExceptionRegions` analysis + verification in
`runtime/molt-tir/src/tir/exception_regions.rs`. It recognizes the current
`Copy` + `_original_kind` exception carriers, computes path-state reachable
`exception_pop` release boundaries for handler MatchRefs, emits diagnostics for
missing/ambiguous/too-early releases, is registered with the analysis
manager/debug freshness check, and fails closed from the pass-manager
verification boundary when those diagnostics are present. An implicit
`TryStart`/`CheckException` transfer may enter handler-owned state only when the
target label is currently active in the lexical exception frame stack; inactive
handler-label targets remain ordinary depth-zero exception observers and must
not manufacture handler owners. This keeps universal `CheckException`
observation from turning post-region or exit-cleanup probes into false
MatchRef-release obligations. Depth-zero exception reads remain ordinary
observers rather than handler-owned MatchRefs, so guard-style `exception_last`
probes outside an open handler region stay on the normal value/lifetime path.
Shared drop insertion materializes TIR `DecRef`s for CreationRefs at `Raise` and
for MatchRefs after the owning `exception_pop` for activated TIR-drop targets,
including native Cranelift. The current analyzer also accepts path-alternative
handler exits, loop re-entry shapes where `try_end` and `exception_pop` are one
close boundary, and shared `exception_pop` blocks with block-arg payloads, where
the splitter now routes the moved tail through fresh continuation args so
inserted MatchRef releases preserve SSA dominance.
Validator fail-closed coverage now includes missing-pop, ambiguous-depth, and
terminal drop-pipeline diagnostics. Checked backend consumption proof covers
LLVM lowering order, WASM host-EH/native-EH import behavior plus the LIR
`dec_ref` runtime-call lane, and Luau checked lowering of shared drop artifacts
as GC no-ops after the Luau target-info terminal drop phase. Luau and LLVM now
also have executed runtime artifact proof for the raise/catch leak loop. The
prior WASM structural-validation blocker is fixed and the linked artifact
validates. The `env::molt_process_terminate_host` host-ABI gap is now covered by
the JS harness import map alongside the real Node/Wasmtime/browser hosts, and
the focused WASM leak-loop differential now passes. Broader WASM
`HandlerState` parity still remains open.

Evidence:
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" exception_region -- --nocapture` (23 passed).
- `cargo test --manifest-path runtime/Cargo.toml -p molt-backend --features wasm-backend tir::exception_regions -- --nocapture` (22 passed; includes inactive `CheckException` handler-target regression).
- `cargo test --manifest-path runtime/Cargo.toml -p molt-backend --features wasm-backend tir::drop_phase -- --nocapture` (4 passed).
- `cargo build --profile release-fast -p molt-backend --no-default-features --features wasm-backend` (passed; existing warnings only).
- `MOLT_WASM_LINKED=0 MOLT_WASM_LINK=0 MOLT_WASM_STAGE_AUDIT=1 MOLT_MODULE_STAGE_AUDIT=1 MOLT_DROP_STAGE_AUDIT=1 MOLT_DROP_STAGE_AUDIT_FUNC=Sequence_index MOLT_DISABLE_INLINING=1 .venv/bin/python3 tools/memory_guard.py --max-rss-gb 12 --max-total-rss-gb 14 -- target/release-fast/molt-backend --target wasm --wasm-data-base 69074944 --wasm-table-base 3367 --output tmp/wasm-rss-repro/20260620-060416-exception-region-active-frame-fix/out.wasm --ir-file logs/wasm-rss-repro/20260620-054224-moduleaudit-noinline/handoff.ir.json` (passed; receipt `logs/wasm-rss-repro/20260620-060416-exception-region-active-frame-fix/memory_guard.summary.json`; `returncode=0`, `violation=null`, rusage peak 0.324 GiB; `_collections_abc__Sequence_index` exception-region analysis produced 0 match-release facts at 259.1 MiB instead of the prior 12 GiB kill).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" lower_to_simple_emits_separate_drop_fact_markers -- --nocapture` (1 passed).
- `cargo test -p molt-backend --features wasm-backend import_transaction_callable_wrapper_matches_runtime_import_abi -- --nocapture` (1 passed).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" compile_checked_ -- --nocapture` (14 passed).
- `cargo test -p molt-backend --lib --features "luau-backend" validate_luau_source -- --nocapture` (6 passed).
- `cargo test -p molt-backend --lib --features "luau-backend" test_luau_tir_roundtrip_raise_catch_closes_pcall_before_handler -- --nocapture` (1 passed).
- `cargo test --manifest-path runtime/Cargo.toml -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" shared_drop -- --nocapture` (3 passed).
- `cargo test --manifest-path runtime/Cargo.toml -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" test_luau_tir_roundtrip_raise_catch_closes_pcall_before_handler -- --nocapture` (1 passed).
- `cargo check --manifest-path runtime/Cargo.toml -p molt-backend --features "native-backend llvm luau-backend wasm-backend" --bin molt-backend` (passed).
- `python3 -m py_compile tests/wasm_harness.py` (passed; pre-existing invalid-escape warnings only).
- `python3 - <<'PY' ... wasm_runner_source() ... envImports.molt_process_* ... PY` (passed; JS harness includes the process host ABI env imports, including `molt_process_terminate_host`).
- `MOLT_DIFF_RESULTS_JSONL=tmp/wasm_diff_exception_regions_20260612_rerun.jsonl MOLT_WASM_DIFF_BUILD_TIMEOUT=1200 MOLT_WASM_DIFF_RUN_TIMEOUT=180 uv run --python 3.12 python3 tools/wasm_diff.py --build-profile release --out-root tmp/wasm_diff_exception_regions_20260612_rerun --jobs 1 tests/differential/memory/exception_raise_catch_loop_leak.py` (passed; raw/resolved status `pass` for the WASM runtime leak-loop differential).
- `uv run python -m molt.cli build tests/differential/memory/exception_raise_catch_loop_leak.py --target luau --profile release --out-dir tmp/exception_regions_luau_proof_fixed6 --rebuild --verbose` (built `tmp/exception_regions_luau_proof_fixed6/exception_raise_catch_loop_leak.luau`).
- `luau tmp/exception_regions_luau_proof_fixed6/exception_raise_catch_loop_leak.luau` (printed `500000`).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend" exception_region -- --nocapture` (12 passed).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend" ambiguous_exception_match -- --nocapture` (1 passed).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend" compile_checked_accepts_shared_drop_artifacts_as_gc_noops -- --nocapture` (1 passed).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" generic_wasm_exception_pop_then_drop_keeps_dec_ref_import_across_eh_modes -- --nocapture` (1 passed).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" lir_fast_lane_dec_ref_emits_named_runtime_call -- --nocapture` (1 passed).
- `cargo test -p molt-backend --lib --features "native-backend llvm luau-backend wasm-backend" lowers_exception_pop_then_dec_ref_from_shared_drop_shape -- --nocapture` (1 passed).
- `cargo test -p molt-backend --lib --features "native-backend" tir::passes::drop_insertion::tests -- --nocapture` (24 passed).
- `cargo test -p molt-backend tir::passes::drop_insertion::tests::exception_region_match_release_splits_shared_pop_with_block_args -- --exact` (passed).
- `MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 1024 --timeout 180 -- uv run python -m molt.cli run tests/differential/memory/exception_raise_catch_loop_leak.py --target native --release --rebuild` (passed; `live_objects=649` after 500,000 raises/catches).
- `MOLT_ASSERT_NO_LEAK=1 python3 tools/memory_guard.py --timeout 1200 --max-rss-gb 18 --max-total-rss-gb 24 -- uv run python -m molt.cli run --target llvm --release --rebuild tests/differential/memory/exception_raise_catch_loop_leak.py` (passed; printed `500000`, `live_objects=649` after 500,000 raises/catches).
- `cargo test -p molt-backend --lib --features "native-backend" representation_plan::tests::reachable_heap_incoming_poisons_raw_loop_phi -- --nocapture` (1 passed).
- `uv run pytest tests/test_native_import_bootstrap_regressions.py::test_native_relative_from_import_direct_call_executes -q` (passed).

The latest targeted 2026-06-12 hot-only `bench_exception_heavy` receipt is
`bench/results/bench_exception_heavy_exception_regions_20260612_after_luau_parity.json`.

This is not full ExceptionRegion completion. The durable end state still needs
the wider `HandlerState` boundary and authoritative exception-heavy speed
evidence before the RED status moves. The
prior WASM runtime-surface blocker that pulled `molt-db`/sqlite into linked
runtime builds is closed at the feature-plane level: wasm micro/full
availability, Cargo command features, fingerprints, and bench-wrapper runtime
feature construction exclude sqlite, while explicit sqlite-on-wasm still fails
closed. The corrected end-to-end WASM proof now builds a structurally valid
linked artifact and advances past the former `func 1233` stack-validation
failure; the JS harness host map now includes the process host ABI imports that
the linked runtime needs. WASM now has runtime differential evidence for
`tests/differential/memory/exception_raise_catch_loop_leak.py`; this closes the
raise/catch leak-loop runtime parity proof, not broader WASM `HandlerState`
parity.
The 2026-06-12 targeted `bench_exception_heavy` hot-only after-Luau-parity rerun was
valid for cycle attribution (`inner_loops=40`, launch/page-in 0.0%, in-binary
100.0%, top in-binary frames `molt_runtime::object::dec_ref_ptr` 10.2%,
`molt_runtime::concurrency::gil::GilGuard::new` 10.1%, and
`bench_exception_heavy__molt_user_main` 8.0%) but non-authoritative because host
load was not quiescent (`loadavg_1m=23.81`, threshold `9.00`), so no
performance claim moved.

**Phase 1 — bare raise/catch loop** (the model proof):
```python
for i in range(N):
    try: raise ValueError(i)
    except ValueError: pass
```
Acceptance: `MOLT_ASSERT_NO_LEAK` passes (RSS plateaus under #76 `--inner-repeat`);
exception_heavy no longer leaks per iteration; `sys.exception()` is the exception
**inside** the handler and `None` **outside**; native + LLVM agree (or the backend
gap is documented); #76 quiet hot profile shows exception RC/churn **moved**.

**Phase 2 — `except E as e`**: `e` alive in-handler, cleared at exit; a stored `e`
remains usable; an unstored `e` does not retain traceback/frame/locals.

**Phase 3 — traceback / `__context__` / `__cause__`**: stored-vs-not lifetime;
`traceback.format_exception` works; chaining matches CPython.

**Phase 4 — finally / re-raise / nested / break-continue-return-from-handler.**

The native slice above is evidence and a production guardrail, not permission to
preserve a backend-specific ownership side channel permanently. Native now reads
TIR-authored release facts instead of recomputing them, and the next convergence
step is to keep every backend consuming the backend-neutral ExceptionRegion /
drop-pass authority directly without restoring the deleted transport lane.

## 9. Validator (Alive2-style, scaled to molt — add as soon as the event model exists; ties #TV-1)

Checkable obligations (not full formal verification):
1. For every `ExceptionMatch` there is exactly one `ExceptionPop` / `Reraise` /
   transfer on **every** exit path.
2. No handler-owned exception root reaches function exit without transfer.
3. No `ExceptionPop` runs before a `sys.exception()`/`sys.exc_info()` use inside
   the handler (the reset must not precede observers).
4. No `except E as e` binding survives handler exit unless explicitly stored.

## 10. The frontier lesson (CPython 3.11 zero-cost exceptions — inspiration, not copy)

3.11 made `try` impose ~zero overhead on the no-throw path and shrank the
catch-time exception representation. molt's AOT analogue: the **normal edge pays
nearly zero** for handler existence (no exception-stack churn on the no-throw
path); the **exceptional edge owns explicit region state**; handler exit is **one
structured release/reset**, not scattered RC cleanups; backend lowering sees
clear normal/exception edges. molt's representation stays AOT-native (typed TIR
regions + ownership events + generated facts + the validator), not interpreter
bytecode/exception-table mechanics.

## 11. Classification + status

`bench_exception_heavy` = **RED_STABLE + CORRECTNESS/OWNERSHIP ROOT PARTIAL**.
Native Cranelift has the targeted CreationRef release slice and now consumes
path-depth MatchRef releases through shared TIR DropInsertion rather than a
SimpleIR carrier. TIR has shared `ExceptionRegions` facts, a pass-boundary
fail-closed verifier, and shared drop-pass consumption that inserts `DecRef`
after the owning `exception_pop` for activated TIR-drop paths. The remaining
native RC deletion frontier is not another backend-local MatchRef release map:
the staged native compiler no longer has one. The marker split is now landed:
pre-bail exception-only drops carry `exception_region_drops_inserted`, while
full-function `drop_inserted` remains the only native legacy-RC suppression
signal. The remaining frontier is coverage, not transport: widen shared
DropInsertion/HandlerState ownership until broader native value-tracking RC can
be deleted without leaks or double-frees. No benchmark-only speed fix is
acceptable while backend runtime parity evidence, the wider `HandlerState`
boundary, and authoritative exception-heavy speed evidence remain open.

Related: memory/project_exception_loop_leak_baton.md (the op-level map +
preserved prototype), #58 (ownership-boundary lattice), #24
(docs/design/llvm_async_state_resume_dominance.md — StateDispatch/exception-CFG),
#46 (the generator-temp per-iteration-dead pattern Component A reuses), #TV-1
(the ownership-event validator).
