# Baton: unpack-error message parity (#39) — RUST RUNTIME FIX REQUIRED

**Status:** Root-caused with exact CPython references across 3.12/3.13/3.14.
The fix lives in **Rust runtime code** (`molt-runtime/src/object/ops.rs`), so it
is out of scope for the Python-only bug-burndown agent (no Rust build loop
allowed). This baton specifies the complete fix.

## Symptom (molt vs CPython divergence)

Two divergences in sequence-unpacking error messages. Repro
(`/tmp/repro_unpack.py` style):

```python
a, b = [1, 2, 3]     # too-many
a, b, c = [1]        # too-few
a, *b, c = [1]       # starred too-few
a, b = 5             # non-iterable
```

| case          | molt (current)                                   | CPython 3.12/3.13                         | CPython 3.14                                  |
|---------------|--------------------------------------------------|-------------------------------------------|-----------------------------------------------|
| too-many      | `too many values to unpack (expected 2, got 3)`  | `too many values to unpack (expected 2)`  | `too many values to unpack (expected 2, got 3)` |
| non-iterable  | `cannot unpack non-sequence`                     | `cannot unpack non-iterable int object`   | `cannot unpack non-iterable int object`         |
| too-few       | `not enough values to unpack (expected 3, got 1)`| (same) ✓                                   | (same) ✓                                       |
| starred-too-few | `not enough values to unpack (expected at least 2, got 1)` | (same) ✓                       | (same) ✓                                       |

So **two bugs**:

1. **too-many is missing the 3.12/3.13 vs 3.14 version gate.** molt always emits
   `(expected N, got M)`. CPython adds `, got M` **only in 3.14+**; 3.12/3.13 emit
   `(expected N)`. (Confirmed: 3.12 and 3.13 both print `(expected 2)`; only 3.14
   prints `(expected 2, got 3)`.)

2. **non-iterable message is wrong.** molt emits the generic
   `cannot unpack non-sequence`. CPython (all versions, identical) emits
   `cannot unpack non-iterable {type} object`, where `{type}` is the runtime type
   name (`int`, `float`, `NoneType`, `object`, ...). Confirmed identical text
   across 3.12 and 3.14.

The too-few and starred-too-few messages are already correct (and starred-too-few
is *frontend*-emitted at `src/molt/frontend/__init__.py:12947`, already correct —
do not touch it).

## Producer (where to fix)

The frontend lowers a no-star unpack to a single `UNPACK_SEQUENCE` op
(`src/molt/frontend/__init__.py:12926-12933`, `metadata={"expected_count": N}`),
which dispatches to the runtime function:

**`runtime/molt-runtime/src/object/ops.rs` — `molt_unpack_sequence` (fn starts line 7255).**

Exact lines to change (as of base `932a4e529`):

- **Non-iterable, fast path** — `ops.rs:7268`:
  ```rust
  raise_exception::<u64>(_py, "TypeError", "cannot unpack non-sequence");
  ```
- **Non-iterable, generic-iter path** — `ops.rs:7324`:
  ```rust
  raise_exception::<u64>(_py, "TypeError", "cannot unpack non-sequence");
  ```
- **too-many, TYPE_ID_LIST_BOOL branch** — `ops.rs:7285-7288`:
  ```rust
  let msg = format!("too many values to unpack (expected {}, got {})", expected, actual);
  ```
- **too-many, LIST/TUPLE branch** — `ops.rs:7308-7311`:
  ```rust
  let msg = format!("too many values to unpack (expected {}, got {})", expected, actual);
  ```
- **too-many, generic-iter exhausted branch** — `ops.rs:7381-7389` (already has a
  two-arm `if exhausted { "...expected {}, got {}" } else { "...expected {}" }`; the
  *exhausted* arm needs the same version gate; the *not-exhausted* arm already emits
  the no-count form which matches the <3.14 shape but is reached only for
  non-terminating iterators — leave its text but see note below).

