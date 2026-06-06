# Task #50 — generic_class differential SIGSEGV — ROOT CAUSE + FIX DESIGN

Status: **Root cause definitively proven. Fix is FRONTEND (Python). NO-CARGO agent — fix not landed; this baton + a known-fail differential regression are committed.**
Base commit: `ce730309007e7c8da820b78f8fd5d5bc52d7e0df` (origin/main at investigation time).
Verified pre-existing and **chain-independent** (frontend lowering bug; not asyncio-related).

---

## TL;DR

`molt build --target native tests/differential/basic/generic_class.py` SIGSEGVs
(`EXC_BAD_ACCESS @ 0xfffffffffffffff0`, exit -11). The crash is a NULL-pointer
deref of a Python object header inside the inlined-constructor **direct field-store
fast path**. The object is NULL because the **class reference passed to the
constructor allocation is a `const_str` of an SSA variable *name*** (e.g. literal
string `"v313"`) instead of the class object.

**Root cause: a module-chunking + `constructor_fold_safe` frontend bug.** When the
module body (`molt_main`) is split into multiple `molt_module_chunk_N` functions
(native default `module_chunk_max_ops = 1400`, `src/molt/cli.py:19810`), a class
defined in chunk N and instantiated in chunk N+M has its `class_value_name` SSA
reference dangle across the chunk boundary. The constructor-fold fast path at
`src/molt/frontend/visitors/calls.py:5461-5468` blindly reuses that stale SSA name
without checking it is still live in the current chunk, and lowering degrades the
dangling SSA ref to a `CONST_STR` of the name. The runtime then sees a string where
a type was expected → `object.__new__ expects type`. Depending on the inlined
store's lowering, this surfaces either as a clean `TypeError` (exit 1) **or** a
**SIGSEGV** (exit -11) when the inlined `store`/`store_init` takes the
direct-field-store path and dereferences the None error-sentinel's header before the
exception check.

**This is NOT specific to PEP 695 generics.** Plain `class A:` / `class B:` also
miscompile across a chunk boundary (verified). Generic classes merely hit the bug
first because their PEP 695 definitions are op-heavy and more readily cross the
1400-op default chunk threshold.

Family classification: **NOVEL.** Not the #43 const-str-in-set hashing family
(no set ops; crash is in `molt_object_new_bound_sized` → direct store, not
`set_find_entry`). Not the #45 SETATTR `__slots__` layout-assert family (the store
fast path itself is correct; it is fed a None object due to an upstream
frontend class-ref bug). Root is a **cross-chunk dangling SSA class-reference** in
the constructor-fold lowering.

---

## Minimal reproductions

### A. Canonical CPython-clean repro (use as the regression)
`tests/differential/basic/generic_class.py` (verbatim). Runs clean under CPython
3.14.5; SIGSEGVs on molt native at HEAD. Crashes because its 3 generic classes +
large `__main__` exceed the 1400-op chunk threshold, splitting the class defs
(chunk_1) from the instantiations (chunk_2).

### B. TINY deterministic repro — chunking is THE trigger (independent of size)
```python
# tiny.py — CPython-clean. Works at default threshold; FAILS when forced to chunk.
class A[T]:
    def __init__(self, v: T):
        self.v = v
    def get(self) -> T:
        return self.v
class B[T]:
    def __init__(self, v: T):
        self.v = v
    def get(self) -> T:
        return self.v
if __name__ == "__main__":
    a = A("x")
    print("a", a.get())
    b = B("y")
    print("b", b.get())
```
- `molt build --target native tiny.py` (default chunk threshold 1400): **exit 0** (single chunk; class defs + instantiation co-located).
- `MOLT_MODULE_CHUNK_OPS=50 molt build --target native tiny.py`: **exit 1**, `TypeError: object.__new__ expects type` (forced 3-way chunk split → dangling cross-chunk class-ref).

### C. Non-generic proof (bug is general, not PEP-695-specific)
Replace `class A[T]:`/`class B[T]:` with plain `class A:`/`class B:` (drop type
params/annotations) in B. With `MOLT_MODULE_CHUNK_OPS=50`: same
`TypeError: object.__new__ expects type`. CPython-clean.

