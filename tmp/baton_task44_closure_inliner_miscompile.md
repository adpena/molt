# Baton: task #44 — closure called within its defining scope miscompiles (TIR inliner)

**Status:** ROOT-CAUSED + repro-locked + regression test landed. Fix is a small,
surgical Rust change in the TIR inliner — held because the investigating agent was
NO-CARGO (both build slots taken; building `molt-backend` was forbidden). The fix
needs a `molt-backend` recompile + the standard gate run. Everything needed to land
it in one sitting is below.

**Reproduces on:** origin/main `ce7303090` (current HEAD as of this session).
The baton's earlier "where to look" (`function_compiler.rs` CALL_GUARDED env
extraction) was a RED HERRING — that path is correct. The real culprit is the TIR
inliner.

---

## 1. The bug

```python
def outer(base):
    def add(x):
        return base + x      # captures `base`
    return add(10)           # called INSIDE outer
print(outer(5))              # molt: TypeError: 'function' object is not subscriptable; CPython: 15
```

A nested function that **captures an enclosing variable** (so it is a closure) and
is **called within its defining scope** miscompiles. Returning the closure and
calling it from outside works.

## 2. Shape boundary (all built no-cargo via the harness in §6; CPython 3.12/3.13/3.14 identical)

| # | shape | molt (inline ON) | CPython | in bug class? |
|---|-------|------------------|---------|---------------|
| a | plain capture + call-in-scope | **FAIL** TypeError | 15 | YES |
| b | NO capture + call-in-scope (`def f(x): return x*2; f(3)`) | OK 6 | 6 | no (see §4) |
| c | capture, assign to alias, call alias | **FAIL** | 15 | YES |
| d | capture, call AND return | **FAIL** | 6/105 | YES |
| e | nested two levels (mid→inner) | OK 115 | 115 | latent (not spliced today) |
| f | capture, call in a loop | **FAIL** | 18 | YES |
| g | method-local closure (`C().m(5)`) | **FAIL** | 15 | YES |
| h | async-def enclosing | OK 15 | 15 | excluded (StateBlock) |
| i | capture, RETURNED, called outside | OK 15 | 15 | no (different call path) |
| j | no-arg capture + call-in-scope | **FAIL** | 6 | YES |
| k | capture a local (not a param) | **FAIL** | 15 | YES |

**Every failing shape produces byte-identical CPython output once the inliner is
disabled** (verified with `MOLT_DISABLE_INLINING=1`). That single toggle is the
proof the inliner is the sole cause; the frontend IR is correct.

## 3. Root cause — `runtime/molt-backend/src/tir/passes/inliner.rs`

The frontend lowers a closure with its captured environment passed as an implicit
FIRST parameter named `__molt_closure__` (frontend constant
`_MOLT_CLOSURE_PARAM = "__molt_closure__"`, `src/molt/frontend/_types.py:468`;
prepended at e.g. `src/molt/frontend/__init__.py:3998`
`params = [_MOLT_CLOSURE_PARAM] + params`). So `__main____add` has
`param_names = ["__molt_closure__", "x"]` (verified by dumping
`compile_to_tir(...)`).

The call `add(10)` lowers (correctly) to:
```
load_var add -> v139
call_guarded s_value="__main____add" args=["v139", "v140"]   # v140 = 10
```
i.e. a TIR `Call` whose **operands are `[callee_function_value, arg0]`** and whose
`s_value` names the callee. The closure body (also correct) does
`index ["__molt_closure__", 0] -> cell; index [cell, 0] -> base`.

Now the inliner:

1. **`collect_call_sites`** (inliner.rs:302) treats any `OpCode::Call` whose
   `s_value` names a module-defined function as an inline candidate. Closures ARE
   module-defined (`run_inliner` collects all functions as `defined`, line 1085),
   so `__main____add` is a candidate. **No closure exclusion.**

2. **`is_inlineable`** (inliner.rs:219) gates on recursion, exception handlers
   (228), generator/async state ops (248), entry-block predecessors (255), and the
   op-count budget (262). **It does NOT exclude closures.** A closure passes.