There is a SECOND producer for the transpile-to-Rust backend target
(`runtime/molt-backend/src/rust.rs:1068,1072,1075`): `panic!("cannot unpack
non-sequence")` and `too many values to unpack (expected {}, got {})`. For full
all-backends parity these must be fixed too. (They use `panic!`, which is itself a
parity gap vs raising ValueError/TypeError — but at minimum the message text and
version gate should match. Note this path is the `--target rust`/transpile path,
lower priority than the native/wasm/llvm runtime above.)

NOTE: `functions_http.rs:321/329` and `ops_sys.rs:1045` are *context-specific*
fixed-arity unpacks (HTTP 2-tuple, os.times 6-tuple) with hardcoded counts; they
are not the general unpack path and are out of scope for #39.

## Correct fix

### (1) Version gate for too-many

The established runtime version-gating helper is:
```rust
crate::object::ops_sys::runtime_target_at_least(_py, MAJOR, MINOR)
```
(see e.g. `ops_dict.rs:173,344`, `ops_builtins.rs:1735,1759`, `bind.rs:5338`).

Replace each unconditional too-many `format!` with:
```rust
let msg = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
    format!("too many values to unpack (expected {}, got {})", expected, actual)
} else {
    format!("too many values to unpack (expected {})", expected)
};
```
Apply at the LIST_BOOL branch (7285), the LIST/TUPLE branch (7308), and the
generic-iter *exhausted* arm (7382-7385 — there `count` is the actual). The
generic-iter *non-exhausted* arm (7388, capped at 1024 extra) already emits the
no-count form; for 3.14 it should ideally still attempt `, got {count}` but
CPython itself caps differently — keep the existing behavior or, for strict 3.14
parity, emit `(expected {}, got {})` with the (capped) count. Lowest-risk: gate
identically to the other two arms using the available `count`.

### (2) non-iterable message with type name

The established type-name helper is:
```rust
let type_name = class_name_for_error(type_of_bits(_py, seq_bits));
```
(used pervasively in ops.rs, e.g. lines 1529, 1693, 3394). Replace both
`cannot unpack non-sequence` raises (7268 and 7324) with:
```rust
let type_name = class_name_for_error(type_of_bits(_py, seq_bits));
let msg = format!("cannot unpack non-iterable {type_name} object");
raise_exception::<u64>(_py, "TypeError", &msg);
```
CPython text is identical across 3.12/3.13/3.14 (confirmed for int/float/
NoneType/object), so NO version gate is needed for the non-iterable message.

IMPORTANT for 7324 (generic-iter path): that branch is reached after
`molt_iter(seq_bits)` returns none — i.e. the object is genuinely not iterable.
But also guard the pre-existing-exception case the same way the too-few path does
at 7399 (`if exception_pending(_py) { return ... }`) so a custom `__iter__` that
raises is propagated rather than masked by the TypeError. (Verify: `molt_iter`
on a non-iterable sets no exception and returns none → safe to raise here.)

## Verification plan (after the Rust fix builds)

CPython references (already captured; all via `uv run --python <v>`):

```
# too-many
3.12/3.13: 'too many values to unpack (expected 2)'
3.14:      'too many values to unpack (expected 2, got 3)'
# non-iterable (all versions identical)
'cannot unpack non-iterable int object'
'cannot unpack non-iterable float object'
'cannot unpack non-iterable NoneType object'
'cannot unpack non-iterable object object'
```

Add a differential regression `tests/differential/basic/unpack_error_messages.py`
that triggers too-many (list + tuple + generic iterator), too-few, starred-too-few,
and non-iterable (int/float/None/custom object), printing `repr(str(e))`. Run via
the harness on 3.12 AND 3.14 (the harness sets `MOLT_SYS_VERSION_INFO` so molt's
`runtime_target_at_least` matches the CPython under test) — it must be
byte-identical on BOTH, which is what proves the version gate.

Use `cmp -s` for verdicts (rtk's `diff` reports false "identical"; confirmed this
session).

## Files
- runtime/molt-runtime/src/object/ops.rs : 7268, 7285-7288, 7308-7311, 7324, 7381-7389
- runtime/molt-backend/src/rust.rs : 1068, 1072, 1075 (transpile target; lower priority)
- src/molt/frontend/__init__.py : 12947 (starred-too-few; already correct, reference only)