`MOLT_MODULE_CHUNK_OPS=<small>` is the deterministic lever to reproduce on a tiny
program without depending on the natural op-cost threshold.

---

## Root-cause evidence chain

### 1. The crash (lldb on the native binary; routed via tools/safe_run.py)
```
stop reason = EXC_BAD_ACCESS (code=1, address=0xfffffffffffffff0)
->  ldur   w1, [x0, #-0x10]     ; load header flags @ obj_ptr-16
    and    x1, x1, #0x1          ; test HEADER_FLAG_HAS_PTRS (bit 0)
    ...
    orr    w1, w1, w2            ; go_slow = has_ptrs_set | (new_val tag==TAG_PTR)
x0 = 0x0                          ; unboxed obj pointer is NULL
[sp+0x678] (the obj box)  = 0x7ffb000000000000   ; = QNAN|TAG_NONE = Python None
[sp+0x620] (the new value) = 0x7ffc04df825259a8  ; = TAG_PTR heap string "original"/value
```
Offset −16 from a payload pointer = `MoltHeader.flags` (header is 24 bytes; flags @
+8; `native_backend_consts.rs:36-49`). The `(flags & 1) | (tag == TAG_PTR)` pattern
is exactly the SETATTR direct-field-store fast path
`runtime/molt-backend/src/native_backend/function_compiler.rs:21700-21718` (and the
`store_init` sibling at 21845-21903). The object being stored into is **None**;
`unbox_ptr_value(None)` yields NULL → header deref faults.

### 2. The None comes from the allocation
The obj box `0x7ffb` (None) is the return value of `molt_object_new_bound_sized`
(`runtime/molt-runtime/src/builtins/types.rs:561`). That function returns a
None/exception sentinel when `cls_bits` is not a valid type:
```rust
let Some(cls_ptr) = cls_obj.as_ptr() else {
    return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
};
if object_type_id(cls_ptr) != TYPE_ID_TYPE {
    return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
}
```

### 3. cls_bits is a const_str of an SSA *name* (the actual defect)
`MOLT_DUMP_IR=full` on the crashing program shows the module body split into
`<mod>__molt_module_chunk_1` (class defs) and `<mod>__molt_module_chunk_2`
(instantiations). In chunk_2:
```
0015: const_str        out=_v6  s_value=v313          # literal string "v313"
0016: object_new_bound out=v630 args=[_v6] value=16   # cls_bits = "v313" (a STRING)
0017: jump 45 / 0018: label 45 / ... / 0023: store args=[v630, _v326]
```
But `v313` is the SSA output of `class_def out=v313` **in chunk_1** (the Stack class
object). The reference dangles across the chunk boundary, so lowering materializes
it as `CONST_STR "v313"`. The working single-chunk build instead has
`object_new_bound args=[v225]` where `v225 = class_def(...)` in the *same* function.

### 4. The lowering site
`src/molt/frontend/visitors/calls.py:5459-5468`:
```python
if class_id is not None:
    class_info = self.classes[class_id]
    if self.current_func_name == "molt_main":
        class_value_name = class_info.get("class_value_name")
        if class_info.get("constructor_fold_safe") and isinstance(class_value_name, str):
            class_ref = MoltValue(class_value_name, type_hint="type")   # <-- BUG: blind SSA ref
        else:
            class_ref = self._emit_module_attr_get(class_id)
    else:
        static_class_ref = self._current_module_static_class_ref(class_id)   # <-- correct path
        ...
```
- `class_value_name` is set once when the class is defined
  (`src/molt/frontend/visitors/classes.py:3402`, `class_info["class_value_name"] =
  class_val.name`) and is stored in `self.classes[...]`.
- `_reset_module_chunk_state` (`src/molt/frontend/__init__.py:806-883`) clears
  `self.globals`, `self.locals`, `self.exact_locals`, `self._module_cache_values`,
  `loop_static_class_refs`, etc. at every chunk boundary — but **does NOT clear
  `class_info["class_value_name"]` / `["constructor_fold_safe"]`** (they live in
  `self.classes`, which is intentionally cross-chunk).
