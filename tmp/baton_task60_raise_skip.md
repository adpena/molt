# Baton — Task #60: `if cond: raise` silently skipped (constructor `__init__` full-binding exception swallow)

**Status:** ROOT-CAUSED. Rust runtime bug. NO-CARGO session → not built/landed.
**Severity:** P0 silent control-flow elision / silent exception swallow (wrong results, exit 0 — the worst class).
**Root cause file:line:** `runtime/molt-runtime/src/call/bind.rs:1454` (the lone `molt_call_bind` call site that omits the `exception_pending` check).
**Verified against:** main @ `2b62810020f64173e3b7c829bdd415f51634779d` (origin/main at session start), native target, CPython 3.12/3.13/3.14.
**Differential regression (committed, header-marked known-fail):** `tests/differential/basic/constructor_init_variadic_raise.py` (`# MOLT_META: xfail=molt xfail_reason=task60_init_full_binding_exc_swallow`).

---

## 1. The real bug (NOT what the original report said)

The task framing (from the membership-family agent) was: *"`if <membership> not in {...}: raise` followed by a `**kwargs`-splat call → raise silently skipped."* That framing is **misleading**. The boundary matrix below proves:

- **Membership is a red herring.** `if self.ea == "bad": raise` (plain `==`) skips identically.
- **The `**kwargs`-SPLAT CALL is a red herring.** A plain call after the raise, or NO statement after the raise, skips identically. Even an UNCONDITIONAL `raise` (no `if` at all) as the first statement skips.
- **The two load-bearing factors are:** (1) the function is a **constructor `__init__`**, AND (2) `__init__` **requires FULL ARGUMENT BINDING** — i.e. it has a variadic parameter (`*args` / `**kwargs`) OR a keyword-only parameter.

Concretely: **an exception raised anywhere inside a full-binding `__init__` is silently swallowed at the `ClassName(...)` construct boundary when the construct's result is consumed by a `check_exception` (a `try`, or the implicit post-construct check).** The `__init__` body *does* run and the raise *does* execute (proven by printing inside the body), but the exception never reaches the caller's handler.

### Minimal reproducer (`q3` / 9 lines)
```python
class W:
    def __init__(self, ea, **kw):
        if ea == "bad":
            raise ValueError("bad")
try:
    W("bad")
except Exception as exc:
    print(type(exc).__name__)
print("end")
# CPython 3.12/3.13/3.14: "ValueError\nend"   (exit 0)
# molt native:            "end"                 (exit 0)  <-- raise SWALLOWED
```

### Even more minimal (`u2c`) — unconditional raise, `**kw` only
```python
class W:
    def __init__(self, **kw):
        raise ValueError("x")
try:
    W()
except Exception as exc:
    print(type(exc).__name__)
print("end")
# molt: "end"  (the ValueError is swallowed)
```

### Body-runs proof (`u2b`)
```python
class W:
    def __init__(self, ea, **kw):
        print("body-ran")
        raise ValueError("always")
        print("after-raise")
try:
    W("x")
    print("construct-returned-normally")
except Exception as exc:
    print("caught:" + type(exc).__name__)
print("end")
# molt prints:  body-ran / construct-returned-normally / end
#   => __init__ ran, the raise executed, but the exception did NOT propagate;
#      the construct returned the (partial) instance as if it succeeded.
```

---

## 2. Boundary matrix (native, vs CPython 3.14, exit-code + behavior)

`SKIP` = molt swallows the raise (BUG). `FIRE` = molt raises correctly (matches CPython).

