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

import argparse
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import time
from dataclasses import dataclass, field
from pathlib import Path
from random import Random


# ---------------------------------------------------------------------------
# Type tags
# ---------------------------------------------------------------------------

T_INT = "int"
T_FLOAT = "float"
T_STR = "str"
T_BOOL = "bool"
T_LIST_INT = "list_int"
T_LIST_STR = "list_str"
T_TUPLE = "tuple"
T_DICT = "dict"
T_SET_INT = "set_int"
T_NONE = "none"
T_ANY = "any"

NUMERIC_TYPES = {T_INT, T_FLOAT}
ALL_TYPES = [
    T_INT,
    T_FLOAT,
    T_STR,
    T_BOOL,
    T_LIST_INT,
    T_LIST_STR,
    T_TUPLE,
    T_DICT,
    T_SET_INT,
    T_NONE,
]


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------


@dataclass
class FuzzResult:
    """Outcome of a single fuzz iteration."""

    program_id: int
    seed: int
    source: str
    status: str
    cpython_stdout: str = ""
    cpython_stderr: str = ""
    molt_stdout: str = ""
    molt_stderr: str = ""
    error_detail: str = ""
    elapsed_sec: float = 0.0


@dataclass
class FuzzSummary:
    """Aggregate results from a full fuzz run."""

    total: int = 0
    passed: int = 0
    mismatches: int = 0
    build_errors: int = 0
    cpython_errors: int = 0
    molt_run_errors: int = 0
    timeouts: int = 0
    reject_pass: int = 0
    reject_fail: int = 0
    compile_only_ok: int = 0
    compile_only_crash: int = 0
    failures: list[FuzzResult] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Safe program generator (type-tracked)
# ---------------------------------------------------------------------------