- Therefore in chunk_2 the molt_main branch still reads `class_value_name="v313"`
  and blindly builds `MoltValue("v313")` **without checking the SSA value is live
  in the current chunk**.
- The CORRECT mechanism already exists: `_current_module_static_class_ref`
  (`__init__.py:4657`) performs exactly the missing liveness guard at lines
  4678-4680:
  ```python
  current = self.globals.get(class_name)
  if current is None or current.name != static_name:
      return None      # post-chunk-reset: self.globals is empty -> None -> caller falls back to MODULE_GET_ATTR
  ```
  The `else` (non-molt_main) branch at 5470 uses it and is chunk-safe. Only the
  `molt_main` branch bypasses it.

---

## FIX DESIGN (frontend, structural — for the Python/cargo-capable next agent)

**Class-level fix:** the molt_main constructor-fold branch must not trust
`class_value_name` as a live SSA value without the same liveness check that
`_current_module_static_class_ref` performs. Route the molt_main branch through the
liveness-guarded resolver and fall back to `MODULE_GET_ATTR` when the class SSA
value is not live in the current chunk.

Concretely, replace `calls.py:5461-5468` with logic equivalent to:
```python
if self.current_func_name == "molt_main":
    static_class_ref = self._current_module_static_class_ref(class_id)
    if (
        static_class_ref is not None
        and class_info.get("constructor_fold_safe")
    ):
        class_ref = static_class_ref          # live, layout-stable, fold-safe SSA ref
    else:
        class_ref = self._emit_module_attr_get(class_id)   # chunk-safe re-fetch
```
Rationale:
- `_current_module_static_class_ref` already enforces: molt_main scope, globals
  not escaped, name not mutated, class layout stable, `class_value_name` set, AND —
  critically — `self.globals[class_id].name == class_value_name` (the chunk-liveness
  guard). After a chunk boundary `self.globals` is cleared, so it returns `None`
  and we correctly fall back to `MODULE_GET_ATTR`.
- The `constructor_fold_safe` gate is preserved (the fast alloc + inlined `__init__`
  still fires when the class ref is a *live* SSA value), so there is no perf
  regression for the in-chunk case (bench_struct etc.).
- This deletes the parallel/duplicated ad-hoc class-ref logic in the molt_main
  branch in favor of the single audited resolver — a de-duplication, not a patch.

**Verify the sibling pre-check too:** `calls.py:5436-5443` also reads
`class_value_name` and compares against `target_info.name` to decide whether to drop
`class_id`. After a chunk reset, `target_info` may itself be stale; confirm this
pre-check does not independently keep a dangling class-ref alive. If it can,
fold the same liveness discipline (prefer `self.globals.get(class_id)`).

**Defense-in-depth (SECONDARY, optional, do NOT use as the primary fix):** the
inlined `store`/`store_init` direct-field-store fast path
(`function_compiler.rs:21542 / 21899`) computes `obj_ptr = unbox_ptr_value(*obj)`
and dereferences the header with **no guard that the preceding allocation
succeeded** — there is no `check_exception` between `object_new_bound` and the
inlined first store (IR ops 16→23 have the `check_exception` only at op 24, AFTER
the store). So *any* alloc-failure path (not just this bug) turns a clean Python
exception into a SIGSEGV. The structurally correct frontend fix above removes the
trigger; if hardening is desired later, the inlined-constructor lowering should
emit the `check_exception` (or a null/None guard) BEFORE the inlined field stores,
not after. This is a separate, lower-priority arc; the #50 fix is the frontend
class-ref correction.

---

## Affected surface (audit)

- **Any module** large enough to chunk-split `molt_main` (native default 1400 ops;
  WASM default 2000; `MOLT_MODULE_CHUNK_OPS` override) where a class is instantiated
  in a later chunk than its definition. Generic (PEP 695) AND plain classes.
- All backends: the defect is in shared frontend lowering (`calls.py`), emitted as
  `CONST_STR` + `OBJECT_NEW_BOUND` into the SimpleIR BEFORE backend selection. The
  native crash is the observed worst case; WASM/LLVM/Luau consume the same wrong IR.
  (Native SIGSEGV + native/forced-split TypeError both verified; the IR defect is
  identical regardless of `--target`.)
