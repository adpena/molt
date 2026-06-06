# Boxed-Lane Family Adjudication — Verdict (LLVM int-carrier tickets #58 / #37 / #61 / bigint-loop-carrier)

Session: `boxedlane`. Investigation via prebuilt binaries + LLVM IR/TIR dumps
(`MOLT_LLVM_DUMP_IR=1`, `TIR_DUMP=1`, `--emit-ir`). One batched build for the
localized fixes (worktree `/tmp/wt_boxedlane`, base `origin/main` = `c07cbc6f1`).

## TL;DR — the family is TWO real roots + ONE stale-binary mirage

| Ticket | Root | Disposition |
|--------|------|-------------|
| **#58** (closure call/return ABI → denormal `3.5e-323` / `5.0`) | **A: dynamic-call args not NaN-boxed** | **FIXED + GATED** |
| **#37** (`sum([..])`→`15.0`; `format(int)` boxes float; `for x in range()` float) | **A: same root** (sum/format = dynamic builtin calls; raw int arg → denormal/float result) | **FIXED + GATED** |
| **#61** (`frozenset([..])` → `None` entirely) | **B: LLVM-only missing `frozenset_new` preserved-op arm** (coverage gap, NOT a carrier bug) | **FIXED + GATED** |
| **bigint-loop-carrier** (`1<<60` / `alias_reassign_bigint` → `None`) | **C: NOT a clean-origin/main bug — stale-binary artifact** (see below) | **DO NOT route to Phase-2 on this evidence; re-verify on current main** |

The hypothesis that #58/#37 are "Phase-2's exact target class reachable today"
is **confirmed in spirit, but the fix needed no Phase-2 machinery.** Root A is
the *direct twin* of a bug the direct-call path already fixed (the
`compute(1000000)` raw-int-as-float miscompile, lowering.rs ~2169-2179): an
**asymmetric coverage gap** where the direct-call and `call_builtin` arg paths
box correctly but the *dynamic-dispatch* arg paths did not. Fixed with the
existing `materialize_dynbox_operand` primitive.

---

## Root A — dynamic-call argument marshalling (FIXED)  →  #58, #37

### IR-level evidence
`def outer(): def ident(x): return x; return ident(7)` on LLVM,
`MOLT_DISABLE_INLINING=1`:
```llvm
; t58c__outer, the ident(7) call site:
%call_func = call i64 @molt_call_func_fast1(i64 %func_new, i64 7)   ; <-- RAW 7
ret i64 %call_func                                                  ; raw 7 returned
; module chunk: call void @molt_print_obj(i64 <raw 7>)  -> 3.5e-323  (7 * 2^-1074)
```
The arg `7` is passed as **raw i64 7**, not the NaN-boxed int
`9221401712017801223`. `molt_call_func_fast{N}` / `molt_call_bind` carry args as
NaN-boxed `DynBox`; the callee trampoline (`unbox_dynbox_to_param_ty_with_builder`,
lowering.rs:254) decodes each as a box into the parameter's raw repr. A raw
scalar makes it decode the raw payload as a boxed tag. `ident`'s body is
`ret i64 %0` (returns the raw arg), so the denormal round-trips and `print` reads
raw 7 as f64 → `3.5e-323`; `base + x` → `15` raw → `7.4e-323`. `sum([0..5])` came
back raw and rendered `15.0`; `format(42)` returned a raw int surfacing as
float (the round-3 "ValueError naming 'float'" is the same raw-int-as-float).
The **module-chunk IR is entirely correct** (everything NaN-boxed, no stray
bitcast-to-double) — corruption is purely the arg-marshalling helper.

### Root cause (single helper family)
`FunctionLowering::ensure_i64` (lowering.rs:6200) is a *bitcast-level* cast into
i64 that does **not** NaN-box. The dynamic-call arg loops used it:
- `emit_call_func_runtime` — the `molt_call_func_fast{N}` fast path.
- `emit_call_bind_runtime` — the `molt_callargs_push_pos` → `molt_call_bind` path.
- the `call_method` path (`OpCode::Call` → `molt_call_bind_ic`, ~2512).

Contrast: direct-call (~2180-2210) boxes via `coerce_to_tir_type`; `call_builtin`
(~2711) boxes via `materialize_dynbox_operand`. **Asymmetric.**

### Fix
Replace `ensure_i64(resolve(arg_id))` with `materialize_dynbox_operand(arg_id)`
(boxes per the value's representation plan — I64 via `box_i64_overflow_safe`,
Bool/F64 via their tag) at all three dynamic-arg sites. `callable`/builder
operands stay `ensure_i64` (function objects / args-builder handles, already
`DynBox`). The denormal *return* was a consequence of the raw *arg* (`ident`
returns what it received); no separate return-side fix — the trampoline already
boxes its return (`materialize_dynbox_bits_with_builder`, lowering.rs:6516).