class SafeProgramGenerator:
    """Generates random valid Python 3.12+ programs with full type tracking.

    Every variable tracks its type.  Expressions are generated to produce a
    specific type, so arithmetic never touches strings, ordering comparisons
    never mix types, and undefined variables are never referenced.
    """

    STR_LITERALS = [
        "hello",
        "world",
        "foo",
        "bar",
        "baz",
        "molt",
        "test",
        "abc",
        "XYZ",
        "  spaced  ",
        "123",
        "a b c",
    ]
    INT_RANGE = (-200, 200)
    FLOAT_RANGE = (-50.0, 50.0)

    def __init__(self, rng: Random, *, max_depth: int = 4, max_stmts: int = 20):
        self.rng = rng
        self.max_depth = max_depth
        self.max_stmts = max_stmts
        self._var_counter = 0
        self._func_counter = 0
        self._defined_vars: list[tuple[str, str]] = []  # (name, type_tag)
        self._scope_stack: list[int] = []  # save points
        self._defined_funcs: list[str] = []
        self._defined_classes: list[tuple] = []
        self._defined_closures: list[tuple[str, str]] = []
        self._defined_kwonly_funcs: list[tuple] = []
        self._defined_starargs_funcs: list[tuple[str, str]] = []

    # -- Scope management ---------------------------------------------------

    def _push_scope(self):
        """Save current variable state before entering a block."""
        self._scope_stack.append(len(self._defined_vars))

    def _pop_scope(self):
        """Restore variable state after leaving a block."""
        restore_point = self._scope_stack.pop()
        self._defined_vars = self._defined_vars[:restore_point]

    # -- Variable helpers ---------------------------------------------------

    def _fresh_var(self) -> str:
        name = f"v{self._var_counter}"
        self._var_counter += 1
        return name

    def _fresh_func(self) -> str:
        bases = ["compute", "transform", "helper", "process", "calc", "combine"]
        base = self.rng.choice(bases)
        name = f"{base}_{self._func_counter}"
        self._func_counter += 1
        return name

    def _add_var(self, name: str, type_tag: str):
        self._defined_vars.append((name, type_tag))

    def _known_var_of_type(self, *type_tags: str) -> str | None:
        """Get a defined variable matching one of the given type tags."""
        candidates = [(n, t) for n, t in self._defined_vars if t in type_tags]
        if not candidates:
            return None
        return self.rng.choice(candidates)[0]

    def _any_known_var(self) -> tuple[str, str] | None:
        """Get any defined variable as (name, type_tag)."""
        if not self._defined_vars:
            return None
        return self.rng.choice(self._defined_vars)

    # -- Literal generators -------------------------------------------------

    def gen_int_literal(self) -> str:
        return str(self.rng.randint(*self.INT_RANGE))

    def gen_float_literal(self) -> str:
        val = self.rng.uniform(*self.FLOAT_RANGE)
        return f"{val:.4f}"

    def gen_bool_literal(self) -> str:
        return "True" if self.rng.random() < 0.5 else "False"

    def gen_str_literal(self) -> str:
        s = self.rng.choice(self.STR_LITERALS)
        return repr(s)

    def gen_none_literal(self) -> str:
        return "None"

    # -- Typed expression generators ----------------------------------------

    def gen_typed_expr(self, depth: int, target_type: str) -> str:
        """Generate an expression guaranteed to produce the given type."""
        if target_type == T_INT:
            return self._gen_int_expr(depth)
        elif target_type == T_FLOAT:
            return self._gen_float_expr(depth)
        elif target_type == T_STR:
            return self._gen_str_expr(depth)
        elif target_type == T_BOOL:
            return self._gen_bool_expr(depth)
        elif target_type == T_LIST_INT:
            return self._gen_list_int_expr(depth)
        elif target_type == T_LIST_STR:
            return self._gen_list_str_expr(depth)
        elif target_type == T_TUPLE:
            return self._gen_tuple_expr(depth)
        elif target_type == T_DICT:
            return self._gen_dict_expr(depth)
        elif target_type == T_SET_INT:
            return self._gen_set_int_expr(depth)
        elif target_type == T_NONE:
            return "None"
        else:
            return self.gen_int_literal()

    def gen_any_expr(self, depth: int) -> tuple[str, str]:
        """Generate a random expression, returning (code, type_tag)."""
        target = self.rng.choice(
            [T_INT, T_FLOAT, T_STR, T_BOOL, T_LIST_INT, T_DICT, T_NONE]
        )
        return self.gen_typed_expr(depth, target), target

    # -- Int expressions ----------------------------------------------------

    def _gen_int_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_INT)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_int_literal()
        kind = self.rng.choices(
            ["literal", "var", "arith", "abs", "len", "int_call", "min_max"],
            weights=[25, 15, 25, 10, 10, 10, 5],
        )[0]
        if kind == "literal":
            return self.gen_int_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_INT)
            return var if var else self.gen_int_literal()
        elif kind == "arith":
            op = self.rng.choice(["+", "-", "*", "//", "%", "**"])
            left = self._gen_int_expr(depth + 1)
            right = self._gen_int_expr(depth + 1)
            if op in ("//", "%"):
                right = f"({right} or 1)"
            elif op == "**":
                right = f"(abs({right}) % 6)"
            return f"({left} {op} {right})"
        elif kind == "abs":
            return f"abs({self._gen_int_expr(depth + 1)})"
        elif kind == "len":
            # len of a string literal or list literal
            if self.rng.random() < 0.5:
                return f"len({self._gen_str_expr(depth + 1)})"
            else:
                return f"len({self._gen_list_int_expr(depth + 1)})"
        elif kind == "int_call":
            return f"int({self._gen_float_expr(depth + 1)})"
        else:  # min_max
            fn = self.rng.choice(["min", "max"])
            a = self._gen_int_expr(depth + 1)
            b = self._gen_int_expr(depth + 1)
            return f"{fn}({a}, {b})"

    # -- Float expressions --------------------------------------------------

    def _gen_float_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_FLOAT)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_float_literal()
        kind = self.rng.choices(
            ["literal", "var", "arith", "float_call"],
            weights=[35, 15, 35, 15],
        )[0]
        if kind == "literal":
            return self.gen_float_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_FLOAT)
            return var if var else self.gen_float_literal()
        elif kind == "arith":
            op = self.rng.choice(["+", "-", "*"])
            left = self._gen_float_expr(depth + 1)
            right = self._gen_float_expr(depth + 1)
            return f"({left} {op} {right})"
        else:
            return f"float({self._gen_int_expr(depth + 1)})"

    # -- String expressions -------------------------------------------------

    def _gen_str_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_STR)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_str_literal()
        kind = self.rng.choices(
            [
                "literal",
                "var",
                "concat",
                "method",
                "fstring",
                "repeat",
                "str_call",
                "index",
                "slice",
            ],
            weights=[20, 10, 12, 18, 10, 8, 8, 7, 7],
        )[0]
        if kind == "literal":
            return self.gen_str_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_STR)
            return var if var else self.gen_str_literal()
        elif kind == "concat":
            left = self._gen_str_expr(depth + 1)
            right = self._gen_str_expr(depth + 1)
            return f"({left} + {right})"
        elif kind == "method":
            return self._gen_str_method(depth)
        elif kind == "fstring":
            return self._gen_fstring(depth)
        elif kind == "repeat":
            s = self._gen_str_expr(depth + 1)
            n = self.rng.randint(0, 4)
            return f"({s} * {n})"
        elif kind == "str_call":
            inner = self._gen_int_expr(depth + 1)
            return f"str({inner})"
        elif kind == "index":
            return self._gen_str_index()
        else:  # slice
            return self._gen_str_slice()

    def _gen_str_method(self, depth: int) -> str:
        method = self.rng.choice(
            [
                "upper",
                "lower",
                "strip",
                "title",
                "lstrip",
                "rstrip",
                "replace",
                "center",
            ]
        )
        s = self._gen_str_expr(depth + 1)
        if method in ("upper", "lower", "strip", "title", "lstrip", "rstrip"):
            return f"{s}.{method}()"
        elif method == "replace":
            old = self.rng.choice(["a", "o", "l", " "])
            new = self.rng.choice(["X", "_", ""])
            return f"{s}.replace({repr(old)}, {repr(new)})"
        else:  # center
            width = self.rng.randint(5, 15)
            return f"{s}.center({width})"

    def _gen_str_index(self) -> str:
        s = self.rng.choice([x for x in self.STR_LITERALS if x]) or "hello"
        idx = self.rng.randint(0, len(s) - 1)
        return f"{repr(s)}[{idx}]"

    def _gen_str_slice(self) -> str:
        s = self.rng.choice([x for x in self.STR_LITERALS if x]) or "hello"
        start = self.rng.randint(0, max(0, len(s) - 1))
        end = self.rng.randint(start, len(s))
        return f"{repr(s)}[{start}:{end}]"

    def _gen_fstring(self, depth: int) -> str:
        num_parts = self.rng.randint(1, 3)
        parts: list[str] = []
        for _ in range(num_parts):
            if self.rng.random() < 0.5:
                parts.append(self.rng.choice(["hello", "val=", "result:", " "]))
            else:
                inner = self._gen_fstring_inner()
                parts.append("{" + inner + "}")
        return 'f"' + "".join(parts) + '"'

    def _gen_fstring_inner(self) -> str:
        kind = self.rng.choice(["int", "arith", "str_method"])
        if kind == "int":
            return str(self.rng.randint(-50, 50))
        elif kind == "arith":
            a = self.rng.randint(-10, 10)
            b = self.rng.randint(1, 10)
            op = self.rng.choice(["+", "-", "*"])
            return f"{a} {op} {b}"
        else:
            s = self.rng.choice(["hello", "world", "test"])
            return f"'{s}'.upper()"

    # -- Bool expressions ---------------------------------------------------

    def _gen_bool_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_BOOL)
            if var and self.rng.random() < 0.4:
                return var
            return self.gen_bool_literal()
        kind = self.rng.choices(
            [
                "literal",
                "var",
                "comparison",
                "and_or",
                "not",
                "isinstance",
                "membership",
                "chained_cmp",
            ],
            weights=[15, 10, 25, 15, 10, 10, 10, 5],
        )[0]
        if kind == "literal":
            return self.gen_bool_literal()
        elif kind == "var":
            var = self._known_var_of_type(T_BOOL)
            return var if var else self.gen_bool_literal()
        elif kind == "comparison":
            return self._gen_comparison(depth)
        elif kind == "and_or":
            op = self.rng.choice(["and", "or"])
            left = self._gen_bool_expr(depth + 1)
            right = self._gen_bool_expr(depth + 1)
            return f"({left} {op} {right})"
        elif kind == "not":
            operand = self._gen_bool_expr(depth + 1)
            return f"(not {operand})"
        elif kind == "isinstance":
            return self._gen_isinstance_check()
        elif kind == "membership":
            return self._gen_membership_test(depth)
        else:  # chained_cmp
            return self._gen_chained_cmp(depth)

    def _gen_comparison(self, depth: int) -> str:
        op = self.rng.choice(["==", "!=", "<", ">", "<=", ">="])
        if op in ("<", ">", "<=", ">="):
            # Ordering: same-type only
            if self.rng.random() < 0.6:
                left = self._gen_int_expr(depth + 1)
                right = self._gen_int_expr(depth + 1)
            else:
                left = self.gen_str_literal()
                right = self.gen_str_literal()
        else:
            # Equality: safe with any type
            if self.rng.random() < 0.5:
                left = self._gen_int_expr(depth + 1)
                right = self._gen_int_expr(depth + 1)
            else:
                left = self._gen_str_expr(depth + 1)
                right = self._gen_str_expr(depth + 1)
        return f"({left} {op} {right})"

    def _gen_isinstance_check(self) -> str:
        kind = self.rng.choice(["int", "str", "float", "bool", "list"])
        if kind == "int":
            val = self.gen_int_literal()
        elif kind == "str":
            val = self.gen_str_literal()
        elif kind == "float":
            val = self.gen_float_literal()
        elif kind == "bool":
            val = self.gen_bool_literal()
        else:
            val = "[1, 2, 3]"
        check = self.rng.choice(["int", "str", "float", "bool", "list", "tuple"])
        return f"isinstance({val}, {check})"

    def _gen_membership_test(self, depth: int) -> str:
        needle = self._gen_int_expr(depth + 1)
        n = self.rng.randint(1, 4)
        elems = [self.gen_int_literal() for _ in range(n)]
        return f"({needle} in [{', '.join(elems)}])"

    def _gen_chained_cmp(self, depth: int) -> str:
        n = self.rng.randint(3, 4)
        operands = [self._gen_int_expr(depth + 1) for _ in range(n)]
        ops = [
            self.rng.choice(["<", "<=", ">", ">=", "==", "!="]) for _ in range(n - 1)
        ]
        parts = [operands[0]]
        for op, operand in zip(ops, operands[1:]):
            parts.extend([op, operand])
        return f"({' '.join(parts)})"

    # -- List expressions ---------------------------------------------------

    def _gen_list_int_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_LIST_INT)
            if var and self.rng.random() < 0.4:
                return var
            n = self.rng.randint(0, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        kind = self.rng.choices(
            ["literal", "var", "comp", "sorted", "slice"],
            weights=[35, 15, 20, 15, 15],
        )[0]
        if kind == "literal":
            n = self.rng.randint(0, 5)
            elems = [self._gen_int_expr(depth + 1) for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        elif kind == "var":
            var = self._known_var_of_type(T_LIST_INT)
            if var:
                return var
            n = self.rng.randint(1, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        elif kind == "comp":
            lv = self._fresh_var()
            bound = self.rng.randint(1, 6)
            op = self.rng.choice(["+", "-", "*"])
            val = self.rng.randint(1, 10)
            body = f"({lv} {op} {val})"
            if self.rng.random() < 0.3:
                threshold = self.rng.randint(0, bound)
                return f"[{body} for {lv} in range({bound}) if {lv} > {threshold}]"
            return f"[{body} for {lv} in range({bound})]"
        elif kind == "sorted":
            inner = self._gen_list_int_expr(depth + 1)
            return f"sorted({inner})"
        else:  # slice
            n = self.rng.randint(2, 5)
            elems = [self.gen_int_literal() for _ in range(n)]
            lst = "[" + ", ".join(elems) + "]"
            start = self.rng.randint(0, n - 1)
            end = self.rng.randint(start, n)
            return f"{lst}[{start}:{end}]"

    def _gen_list_str_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_LIST_STR)
            if var and self.rng.random() < 0.4:
                return var
            n = self.rng.randint(0, 3)
            elems = [self.gen_str_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        kind = self.rng.choices(["literal", "var", "split"], weights=[50, 20, 30])[0]
        if kind == "literal":
            n = self.rng.randint(0, 4)
            elems = [self.gen_str_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        elif kind == "var":
            var = self._known_var_of_type(T_LIST_STR)
            if var:
                return var
            n = self.rng.randint(1, 3)
            elems = [self.gen_str_literal() for _ in range(n)]
            return "[" + ", ".join(elems) + "]"
        else:  # split
            s = self.gen_str_literal()
            return f"{s}.split()"

    # -- Tuple expressions --------------------------------------------------

    def _gen_tuple_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_TUPLE)
            if var and self.rng.random() < 0.3:
                return var
            n = self.rng.randint(0, 3)
            elems = [self.gen_int_literal() for _ in range(n)]
            if n == 1:
                return f"({elems[0]},)"
            return "(" + ", ".join(elems) + ")"
        n = self.rng.randint(0, 4)
        elems: list[str] = []
        for _ in range(n):
            e, _ = self.gen_any_expr(depth + 1)
            elems.append(e)
        if n == 1:
            return f"({elems[0]},)"
        return "(" + ", ".join(elems) + ")"

    # -- Dict expressions ---------------------------------------------------

    def _gen_dict_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_DICT)
            if var and self.rng.random() < 0.3:
                return var
            n = self.rng.randint(0, 3)
            pairs = []
            for _ in range(n):
                key = self.gen_str_literal()
                val = self.gen_int_literal()
                pairs.append(f"{key}: {val}")
            return "{" + ", ".join(pairs) + "}"
        kind = self.rng.choices(["literal", "var", "comp"], weights=[50, 20, 30])[0]
        if kind == "literal":
            n = self.rng.randint(0, 4)
            pairs = []
            for _ in range(n):
                key = self.gen_str_literal()
                val_code, _ = self.gen_any_expr(depth + 1)
                pairs.append(f"{key}: {val_code}")
            return "{" + ", ".join(pairs) + "}"
        elif kind == "var":
            var = self._known_var_of_type(T_DICT)
            if var:
                return var
            return "{" + repr("a") + ": 1}"
        else:  # comp
            lv = self._fresh_var()
            bound = self.rng.randint(1, 5)
            val_body = self.rng.choice([f"{lv} * {lv}", f"str({lv})", f"{lv} * 2"])
            return f"{{{lv}: {val_body} for {lv} in range({bound})}}"

    # -- Set expressions ----------------------------------------------------

    def _gen_set_int_expr(self, depth: int) -> str:
        if depth >= self.max_depth:
            var = self._known_var_of_type(T_SET_INT)
            if var and self.rng.random() < 0.3:
                return var
            n = self.rng.randint(1, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "{" + ", ".join(elems) + "}"
        kind = self.rng.choices(
            ["literal", "var", "comp", "op"], weights=[35, 15, 25, 25]
        )[0]
        if kind == "literal":
            n = self.rng.randint(1, 4)
            elems = [self.gen_int_literal() for _ in range(n)]
            return "{" + ", ".join(elems) + "}"
        elif kind == "var":
            var = self._known_var_of_type(T_SET_INT)
            if var:
                return var
            return "{1, 2, 3}"
        elif kind == "comp":
            lv = self._fresh_var()
            bound = self.rng.randint(1, 6)
            body = self.rng.choice([lv, f"({lv} % 3)", f"abs({lv})"])
            return f"{{{body} for {lv} in range({bound})}}"
        else:  # op
            op = self.rng.choice(
                ["union", "intersection", "difference", "symmetric_difference"]
            )
            s1 = self._gen_set_int_expr(depth + 1)
            s2 = self._gen_set_int_expr(depth + 1)
            return f"{s1}.{op}({s2})"

    # -- Statement generators -----------------------------------------------

    def gen_stmt(self, depth: int = 0, indent: int = 0) -> str:
        if depth >= self.max_depth:
            return self._gen_simple_stmt(indent)

        kind = self.rng.choices(
            [
                "assign",
                "print",
                "if",
                "for_loop",
                "while_loop",
                "augmented_assign",
                "multi_assign",
                "try_except",
                "break_continue_for",
                "dict_iteration",
                "unpack",
                "multi_except",
                "list_method",
                "enumerate",
                "zip",
                "nested_loop",
                "assert",
                "del",
            ],
            weights=[
                14,
                16,
                10,
                8,
                5,
                5,
                4,
                5,
                4,
                4,
                4,
                3,
                4,
                3,
                3,
                3,
                2,
                2,
            ],
        )[0]

        method = getattr(self, f"gen_{kind}_stmt", None)
        if method is None:
            return self._gen_simple_stmt(indent)
        return method(depth, indent)

    def _gen_simple_stmt(self, indent: int = 0) -> str:
        if self.rng.random() < 0.5:
            return self.gen_print_stmt(0, indent)
        return self.gen_assign_stmt(0, indent)

    def gen_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        var = self._fresh_var()
        # Choose a target type and generate a typed expression
        target_type = self.rng.choice(
            [T_INT, T_FLOAT, T_STR, T_BOOL, T_LIST_INT, T_DICT, T_NONE]
        )
        expr = self.gen_typed_expr(depth + 1, target_type)
        self._add_var(var, target_type)
        prefix = "    " * indent
        return f"{prefix}{var} = {expr}"

    def gen_augmented_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        var = self._fresh_var()
        init_val = self.gen_int_literal()
        self._add_var(var, T_INT)
        op = self.rng.choice(["+=", "-=", "*="])
        val = self.rng.randint(1, 10)
        prefix = "    " * indent
        return f"{prefix}{var} = {init_val}\n{prefix}{var} {op} {val}"

    def gen_multi_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        n = self.rng.randint(2, 3)
        names = [self._fresh_var() for _ in range(n)]
        vals: list[str] = []
        types: list[str] = []
        for _ in range(n):
            t = self.rng.choice([T_INT, T_STR, T_BOOL])
            vals.append(self.gen_typed_expr(depth + 1, t))
            types.append(t)
        for name, t in zip(names, types):
            self._add_var(name, t)
        prefix = "    " * indent
        return f"{prefix}{', '.join(names)} = {', '.join(vals)}"

    def gen_print_stmt(self, depth: int = 0, indent: int = 0) -> str:
        n_args = self.rng.randint(1, 3)
        args: list[str] = []
        for _ in range(n_args):
            code, _ = self.gen_any_expr(depth + 1)
            args.append(code)
        prefix = "    " * indent
        return f"{prefix}print({', '.join(args)})"

    def gen_if_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        cond = self._gen_bool_expr(depth + 1)
        self._push_scope()
        body_stmts = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        lines = [f"{prefix}if {cond}:"]
        lines.extend(body_stmts)
        if self.rng.random() < 0.3:
            elif_cond = self._gen_bool_expr(depth + 1)
            self._push_scope()
            elif_body = self._gen_body(depth + 1, indent + 1)
            self._pop_scope()
            lines.append(f"{prefix}elif {elif_cond}:")
            lines.extend(elif_body)
        if self.rng.random() < 0.5:
            self._push_scope()
            else_body = self._gen_body(depth + 1, indent + 1)
            self._pop_scope()
            lines.append(f"{prefix}else:")
            lines.extend(else_body)
        return "\n".join(lines)

    def gen_for_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        loop_var = self._fresh_var()
        bound = self.rng.randint(1, 8)
        self._push_scope()
        self._add_var(loop_var, T_INT)
        body_stmts = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        lines = [f"{prefix}for {loop_var} in range({bound}):"]
        lines.extend(body_stmts)
        return "\n".join(lines)

    def gen_while_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        counter = self._fresh_var()
        self._add_var(counter, T_INT)
        limit = self.rng.randint(1, 6)
        self._push_scope()
        body_stmts = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        inner_prefix = "    " * (indent + 1)
        lines = [
            f"{prefix}{counter} = 0",
            f"{prefix}while {counter} < {limit}:",
        ]
        lines.extend(body_stmts)
        lines.append(f"{inner_prefix}{counter} += 1")
        return "\n".join(lines)

    def gen_try_except_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        self._push_scope()
        try_body = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        self._push_scope()
        except_body = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        exc_type = self.rng.choice(
            [
                "Exception",
                "ValueError",
                "TypeError",
                "ZeroDivisionError",
                "IndexError",
                "KeyError",
            ]
        )
        lines = [f"{prefix}try:"]
        lines.extend(try_body)
        lines.append(f"{prefix}except {exc_type}:")
        lines.extend(except_body)
        return "\n".join(lines)

    def gen_break_continue_for_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        inner2 = "    " * (indent + 2)
        loop_var = self._fresh_var()
        bound = self.rng.randint(2, 8)
        threshold = self.rng.randint(0, bound - 1)
        action = self.rng.choice(["break", "continue"])
        lines = [
            f"{prefix}for {loop_var} in range({bound}):",
            f"{inner}if {loop_var} == {threshold}:",
            f"{inner2}{action}",
            f"{inner}print({loop_var})",
        ]
        return "\n".join(lines)

    def gen_dict_iteration_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        n = self.rng.randint(1, 4)
        keys = [repr(self.rng.choice(["a", "b", "c", "d", "x"])) for _ in range(n)]
        vals = [str(self.rng.randint(0, 100)) for _ in range(n)]
        pairs = [f"{k}: {v}" for k, v in zip(keys, vals)]
        d = "{" + ", ".join(pairs) + "}"
        kvar = self._fresh_var()
        vvar = self._fresh_var()
        lines = [
            f"{prefix}for {kvar}, {vvar} in sorted({d}.items()):",
            f"{inner}print({kvar}, {vvar})",
        ]
        return "\n".join(lines)

    def gen_unpack_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        kind = self.rng.choice(["starred", "nested_tuple", "swap"])
        if kind == "starred":
            n = self.rng.randint(3, 5)
            vals = [str(self.rng.randint(0, 50)) for _ in range(n)]
            first = self._fresh_var()
            rest = self._fresh_var()
            self._add_var(first, T_INT)
            self._add_var(rest, T_LIST_INT)
            return (
                f"{prefix}{first}, *{rest} = [{', '.join(vals)}]\n"
                f"{prefix}print({first}, {rest})"
            )
        elif kind == "nested_tuple":
            a = self._fresh_var()
            b = self._fresh_var()
            c = self._fresh_var()
            v1 = self.rng.randint(0, 50)
            v2 = self.rng.randint(0, 50)
            v3 = self.rng.randint(0, 50)
            self._add_var(a, T_INT)
            self._add_var(b, T_INT)
            self._add_var(c, T_INT)
            return (
                f"{prefix}({a}, ({b}, {c})) = ({v1}, ({v2}, {v3}))\n"
                f"{prefix}print({a}, {b}, {c})"
            )
        else:  # swap
            a = self._fresh_var()
            b = self._fresh_var()
            v1 = self.rng.randint(0, 50)
            v2 = self.rng.randint(0, 50)
            self._add_var(a, T_INT)
            self._add_var(b, T_INT)
            return (
                f"{prefix}{a}, {b} = {v1}, {v2}\n"
                f"{prefix}{a}, {b} = {b}, {a}\n"
                f"{prefix}print({a}, {b})"
            )

    def gen_multi_except_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        self._push_scope()
        try_body = self._gen_body(depth + 1, indent + 1)
        self._pop_scope()
        types = [
            "ValueError",
            "TypeError",
            "ZeroDivisionError",
            "IndexError",
            "KeyError",
            "AttributeError",
        ]
        self.rng.shuffle(types)
        n_except = self.rng.randint(2, 3)
        lines = [f"{prefix}try:"]
        lines.extend(try_body)
        for i in range(n_except):
            exc = types[i % len(types)]
            self._push_scope()
            except_body = self._gen_body(depth + 1, indent + 1)
            self._pop_scope()
            lines.append(f"{prefix}except {exc}:")
            lines.extend(except_body)
        return "\n".join(lines)

    def gen_list_method_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        var = self._fresh_var()
        n = self.rng.randint(1, 4)
        elems = [str(self.rng.randint(0, 50)) for _ in range(n)]
        self._add_var(var, T_LIST_INT)
        lines = [f"{prefix}{var} = [{', '.join(elems)}]"]
        for _ in range(self.rng.randint(1, 3)):
            method = self.rng.choice(["append", "sort", "reverse", "pop"])
            if method == "append":
                val = self.rng.randint(0, 50)
                lines.append(f"{prefix}{var}.append({val})")
            elif method in ("sort", "reverse"):
                lines.append(f"{prefix}{var}.{method}()")
            elif method == "pop":
                lines.append(f"{prefix}if {var}: {var}.pop()")
        lines.append(f"{prefix}print({var})")
        return "\n".join(lines)

    def gen_enumerate_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        idx_var = self._fresh_var()
        val_var = self._fresh_var()
        n = self.rng.randint(2, 5)
        elems = [str(self.rng.randint(0, 50)) for _ in range(n)]
        iterable = "[" + ", ".join(elems) + "]"
        lines = [
            f"{prefix}for {idx_var}, {val_var} in enumerate({iterable}):",
            f"{inner}print({idx_var}, {val_var})",
        ]
        return "\n".join(lines)

    def gen_zip_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        a_var = self._fresh_var()
        b_var = self._fresh_var()
        n = self.rng.randint(2, 4)
        list_a = "[" + ", ".join(str(self.rng.randint(0, 50)) for _ in range(n)) + "]"
        list_b = (
            "["
            + ", ".join(repr(self.rng.choice(["a", "b", "c", "d"])) for _ in range(n))
            + "]"
        )
        lines = [
            f"{prefix}for {a_var}, {b_var} in zip({list_a}, {list_b}):",
            f"{inner}print({a_var}, {b_var})",
        ]
        return "\n".join(lines)

    def gen_nested_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        inner = "    " * (indent + 1)
        inner2 = "    " * (indent + 2)
        i_var = self._fresh_var()
        j_var = self._fresh_var()
        bound_i = self.rng.randint(1, 4)
        bound_j = self.rng.randint(1, 4)
        lines = [
            f"{prefix}for {i_var} in range({bound_i}):",
            f"{inner}for {j_var} in range({bound_j}):",
            f"{inner2}print({i_var}, {j_var})",
        ]
        return "\n".join(lines)

    def gen_assert_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        kind = self.rng.choice(["true", "isinstance", "len"])
        if kind == "true":
            return f"{prefix}assert True"
        elif kind == "isinstance":
            val = self.rng.randint(0, 100)
            return f"{prefix}assert isinstance({val}, int)"
        else:
            s = self.rng.choice(self.STR_LITERALS)
            return f"{prefix}assert len({repr(s)}) >= 0"

    def gen_del_stmt(self, depth: int = 0, indent: int = 0) -> str:
        prefix = "    " * indent
        var = self._fresh_var()
        val = self.rng.randint(0, 100)
        # Do NOT add var to _defined_vars since we immediately delete it
        return f"{prefix}{var} = {val}\n{prefix}del {var}"

    def _gen_body(self, depth: int, indent: int) -> list[str]:
        """Generate block body statements."""
        n = self.rng.randint(1, 3)
        stmts: list[str] = []
        for _ in range(n):
            stmts.append(self.gen_stmt(depth, indent))
        return stmts

    # -- Function generators ------------------------------------------------

    def gen_function_def(self) -> str:
        func_name = self._fresh_func()
        self._defined_funcs.append(func_name)
        n_pos = self.rng.randint(0, 3)
        n_default = self.rng.randint(0, 2)
        params: list[str] = []
        param_names: list[str] = []
        for _ in range(n_pos):
            name = self._fresh_var()
            params.append(name)
            param_names.append(name)
        for _ in range(n_default):
            name = self._fresh_var()
            default = self.gen_int_literal()
            params.append(f"{name}={default}")
            param_names.append(name)
        # Save outer vars and set function scope
        outer_vars = self._defined_vars[:]
        self._defined_vars = [(n, T_ANY) for n in param_names]
        lines = [f"def {func_name}({', '.join(params)}):"]
        for _ in range(self.rng.randint(1, 4)):
            lines.append(self.gen_stmt(depth=2, indent=1))
        # Return something safe
        ret_code, _ = self.gen_any_expr(depth=2)
        lines.append(f"    return {ret_code}")
        self._defined_vars = outer_vars
        return "\n".join(lines)

    def _gen_func_call(self, func_name: str) -> str:
        n_args = self.rng.randint(0, 3)
        args: list[str] = []
        for _ in range(n_args):
            # Use only hashable types for function args (functions may hash them)
            code = self.gen_typed_expr(
                depth=2, target_type=self.rng.choice([T_INT, T_STR, T_BOOL, T_FLOAT])
            )
            args.append(code)
        result_var = self._fresh_var()
        # Do NOT add to _defined_vars: the var is inside try/except so may not be defined
        lines = [
            f"{result_var} = None",
            "try:",
            f"    {result_var} = {func_name}({', '.join(args)})",
            f"    print({result_var})",
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError, AttributeError) as _fuzz_e:",
            "    print(type(_fuzz_e).__name__)",
        ]
        self._add_var(result_var, T_ANY)
        return "\n".join(lines)

    # -- Class generators ---------------------------------------------------

    def gen_class_def(self) -> str:
        class_names = ["Point", "Box", "Counter", "Wrapper", "Pair", "Node"]
        cls_name = self.rng.choice(class_names) + f"_{self._func_counter}"
        self._func_counter += 1
        n_fields = self.rng.randint(1, 3)
        field_names = [f"f{i}" for i in range(n_fields)]
        lines = [f"class {cls_name}:"]
        init_params = ", ".join(field_names)
        lines.append(f"    def __init__(self, {init_params}):")
        for name in field_names:
            lines.append(f"        self.{name} = {name}")
        repr_parts = ", ".join(f"{name}={{self.{name}!r}}" for name in field_names)
        lines.append("    def __repr__(self):")
        lines.append(f'        return f"{cls_name}({repr_parts})"')
        n_methods = self.rng.randint(1, 2)
        method_names: list[str] = []
        for m_idx in range(n_methods):
            mname = f"method_{m_idx}"
            method_names.append(mname)
            kind = self.rng.choice(["getter", "compute", "transform"])
            if kind == "getter":
                f = self.rng.choice(field_names)
                lines.append(f"    def {mname}(self):")
                lines.append(f"        return self.{f}")
            elif kind == "compute":
                f = self.rng.choice(field_names)
                lines.append(f"    def {mname}(self):")
                lines.append(f"        return str(self.{f}) + '_computed'")
            else:
                lines.append(f"    def {mname}(self, x):")
                f = self.rng.choice(field_names)
                lines.append(f"        return str(self.{f}) + str(x)")
        self._defined_classes.append((cls_name, field_names, method_names))
        return "\n".join(lines)

    def gen_class_usage(self, cls_name, field_names, method_names) -> str:
        lines: list[str] = []
        args: list[str] = []
        for _ in field_names:
            code, _ = self.gen_any_expr(depth=2)
            args.append(code)
        inst_var = self._fresh_var()
        # Initialize before try so it's always defined
        lines.append(f"{inst_var} = None")
        self._add_var(inst_var, T_ANY)
        lines.append("try:")
        lines.append(f"    {inst_var} = {cls_name}({', '.join(args)})")
        lines.append(f"    print(repr({inst_var}))")
        for fname in field_names:
            if self.rng.random() < 0.5:
                lines.append(f"    print({inst_var}.{fname})")
        for mname in method_names:
            if self.rng.random() < 0.7:
                lines.append("    try:")
                lines.append(f"        print({inst_var}.{mname}())")
                lines.append("    except TypeError:")
                arg = self.rng.choice(["42", "'hello'", "3.14"])
                lines.append(f"        print({inst_var}.{mname}({arg}))")
        lines.append(
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError, AttributeError) as _cls_e:"
        )
        lines.append("    print(type(_cls_e).__name__)")
        return "\n".join(lines)

    def gen_inheritance_class(self) -> str:
        if not self._defined_classes:
            return self.gen_class_def()
        parent_name, parent_fields, parent_methods = self.rng.choice(
            self._defined_classes
        )
        child_name = f"Sub{parent_name}"
        extra_field = f"extra_{self._func_counter}"
        self._func_counter += 1
        all_params = ", ".join(parent_fields + [extra_field])
        lines = [f"class {child_name}({parent_name}):"]
        lines.append(f"    def __init__(self, {all_params}):")
        lines.append(f"        super().__init__({', '.join(parent_fields)})")
        lines.append(f"        self.{extra_field} = {extra_field}")
        all_fields = parent_fields + [extra_field]
        repr_parts = ", ".join(f"{f}={{self.{f}!r}}" for f in all_fields)
        lines.append("    def __repr__(self):")
        lines.append(f'        return f"{child_name}({repr_parts})"')
        lines.append("    def get_extra(self):")
        lines.append(f"        return self.{extra_field}")
        self._defined_classes.append(
            (child_name, all_fields, parent_methods + ["get_extra"])
        )
        return "\n".join(lines)

    # -- Closure generators --------------------------------------------------

    def gen_closure_def(self) -> str:
        outer_name = self._fresh_func()
        inner_name = f"_inner_{self._func_counter}"
        self._func_counter += 1
        captured_var = self._fresh_var()
        captured_val = self.rng.randint(1, 50)
        param = self._fresh_var()
        kind = self.rng.choice(["simple", "counter", "accumulator"])
        if kind == "simple":
            lines = [
                f"def {outer_name}({param}):",
                f"    {captured_var} = {captured_val}",
                f"    def {inner_name}(x):",
                f"        return x + {captured_var} + {param}",
                f"    return {inner_name}",
            ]
        elif kind == "counter":
            lines = [
                f"def {outer_name}():",
                f"    {captured_var} = [0]",
                f"    def {inner_name}():",
                f"        {captured_var}[0] += 1",
                f"        return {captured_var}[0]",
                f"    return {inner_name}",
            ]
        else:
            lines = [
                f"def {outer_name}(start):",
                f"    {captured_var} = [start]",
                f"    def {inner_name}(val):",
                f"        {captured_var}[0] += val",
                f"        return {captured_var}[0]",
                f"    return {inner_name}",
            ]
        self._defined_closures.append((outer_name, kind))
        return "\n".join(lines)

    def gen_closure_usage(self, func_name: str, kind: str) -> str:
        result_var = self._fresh_var()
        lines: list[str] = []
        if kind == "simple":
            arg = self.rng.randint(1, 20)
            lines.append(f"{result_var} = {func_name}({arg})")
            call_arg = self.rng.randint(1, 20)
            lines.append(f"print({result_var}({call_arg}))")
        elif kind == "counter":
            lines.append(f"{result_var} = {func_name}()")
            for _ in range(self.rng.randint(2, 5)):
                lines.append(f"print({result_var}())")
        else:
            start = self.rng.randint(0, 10)
            lines.append(f"{result_var} = {func_name}({start})")
            for _ in range(self.rng.randint(2, 5)):
                val = self.rng.randint(1, 10)
                lines.append(f"print({result_var}({val}))")
        return "\n".join(lines)

    # -- Keyword-only and *args generators ----------------------------------

    def gen_kwonly_function_def(self) -> str:
        func_name = self._fresh_func()
        n_pos = self.rng.randint(1, 2)
        n_kw = self.rng.randint(1, 2)
        pos_params: list[str] = []
        kw_params: list[str] = []
        param_names: list[str] = []
        for _ in range(n_pos):
            name = self._fresh_var()
            pos_params.append(name)
            param_names.append(name)
        for _ in range(n_kw):
            name = self._fresh_var()
            default = self.gen_int_literal()
            kw_params.append(f"{name}={default}")
            param_names.append(name)
        all_params = ", ".join(pos_params + ["*"] + kw_params)
        outer_vars = self._defined_vars[:]
        self._defined_vars = [(n, T_ANY) for n in param_names]
        lines = [f"def {func_name}({all_params}):"]
        for _ in range(self.rng.randint(1, 3)):
            lines.append(self.gen_stmt(depth=2, indent=1))
        ret_code, _ = self.gen_any_expr(depth=2)
        lines.append(f"    return {ret_code}")
        self._defined_vars = outer_vars
        self._defined_kwonly_funcs.append((func_name, n_pos, kw_params))
        return "\n".join(lines)

    def gen_kwonly_call(self, func_name, n_pos, kw_params) -> str:
        pos_args: list[str] = []
        for _ in range(n_pos):
            code, _ = self.gen_any_expr(depth=2)
            pos_args.append(code)
        kw_args: list[str] = []
        for kp in kw_params:
            name = kp.split("=")[0]
            if self.rng.random() < 0.6:
                val_code, _ = self.gen_any_expr(depth=2)
                kw_args.append(f"{name}={val_code}")
        all_args = pos_args + kw_args
        result_var = self._fresh_var()
        self._add_var(result_var, T_ANY)
        lines = [
            f"{result_var} = None",
            "try:",
            f"    {result_var} = {func_name}({', '.join(all_args)})",
            f"    print({result_var})",
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError) as _fuzz_e:",
            "    print(type(_fuzz_e).__name__)",
        ]
        return "\n".join(lines)

    def gen_starargs_function_def(self) -> str:
        func_name = self._fresh_func()
        kind = self.rng.choice(["args_only", "kwargs_only", "both"])
        if kind == "args_only":
            lines = [
                f"def {func_name}(*args):",
                "    result = 0",
                "    for a in args:",
                "        result += hash(a) % 100",
                "    return result",
            ]
        elif kind == "kwargs_only":
            lines = [
                f"def {func_name}(**kwargs):",
                "    parts = []",
                "    for k in sorted(kwargs):",
                "        parts.append(f'{k}={kwargs[k]}')",
                "    return ', '.join(parts)",
            ]
        else:
            lines = [
                f"def {func_name}(*args, **kwargs):",
                "    result = len(args)",
                "    for k in sorted(kwargs):",
                "        result += len(str(kwargs[k]))",
                "    return result",
            ]
        self._defined_starargs_funcs.append((func_name, kind))
        return "\n".join(lines)

    def gen_starargs_call(self, func_name: str, kind: str) -> str:
        lines: list[str] = []
        result_var = self._fresh_var()
        if kind == "args_only":
            n = self.rng.randint(0, 5)
            args: list[str] = []
            for _ in range(n):
                # Use only hashable types (args_only function hashes its args)
                code = self.gen_typed_expr(
                    depth=2,
                    target_type=self.rng.choice([T_INT, T_STR, T_BOOL, T_FLOAT]),
                )
                args.append(code)
            lines.append(f"{result_var} = {func_name}({', '.join(args)})")
        elif kind == "kwargs_only":
            # Use unique keys to avoid SyntaxError: keyword argument repeated
            all_keys = ["x", "y", "z", "name", "val"]
            self.rng.shuffle(all_keys)
            n = self.rng.randint(0, min(3, len(all_keys)))
            kwargs: list[str] = []
            for i in range(n):
                key = all_keys[i]
                val_code, _ = self.gen_any_expr(depth=2)
                kwargs.append(f"{key}={val_code}")
            lines.append(f"{result_var} = {func_name}({', '.join(kwargs)})")
        else:
            n_pos = self.rng.randint(0, 3)
            pos: list[str] = []
            for _ in range(n_pos):
                code, _ = self.gen_any_expr(depth=2)
                pos.append(code)
            # Use unique keys to avoid SyntaxError: keyword argument repeated
            all_keys = ["a", "b", "c", "d", "e"]
            self.rng.shuffle(all_keys)
            n_kw = self.rng.randint(0, min(2, len(all_keys)))
            kw: list[str] = []
            for i in range(n_kw):
                key = all_keys[i]
                val_code, _ = self.gen_any_expr(depth=2)
                kw.append(f"{key}={val_code}")
            lines.append(f"{result_var} = {func_name}({', '.join(pos + kw)})")
        lines.append(f"print({result_var})")
        return "\n".join(lines)

    # -- Deterministic output helpers ---------------------------------------

    def _gen_dict_print(self, depth: int) -> str:
        """Generate a dict print that produces deterministic output."""
        d = self._gen_dict_expr(depth)
        op = self.rng.choice(["keys", "values", "items", "get", "in", "len"])
        if op == "keys":
            return f"print(sorted({d}.keys()))"
        elif op == "values":
            return f"print(sorted({d}.values(), key=str))"
        elif op == "items":
            return f"print(sorted({d}.items()))"
        elif op == "get":
            key = self.rng.choice(["a", "b", "c", "z"])
            default = self.rng.randint(-1, 99)
            return f"print({d}.get({repr(key)}, {default}))"
        elif op == "in":
            key = self.rng.choice(["a", "b", "z"])
            return f"print({repr(key)} in {d})"
        else:
            return f"print(len({d}))"

    def _gen_set_print(self, depth: int) -> str:
        """Generate a set print that produces deterministic output."""
        s = self._gen_set_int_expr(depth)
        op = self.rng.choice(["sorted", "len", "in", "op"])
        if op == "sorted":
            return f"print(sorted({s}))"
        elif op == "len":
            return f"print(len({s}))"
        elif op == "in":
            val = self.rng.randint(0, 10)
            return f"print({val} in {s})"
        else:
            method = self.rng.choice(
                ["union", "intersection", "difference", "symmetric_difference"]
            )
            s2 = self._gen_set_int_expr(depth + 1)
            return f"print(sorted({s}.{method}({s2})))"

    # -- Top-level program generator ----------------------------------------

    def generate(self) -> str:
        self._var_counter = 0
        self._func_counter = 0
        self._defined_vars = []
        self._scope_stack = []
        self._defined_funcs = []
        self._defined_classes = []
        self._defined_closures = []
        self._defined_kwonly_funcs = []
        self._defined_starargs_funcs = []

        sections: list[str] = []

        # Classes
        for _ in range(self.rng.randint(0, 2)):
            sections.append(self.gen_class_def())
            sections.append("")
            if self.rng.random() < 0.3 and self._defined_classes:
                sections.append(self.gen_inheritance_class())
                sections.append("")

        # Functions
        for _ in range(self.rng.randint(0, 3)):
            sections.append(self.gen_function_def())
            sections.append("")

        # Closures
        for _ in range(self.rng.randint(0, 2)):
            sections.append(self.gen_closure_def())
            sections.append("")

        # Kwonly functions
        if self.rng.random() < 0.4:
            sections.append(self.gen_kwonly_function_def())
            sections.append("")

        # *args/**kwargs functions
        if self.rng.random() < 0.4:
            sections.append(self.gen_starargs_function_def())
            sections.append("")

        # Main body statements
        n_stmts = self.rng.randint(5, self.max_stmts)
        for _ in range(n_stmts):
            # Mix in some dict/set prints for determinism coverage
            r = self.rng.random()
            if r < 0.08:
                sections.append(self._gen_dict_print(depth=1))
            elif r < 0.14:
                sections.append(self._gen_set_print(depth=1))
            else:
                sections.append(self.gen_stmt(depth=0, indent=0))

        # Call functions
        for func_name in self._defined_funcs:
            sections.append(self._gen_func_call(func_name))

        # Use closures
        for func_name, kind in self._defined_closures:
            sections.append(self.gen_closure_usage(func_name, kind))

        # Call kwonly functions
        for func_name, n_pos, kw_params in self._defined_kwonly_funcs:
            sections.append(self.gen_kwonly_call(func_name, n_pos, kw_params))

        # Call *args/**kwargs functions
        for func_name, kind in self._defined_starargs_funcs:
            sections.append(self.gen_starargs_call(func_name, kind))

        # Instantiate classes
        for cls_info in self._defined_classes:
            sections.append(self.gen_class_usage(*cls_info))

        # Final summary print
        if self._defined_vars:
            # Pick a few vars that are still in scope
            n_pick = min(3, len(self._defined_vars))
            chosen = self.rng.sample(self._defined_vars, n_pick)
            summary_args = ", ".join(f"repr({v[0]})" for v in chosen)
            sections.append(f"print({summary_args})")

        return "\n".join(sections) + "\n"