| # | shape | molt |
|---|-------|------|
| core | `def f(k,**kw): if k not in {..}: raise; return g(x=1,**kw)` (plain func) | FIRE |
| a | `def f(k): if k not in {..}: raise` (plain func, no kw) | FIRE |
| n1 | plain func, `**kw`, `not in {..}` raise, `**kw`-splat after | FIRE |
| s2 | plain func, `**kw`, `== "bad"` raise | FIRE |
| s3 | plain func, no kw, `== "bad"` raise | FIRE |
| q1 | `__init__(self, ea)` (simple), `not in {..}` raise | FIRE |
| **q3** | **`__init__(self, ea, **kw)`, `== "bad"` raise** | **SKIP** |
| **u2c** | **`__init__(self, **kw)`, unconditional raise** | **SKIP** |
| **u2d** | **`__init__(self, *args)`, unconditional raise** | **SKIP** |
| **v3** | **`__init__(self, *, ea)` (keyword-only), raise** | **SKIP** |
| **S** | **the exact csv `DictWriter.__init__` shape (attr=x.lower(); not-in-set; raise; then `**fmtparams` call)** | **SKIP** |
| v1 | regular method `m(self, x, **kw)`, raise (NOT `__init__`) | FIRE |
| v2 | `__new__(cls, *a, **kw)`, raise | FIRE |
| v4 | `__init__(self, a,b,c,d,e,f)` (6 positional, NO full binding), raise | FIRE |
| x2 | v4 wrapped in try | FIRE (caught) |
| B | `__init__(self, **kw)` that does NOT raise | OK (constructs fine) |
| w1/x1 | full-binding `__init__` raise, NO `try` (bare construct) | FIRE (propagates to top-level abort) |

Key discriminators proven by the matrix:
- `__init__` vs any other callable: `v1` (method+`**kw`) and `v2` (`__new__`+`**kw`) FIRE; only `__init__` SKIPs.
- full-binding vs not: `v4` (6 positional, excluded from fast path by arity but NOT full-binding) FIRES; `v3` (keyword-only, full-binding) SKIPs.
- the bug needs the construct result consumed by a `check_exception`: a bare `W()` with no `try` (`w1`/`x1`) DOES propagate (the pending flag is set; the top-level harness aborts). Inside a `try` (`q3`/`u2*`/`S`) the handler is bypassed. So the pending flag *is* set; the construct-site propagation simply does not act on it because the construct returned a live instance.

---

## 3. Frontend vs backend verdict: BACKEND (runtime), with IR evidence

**The frontend IR is correct.** `--emit-ir` on `q3` shows `W.__init__` lowered with the raise branch present and reachable:
```
[ 8] const_str  out=v106 s_value=bad
[ 9] eq         out=v107 args=[ea, v106]
[11] if         args=[v107]
[12] line  [13] const_str "bad"  [14] exception_new_builtin_one out=v109 ValueError  [15] raise v109
[16] end_if
```
The module-body IR for the construct site is ALSO correct — it emits the post-call exception check:
```
[49] callargs_new   out=v122
[50] call_bind      out=v123 args=[v119, v122]    ; W() construct
[51] store_var      var=_bb1_arg0 ...
[52] check_exception value=5                        ; <-- the post-construct check IS emitted
```
So the frontend lowers `try: W()` → construct → `check_exception` → handler correctly. The bug is entirely in the **runtime `call_bind` constructor path** not surfacing the `__init__` exception so that the (correctly-emitted) `check_exception` and the IC propagation guards act on it.

(Codegen-level note: dumping the CLIF for `q3.__init__` vs `q1.__init__` vs `s2.f` shows IDENTICAL `eq`/`if`/`br_if` structure — the per-function codegen is fine; this is not a phi/branch/eq miscompile. The divergence is purely in *how the constructor invokes `__init__` and propagates its exception*.)

---

## 4. Exact root cause

`runtime/molt-runtime/src/call/bind.rs`, function `call_type_with_builder` (defined at line 689), the `InitArgPolicy::ForwardArgs` arm:

