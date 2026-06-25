# Defaults-bearing method/function devirtualization with a __defaults__-mutation deopt guard

## Problem (measured)
- microbench `obj.m(i)` / `def m(self,x,bump=1)`: dormant molt ~0.88s vs CPython 3.14 ~0.19s.
  Cause: defaults-bearing methods are NOT devirtualized to a direct compiled CALL; they go
  through runtime IC dispatch (`call_method_ic_dispatch`). No-default methods ARE devirtualized.
- PRE-EXISTING DIVERGENCE (confirmed): module-level `add(5)` after `add.__defaults__=(100,)`
  gives molt 6 / CPython 105. `_apply_default_specs` bakes CONST defaults at compile time via
  `_emit_const_value`, ignoring runtime `__defaults__`/`__kwdefaults__` reassignment.
  Same for `__kwdefaults__` (scale(3) -> molt 30 / CPython 12).

## Observable to preserve
CPython binds `__defaults__`/`__kwdefaults__` at CALL time. A short call must use the LIVE
default tuple/dict, even after reassignment, even mid-loop. The reentrancy tripwire
(method_defaults_reentrancy.py) pins the dynamic path's live-read.

## Design: version stamp + guarded baked-const default values

### Runtime: defaults version stamp on the function object
- Function object layout currently 10 u64 slots (0..9). APPEND slot 10 = `defaults_version: u64`.
  - `alloc_function_obj`: total = header + 11*u64; init slot 10 = 0.
  - layout.rs: `function_defaults_version(ptr) -> u64` (read slot 10);
    `function_bump_defaults_version(ptr)` (slot 10 += 1, saturating not needed — u64 wraps only
    after 2^64 mutations; treat 0 as "never mutated").
  - Version slot is a PLAIN u64 (no refcount) -> dealloc (object/mod.rs TYPE_ID_FUNCTION) UNCHANGED.
- CREATION sets defaults via `molt_function_init_metadata{,_packed}` / `molt_function_set_defaults`
  -> these go through `function_set_attr_bits` (low-level). DO NOT bump there: creation keeps
  version == 0 (the baked-literal-valid invariant).
- MUTATION (user `f.__defaults__ = x` / `f.__kwdefaults__ = x`): the ONLY user-reachable path is
  `molt_set_attr_generic` TYPE_ID_FUNCTION branch (attributes.rs ~3565, the generic dict write).
  After `dict_set_in_place`, if attr_name is `__defaults__` or `__kwdefaults__` -> bump.
  - `molt_function_set_defaults` (functions.rs:4979) is a CREATION helper (frontend
    `_emit_function_defaults` at def-execution) — NOT a mutation — so it does NOT bump.
    (Verified: classes.py:198 emits it for the def's own defaults.)
  - Mirrors the existing `class_bump_layout_version` precedent (attributes.rs:3179).

### Runtime: cheap version read for compiled code
- New op-kind `function_defaults_version` -> TIR opcode `FunctionDefaultsVersion` -> backend lowers
  to a single slot-load returning the version as an inline int (MoltObject int) for the compare.
  side_effecting = TRUE (like ExceptionPending): the value can change mid-loop (reentrancy /
  mid-loop reassignment), so it MUST NOT be LICM-hoisted or CSE'd across a potential mutation.
  Cost: one load + one compare per call — the PIC guard. The func-object LOAD (class attr) IS
  loop-invariant and hoists; only the version read+compare is per-iteration (cheap).
  - DECISION: reuse the BUILTIN_FUNC + CALL_FUNC intrinsic path is REJECTED — CALL overhead per
    iteration is too high and CALL is opaque to the value compare. A dedicated pure-read op marked
    side_effecting is the correct primitive.

### Frontend: guard the baked-const default VALUES (single choke point)
`_apply_default_specs` (calls.py:440) is the ONE place const defaults are baked. Change ONLY the
const-default arm:
- Before baking, obtain the function object (caller passes `func_obj`; for method/module direct
  calls it is the class/module attr load — already plumbed for the non-const arm).
- If `func_obj` is available AND there is at least one const default being filled:
  emit `version = FunctionDefaultsVersion(func_obj)`; `is_pristine = (version == 0)`.
  For EACH missing const default value:
    `v = IF is_pristine -> baked_const ELSE -> live __defaults__[idx] (or __kwdefaults__[name]); PHI`.
  The direct CALL stays a direct CALL (the devirt win); only default VALUES are guarded.
- If `func_obj` is NOT available (cannot load the function — e.g. truly anonymous): keep baking the
  const unguarded ONLY when the callee identity guarantees no mutation is observable... but to be
  SOUND we instead require func_obj for any const-default guard; if it cannot be obtained, fall back
  to the existing live-read requirement (raise unsupported is current behavior for non-const w/o
  func_obj). For const w/o func_obj we keep the existing bake (callers that lack a function object
  are def-site-local lambdas/closures whose `__defaults__` is not user-reachable by name) — AUDIT.

This single change:
1. Heals the module-level divergence (every `_apply_default_specs` const bake is now guarded).
2. Enables the method-static-call fast path: `_try_emit_user_method_static_call` stops bailing on
   `defaults`/`kwonly`, routes through `_apply_default_specs` (producing guarded args), then emits
   the SAME direct CALL it already emits for no-default methods.

### Method static-call path (`_try_emit_user_method_static_call`, calls.py:2164)
- Lift the `defaults` (2230) and `kwonly_count` (2228) bails for the direct-CALL (non-inline) path.
- Keep bails: vararg, varkw, closure (non-inline), descriptor!=function, getattr/getattribute,
  keywords, starred args.
- Load the function object from the class (`_emit_module_attr_get_on(module, class_name)` +
  `_emit_class_method_func(class_ref, method_name)`) for the version read + live fallback.
- Compute missing = (param_count-1) - len(args); route through `_apply_default_specs` with
  implicit_self handled (self already excluded from defaults). Then emit the direct CALL to
  `method_symbol` with `[receiver] + padded_args`.
- positional_limit from kwonly_count so over-supply falls back.

## Cross-backend
- `FunctionDefaultsVersion` + IF/ELSE/PHI lower on native (Cranelift) first-class. IF/ELSE/PHI
  already portable. The new op needs a lowering in native; WASM/LLVM either implement the same
  slot-load or the op is gated so those backends keep the dynamic path (documented). Target:
  native first-class this change; LLVM/WASM same slot-load (it is a trivial pointer-offset load).

## Soundness checklist
- creation version == 0 -> baked path taken (fast) until first mutation. ✓
- any `__defaults__`/`__kwdefaults__` reassignment -> version != 0 -> live read. ✓ (heals divergence)
- mid-loop mutation -> version read per-iteration (side_effecting) sees the bump. ✓
- reentrancy (mutate inside the method) -> the in-flight call already padded its args before the
  store; the NEXT call reads version != 0 -> live. The tripwire's RC retention is on the dynamic
  path which is unchanged. ✓
- explicit arg supplied (no padding) -> `_apply_default_specs` returns early (missing<=0), no guard,
  direct CALL unchanged. ✓
