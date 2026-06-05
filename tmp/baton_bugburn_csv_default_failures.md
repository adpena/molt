# Baton: csv differential failures on the default (satellite) tier — TWO RUST BUGS

**Status:** csv-default DIFF-FAIL re-confirmed at base `932a4e529`. Seven csv
differential tests fail on the default native build. Root-caused to **two
distinct Rust bugs**, both out of scope for the Python-only burndown pass
(no Rust build loop allowed). One of them (the SEGV) is NOT csv-specific and is
the higher-severity item.

## Failing tests (default tier, `tests/differential/stdlib/`)

extrasaction family (3): `csv_extrasaction.py`, `csv_extrasaction_raise.py`,
`csv_extrasaction_validation.py`
quoting family (4): `csv_newline_quoting.py`, `csv_quote_minimal_edge.py`,
`csv_quote_notnull_optional.py`, `csv_quote_strings_optional.py`

(The full 39-test csv sweep could not be completed in one harness run — it trips
a SIGURG/exit-144 on long parallel runs under agent load. The 7 above are the
confirmed failures from a partial sweep; the remaining csv tests passed where
observed, e.g. csv_basic. Re-run the full sweep with `--jobs 1` to enumerate any
others, but they will almost certainly bucket into the two bugs below.)

NOTE: csv is "fully intrinsic-backed" — `src/molt/stdlib/csv.py` is a thin
wrapper over Rust `molt_csv_*` intrinsics. The reader/writer/parser logic and
set-membership are all Rust.

---

## BUG 1 (HIGH SEVERITY, not csv-specific): SIGSEGV on `<const-str> in/not in {str set literal}`

The 3 extrasaction failures are a SIGSEGV (exit -11, raw segfault, NO Rust
panic), triggered by `csv.DictWriter(..., extrasaction="raise")` →
`DictWriter.__init__` (csv.py:513-517) running
`self.extrasaction = extrasaction.lower(); if self.extrasaction not in {"raise","ignore"}: raise ...`.

**Reduced to a zero-csv repro:**
```python
x = "raise"
print(x not in {"raise", "ignore"})   # SIGSEGV on molt; CPython -> False
```

### Exact characterization (built + run via tools/safe_run.py)
| repro                                             | molt   | note |
|---------------------------------------------------|--------|------|
| `def f(x): return x not in {"raise","ignore"}` ; `f("raise")` | OK (False) | LHS is a *param* (non-const) |
| `x = "raise"; print(x not in {"raise","ignore"})` | SIGSEGV | LHS is a **const** string |
| `print("raise" not in {"raise","ignore"})`        | SIGSEGV | LHS literal |
| `x = "raise"; print(x in {"raise","ignore"})`     | SIGSEGV | `in` too |
| `x = "zzz";  print(x not in {"raise","ignore"})`  | SIGSEGV | non-member const |
| `x = 1; print(x not in {1, 2})`                   | OK (False) | **int** set is fine |

So the trigger is precisely: **set-membership (`in`/`not in`) where the item
operand is a compile-time-constant STRING and the container is a set literal of
strings.** A *param* (dynamic) string item works; an *int* set works. Only the
const-string item over a string set faults.

### IR / dispatch (already traced)
The frontend does NOT fold this — it emits real ops:
`const_str` (item) ; `const_str` x2 (set elems) ; `set_new` ; `contains`
(see `molt debug ir <repro> --stage pre-midend`).

Backend lowering (`runtime/molt-backend/src/native_backend/function_compiler.rs:13138`
`"contains" =>`) dispatches on `representation_plan.name_container_kind(&args[0])`
→ for a set → `molt_set_contains(container, item)`. args[0]=set (same in both the
working param case and the faulting const case), args[1]=item. So the dispatch is
identical; only the item operand differs (const_str vs param).

Runtime: `runtime/molt-runtime/src/object/ops_set.rs:17` `molt_set_contains` →
`ensure_hashable(item)` then `set_find_entry(_py, order, table, item_bits)`. The
SIGSEGV is in the hash/probe of a **const (statically-allocated / interned)
string** item against the set table — `set_find_entry` (and the string hash/eq it
calls) likely dereferences a field that is laid out differently for a static/const
string object than for a heap string. Since the *param* path (heap/dynamic string)
works and the *int* path works, the fault is specific to the const-string object
representation flowing into `set_find_entry`.

### Where to look / fix (Rust)
- `runtime/molt-runtime/src/object/ops_set.rs:17` `molt_set_contains` and the
  `set_find_entry` it calls (find in the same module / `object/layout` set table
  helpers). Compare how a const/interned string is hashed vs a heap string;
  the const-string `item_bits` is mis-dereferenced.