### Coverage completeness (no remaining asymmetry)
Audited every `molt_callargs_push_pos` / `molt_call_func_fast` emission site.
The preserved-op arms `callargs_push_pos` / `callargs_push_kw` (~7110/7134)
already box the *value* operand; `call_bind_ic`/`call_indirect_ic` (~2104) take
an already-built args-builder, not raw scalars. After this fix **all** arg paths
box. Verified e2e: `call_func_fast1/2/3`, `call_bind` (defaults + kwargs),
method-multi-arg via `call_bind_ic`, and int/float/str mixed args — all
byte-identical CPython on LLVM.

---

## Root B — frozenset construction (FIXED)  →  #61

### IR-level evidence
`fs = frozenset([1,2,3])` on LLVM:
```llvm
%frozenset_add = call i64 @molt_frozenset_add(i64 9221964661971222528, i64 %item)
;                                              ^^^^^^^^^^^^^^^^^^^^^^^^ None sentinel
%module_set_attr = call i64 @molt_module_set_attr(i64 %0, i64 %str, i64 9221964661971222528)
```
The frozenset accumulator is the **None** NaN-box: `molt_frozenset_new` is never
emitted, so `frozenset_add(None, item)` no-ops and `fs` is `None`.

### Root cause — preserved-op default-miss (NOT a carrier bug)
`--emit-ir` shows `{"kind":"frozenset_new","args":[],"out":"v77"}` + separate
`frozenset_add(v77, item)` ops. `frozenset_new` has **no dedicated TIR `OpCode`**
— it survives as a `Copy` carrying `_original_kind="frozenset_new"`, dispatched
by `lower_preserved_simpleir_op`'s `match kind`. The LLVM backend had a
`frozenset_add` arm but **no `frozenset_new` arm** → fell through to Copy
passthrough → None sentinel (same default-miss class as `list_from_range`/`vec_*`).
**Native, WASM, and Luau all carry an explicit `frozenset_new` arm**
(`function_compiler/fc/set_ops.rs:117`, `wasm.rs:9014`, `luau.rs:2342`). Pure
LLVM-only coverage gap.

### Fix
Add the `"frozenset_new"` arm to `lower_preserved_simpleir_op`, mirroring
native exactly: `molt_frozenset_new(operands.len())` then a `molt_frozenset_add`
per operand inline (frozenset mutated in place during construction; no
builder/finish). Robust to both the bundled-operand and zero-operand-plus-
separate-`frozenset_add` shapes (mutually exclusive). Verified e2e on LLVM:
`frozenset_basic.py` differential byte-identical; the carrier test's frozenset
shape byte-identical.

### Adjudication vs the #57 audit drift matrix
`tools/op_kinds_baseline.json :: dangerous.llvm_coverage_gap` lists 28 ops
(async/cancel/buffer2d/cast/widen/…) but does **not** include `frozenset_new`,
i.e. the audit's own coverage list missed it. **Follow-up (registry family):**
add `frozenset_new` (now arm-covered) and re-confirm sibling container
constructors (`tuple_new`, list conversions) are arm-covered on LLVM.

---

## Root C — `1<<60` → `None` on LLVM: STALE-BINARY ARTIFACT, not a clean-main bug