- The `constructor_fold_safe` + `class_value_name` path is the only one missing the
  liveness guard; the loop-static and non-molt_main paths already guard correctly.
- Manifestation varies by the inlined store's lowering: direct-field-store path →
  SIGSEGV; otherwise → clean `TypeError: object.__new__ expects type`. Both are
  CPython-divergences (CPython constructs the instance correctly).

---

## Landing checklist (next agent, cargo-capable)

1. Apply the `calls.py:5461-5468` fix (route molt_main constructor-fold through
   `_current_module_static_class_ref` with `MODULE_GET_ATTR` fallback). Audit the
   5436-5443 sibling pre-check.
2. Build native: `cargo build --profile release-fast -p molt-backend --features native-backend`
   (frontend-only change actually needs no Rust rebuild — but rebuild if touching
   any `.rs`; this fix is pure Python).
3. Regressions (add + verify byte-identical vs CPython 3.12/3.13/3.14):
   - `tests/differential/basic/generic_class.py` (already exists; currently
     known-fail — see committed header note). Must go green.
   - A NEW small differential that forces a chunk split deterministically, e.g.
     a 2-class file built/run with `MOLT_MODULE_CHUNK_OPS=50`, asserting both
     generic and non-generic classes instantiate correctly across chunks. (If the
     differential harness cannot thread a per-test env var, add a Python file with
     enough class op-cost to cross the natural 1400 threshold — model on
     generic_class.py — plus a non-generic sibling.)
   - A non-generic cross-chunk instantiation case (proves the general fix).
4. Verify on ALL targets (native/WASM/LLVM/Luau) — frontend bug ⇒ all must be
   checked, not just native.
5. Run the existing compliance + differential suites; confirm no constructor-fold
   perf regression on bench_struct (the in-chunk fold path must still fire).
6. Consider the secondary inlined-store exception-guard hardening as a follow-up
   task (separate baton), NOT bundled into the #50 correctness fix.

---

## Repro/inspection commands used (no-cargo harness)

```
unset MOLT_SESSION_ID; export MOLT_SKIP_RUNTIME_REBUILD=1; export PYTHONPATH=<wt>/src
# build (uses solo target/release-fast/molt-backend, no cargo):
python3 -m molt build --target native --output /tmp/out <file>.py
# run guarded (SIGSEGV = exit -11/245):
python3 tools/safe_run.py --rss-mb 512 --timeout 8 -- /tmp/out
# force the chunk split on a tiny program:
MOLT_MODULE_CHUNK_OPS=50 python3 -m molt build --target native --output /tmp/out tiny.py
# dump SimpleIR to see the const_str class-ref defect:
MOLT_DUMP_IR=full python3 -m molt build --target native --output /tmp/out <file>.py 2>ir.txt
#   then: grep -E 'class_def|object_new_bound|const_str .*s_value=v[0-9]+$' ir.txt
# crash backtrace (binary already built):
lldb -b -o 'b -a <faultPC>' -o run -o 'register read x0' -o 'bt' /tmp/out   # via safe_run wrapper
```

Key source coordinates:
- BUG: `src/molt/frontend/visitors/calls.py:5461-5468` (+ sibling 5436-5443)
- correct resolver: `src/molt/frontend/__init__.py:4657-4681` (`_current_module_static_class_ref`)
- class_value_name set: `src/molt/frontend/visitors/classes.py:3402`
- chunk reset (clears globals, not class_value_name): `src/molt/frontend/__init__.py:806-883`
- chunk threshold default 1400: `src/molt/cli.py:19810` (env `MOLT_MODULE_CHUNK_OPS`)
- runtime sentinel: `runtime/molt-runtime/src/builtins/types.rs:561-587`
- direct-store deref (SIGSEGV site): `runtime/molt-backend/src/native_backend/function_compiler.rs:21542,21700-21718` (store) and `:21899-21903` (store_init)
- header layout (−16 = flags): `runtime/molt-backend/src/native_backend_consts.rs:36-50`