```rust
// bind.rs ~1441-1456
            InitArgPolicy::ForwardArgs => {}
        }
        if builder_ptr.is_null() {
            dec_ref_bits(_py, init_bits);
            return inst_bits;
        }
        builder_guard.release();
        let args_ptr = callargs_ptr(builder_ptr);
        if !args_ptr.is_null() {
            inc_ref_bits(_py, inst_bits);
            (*args_ptr).pos.insert(0, inst_bits);   // inject self
        }
        let _ = molt_call_bind(init_bits, builder_bits);   // <-- calls __init__ (full binding)
        dec_ref_bits(_py, init_bits);
        inst_bits                                          // <-- returns the INSTANCE unconditionally
```

`resolved_constructor_init_policy` (runtime/molt-runtime/src/call/type_policy.rs:66-86) returns `ForwardArgs` for **every user-defined `__init__`** (anything that is not `object.__init__`). But:

- A **simple** (non-full-binding, arity ≤ 5) `__init__` is intercepted earlier by the IC fast path (`call_bind_ic_entry_for_call`, bind.rs:2506-2536 — gated by `function_requires_full_binding` at 2514 and arity `1..=5` at 2520) and called via the fast path at bind.rs:2740-2826, which **does** `if exception_pending(_py) { dec_ref_bits(inst_bits); return Some(none) }` (lines 2822-2826). → `q1`, `Simple` FIRE.
- A **full-binding** `__init__` (`*args`/`**kwargs`/keyword-only) is rejected by the fast path (line 2514 `function_requires_full_binding` → `return None`) and falls through to `call_type_with_builder` → the `ForwardArgs` arm above → **line 1454, which has NO `exception_pending` check and unconditionally returns `inst_bits`.**

Because `call_type_with_builder` returns a real instance (not `none`) with the exception still pending, the IC-dispatch propagation guards downstream (`if obj_from_bits(res).is_none() && exception_pending(_py)` at bind.rs:2091, 2131, 2205) do not fire — they only propagate when the result is `none`. The instance is bound to the target, execution continues into the `try` body's success path, and the pending flag is later cleared by the next op's exception baseline. Net effect: the raise is silently dropped inside a `try`/`check_exception` context.

### Why line 1454 is provably the bug (asymmetry argument)
`grep -n "molt_call_bind\|exception_pending" bind.rs` shows that **every** other `molt_call_bind` call site is immediately followed by an `if exception_pending(_py)` check:
- 100 → 102 (`build_class_from_args` / Meta path)
- 792 → 795, 835 → 837 (`__new__` / Meta.`__init__`)
- 1279 → 1281, 1383 → 1385 (other construct paths)
- 1591 → 1618 (metaclass winner)
- 2786 / 2816 → 2822 (the simple-`__init__` fast path)

**Line 1454 is the ONLY `molt_call_bind` site followed by an unconditional return with no pending check.** This is a textbook asymmetric-coverage omission (cf. CLAUDE.md "asymmetric coverage of a structural fix").

---

## 5. Fix design

Mirror the fast-path pattern (bind.rs:2822-2826) and the universal sibling pattern (bind.rs:100-104) at line 1454. After `molt_call_bind` returns, check the pending flag; if set, drop the instance reference and return the `none` sentinel so the construct yields `none` (which the downstream IC propagation guards and the frontend `check_exception` both correctly act on):

```rust
        let _ = molt_call_bind(init_bits, builder_bits);
        dec_ref_bits(_py, init_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, inst_bits);
            return MoltObject::none().bits();
        }
        inst_bits
```

Notes:
- `exception_pending`, `dec_ref_bits`, and `MoltObject` are all already imported/used throughout this file (e.g. lines 2822-2824 do exactly `dec_ref_bits(_py, inst_bits); return Some(MoltObject::none().bits())` — here the function returns bare `u64`, so return `MoltObject::none().bits()` directly, matching the `builder_ptr.is_null()` early-returns in the same function that return `inst_bits` as a bare `u64`).
- Ownership: the `ForwardArgs` arm did `inc_ref_bits(_py, inst_bits)` before injecting `self` into `args.pos[0]` (line 1451). `molt_call_bind` consumes the callargs (and thus that injected `self` ref) during binding/cleanup — verify this against `callargs_dec_ref_all` ownership so the extra `dec_ref_bits(inst_bits)` on the error path balances exactly the way the fast path's `dec_ref_bits(inst_bits)` (line 2823) balances its `alloc_instance_for_default_object_new`. The fast path is the ownership template: confirm the `ForwardArgs` instance has the same single owning ref at this point (the constructor's return ref) and that the error path releases it once. Do NOT double-drop.