3. **`splice_call_site`** (inliner.rs:796) reads `op.operands` as `call_args`
   (line 807 = `[v139(func), v140(10)]`, len 2), then the **arity guard**
   `callee_entry.args.len() != call_args.len()` (line 821) compares to the
   callee's param count. `__main____add` has 2 params (`__molt_closure__`, `x`),
   `call_args` has 2 operands → **2 == 2, guard PASSES (false match).** It then
   binds params 1:1 to operands (line 407):
   `value_map[__molt_closure__] = v139 (the FUNCTION OBJECT)`,
   `value_map[x] = v140 (10)`.

Inside the inlined body, `__molt_closure__` is now the `add` function object, so
`index [__molt_closure__, 0]` subscripts a function → **`TypeError: 'function'
object is not subscriptable`**.

### The exact defect

The inliner conflates the **call ABI** (`Call.operands = [callee_value, arg0,
arg1, ...]`) with the **callee parameter list**. For a NON-closure those differ by
one (callee has N params; the Call carries N+1 operands incl. the function value)
so the arity guard at line 821 *accidentally* refuses them (this is why shape **b**
works). A closure adds exactly one parameter (`__molt_closure__`), re-balancing the
counts and defeating the only guard that was protecting correctness. The arity
guard is load-bearing for the wrong reason and silently fails for closures.

## 4. Why b/e/h don't fail today (so you don't "fix" a non-bug)

- **b (no capture):** callee `__main____f` params `["x"]` (len 1); Call operands
  `[func, 10]` (len 2) → arity mismatch → refused → correct. (This is the
  accidental guard, not intent.)
- **h (async):** `is_inlineable` refuses StateBlock/async ops (lines 248–254).
- **e (nested):** same closure SHAPE as `a` (`mid`/`inner` both
  `[__molt_closure__, arg]`, 2-operand Call) but happens not to be spliced today
  (call-site arg materialization / ordering). It is a **latent** member of the bug
  class — the fix must cover it, and the regression test includes it.

## 5. The fix (structurally correct, minimal — matches this file's own pattern)

`is_inlineable` already uses **conservative-correct exclusion** for shapes the
splice cannot safely handle (exception handlers line 228, generator/async ops line
248). A closure whose env-param the splice cannot bind is exactly such a shape.
Add one gate to `is_inlineable` (inliner.rs:219), before the budget check:

```rust
// A closure carries its captured environment as an implicit first parameter
// (`__molt_closure__`, prepended by the frontend). The direct param->operand
// splice binds that env-param to the call's leading FUNCTION-VALUE operand
// (Call.operands = [callee_value, args...]) instead of the captured env,
// miscompiling `__molt_closure__[i]` into a subscript of the function object
// ('function' object is not subscriptable). Threading the real env (extract via
// the function object's closure bits and bind it to param[0]) is a separate
// perf arc; until then, refuse. Conservative-correct exclusion.
if callee
    .param_names
    .first()
    .is_some_and(|p| p == crate::MOLT_CLOSURE_PARAM_NAME /* "__molt_closure__" */)
{
    return false;
}
```

`TirFunction::param_names` is populated on the production lift
(`lower_from_simple.rs:415 param_names: ir.params.clone()`), aligned 1:1 with
entry-block args, and reliably contains `__molt_closure__` for closures (the
`p{idx}` default in `TirFunction::new` is test-only). Define the literal
`"__molt_closure__"` once as a shared backend const (mirror the frontend's
`_MOLT_CLOSURE_PARAM`) rather than inlining the string, so the two sides stay a
single source of truth — do NOT hardcode the bare string at the use site.

**Precedent (use the exact same detection):** the WASM backend ALREADY identifies a
closure by this marker — `runtime/molt-backend/src/wasm.rs:2398`:
```rust
let default_has_closure = func_ir.params.first()
    .is_some_and(|name| name == "__molt_closure__");
```
(it then subtracts 1 from the arity for the env param). The new `is_inlineable`
gate is the same predicate; factor the `"__molt_closure__"` literal into one shared
const and have BOTH wasm.rs:2398 and the inliner reference it (kills the duplicate
raw string — the only two backend uses of it).

**Coverage / asymmetry note (CLAUDE.md):** the gate belongs in `is_inlineable`,
which is the single chokepoint `run_inliner` filters through (line 1104). Confirm
no OTHER inline entry point bypasses `is_inlineable` (grep `splice_call_site`
callers). The native SimpleIR inliner (`passes.rs` / `simple_backend.rs:2689`) is
a SEPARATE inliner — verify whether it has the same closure-splice hazard and, if
so, apply the mirrored exclusion there (asymmetry trap). At the time of writing the
TIR inliner is the one that produced this miscompile on native.

