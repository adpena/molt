# Translation Validation Infrastructure

Translation validation verifies that Molt's midend optimization passes preserve
program semantics. Unlike unit tests for individual passes, translation
validation checks end-to-end observable behavior.

## Approach

### Tier 1: Concrete Validation (Implemented)

For each input program, compile and run three ways:

1. **CPython** (ground truth) -- the reference interpreter
2. **Molt with all midend passes enabled** -- the optimized pipeline
3. **Molt with midend disabled** (`MOLT_MIDEND_DISABLE=1`) -- unoptimized baseline

If (2) and (3) produce identical stdout, the midend passes *collectively*
preserve semantics for that program. If they differ, a pass introduced a
miscompilation.

Comparing both against CPython additionally validates that the compiler
as a whole (with and without optimization) produces correct results.

### Tier 2: Symbolic Validation (Future)

Interpret pre-pass and post-pass IR symbolically to prove equivalence
for all possible inputs, not just concrete test cases.

### Tier 3: SMT-Based Verification (Future)

Encode IR semantics as SMT formulas and use a solver (e.g., Z3) to
prove or disprove equivalence of pre/post pass IR.

## Tools

### `tools/translation_validate.py`

Main validation driver. Compiles programs with and without midend passes,
runs them, and compares outputs.

```bash
# Single file
uv run --python 3.12 python3 tools/translation_validate.py examples/hello.py

# Directory (recursive)
uv run --python 3.12 python3 tools/translation_validate.py tests/differential/basic/

# Verbose with output diffs
uv run --python 3.12 python3 tools/translation_validate.py --verbose examples/hello.py

# JSON output for CI
uv run --python 3.12 python3 tools/translation_validate.py --json examples/hello.py

# Skip CPython comparison (faster, only checks midend on vs off)
uv run --python 3.12 python3 tools/translation_validate.py --no-cpython examples/hello.py
```

### `molt debug ir`

IR snapshot utility. Dumps TIR at pre-midend and post-midend stages for
manual inspection and automated diffing.

```bash
# Dump all stages (text)
uv run --python 3.12 python3 -m molt.cli debug ir examples/hello.py

# Pre-midend only
uv run --python 3.12 python3 -m molt.cli debug ir --stage=pre-midend examples/hello.py

# JSON output with retained artifact
uv run --python 3.12 python3 -m molt.cli debug ir --stage=all --format json --out logs/debug/ir/hello.json examples/hello.py
```

## Midend Pass Pipeline

The midend runs a fixed-point loop with these passes (in order):

1. **simplify** -- structural region canonicalization
2. **sccp_edge_thread** -- sparse conditional constant propagation + edge threading
3. **join_canonicalize** -- try/except join label normalization
4. **guard_hoist** -- redundant guard elimination
5. **licm** -- loop-invariant code motion
6. **prune** -- unreachable region/label/jump pruning
7. **verifier** -- definite-assignment verification
8. **dce** -- dead trivial constant elimination
9. **cse** -- common subexpression elimination

The loop repeats until convergence (no changes) or the round cap is reached.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MOLT_TV_TIMEOUT` | 60 | Per-file timeout in seconds |
| `MOLT_TV_BUILD_PROFILE` | dev | Build profile (dev or release) |
| `MOLT_TV_JOBS` | 4 | Parallel validation jobs |
| `MOLT_TV_PYTHON` | sys.executable | CPython for baseline runs |
| `MOLT_MIDEND_DISABLE` | (unset) | Set to 1 to skip all midend passes |
| `MOLT_MIDEND_MAX_ROUNDS` | (varies) | Cap fixed-point iteration rounds |
| `MOLT_EXT_ROOT` | /Volumes/APDataStore/Molt | External volume root |
| `MOLT_DIFF_TMPDIR` | /tmp | Temp directory root |

## Exit Codes

- `0` -- All validated programs show consistent midend behavior
- `1` -- At least one program shows different behavior with midend on vs off