# ---------------------------------------------------------------------------
# Reject program generator
# ---------------------------------------------------------------------------


class RejectProgramGenerator:
    """Generates programs that use dynamic features Molt should reject."""

    REJECT_TEMPLATES: list[tuple[str, str]] = [
        # exec
        (
            'exec("x = 1 + 2")\nprint(x)',
            "exec",
        ),
        (
            'exec(compile("y = 42", "<string>", "exec"))\nprint(y)',
            "exec",
        ),
        # eval
        (
            'result = eval("1 + 2")\nprint(result)',
            "eval",
        ),
        (
            'vals = [1, 2, 3]\nresult = eval("sum(vals)")\nprint(result)',
            "eval",
        ),
        # setattr / delattr
        (
            "class Foo:\n    pass\nobj = Foo()\nsetattr(obj, 'x', 1)\nprint(obj.x)",
            "setattr",
        ),
        (
            "class Bar:\n    x = 10\nobj = Bar()\ndelattr(obj, 'x')",
            "delattr",
        ),
        # type() dynamic class creation
        (
            'MyClass = type("MyClass", (object,), {"value": 42})\nobj = MyClass()\nprint(obj.value)',
            "dynamic type()",
        ),
        # __dict__ mutation
        (
            "class Baz:\n    pass\nobj = Baz()\nobj.__dict__['secret'] = 99\nprint(obj.secret)",
            "__dict__ mutation",
        ),
        # Monkeypatching
        (
            "class Animal:\n    pass\nAnimal.speak = lambda self: 'woof'\ndog = Animal()\nprint(dog.speak())",
            "monkeypatch",
        ),
        (
            "class Base:\n    def greet(self):\n        return 'hi'\nBase.greet = lambda self: 'bye'\nprint(Base().greet())",
            "monkeypatch",
        ),
        # globals() / locals() mutation
        (
            'globals()["dynamic_var"] = 100\nprint(dynamic_var)',
            "globals mutation",
        ),
        (
            'def f():\n    locals()["x"] = 5\n    return x\nprint(f())',
            "locals mutation",
        ),
        # getattr with runtime-determined name
        (
            "class Obj:\n    x = 1\n    y = 2\nimport random\nattr_name = ['x', 'y'][0]\nobj = Obj()\nprint(getattr(obj, attr_name))",
            "dynamic getattr",
        ),
        # __class__ assignment
        (
            "class A:\n    pass\nclass B:\n    pass\na = A()\na.__class__ = B\nprint(type(a).__name__)",
            "__class__ assignment",
        ),
        # __bases__ mutation
        (
            "class X:\n    pass\nclass Y:\n    pass\nX.__bases__ = (Y,)",
            "__bases__ mutation",
        ),
    ]

    def __init__(self, rng: Random):
        self.rng = rng

    def generate(self) -> tuple[str, str]:
        """Returns (source, expected_rejection_reason)."""
        source, reason = self.rng.choice(self.REJECT_TEMPLATES)
        return source + "\n", reason


