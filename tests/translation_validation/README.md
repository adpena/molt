# Translation validation: what Molt checks today, and what it does not

No checker yet proves that a compiler pass preserves meaning. Keep claims to the
scope below.

## Differential execution (implemented)

`tests/molt_diff.py` is the canonical differential runner. It compiles each
program with Molt, runs it, and compares the result with the exact pinned
CPython release through one comparison law, `compat_comparison.compare_outputs`:

- stdout and the exit status are compared exactly, from raw bytes decoded
  losslessly, unless a test declares another `stdout_mode` in its metadata;
- stderr is ignored by default (`stderr_mode = "ignore"`); a test that declares
  a stderr mode is compared on the program's own stderr (`child_stderr`), never
  on guard diagnostics;
- a timeout, an out-of-memory kill and an uncalibrated backend count as failures
  (exit 1), not as separate outcomes;
- the run exits 2 when nothing executed or files were not run, and 0 only when
  files executed and every one passed.

Nightly runs it over the policy-selected `tests/differential` corpus in shards;
`nightly.differential.aggregate` in `tools/proof_plan.toml` writes the combined
evidence. To run a subset, pass the files to `tests/molt_diff.py`.

`tools/molt_dev.py difftest` and `tools/parity_gate.py` are still separate
runners with rules of their own; HF-801 consolidates them.

Differential execution is evidence for the programs it runs. It does not prove
that a compiler pass preserves meaning for other programs.

## Pass-level checks

- `tools/translation_validator.py` checks structural invariants of the Python
  mid-end after each pass (`src/molt/frontend/tv_hooks.py`).
- `runtime/molt-passes/src/tir/translation_validator.rs` checks one narrow
  representation invariant: a surviving value's representation proof only moves
  upward.
- `tools/z3_pass_verify.py` reports candidate missed optimizations. Its
  integer encoding is not yet sound for Python semantics (HF-799), so its
  output is never rewrite evidence.
- A bounded before/after validator for a typed integer TIR fragment, with
  proved, disproved, unsupported and timeout outcomes, is open work (HF-800).

## IR snapshots

`molt debug ir` dumps TIR before and after the mid-end for inspection and
diffing:

```bash
uv run --python 3.12 python3 -m molt.cli debug ir --stage=all --format json --out logs/debug/ir/hello.json examples/hello.py
```