- Cross-check the const-string object construction path: how `const_str`
  lowers to a runtime string object in
  `runtime/molt-backend/src/native_backend/function_compiler.rs` (CONST_STR) and
  whether it produces a tagged/static representation that `set_find_entry`'s
  hash (likely `molt_hash`/`str` hash in `object/ops_hash.rs`) cannot deref.
- Also audit `var_get_boxed_overflow_safe` for args[1] in the `"contains"` arm —
  if a const string is mis-boxed there, the item_bits handed to
  molt_set_contains would be a bad pointer.

### Reproducer to add (after fix): `tests/differential/basic/set_contains_const_str.py`
covering const-str `in`/`not in` member + non-member, const-str via `.lower()`,
and (regression-guard) the param and int-set cases that already work.

---

## BUG 2 (csv reader): quoted field with embedded newline is split across rows

The 4 quote failures are WRONG OUTPUT (not a crash). The csv **writer is
correct**; the **reader** mis-parses a quoted field containing an embedded
newline.

**Repro:**
```python
import csv, io
rows = [['a"b', "c"], ["line\nbreak", "x"]]
buf = io.StringIO()
csv.writer(buf, quoting=csv.QUOTE_MINIMAL, lineterminator="\n").writerows(rows)
# buf now == '"a""b",c\n"line\nbreak",x\n'   (CORRECT, matches CPython)
buf.seek(0)
print(list(csv.reader(buf)))
# molt:    [['a"b', 'c'], ['line'], ['break"', 'x']]   (WRONG: 3 rows)
# CPython: [['a"b', 'c'], ['line\nbreak', 'x']]        (2 rows)
```

### Root cause
`_Reader.__next__` (csv.py:383-401) reads one physical line at a time
(`_iter_csvfile` → `iter(StringIO)` splits on `\n`, so the quoted `"line\nbreak"`
arrives as two physical lines `'"line\n'` then `'break",x\n'`), accumulates into
`self._pending`, and calls `_MOLT_CSV_READER_PARSE_LINE(handle, pending)`. The
multi-line-record continuation relies on the parser raising `ValueError(
"unexpected end of data")` for an UNTERMINATED quoted field (csv.py:396 — on that
message and not-EOF it `continue`s to append the next physical line).

The Rust parser `_MOLT_CSV_READER_PARSE_LINE` does NOT signal "unexpected end of
data" for the incomplete record `'"line\n'`; instead it returns a *partial* row
`['line']` (and on the next call parses `'break",x\n'` → `['break"','x']`). So the
continuation never fires and the quoted field is torn in two.

This is a Rust parser bug: an unterminated quoted field (open quote, no closing
quote before end-of-input-chunk) must be reported as incomplete (the
`"unexpected end of data"` signal the Python continuation expects), not returned
as a completed row.

### Where to look / fix (Rust)
- The `molt_csv_reader_parse_line` intrinsic (grep `csv` under
  `runtime/molt-runtime/src/` and the satellite `runtime/molt-runtime-csv/src/`;
  csv has an in-tree copy gated `#[cfg(not(feature="stdlib_csv"))]` AND a
  satellite copy — BOTH must be fixed for micro/edge/wasm + default parity, per
  tools/check_satellite_parity.py). The parser's end-of-input handling inside a
  quoted field must emit the incomplete/"unexpected end of data" condition.
- Verify against CPython's `_csv` state machine: an open quote at end of the
  current data chunk → state stays IN_QUOTED_FIELD → caller feeds more data.

### Reproducers to add (after fix)
`tests/differential/stdlib/csv_embedded_newline_roundtrip.py` (the repro above)
plus the 4 existing quote tests should pass.

---

## Verification
- Use `cmp -s` for verdicts (rtk `diff` reports false "identical"; confirmed).
- Re-run each fixed test via `tests/molt_diff.py <test> --python-version 3.14`
  (and 3.12 — csv messages have no version gates observed, but confirm).
- The full csv sweep: `python3 tests/molt_diff.py --files-from <list> --jobs 1`.

## Files
- BUG 1: runtime/molt-runtime/src/object/ops_set.rs:17 (molt_set_contains) +
  set_find_entry + const-string CONST_STR lowering
  (native_backend/function_compiler.rs) + object/ops_hash.rs string hash.
- BUG 2: molt_csv_reader_parse_line in runtime/molt-runtime/src/ (in-tree, gated)
  AND runtime/molt-runtime-csv/src/ (satellite). csv.py:383-401 is the (correct)
  Python continuation consumer — reference only.