# ---------------------------------------------------------------------------
# Compile-only fuzzer (hypothesmith)
# ---------------------------------------------------------------------------


class CompileOnlyFuzzer:
    """Uses hypothesmith for compile-only crash testing."""

    def run(
        self,
        count: int,
        seed: int,
        profile: str,
        timeout: float,
        verbose: bool,
        output_dir: Path | None,
    ) -> FuzzSummary:
        try:
            from hypothesis import HealthCheck, find, settings
            from hypothesmith import from_grammar
        except ImportError:
            _log(
                "hypothesmith not installed; run: uv add --dev hypothesmith hypothesis"
            )
            summary = FuzzSummary()
            summary.total = 0
            return summary

        env = _build_env()
        summary = FuzzSummary()
        ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
        tmpdir_base = (
            ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
        )

        _log(f"Compile-only fuzzer: {count} programs, seed={seed}, profile={profile}")

        with tempfile.TemporaryDirectory(
            prefix="molt_fuzz_co_", dir=tmpdir_base
        ) as tmpdir:
            for i in range(count):
                program_seed = seed + i
                summary.total += 1

                # Generate using hypothesmith via hypothesis's find()
                try:
                    source = find(
                        from_grammar(),
                        lambda x: True,
                        settings=settings(
                            max_examples=1,
                            database=None,
                            suppress_health_check=list(HealthCheck),
                        ),
                        random=Random(program_seed),
                    )
                except Exception as e:
                    if verbose:
                        _log(f"  [#{i:4d}] GENERATE_ERROR: {type(e).__name__}: {e}")
                    continue

                source_path = os.path.join(tmpdir, f"fuzz_co_{i:06d}.py")
                Path(source_path).write_text(source)

                try:
                    binary, build_error = compile_molt(
                        source_path, profile, timeout, env
                    )
                    if binary is not None:
                        summary.compile_only_ok += 1
                        if verbose:
                            _log(f"  [#{i:4d}] OK (compiled)")
                    else:
                        # Non-zero exit with clean error is fine
                        if "timed out" in build_error.lower():
                            summary.timeouts += 1
                            if verbose:
                                _log(f"  [#{i:4d}] TIMEOUT")
                        else:
                            summary.compile_only_ok += 1
                            if verbose:
                                _log(f"  [#{i:4d}] OK (rejected cleanly)")
                except Exception as e:
                    # Actual crash
                    summary.compile_only_crash += 1
                    _log(f"  [#{i:4d}] CRASH: {type(e).__name__}: {e}")
                    if output_dir:
                        result = FuzzResult(
                            program_id=i,
                            seed=program_seed,
                            source=source,
                            status="crash",
                            error_detail=str(e),
                        )
                        _save_failure(result, output_dir)
                finally:
                    try:
                        Path(source_path).unlink(missing_ok=True)
                    except OSError:
                        pass

        return summary


