#!/usr/bin/env python3
"""Comprehensive compiler fuzzer for Molt.

Three modes:

  --mode safe (default)
    Generates programs GUARANTEED to execute cleanly on CPython (99%+ success
    rate).  Type-tracked expression system ensures arithmetic only uses numeric
    sub-expressions, ordering comparisons only use same-type operands, string
    methods only touch strings, and variables are never referenced before
    definition.  Primary differential testing mode: generate -> CPython ->
    Molt compile -> Molt run -> compare stdout.

  --mode reject
    Generates programs using Python dynamic features Molt explicitly rejects
    (exec, eval, setattr, monkeypatching, etc).  Verifies Molt produces a
    clean compile-time error (non-zero exit, stderr message) rather than a
    crash.

  --mode compile-only
    Uses hypothesmith to generate syntactically valid Python from the grammar.
    Only checks that ``molt.cli build`` does not crash -- does not execute or
    compare output.

Coverage (safe mode):
  - Basic types: int, float, str, bool, None
  - Arithmetic: +, -, *, //, %, ** (safe divisors)
  - Comparisons: ==, !=, <, >, <=, >= (type-safe)
  - Boolean: and, or, not
  - Unary: -, + (numeric only)
  - String ops: upper, lower, strip, replace, split, join, startswith,
    endswith, find, count, center, isdigit, isalpha, title, lstrip, rstrip,
    repeat (*)
  - String indexing and slicing (bounds-safe via string literals)
  - F-strings with safe interpolation
  - Lists: literal, len, index, slice, sorted, append, pop (guarded), sort,
    reverse
  - Tuples: literal, indexing, unpacking
  - Dicts: literal, .get, .keys, .values, .items (sorted), len, ``in``
  - Sets: literal, union, intersection, difference, symmetric_difference
    (sorted for determinism)
  - List/dict/set comprehensions (fresh loop vars)
  - For loops (range-based, guaranteed iteration)
  - While loops (bounded counter)
  - If/elif/else (scope-isolated)
  - Try/except (specific exception types)
  - Functions: positional, default args, return values (try/except wrapped)
  - Keyword-only parameters (*, kw=default)
  - *args, **kwargs
  - Classes: __init__, __repr__, methods, field access (try/except wrapped)
  - Inheritance with super()
  - Closures: simple capture, counter, accumulator
  - break/continue in for loops
  - enumerate(), zip()
  - Nested loops
  - Dict iteration with tuple unpacking (sorted)
  - Starred unpacking, nested tuple unpacking, swap
  - assert (always-true)
  - del (define then delete)
  - isinstance checks
  - Membership tests (x in list)
  - Chained comparisons (same-type numeric)
  - Negative indexing on known-length lists
  - Multiple except clauses

Usage:
    python tools/fuzz_compiler.py --mode safe --count 100 --seed 42
    python tools/fuzz_compiler.py --mode reject --count 50
    python tools/fuzz_compiler.py --mode compile-only --count 200
    python tools/fuzz_compiler.py --count 10 --seed 0 --verbose
    python tools/fuzz_compiler.py --count 500 --output-dir /tmp/fuzz_failures

Environment:
    PYTHONPATH        Defaults to "src" if not set.
    PYTHONHASHSEED    Forced to "0" for deterministic dict ordering.
    MOLT_DETERMINISTIC  Forced to "1".
    CARGO_TARGET_DIR  Respected from environment (use throughput_env.sh).
    MOLT_DIFF_TMPDIR / TMPDIR  For temp directory placement.
"""

from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools.fuzz_compiler_cli import main  # noqa: E402
from tools.fuzz_compiler_compile_only import CompileOnlyFuzzer  # noqa: E402
from tools.fuzz_compiler_core import fuzz_one_reject, fuzz_one_safe  # noqa: E402
from tools.fuzz_compiler_driver import (  # noqa: E402
    run_generate_only,
    run_reject_fuzzer,
    run_safe_fuzzer,
)
from tools.fuzz_compiler_execution import (  # noqa: E402
    _build_env,
    _extract_binary,
    _repo_root,
    compile_molt,
    run_cpython,
    run_molt_binary,
)
from tools.fuzz_compiler_reject import RejectProgramGenerator  # noqa: E402
from tools.fuzz_compiler_reporting import (  # noqa: E402
    _log,
    _print_diff_snippet,
    _save_failure,
)
from tools.fuzz_compiler_safe import SafeProgramGenerator  # noqa: E402
from tools.fuzz_compiler_shrink import (  # noqa: E402
    _fuzz_one_program,
    _shrink_program,
    _validate_syntax,
)
from tools.fuzz_compiler_types import (  # noqa: E402
    ALL_TYPES,
    NUMERIC_TYPES,
    T_ANY,
    T_BOOL,
    T_DICT,
    T_FLOAT,
    T_INT,
    T_LIST_INT,
    T_LIST_STR,
    T_NONE,
    T_SET_INT,
    T_STR,
    T_TUPLE,
    FuzzResult,
    FuzzSummary,
)

__all__ = [
    "ALL_TYPES",
    "CompileOnlyFuzzer",
    "FuzzResult",
    "FuzzSummary",
    "NUMERIC_TYPES",
    "RejectProgramGenerator",
    "SafeProgramGenerator",
    "T_ANY",
    "T_BOOL",
    "T_DICT",
    "T_FLOAT",
    "T_INT",
    "T_LIST_INT",
    "T_LIST_STR",
    "T_NONE",
    "T_SET_INT",
    "T_STR",
    "T_TUPLE",
    "_build_env",
    "_extract_binary",
    "_fuzz_one_program",
    "_log",
    "_print_diff_snippet",
    "_repo_root",
    "_save_failure",
    "_shrink_program",
    "_validate_syntax",
    "compile_molt",
    "fuzz_one_reject",
    "fuzz_one_safe",
    "main",
    "run_cpython",
    "run_generate_only",
    "run_molt_binary",
    "run_reject_fuzzer",
    "run_safe_fuzzer",
]

if __name__ == "__main__":
    sys.exit(main())
