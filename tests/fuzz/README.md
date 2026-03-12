# Fuzz Testing Infrastructure

Grammar-based fuzzing for the Molt compiler. Generates random valid Python
programs within Molt's supported subset and uses differential testing
(Molt vs CPython) to find compiler bugs.

## Quick Start

```bash
# Generate and differentially test 100 random programs
uv run --python 3.12 python3 tools/fuzz_compiler.py --count 100

# Reproducible run with a fixed seed
uv run --python 3.12 python3 tools/fuzz_compiler.py --seed 42 --count 1000

# Just generate programs (no compilation or testing)
uv run --python 3.12 python3 tools/fuzz_compiler.py --generate-only --count 10 --out-dir /tmp/fuzz

# Shrink failing programs to minimal reproducers
uv run --python 3.12 python3 tools/fuzz_compiler.py --count 500 --shrink --output-dir /tmp/fuzz_failures

# Test rejected-dynamic-feature programs
uv run --python 3.12 python3 tools/fuzz_compiler.py --mode reject --count 50

# Compile-only mode (no execution, just check for crashes)
uv run --python 3.12 python3 tools/fuzz_compiler.py --mode compile-only --count 200
```

## Modes

| Mode | Purpose |
|------|---------|
| `safe` (default) | Generate programs guaranteed to run on CPython. Differential test: CPython stdout == Molt stdout. |
| `reject` | Generate programs using forbidden dynamic features. Verify Molt rejects them cleanly (no crash). |
| `compile-only` | Generate syntactically valid Python. Only check that `molt.cli build` does not crash. |

## What Gets Generated (safe mode)

The `SafeProgramGenerator` produces type-tracked programs covering:

- **Types**: int, float, str, bool, None, list, tuple, dict, set
- **Operators**: arithmetic, comparison, boolean, unary, string ops
- **Control flow**: if/elif/else, for, while (bounded), break/continue
- **Functions**: positional args, defaults, keyword-only, *args/**kwargs, closures
- **Classes**: init, repr, methods, inheritance, super()
- **Comprehensions**: list, dict, set
- **Error handling**: try/except with specific exception types
- **Builtins**: len, abs, int, str, bool, repr, min, max, sorted, enumerate, zip, isinstance

## Excluded (per Molt policy)

- Dynamic code generation and arbitrary code execution at runtime
- setattr(), delattr(), __dict__ mutation
- type() as class constructor
- Import of arbitrary modules
- Generators / yield
- async / await

## Smoke Test

```bash
uv run --python 3.12 python3 -m pytest tests/fuzz/test_fuzz_smoke.py -v
```

## Environment

All temporary and output files are placed on `$MOLT_EXT_ROOT` or `/tmp`.
Nothing is written under the repository tree.

Set up the environment before heavy fuzz runs:

```bash
eval "$(tools/throughput_env.sh --print)"
```