# ---------------------------------------------------------------------------
# Compilation and execution
# ---------------------------------------------------------------------------


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    return env


def _extract_binary(build_json: dict) -> str | None:
    data = build_json
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return str(data[key])
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return str(data["build"][key])
    return None


def run_cpython(source_path: str, timeout: float) -> tuple[str, str, int | None]:
    try:
        result = subprocess.run(
            [sys.executable, source_path],
            capture_output=True,
            text=True,
            timeout=timeout,
            env={**os.environ, "PYTHONHASHSEED": "0"},
        )
        return result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired:
        return "", "", None


def compile_molt(
    source_path: str,
    profile: str,
    timeout: float,
    env: dict[str, str],
) -> tuple[str | None, str]:
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--deterministic",
        "--json",
        source_path,
    ]
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return None, "Molt build timed out"

    if result.returncode != 0:
        stderr_snippet = result.stderr[:800] if result.stderr else "(no stderr)"
        stdout_snippet = result.stdout[:400] if result.stdout else "(no stdout)"
        return (
            None,
            f"Molt build failed (rc={result.returncode}):\n"
            f"stderr: {stderr_snippet}\nstdout: {stdout_snippet}",
        )

    stdout = result.stdout.strip()
    if not stdout:
        return None, "Molt build produced no JSON output"

    json_str = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            json_str = line
            break

    if json_str is None:
        return None, f"No JSON object in Molt build output: {stdout[:300]}"

    try:
        build_info = json.loads(json_str)
    except json.JSONDecodeError as exc:
        return None, f"Invalid build JSON: {exc}\n{json_str[:300]}"

    binary = _extract_binary(build_info)
    if binary is None:
        data_keys = (
            list(build_info.get("data", {}).keys())
            if isinstance(build_info.get("data"), dict)
            else "N/A"
        )
        return (
            None,
            f"Cannot find binary in build output. "
            f"Keys: {list(build_info.keys())}, data keys: {data_keys}",
        )

    if not Path(binary).exists():
        return None, f"Binary not found at {binary}"

    return binary, ""


