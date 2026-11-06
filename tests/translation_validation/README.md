# Translation validation: what Molt checks today, and what it does not

Molt has one differential runner and no pass-level equivalence prover yet. Keep
claims to the scope below.

## Differential execution (implemented)

`tests/molt_diff.py` compiles each program with Molt, runs it, and compares the
observable result with the exact pinned CPython release through one comparison
law, `compat_comparison.compare_outputs`:

- stdout and the exit status are compared exactly by default;
- a test may declare a different `stdout_mode` or `stderr_mode` in its own
  metadata, as rustc's compiletest declares `normalize-*` rules;
- the program's own stderr (`child_stderr`) is compared, never guard
  diagnostics;
- a timeout, an out-of-memory kill and a guard infrastructure failure are typed
  outcomes, not mismatches.

The nightly cell `nightly.verification-t3.differential` runs it over
`tests/differential` and writes `proof-results/differential-tier3.json`. The
canonical command line is in `tools/proof_plan.toml`; run a subset with
`python -B tools/molt_dev.py difftest`.

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
