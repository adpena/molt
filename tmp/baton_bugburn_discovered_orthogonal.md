# Baton: orthogonal pre-existing bugs discovered during the burndown pass

These were uncovered while fixing/verifying the queued bugs. All are PRE-EXISTING
at base `932a4e529` (confirmed independent of this session's commits) and are
Rust runtime/backend issues (out of scope for the Python-only pass). Recorded so
the signal is not lost.

---

## A (HIGH): closure capturing an enclosing var + called *within* its defining scope → wrong codegen

`TypeError: 'function' object is not subscriptable` (exit 1) when a nested
function that closes over an enclosing variable is **called immediately inside
the defining function**. Returning the closure and calling it from outside works.

```python
def outer(base):
    def add(x):
        return base + x      # captures `base`
    return add(10)           # called inside outer -> molt: TypeError; CPython: 15
print(outer(5))
```

### Characterization (built + run via tools/safe_run.py; CPython all correct)
| repro | molt | note |
|-------|------|------|
| `def outer(base): def add(x): return base+x; return add` ; `outer(5)(10)` | OK (15) | closure RETURNED, called outside |
| `def outer(base): def add(x): return base+x; return add(10)` | FAIL | called inside, captures param |
| `def outer(): base=5; def add(x): return base+x; return add(10)` | FAIL | called inside, captures local |
| `def outer(base): def get(): return base+1; return get()` | FAIL | called inside, inner no-arg |

So the trigger is **calling a cell-capturing nested function from within the
scope that defines it**. The `'function' object is not subscriptable` strongly
suggests the call site treats the closure value as an indexable (the closure
cell / a `__molt_closure__` tuple) and emits an INDEX where it should emit a
call — i.e. the local-closure call path confuses the closure object with its
captured-cell container. Also surfaces inside methods (originally found as
`Outer.Inner().adder(5)` with an inner `add` closure during nested-class work).

### Where to look (Rust backend)
The native call lowering for calling a *local* function value that is a closure
(captures cells): `runtime/molt-backend/src/native_backend/function_compiler.rs`
CALL / CALL_BIND / FUNC_NEW_CLOSURE handling, and how a `ClosureFunc:`-typed
local value is invoked. Compare the returned-and-called-outside path (works) with
the called-in-place path (emits a subscript). Likely the in-scope closure value
is still bound to its cell-container representation at the call site.

### Repro files (this session): /tmp/clo_a.py (works) .. /tmp/clo_d.py (fail)

---

## B (MEDIUM): SETATTR on a non-writable target panics instead of raising AttributeError/TypeError

Found while verifying `typing.final` (which does `try: f.__final__ = True except
(AttributeError, TypeError): pass`). The Python try/except is correct, but molt's
runtime SETATTR on certain targets PANICS (so the except can never catch it):

1. `final(<instance of a class with __slots__ and no matching slot>)` →
   `panic at runtime/molt-runtime/src/call/class_init.rs:245` —
   `alloc_instance_for_class_sized: caller-supplied size must match
   class_layout_size — frontend layout drift detected (left: 8, right: 16)`.
   (This is actually triggered at *instantiation* of a single-slot class
   `class Slotted: __slots__ = ("a",)` under the harness's freethreaded 3.14t
   build; a single-`__slots__` class has a layout-size mismatch.)
2. `final(42)` (SETATTR `__final__` on a tagged/unboxed int) →
   `panic at runtime/molt-runtime/src/object/mod.rs:1294` —
   `misaligned pointer dereference: address must be a multiple of 0x4 but is
   0x12` (0x12 = tagged-int bits). SETATTR over a tagged-int receiver derefs the
   tag as a pointer.

CPython: setting an attribute on a builtin/slots target raises AttributeError or
TypeError. molt must raise (catchable), not panic. Both are SETATTR-failure
runtime bugs.

### Where to look (Rust runtime)
- (2) the generic SETATTR path (`molt_setattr*` in
  `runtime/molt-runtime/src/object/`) must check for a non-pointer (tagged)
  receiver and raise TypeError("'int' object has no attribute ...") /
  AttributeError instead of dereferencing. See object/mod.rs:1294.
- (1) class_init.rs:245 `alloc_instance_for_class_sized` layout-size assert for a
  single-`__slots__` class — separate layout-drift bug (the computed
  class_layout_size disagrees with the caller size 8 vs 16). May be specific to
  the freethreaded (`t`) / specific stdlib-profile build the harness uses.

### Impact
`typing.final` (landed this session) is correct for all writable targets; its
silently-ignored branch can't be exercised e2e until these are fixed. More
broadly, ANY user code doing `try: obj.attr = v except (AttributeError,
TypeError)` over a builtin/slots target will crash.

### Repro files (this session): /tmp/repro_final2.py had both; minimal:
`x=42` then a SETATTR of `x.__final__` (via `final(42)`); and instantiating
`class Slotted: __slots__=("a",)`.