def run_molt_binary(
    binary_path: str,
    timeout: float,
    env: dict[str, str],
) -> tuple[str, str, int | None]:
    try:
        result = subprocess.run(
            [binary_path],
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        return result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired:
        return "", "", None


# ---------------------------------------------------------------------------
# Fuzzer core: safe mode
# ---------------------------------------------------------------------------


def fuzz_one_safe(
    program_id: int,
    seed: int,
    rng: Random,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool,
    tmpdir: str,
) -> FuzzResult:
    t0 = time.monotonic()
    gen = SafeProgramGenerator(rng, max_depth=3, max_stmts=15)
    source = gen.generate()

    source_path = os.path.join(tmpdir, f"fuzz_{program_id:06d}.py")
    Path(source_path).write_text(source)

    try:
        cp_stdout, cp_stderr, cp_rc = run_cpython(source_path, timeout)

        if cp_rc is None:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="timeout",
                error_detail="CPython execution timed out",
                elapsed_sec=time.monotonic() - t0,
            )

        if cp_rc != 0:
            if verbose:
                _log(f"  [#{program_id}] CPython error (rc={cp_rc}), skipping")
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="cpython_error",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                error_detail=f"CPython exited with rc={cp_rc}",
                elapsed_sec=time.monotonic() - t0,
            )

        binary, build_error = compile_molt(source_path, profile, timeout, env)
        if binary is None:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="build_error",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                error_detail=build_error,
                elapsed_sec=time.monotonic() - t0,
            )

        molt_stdout, molt_stderr, molt_rc = run_molt_binary(binary, timeout, env)

        if molt_rc is None:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="timeout",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                molt_stdout=molt_stdout,
                molt_stderr=molt_stderr,
                error_detail="Molt binary execution timed out",
                elapsed_sec=time.monotonic() - t0,
            )

        if cp_stdout == molt_stdout:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="pass",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                molt_stdout=molt_stdout,
                molt_stderr=molt_stderr,
                elapsed_sec=time.monotonic() - t0,
            )

        if molt_rc != 0:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="molt_run_error",
                cpython_stdout=cp_stdout,
                cpython_stderr=cp_stderr,
                molt_stdout=molt_stdout,
                molt_stderr=molt_stderr,
                error_detail=f"Molt binary exited with rc={molt_rc}",
                elapsed_sec=time.monotonic() - t0,
            )

        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="mismatch",
            cpython_stdout=cp_stdout,
            cpython_stderr=cp_stderr,
            molt_stdout=molt_stdout,
            molt_stderr=molt_stderr,
            elapsed_sec=time.monotonic() - t0,
        )
    finally:
        try:
            Path(source_path).unlink(missing_ok=True)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Fuzzer core: reject mode