### STRUCTURAL completeness (do not ship the one-liner alone)
The one-line check at 1454 fixes the observed bug, but the deeper structural smell is that `call_type_with_builder`'s `ForwardArgs` arm hand-rolls the `__init__` invocation + ownership separately from the fast path, and the two have drifted (the fast path checks exceptions; this one doesn't). Two options, pick the structurally correct one:

1. **Preferred (eliminate the divergence):** factor the "call `__init__`, on pending-exception drop the instance and yield `none`, else yield the instance" sequence into ONE helper used by BOTH the fast path (2740-2826) and the `ForwardArgs` arm (1447-1456). This removes the parallel source of truth that let them drift. This is the Lattner/NASA move — fix the abstraction, not the instance.
2. **Acceptable minimum:** add the check at 1454 AND add a debug `MOLT_VERIFY`-gated assertion (or a unit test) that asserts the invariant "after any `molt_call_bind` of a constructor `__init__`, the constructor path returns `none` iff `exception_pending`" so the two paths can't silently re-diverge.

Whichever path: the differential test below + a runtime unit test in `bind.rs`'s `#[cfg(test)] mod tests` (there is one at ~line 4741) exercising a full-binding `__init__` that raises, asserting `exception_pending` after the construct and a `none` return, must land in the SAME change.

---

## 6. Regression spec (the boundary matrix as the test)