### Optional follow-up (perf, NOT required for correctness)
To actually inline closures: at the splice, treat `operands[0]` as the callee
function value, extract its captured env (the TIR equivalent of
`molt_function_closure_bits`, runtime `builtins/callable.rs:186`) into a fresh
value, bind THAT to `__molt_closure__`, and bind `operands[1..]` to the remaining
params. Mind the +0-borrowed refcount convention (invariant 2 in the file header)
for the synthesized env value. Land only with its own differential coverage.

## 6. No-cargo repro/verify harness (how this was characterized without building)

The investigating agent could not recompile `molt-backend`. The verified-current
solo binary `target/release-fast/molt-backend` (no source newer — checked) was
reused via the supported escape hatch:

```bash
# Build a repro to a native binary WITHOUT triggering cargo (uses the warm binary):
cd /Users/adpena/Projects/molt              # main repo; frontend == origin/main
unset MOLT_SESSION_ID                        # solo mode -> target/release-fast
export MOLT_SKIP_RUNTIME_REBUILD=1           # cli.py:24832 — skip fingerprint/rebuild
python3 -m molt build --target native --output /tmp/out prog.py --rebuild
python3 tools/safe_run.py --rss-mb 1024 --timeout 10 -- /tmp/out   # guarded run

# Toggle the inliner to confirm cause:
MOLT_DISABLE_INLINING=1 python3 -m molt build ... --rebuild   # -> all shapes pass

# Dump the exact frontend IR (proves frontend is correct, no build needed):
PYTHONPATH=/path/to/src python3 -c \
 'import json,molt.frontend as f; print(json.dumps(f.compile_to_tir(open("prog.py").read())))'

# Dump CLIF straight from the backend binary (bypasses the daemon):
MOLT_DUMP_CLIF_FILE=/tmp/x.clif MOLT_DUMP_CLIF_FILE_FILTER=__main____outer \
  python3 tools/safe_run.py --rss-mb 2048 --timeout 30 -- \
  ./target/release-fast/molt-backend --ir-file prog.ir.json --output /tmp/x.o
```
NOTE: setting `MOLT_SESSION_ID` points at an empty `target/sessions/<id>` and
forces a full cranelift recompile — that is what to avoid under the no-cargo
constraint. Solo + `MOLT_SKIP_RUNTIME_REBUILD=1` reuses the warm binary.

## 7. Regression test (LANDED this session)

`tests/differential/basic/closure_call_in_defining_scope.py` — covers shapes
a/c/d/f/g/j/k (failing class) + b/e plus a 2-capture `multi_capture` and a
method-local closure. CPython 3.12/3.13/3.14 all emit:
```
15
6
15
15
111
18
115
14
15
```
Verified: FAILS today with inlining ON (`TypeError: 'function' object is not
subscriptable`); BYTE-IDENTICAL to CPython with `MOLT_DISABLE_INLINING=1` (the
fixed behavior). After landing the §5 gate, this test passes with inlining ON and
must be wired into the differential suite.

## 8. Landing checklist for the cargo-capable agent

1. Add the §5 closure-exclusion gate to `is_inlineable` (inliner.rs:219) + a
   shared `__molt_closure__` backend const.
2. Audit `splice_call_site` callers / the SimpleIR inliner for the same hazard;
   mirror the exclusion if present (no asymmetry).
3. `cargo build --profile release-fast -p molt-backend --features native-backend`.
4. Build + run `tests/differential/basic/closure_call_in_defining_scope.py` on
   native WITH inlining (default) → must match CPython byte-for-byte (`cmp -s`,
   NOT rtk diff).
5. Backend lib tests + the inliner unit tests (inliner.rs has a `tests` module —
   add a `is_inlineable` unit test asserting a closure callee is refused).
6. Determinism: `pytest tests/determinism/test_ir_determinism.py -q` (the gate is
   read-only over `param_names`, so it should stay green — confirm).
7. Verify perf parity: the gate only REFUSES inlining a small class; confirm no
   benchmark regresses (closures-called-in-scope were being MIScompiled, not
   fast). If a real perf need appears, do the §5 env-threading follow-up with its
   own coverage.