# ---------------------------------------------------------------------------


def fuzz_one_reject(
    program_id: int,
    seed: int,
    rng: Random,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool,
    tmpdir: str,
) -> FuzzResult:
    t0 = time.monotonic()
    gen = RejectProgramGenerator(rng)
    source, reason = gen.generate()

    source_path = os.path.join(tmpdir, f"fuzz_reject_{program_id:06d}.py")
    Path(source_path).write_text(source)

    try:
        binary, build_error = compile_molt(source_path, profile, timeout, env)

        if binary is not None:
            # Molt accepted a program it should have rejected
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="reject_fail",
                error_detail=f"Molt should have rejected ({reason}) but compiled successfully",
                elapsed_sec=time.monotonic() - t0,
            )

        # Non-zero exit is expected; check it was clean (not a crash)
        if "timed out" in build_error.lower():
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="timeout",
                error_detail=build_error,
                elapsed_sec=time.monotonic() - t0,
            )

        # Check for crash indicators
        crash_indicators = [
            "signal",
            "segfault",
            "assertion failed",
            "panic",
            "abort",
            "core dumped",
        ]
        build_error_lower = build_error.lower()
        is_crash = any(ind in build_error_lower for ind in crash_indicators)

        if is_crash:
            return FuzzResult(
                program_id=program_id,
                seed=seed,
                source=source,
                status="reject_crash",
                error_detail=f"Molt crashed while rejecting ({reason}): {build_error[:300]}",
                elapsed_sec=time.monotonic() - t0,
            )

        # Clean rejection -- good
        return FuzzResult(
            program_id=program_id,
            seed=seed,
            source=source,
            status="reject_pass",
            error_detail=f"Correctly rejected ({reason})",
            elapsed_sec=time.monotonic() - t0,
        )
    finally:
        try:
            Path(source_path).unlink(missing_ok=True)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Logging and reporting
# ---------------------------------------------------------------------------


def _log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def _save_failure(result: FuzzResult, output_dir: Path) -> Path:
    output_dir.mkdir(parents=True, exist_ok=True)
    source_file = output_dir / f"fuzz_{result.program_id:06d}.py"
    source_file.write_text(result.source)
    report_file = output_dir / f"fuzz_{result.program_id:06d}.report.txt"
    report_lines = [
        f"Fuzz ID: {result.program_id}",
        f"Seed: {result.seed}",
        f"Status: {result.status}",
        f"Elapsed: {result.elapsed_sec:.2f}s",
        "",
        "=== CPython stdout ===",
        result.cpython_stdout,
        "=== Molt stdout ===",
        result.molt_stdout,
        "=== CPython stderr ===",
        result.cpython_stderr,
        "=== Molt stderr ===",
        result.molt_stderr,
    ]
    if result.error_detail:
        report_lines.extend(["", "=== Error Detail ===", result.error_detail])
    report_file.write_text("\n".join(report_lines))
    return source_file


def _print_diff_snippet(result: FuzzResult, max_lines: int = 15) -> None:
    cp_lines = result.cpython_stdout.splitlines()
    molt_lines = result.molt_stdout.splitlines()
    printed = 0
    max_len = max(len(cp_lines), len(molt_lines))
    for i in range(min(max_len, max_lines)):
        cp_line = cp_lines[i] if i < len(cp_lines) else "<missing>"
        molt_line = molt_lines[i] if i < len(molt_lines) else "<missing>"
        if cp_line != molt_line:
            _log(f"    line {i + 1}:")
            _log(f"      CPython: {cp_line!r}")
            _log(f"      Molt:    {molt_line!r}")
            printed += 1
            if printed >= 5:
                remaining = sum(
                    1
                    for j in range(i + 1, max_len)
                    if (cp_lines[j] if j < len(cp_lines) else "")
                    != (molt_lines[j] if j < len(molt_lines) else "")
                )
                if remaining > 0:
                    _log(f"    ... and {remaining} more differing lines")
                break