Committed: `tests/differential/basic/constructor_init_variadic_raise.py`, header-marked
`# MOLT_META: xfail=molt xfail_reason=task60_init_full_binding_exc_swallow` so the suite-honesty
ratchet (#46) tracks it LOUD: it must flip to PASS (and the `xfail` line be removed) in the fix commit,
or the ratchet will xpass-fail.

The test covers, in one file, the full matrix:
- BUG cells (must start failing, then pass after fix): `K` (`**kw` conditional raise = csv shape),
  `R` (`**kw` unconditional raise), `A` (`*args`), `O` (keyword-only), `S` (the verbatim csv
  `DictWriter.__init__` body incl. the trailing `**fmtparams` splat call).
- CONTROL cells (must pass before AND after — guard against the fix perturbing them): `Simple`
  (simple `__init__`), `SixArg` (6-positional non-full-binding), `Meth` (regular method+`**kw`),
  `NewRaise` (`__new__`+variadic), `B` (non-raising `**kw` `__init__`).

CPython 3.12/3.13/3.14 output is byte-identical (verified). Expected CPython output:
```
K caught: K: bad
R caught: R: always
A caught: A: args
O caught: O: kwonly
S caught: extrasaction (bad) must be 'raise' or 'ignore'
S ok: raise
B ok: True
Simple caught: Simple: bad
SixArg caught: SixArg
Meth caught: Meth.m
NewRaise caught: NewRaise
end
```
Current molt output (the divergence): cells K/R/A/O/S print `<X>: NO RAISE (bug)` instead of `<X> caught: ...`; all control cells already match.

Determinism: `q3_bin` ran 5/5 identical (`end`). The bug is fully deterministic (no hash-order dependence).

### Parity across targets
This fix is in the shared runtime (`molt-runtime`), so it covers native, WASM, LLVM, and Luau uniformly (all four link the same `call_type_with_builder`). The differential test header says "every target"; the fix author should still spot-check WASM (`--target wasm`) and LLVM once built, but no per-backend work is expected — the divergence is target-independent runtime logic.

---

## 7. Relationship to adjacent landed work (cross-reference)

- **#52/#43 (membership-family, `f6c946b5c`)** is a DIFFERENT, already-fixed bug. That was the
  `representation_plan` mistyping a `set`/`dict` constructor result as its first element (Copy
  passthrough aliasing) → wrong `contains` intrinsic. The original task #60 report conflated the two
  because both first surfaced via `csv` extrasaction; but #52 was the `writerow` membership crash/
  wrong-result, while #60 is the `__init__` exception swallow. `membership_container_dispatch.py`
  (landed with `f6c946b5c`) is byte-identical to CPython at HEAD — #52 is genuinely fixed.
- **C2 (`430e09793`, needs_exception_stack polarity)** and the **exception-CFG work (`2a450ecfe`)**
  are NOT the cause here — the frontend correctly emits `check_exception` after the construct, and the
  needs-exception classification is fine (bare construct propagates). The defect is purely the runtime
  constructor path failing to surface the exception so those mechanisms can act.
- **`2b6281002` (CallArgs double-free + cross-batch closure metadata)** touched the same
  `call_bind`/`callargs` ownership area; the fix here must be reconciled with that commit's ownership
  model (PtrDropGuard / `callargs_dec_ref_all`) — see the ownership note in §5.
- Sibling pattern lesson (the iter-consume `effects.rs` keystone): same family of "the authoritative
  flag is set but a code path fails to *act* on it." Here the authoritative flag is
  `exception_pending` and the failing actor is the `ForwardArgs` constructor return.

---

## 8. Repro / build recipe (NO-CARGO; reuse the solo prebuilt binary)

The session daemon/build plumbing fights backgrounding (exit 144 detach) and per-session target dirs
lack the prebuilt runtime. What works:

```bash
unset MOLT_SESSION_ID                                    # solo mode -> uses target/ (fully populated)
export PYTHONPATH=/tmp/wt_raiseskip/src
export MOLT_SKIP_RUNTIME_REBUILD=1
export MOLT_STDLIB_PROFILE=micro                         # reuses libmolt_runtime.stdlib_micro.a (exists);
                                                         # `full` would force a Rust compile (no full archive cached)
# build FOREGROUND wrapped in safe_run (background molt builds get killed by the harness):
python3 tools/safe_run.py --rss-mb 4096 --timeout 90 -- \
  .venv/bin/python -m molt build --target native --output /tmp/out PROG.py
python3 tools/safe_run.py --rss-mb 2048 --timeout 15 -- /tmp/out   # run via safe_run (NEVER bare)
```

Diagnostic env vars that ARE forwarded to the daemon (cli.py allowlist ~line 180-212):
`TIR_DUMP=1`, `MOLT_DUMP_IR=full:<fn>` (post-rewrite SimpleIR), `MOLT_DUMP_CLIF_FUNC=<fn>` (final
CLIF), `MOLT_DISABLE_INLINING=1`. NOT forwarded: `MOLT_DUMP_FUNC_IR`, `MOLT_DUMP_REWRITTEN_FUNC`.
Use `--rebuild` to bypass the artifact cache when changing dump filters. The frontend IR JSON is
`molt build --emit-ir PROG.ir.json` (top-level key `functions`; each has `name`/`params`/`ops`).
Mangled name for class `C` method `__init__` in `prog.py` = `prog__C___init__`.

---

## 9. Definition of done

1. Fix `bind.rs:1454` (preferably via the shared-helper refactor in §5 option 1).
2. Add a `bind.rs` unit test (full-binding `__init__` raises → construct returns `none` + `exception_pending`).
3. `constructor_init_variadic_raise.py` flips to PASS; REMOVE its `# MOLT_META: xfail=...` line in the same commit.
4. Verify byte-identical vs CPython 3.12/3.13/3.14 on native; spot-check WASM + LLVM.
5. Run the suite-honesty ratchet + the differential basics; confirm no control-cell regression and
   `MOLT_ASSERT_NO_LEAK` clean (the error path adds a `dec_ref_bits` — leak-check it).
6. Gates: backend lib tests, runtime tests, compliance.