### What I first saw (and why it was misleading)
With the **main repo's transient prebuilt binary** (`CARGO_TARGET_DIR=.../target`),
`zz = 1 << 60; print(zz)` produced `None`; `before_opt.ll` showed
`module_set_attr(..., i64 9221964661971222528)` (None) — the raw value
`1152921504606846976` appeared nowhere. Reproduced for module scope, function
return, and large-int-as-arg. That binary, however, was built from an
in-progress / partner-WIP state of the shared `target/` (it was deleted by a
sibling agent's clean mid-session), **not** from clean `origin/main`.

### What clean origin/main actually does
The freshly-built worktree binary (clean `origin/main` `c07cbc6f1` + only my four
edits) renders **all** bigint cases correctly on LLVM, byte-identical CPython:
- `zz = 1<<60; print(zz); print(zz+1); y=zz; print(y)` → `1152921504606846976 / …977 / …976`.
- `def f(): return 1<<60` → `1152921504606846976`.
- `g(1<<60)` → `1152921504606846977`.
- `alias_reassign_bigint` → `(2305843009213693952, 1152921504606846976)`.

My four edits do **not** touch `ConstInt`/`ConstBigInt`/`module_set_attr`
(verified by `git diff` grep) — they are orthogonal to constant lowering. So the
correct attribution is: **clean origin/main already lowers large-int
`ConstBigInt` constants correctly on LLVM** (the `ConstBigInt` arm at
lowering.rs:1112 via `molt_bigint_from_str`). The None I observed was a property
of the stale/partner-modified main-repo binary, not of origin/main source.

### Routing recommendation for the bigint ticket
- **Do NOT add these as typed-IR Phase-2 (#5) acceptance shapes on this
  evidence** — that would design-route a non-bug.
- **Re-verify `alias_reassign_bigint` against a freshly-built CURRENT main**
  (main advanced to ~`c2dad9e89` during this session). If GREEN, close as
  already-resolved. If RED, the regression lives in a *partner's uncommitted WIP*
  (the staged in-place-dunder change on `lowering.rs`, or another in-flight
  branch) — bisect to that, not to a carrier-architecture gap.
- The genuine typed-IR-convergence loop-accumulator cliff (the documented
  `apply(1<<60,7)`-class unbounded-accumulator that S6 RawI64Safe *correctly
  refuses* → needs the dual-loop peel, MEMORY bug#15) is a **separate, real**
  open item and remains Phase-2/peel territory — it is NOT what
  `alias_reassign_bigint` (a straight-line reassign) exercises.

---

## Fixes landed this session (worktree `/tmp/wt_boxedlane`, base `c07cbc6f1` = origin/main)

Staged (NOT committed, NOT pushed). Main-repo `lowering.rs` has **staged
uncommitted partner work** (+96/-10, in-place dunder dispatch) at disjoint
regions (~4869/5091/9817) from these edits — reconcile on integration.

- `runtime/molt-backend/src/llvm_backend/lowering.rs`
  - `emit_call_func_runtime`: arg loop `ensure_i64` → `materialize_dynbox_operand`.
  - `emit_call_bind_runtime`: arg loop `ensure_i64` → `materialize_dynbox_operand`.
  - `call_method` path (`OpCode::Call` → `molt_call_bind_ic`): arg loop boxed.
  - new `"frozenset_new"` arm in `lower_preserved_simpleir_op` (mirrors native).
- `tests/differential/basic/llvm_boxed_lane_call_carrier.py` (new) — named
  acceptance gate for Roots A + B (closure-returns-arg, base+x, method+int-arg,
  sum, format, frozenset, float carrier).

### Gates (all GREEN)
- Build: `cargo build --profile release-fast -p molt-backend --features
  "native-backend llvm"` — clean (2m40s).
- Clippy: `-D warnings`, `native-backend` AND `native-backend llvm` — both rc=0,
  0 warnings.
- Differentials (LLVM + native, byte-identical CPython via `cmp -s`):
  - new `llvm_boxed_lane_call_carrier.py`: LLVM ✓, native ✓.
  - `closure_call_in_defining_scope.py` (9 shapes, #44 follow-up #2): LLVM ✓.
  - `frozenset_basic.py`: LLVM ✓.
  - `call_indirect_dynamic_callable.py`: LLVM ✓.
  - extended multi-arg/kwargs/method-multi-arg/int*float: LLVM ✓.
  - original #58/#37/#61 reduced repros: LLVM ✓ (RED→GREEN).
- Backend lib tests both feature forms: see `tmp/boxedlane/libtest.log`.
- CPython parity ×3: the Python sources are documented byte-identical CPython
  3.12/3.13/3.14; the differential `cmp -s` compares molt output to the CPython
  oracle stdout.

## Task routing
- **Close #58** (Root A fixed + gated).
- **Close #37** (Root A fixed — sum/format/range-int flow through the boxed
  dynamic-call arg path; verify the round-3 report's exact shapes on a fresh
  current-main build).
- **Close #61** (Root B fixed — LLVM `frozenset_new` arm added + gated).
- **bigint-loop-carrier**: re-verify on fresh current main; close if GREEN, else
  bisect into partner WIP. Do NOT fold into #5 on this evidence. Keep the
  *separate* loop-accumulator-peel item (bug#15) open as the real Phase-2 work.
- **Registry follow-up family**: add `frozenset_new` to the #57 audit baseline;
  re-confirm sibling container constructors are LLVM-arm-covered.

## Hazards observed (for the next session)
- The shared main-repo `target/` binary churns under sibling-agent load and was
  *deleted* mid-session. **Do not trust the main-repo prebuilt binary for
  bug attribution** when partner WIP is staged — build a clean worktree from a
  known base and attribute against THAT. (This is exactly what turned Root C from
  a "carrier bug" into a "stale-binary artifact".)
- Main-repo `lowering.rs` has staged uncommitted partner work (in-place dunder).
  Carrier fixes were done in a clean worktree; regions are disjoint.
- LLVM builds materialize a separate `molt-backend.llvm_native_backend` daemon
  binary; the first LLVM build of a session triggers "Backend binary changed;
  restarting daemon".
- The CLI session target is `target/sessions/<id>` (NOT `target-<id>`); an
  explicit-`CARGO_TARGET_DIR` build must be relocated there for the CLI to find
  it (or just let the CLI drive the build).
- Module-chunk (module-init) functions are NOT dumped by `TIR_DUMP` and do NOT
  go through `run_module_pipeline` — a diagnostic blind spot at the TIR level
  (only `before_opt.ll` showed module-init constant state).