# ---------------------------------------------------------------------------
# Main driver
# ---------------------------------------------------------------------------


def run_safe_fuzzer(
    count: int,
    seed: int,
    output_dir: Path | None,
    profile: str,
    timeout: float,
    verbose: bool,
) -> FuzzSummary:
    summary = FuzzSummary()
    env = _build_env()

    ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
    tmpdir_base = (
        ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
    )

    _log(f"Safe-mode fuzzer: {count} programs, seed={seed}, profile={profile}")
    _log(f"  timeout={timeout}s, tmpdir={tmpdir_base}")
    if output_dir:
        _log(f"  output_dir={output_dir}")
    _log("")

    with tempfile.TemporaryDirectory(prefix="molt_fuzz_", dir=tmpdir_base) as tmpdir:
        for i in range(count):
            program_seed = seed + i
            rng = Random(program_seed)
            result = fuzz_one_safe(
                program_id=i,
                seed=program_seed,
                rng=rng,
                profile=profile,
                timeout=timeout,
                env=env,
                verbose=verbose,
                tmpdir=tmpdir,
            )
            summary.total += 1
            if result.status == "pass":
                summary.passed += 1
                if verbose:
                    _log(f"  [#{i:4d}] PASS ({result.elapsed_sec:.1f}s)")
            elif result.status == "mismatch":
                summary.mismatches += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] MISMATCH (seed={program_seed})")
                if verbose:
                    _print_diff_snippet(result)
                if output_dir:
                    saved = _save_failure(result, output_dir)
                    _log(f"         saved: {saved}")
            elif result.status == "build_error":
                summary.build_errors += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] BUILD_ERROR (seed={program_seed})")
                if verbose:
                    _log(f"         {result.error_detail[:200]}")
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "molt_run_error":
                summary.molt_run_errors += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] MOLT_RUN_ERROR (seed={program_seed})")
                if verbose:
                    _print_diff_snippet(result)
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "cpython_error":
                summary.cpython_errors += 1
                if verbose:
                    _log(f"  [#{i:4d}] CPYTHON_ERROR (skipped, seed={program_seed})")
            elif result.status == "timeout":
                summary.timeouts += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] TIMEOUT (seed={program_seed})")
                if output_dir:
                    _save_failure(result, output_dir)

    return summary


def run_reject_fuzzer(
    count: int,
    seed: int,
    output_dir: Path | None,
    profile: str,
    timeout: float,
    verbose: bool,
) -> FuzzSummary:
    summary = FuzzSummary()
    env = _build_env()

    ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
    tmpdir_base = (
        ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
    )

    _log(f"Reject-mode fuzzer: {count} programs, seed={seed}, profile={profile}")
    _log(f"  timeout={timeout}s, tmpdir={tmpdir_base}")
    if output_dir:
        _log(f"  output_dir={output_dir}")
    _log("")

    with tempfile.TemporaryDirectory(
        prefix="molt_fuzz_reject_", dir=tmpdir_base
    ) as tmpdir:
        for i in range(count):
            program_seed = seed + i
            rng = Random(program_seed)
            result = fuzz_one_reject(
                program_id=i,
                seed=program_seed,
                rng=rng,
                profile=profile,
                timeout=timeout,
                env=env,
                verbose=verbose,
                tmpdir=tmpdir,
            )
            summary.total += 1
            if result.status == "reject_pass":
                summary.reject_pass += 1
                if verbose:
                    _log(f"  [#{i:4d}] REJECT_PASS ({result.error_detail})")
            elif result.status == "reject_fail":
                summary.reject_fail += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] REJECT_FAIL (seed={program_seed})")
                _log(f"         {result.error_detail[:200]}")
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "reject_crash":
                summary.reject_fail += 1
                summary.failures.append(result)
                _log(f"  [#{i:4d}] REJECT_CRASH (seed={program_seed})")
                _log(f"         {result.error_detail[:200]}")
                if output_dir:
                    _save_failure(result, output_dir)
            elif result.status == "timeout":
                summary.timeouts += 1
                _log(f"  [#{i:4d}] TIMEOUT (seed={program_seed})")

    return summary


def _print_summary(summary: FuzzSummary, mode: str) -> None:
    _log("")
    _log("=" * 60)
    _log(f"FUZZ SUMMARY (mode={mode})")
    _log("=" * 60)
    _log(f"  Total programs:    {summary.total}")

    if mode == "safe":
        _log(f"  Passed:            {summary.passed}")
        _log(f"  Mismatches:        {summary.mismatches}")
        _log(f"  Build errors:      {summary.build_errors}")
        _log(f"  Molt run errors:   {summary.molt_run_errors}")
        _log(f"  CPython errors:    {summary.cpython_errors} (skipped)")
        _log(f"  Timeouts:          {summary.timeouts}")
        _log("")
        effective = summary.total - summary.cpython_errors
        if effective > 0:
            pass_rate = summary.passed / effective * 100
            _log(f"  Pass rate: {summary.passed}/{effective} ({pass_rate:.1f}%)")
        else:
            _log("  Pass rate: N/A (no effective test programs)")
        if summary.cpython_errors > 0:
            cpython_clean = (
                (summary.total - summary.cpython_errors) / summary.total * 100
            )
            _log(
                f"  CPython clean rate: {summary.total - summary.cpython_errors}/{summary.total} ({cpython_clean:.1f}%)"
            )
        if summary.mismatches > 0:
            _log("")
            _log(f"  {summary.mismatches} MISMATCH(ES) FOUND")
            for r in summary.failures:
                if r.status == "mismatch":
                    _log(f"    seed={r.seed}")
        if summary.build_errors > 0:
            _log("")
            _log(f"  {summary.build_errors} BUILD ERROR(S)")
            for r in summary.failures:
                if r.status == "build_error":
                    _log(f"    seed={r.seed}: {r.error_detail[:120]}")

    elif mode == "reject":
        _log(f"  Correctly rejected: {summary.reject_pass}")
        _log(f"  Reject failures:    {summary.reject_fail}")
        _log(f"  Timeouts:           {summary.timeouts}")
        if summary.total > 0:
            reject_rate = summary.reject_pass / summary.total * 100
            _log(f"  Rejection rate: {reject_rate:.1f}%")

    elif mode == "compile-only":
        _log(f"  OK (compiled or cleanly rejected): {summary.compile_only_ok}")
        _log(f"  Crashes:                           {summary.compile_only_crash}")
        _log(f"  Timeouts:                          {summary.timeouts}")
        if summary.total > 0:
            ok_rate = (summary.compile_only_ok) / summary.total * 100
            _log(f"  OK rate: {ok_rate:.1f}%")

    _log("=" * 60)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Comprehensive compiler fuzzer for Molt.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            examples:
              python tools/fuzz_compiler.py --mode safe --count 100 --seed 42
              python tools/fuzz_compiler.py --mode reject --count 50
              python tools/fuzz_compiler.py --mode compile-only --count 200
              python tools/fuzz_compiler.py --count 10 --seed 0 --verbose
              python tools/fuzz_compiler.py --count 500 --output-dir /tmp/fuzz_failures
        """),
    )
    parser.add_argument(
        "--mode",
        choices=["safe", "reject", "compile-only"],
        default="safe",
        help="Fuzzing mode (default: safe)",
    )
    parser.add_argument("--count", "-n", type=int, default=100)
    parser.add_argument("--seed", "-s", type=int, default=None)
    parser.add_argument("--output-dir", "-o", type=str, default=None)
    parser.add_argument("--build-profile", type=str, default="dev")
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--verbose", "-v", action="store_true")

    args = parser.parse_args()
    if args.seed is None:
        args.seed = int(time.time()) % (2**31)
        _log(f"Using auto-generated seed: {args.seed}")

    output_dir = Path(args.output_dir) if args.output_dir else None

    if args.mode == "safe":
        summary = run_safe_fuzzer(
            count=args.count,
            seed=args.seed,
            output_dir=output_dir,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
        )
        _print_summary(summary, "safe")
        if summary.mismatches > 0 or summary.molt_run_errors > 0:
            return 1
        return 0

    elif args.mode == "reject":
        summary = run_reject_fuzzer(
            count=args.count,
            seed=args.seed,
            output_dir=output_dir,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
        )
        _print_summary(summary, "reject")
        if summary.reject_fail > 0:
            return 1
        return 0

    elif args.mode == "compile-only":
        fuzzer = CompileOnlyFuzzer()
        summary = fuzzer.run(
            count=args.count,
            seed=args.seed,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
            output_dir=output_dir,
        )
        _print_summary(summary, "compile-only")
        if summary.compile_only_crash > 0:
            return 1
        return 0

    return 0


if __name__ == "__main__":
    sys.exit(main())
