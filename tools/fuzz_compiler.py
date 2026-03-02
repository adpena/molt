#!/usr/bin/env python3
"""Compiler fuzzer for Molt: generates random valid Python programs, compiles
them with Molt, runs both the Molt binary and CPython, and reports mismatches.

This is a differential testing tool that exercises Molt's Tier 0 operations
(arithmetic, strings, control flow, functions, collections, try/except) with
randomly generated but deterministic programs.

Usage:
    # Run 100 random programs with default seed
    python tools/fuzz_compiler.py

    # Run 500 programs with a specific seed for reproducibility
    python tools/fuzz_compiler.py --count 500 --seed 42

    # Save failing programs to a directory
    python tools/fuzz_compiler.py --count 1000 --output-dir /tmp/fuzz_failures

    # Use release profile and verbose output
    python tools/fuzz_compiler.py --build-profile release --verbose

    # Quick smoke test
    python tools/fuzz_compiler.py --count 10 --seed 0 --verbose

Environment:
    PYTHONPATH        Defaults to "src" if not set.
    PYTHONHASHSEED    Forced to "0" for deterministic dict ordering.
    MOLT_DETERMINISTIC  Forced to "1".
    CARGO_TARGET_DIR  Respected from environment (use throughput_env.sh).
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
# Result types
# ---------------------------------------------------------------------------


@dataclass
class FuzzResult:
    """Outcome of a single fuzz iteration."""

    program_id: int
    seed: int
    source: str
    status: str  # "pass", "mismatch", "build_error", "cpython_error", "molt_run_error", "timeout"
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
    failures: list[FuzzResult] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Program generator
# ---------------------------------------------------------------------------


class ProgramGenerator:
    """Generates random valid Python 3.12+ programs from a modular grammar.

    Each node type (expression, statement, function, etc.) has its own
    generator method so the grammar is easy to extend.
    """

    # Variable names used throughout generated programs
    VAR_NAMES = ["a", "b", "c", "d", "e", "x", "y", "z", "n", "m", "val", "res", "tmp"]
    FUNC_NAMES = ["compute", "transform", "helper", "process", "calc", "combine"]
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
        "",
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
        self._defined_vars: list[str] = []
        self._defined_funcs: list[str] = []

    # -- Helpers ------------------------------------------------------------

    def _fresh_var(self) -> str:
        name = f"v{self._var_counter}"
        self._var_counter += 1
        return name

    def _fresh_func(self) -> str:
        base = self.rng.choice(self.FUNC_NAMES)
        name = f"{base}_{self._func_counter}"
        self._func_counter += 1
        return name

    def _known_var(self) -> str | None:
        if not self._defined_vars:
            return None
        return self.rng.choice(self._defined_vars)

    def _indent(self, code: str, level: int) -> str:
        prefix = "    " * level
        return "\n".join(prefix + line for line in code.splitlines())

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

    def gen_literal(self) -> str:
        """Generate a random literal value."""
        kind = self.rng.choices(
            ["int", "float", "bool", "str", "none"],
            weights=[40, 15, 15, 25, 5],
        )[0]
        return getattr(self, f"gen_{kind}_literal")()

    # -- Expression generators ----------------------------------------------

    def gen_expr(self, depth: int = 0) -> str:
        """Generate a random expression, with depth limiting to prevent
        unbounded recursion."""
        if depth >= self.max_depth:
            return self.gen_leaf_expr()

        kind = self.rng.choices(
            [
                "literal",
                "variable",
                "arithmetic",
                "comparison",
                "boolean",
                "string_op",
                "fstring",
                "unary",
                "list_literal",
                "tuple_literal",
                "dict_literal",
                "list_op",
                "function_call",
                "ternary",
            ],
            weights=[20, 15, 15, 8, 8, 6, 5, 4, 5, 4, 4, 2, 2, 2],
        )[0]

        method = getattr(self, f"gen_{kind}_expr", None)
        if method is None:
            return self.gen_leaf_expr()
        return method(depth)

    def gen_leaf_expr(self) -> str:
        """Generate a leaf expression (no recursion)."""
        var = self._known_var()
        if var and self.rng.random() < 0.5:
            return var
        return self.gen_literal()

    def gen_literal_expr(self, depth: int = 0) -> str:
        return self.gen_literal()

    def gen_variable_expr(self, depth: int = 0) -> str:
        var = self._known_var()
        if var:
            return var
        return self.gen_literal()

    def gen_arithmetic_expr(self, depth: int = 0) -> str:
        """Generate an arithmetic expression like (a + b) or (a // b)."""
        op = self.rng.choice(["+", "-", "*", "//", "%", "**"])
        left = self.gen_expr(depth + 1)
        right = self.gen_expr(depth + 1)
        # Avoid division by zero and excessively large exponents
        if op in ("//", "%"):
            # Wrap divisor to avoid zero
            right = f"({right} or 1)"
        elif op == "**":
            # Constrain exponent to small positive values
            right = f"(abs({right}) % 6)"
        return f"({left} {op} {right})"

    def gen_comparison_expr(self, depth: int = 0) -> str:
        op = self.rng.choice(["==", "!=", "<", ">", "<=", ">="])
        left = self.gen_expr(depth + 1)
        right = self.gen_expr(depth + 1)
        return f"({left} {op} {right})"

    def gen_boolean_expr(self, depth: int = 0) -> str:
        kind = self.rng.choice(["and", "or", "not"])
        if kind == "not":
            operand = self.gen_expr(depth + 1)
            return f"(not {operand})"
        left = self.gen_expr(depth + 1)
        right = self.gen_expr(depth + 1)
        return f"({left} {kind} {right})"

    def gen_unary_expr(self, depth: int = 0) -> str:
        op = self.rng.choice(["-", "+"])
        operand = self.gen_expr(depth + 1)
        return f"({op}{operand})"

    def gen_string_op_expr(self, depth: int = 0) -> str:
        """Generate a string method call or concatenation."""
        kind = self.rng.choice(
            ["concat", "upper", "lower", "strip", "replace", "repeat"]
        )
        if kind == "concat":
            left = self.gen_str_literal()
            right = self.gen_str_literal()
            return f"({left} + {right})"
        elif kind == "upper":
            s = self.gen_str_literal()
            return f"{s}.upper()"
        elif kind == "lower":
            s = self.gen_str_literal()
            return f"{s}.lower()"
        elif kind == "strip":
            s = self.gen_str_literal()
            return f"{s}.strip()"
        elif kind == "replace":
            s = self.gen_str_literal()
            old = self.rng.choice(["a", "o", "l", " "])
            new = self.rng.choice(["X", "_", ""])
            return f"{s}.replace({repr(old)}, {repr(new)})"
        else:  # repeat
            s = self.gen_str_literal()
            n = self.rng.randint(0, 4)
            return f"({s} * {n})"

    def gen_fstring_expr(self, depth: int = 0) -> str:
        """Generate an f-string with embedded expressions."""
        num_parts = self.rng.randint(1, 3)
        parts: list[str] = []
        for _ in range(num_parts):
            if self.rng.random() < 0.5:
                parts.append(self.rng.choice(["hello", "val=", "result:", " "]))
            else:
                # Use a simple sub-expression inside the f-string
                inner = self._gen_fstring_inner_expr()
                parts.append("{" + inner + "}")
        return 'f"' + "".join(parts) + '"'

    def _gen_fstring_inner_expr(self) -> str:
        """Generate a simple expression safe for use inside an f-string."""
        kind = self.rng.choice(["int", "var", "arith", "str_method"])
        if kind == "int":
            return str(self.rng.randint(-50, 50))
        elif kind == "var":
            var = self._known_var()
            return var if var else str(self.rng.randint(0, 10))
        elif kind == "arith":
            a = self.rng.randint(-10, 10)
            b = self.rng.randint(1, 10)
            op = self.rng.choice(["+", "-", "*"])
            return f"{a} {op} {b}"
        else:
            s = self.rng.choice(["hello", "world", "test"])
            return f"'{s}'.upper()"

    def gen_list_literal_expr(self, depth: int = 0) -> str:
        n = self.rng.randint(0, 5)
        elems = [self.gen_expr(depth + 1) for _ in range(n)]
        return "[" + ", ".join(elems) + "]"

    def gen_tuple_literal_expr(self, depth: int = 0) -> str:
        n = self.rng.randint(0, 4)
        elems = [self.gen_expr(depth + 1) for _ in range(n)]
        if n == 1:
            return f"({elems[0]},)"
        return "(" + ", ".join(elems) + ")"

    def gen_dict_literal_expr(self, depth: int = 0) -> str:
        n = self.rng.randint(0, 4)
        pairs: list[str] = []
        for _ in range(n):
            key = self.gen_str_literal()
            val = self.gen_expr(depth + 1)
            pairs.append(f"{key}: {val}")
        return "{" + ", ".join(pairs) + "}"

    def gen_list_op_expr(self, depth: int = 0) -> str:
        """Generate a list operation: indexing, len, slicing, or method."""
        kind = self.rng.choice(["len", "index", "slice", "in"])
        if kind == "len":
            lst = self.gen_list_literal_expr(depth + 1)
            return f"len({lst})"
        elif kind == "index":
            n = self.rng.randint(1, 4)
            elems = [self.gen_literal() for _ in range(n)]
            lst = "[" + ", ".join(elems) + "]"
            idx = self.rng.randint(0, n - 1)
            return f"{lst}[{idx}]"
        elif kind == "slice":
            n = self.rng.randint(2, 5)
            elems = [str(self.rng.randint(0, 20)) for _ in range(n)]
            lst = "[" + ", ".join(elems) + "]"
            start = self.rng.randint(0, n - 1)
            end = self.rng.randint(start, n)
            return f"{lst}[{start}:{end}]"
        else:  # in
            needle = self.gen_literal()
            lst = self.gen_list_literal_expr(depth + 1)
            return f"({needle} in {lst})"

    def gen_function_call_expr(self, depth: int = 0) -> str:
        """Generate a builtin function call."""
        fn = self.rng.choice(
            ["abs", "len", "int", "str", "bool", "min", "max", "repr", "type"]
        )
        if fn == "abs":
            return f"abs({self.gen_int_literal()})"
        elif fn == "len":
            return f"len({self.gen_str_literal()})"
        elif fn in ("int", "str", "bool"):
            return f"{fn}({self.gen_literal()})"
        elif fn in ("min", "max"):
            a = self.gen_int_literal()
            b = self.gen_int_literal()
            return f"{fn}({a}, {b})"
        elif fn == "repr":
            return f"repr({self.gen_literal()})"
        else:  # type
            return f"type({self.gen_literal()}).__name__"

    def gen_ternary_expr(self, depth: int = 0) -> str:
        cond = self.gen_expr(depth + 1)
        true_val = self.gen_expr(depth + 1)
        false_val = self.gen_expr(depth + 1)
        return f"({true_val} if {cond} else {false_val})"

    # -- Statement generators -----------------------------------------------

    def gen_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate a random statement."""
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
            ],
            weights=[20, 25, 15, 12, 8, 8, 5, 7],
        )[0]

        method = getattr(self, f"gen_{kind}_stmt", None)
        if method is None:
            return self._gen_simple_stmt(indent)
        return method(depth, indent)

    def _gen_simple_stmt(self, indent: int = 0) -> str:
        """Generate a simple print or assignment statement."""
        if self.rng.random() < 0.5:
            return self.gen_print_stmt(0, indent)
        return self.gen_assign_stmt(0, indent)

    def gen_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        var = self._fresh_var()
        expr = self.gen_expr(depth + 1)
        self._defined_vars.append(var)
        prefix = "    " * indent
        return f"{prefix}{var} = {expr}"

    def gen_augmented_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate an augmented assignment like x += 1."""
        var = self._known_var()
        if not var:
            return self.gen_assign_stmt(depth, indent)
        op = self.rng.choice(["+=", "-=", "*="])
        val = self.gen_int_literal()
        prefix = "    " * indent
        return f"{prefix}{var} {op} {val}"

    def gen_multi_assign_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate a multiple assignment like a, b = 1, 2."""
        n = self.rng.randint(2, 3)
        names = [self._fresh_var() for _ in range(n)]
        vals = [self.gen_literal() for _ in range(n)]
        for name in names:
            self._defined_vars.append(name)
        prefix = "    " * indent
        return f"{prefix}{', '.join(names)} = {', '.join(vals)}"

    def gen_print_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate a print statement."""
        n_args = self.rng.randint(1, 3)
        args = [self.gen_expr(depth + 1) for _ in range(n_args)]
        prefix = "    " * indent
        return f"{prefix}print({', '.join(args)})"

    def gen_if_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate an if/elif/else statement."""
        prefix = "    " * indent
        cond = self.gen_expr(depth + 1)
        body_stmts = self._gen_body(depth + 1, indent + 1)

        lines = [f"{prefix}if {cond}:"]
        lines.extend(body_stmts)

        # Optional elif
        if self.rng.random() < 0.3:
            elif_cond = self.gen_expr(depth + 1)
            elif_body = self._gen_body(depth + 1, indent + 1)
            lines.append(f"{prefix}elif {elif_cond}:")
            lines.extend(elif_body)

        # Optional else
        if self.rng.random() < 0.5:
            else_body = self._gen_body(depth + 1, indent + 1)
            lines.append(f"{prefix}else:")
            lines.extend(else_body)

        return "\n".join(lines)

    def gen_for_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate a bounded for loop using range()."""
        prefix = "    " * indent
        loop_var = self._fresh_var()
        self._defined_vars.append(loop_var)
        bound = self.rng.randint(0, 8)
        body_stmts = self._gen_body(depth + 1, indent + 1)

        lines = [f"{prefix}for {loop_var} in range({bound}):"]
        lines.extend(body_stmts)
        return "\n".join(lines)

    def gen_while_loop_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate a bounded while loop with a counter."""
        prefix = "    " * indent
        counter = self._fresh_var()
        self._defined_vars.append(counter)
        limit = self.rng.randint(1, 6)
        body_stmts = self._gen_body(depth + 1, indent + 1)

        lines = [
            f"{prefix}{counter} = 0",
            f"{prefix}while {counter} < {limit}:",
        ]
        lines.extend(body_stmts)
        inner_prefix = "    " * (indent + 1)
        lines.append(f"{inner_prefix}{counter} += 1")
        return "\n".join(lines)

    def gen_try_except_stmt(self, depth: int = 0, indent: int = 0) -> str:
        """Generate a try/except statement."""
        prefix = "    " * indent
        try_body = self._gen_body(depth + 1, indent + 1)
        except_body = self._gen_body(depth + 1, indent + 1)

        lines = [f"{prefix}try:"]
        lines.extend(try_body)
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
        lines.append(f"{prefix}except {exc_type}:")
        lines.extend(except_body)
        return "\n".join(lines)

    def _gen_body(self, depth: int, indent: int) -> list[str]:
        """Generate a list of statements for a block body."""
        n = self.rng.randint(1, 3)
        stmts: list[str] = []
        for _ in range(n):
            stmts.append(self.gen_stmt(depth, indent))
        return stmts

    # -- Function generators ------------------------------------------------

    def gen_function_def(self) -> str:
        """Generate a function definition with print output."""
        func_name = self._fresh_func()
        self._defined_funcs.append(func_name)

        # Parameters: mix of positional and default-valued
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
            default = self.gen_literal()
            params.append(f"{name}={default}")
            param_names.append(name)

        # Save/restore defined_vars so the function body has its own scope
        outer_vars = self._defined_vars[:]
        self._defined_vars = list(param_names)

        lines = [f"def {func_name}({', '.join(params)}):"]

        # Function body: a few statements plus a return or print
        n_body = self.rng.randint(1, 4)
        for _ in range(n_body):
            stmt = self.gen_stmt(depth=2, indent=1)
            lines.append(stmt)

        # Return or final print
        if self.rng.random() < 0.6 and self._defined_vars:
            ret_var = self.rng.choice(self._defined_vars)
            lines.append(f"    return {ret_var}")
        else:
            ret_expr = self.gen_expr(depth=2)
            lines.append(f"    return {ret_expr}")

        self._defined_vars = outer_vars

        return "\n".join(lines)

    def gen_function_call_stmt(self, func_name: str) -> str:
        """Generate a call to a previously defined function and print its result."""
        # We don't know exact arg count, so pass a few safe args
        n_args = self.rng.randint(0, 3)
        args = [self.gen_literal() for _ in range(n_args)]
        result_var = self._fresh_var()
        self._defined_vars.append(result_var)
        lines = [
            "try:",
            f"    {result_var} = {func_name}({', '.join(args)})",
            f"    print({result_var})",
            "except (TypeError, ValueError, ZeroDivisionError, OverflowError) as _fuzz_e:",
            "    print(type(_fuzz_e).__name__)",
        ]
        return "\n".join(lines)

    # -- Top-level program generator ----------------------------------------

    def generate(self) -> str:
        """Generate a complete random Python program."""
        self._var_counter = 0
        self._func_counter = 0
        self._defined_vars = []
        self._defined_funcs = []

        sections: list[str] = []

        # Optionally define some functions at the top
        n_funcs = self.rng.randint(0, 3)
        for _ in range(n_funcs):
            sections.append(self.gen_function_def())
            sections.append("")  # blank line after function

        # Main body: a sequence of statements
        n_stmts = self.rng.randint(5, self.max_stmts)
        for _ in range(n_stmts):
            stmt = self.gen_stmt(depth=0, indent=0)
            sections.append(stmt)

        # Call defined functions
        for func_name in self._defined_funcs:
            sections.append(self.gen_function_call_stmt(func_name))

        # Final summary print to ensure at least one output line
        if self._defined_vars:
            chosen = self.rng.sample(
                self._defined_vars,
                min(3, len(self._defined_vars)),
            )
            summary_args = ", ".join(f"repr({v})" for v in chosen)
            sections.append(f"print({summary_args})")

        program = "\n".join(sections) + "\n"
        return program


# ---------------------------------------------------------------------------
# Compilation and execution
# ---------------------------------------------------------------------------


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _build_env() -> dict[str, str]:
    """Prepare the environment for Molt CLI invocations."""
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    return env


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from build JSON, unwrapping the data envelope."""
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
    """Run a Python source file with CPython.

    Returns (stdout, stderr, returncode). returncode is None on timeout.
    """
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
    """Compile a Python source file with Molt.

    Returns (binary_path_or_None, error_message).
    """
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
            f"Molt build failed (rc={result.returncode}):\nstderr: {stderr_snippet}\nstdout: {stdout_snippet}",
        )

    # Parse build JSON
    stdout = result.stdout.strip()
    if not stdout:
        return None, "Molt build produced no JSON output"

    # The JSON may be preceded by non-JSON diagnostic lines; find the last
    # JSON object on stdout.
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
        return (
            None,
            f"Cannot find binary in build output. Keys: {list(build_info.keys())}, data keys: {list(build_info.get('data', {}).keys()) if isinstance(build_info.get('data'), dict) else 'N/A'}",
        )

    if not Path(binary).exists():
        return None, f"Binary not found at {binary}"

    return binary, ""


def run_molt_binary(
    binary_path: str,
    timeout: float,
    env: dict[str, str],
) -> tuple[str, str, int | None]:
    """Run a compiled Molt binary.

    Returns (stdout, stderr, returncode). returncode is None on timeout.
    """
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
# Fuzzer core
# ---------------------------------------------------------------------------


def fuzz_one(
    program_id: int,
    seed: int,
    rng: Random,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool,
    tmpdir: str,
) -> FuzzResult:
    """Run a single fuzz iteration: generate, compile, execute, compare."""
    t0 = time.monotonic()

    gen = ProgramGenerator(rng, max_depth=3, max_stmts=15)
    source = gen.generate()

    # Write source to temp file
    source_path = os.path.join(tmpdir, f"fuzz_{program_id:06d}.py")
    Path(source_path).write_text(source)

    try:
        # Run with CPython
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
            # CPython itself errored — this is a bad program, skip it
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

        # Compile with Molt
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

        # Run Molt binary
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

        # Compare outputs
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

        # Mismatch detected: if Molt exited non-zero, classify as run error
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
        # Clean up the source file (binary cleanup is left to the caller or
        # temp directory cleanup)
        try:
            Path(source_path).unlink(missing_ok=True)
        except OSError:
            pass


def _log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def _save_failure(result: FuzzResult, output_dir: Path) -> Path:
    """Save a failing program and its diff report to the output directory."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # Save the source
    source_file = output_dir / f"fuzz_{result.program_id:06d}.py"
    source_file.write_text(result.source)

    # Save a companion report
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
    """Print a compact diff of CPython vs Molt stdout."""
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


def run_fuzzer(
    count: int,
    seed: int,
    output_dir: Path | None,
    profile: str,
    timeout: float,
    verbose: bool,
) -> FuzzSummary:
    """Run the full fuzzing campaign."""
    summary = FuzzSummary()
    env = _build_env()

    # Use external volume tmpdir if available, otherwise system temp
    ext_tmp = os.environ.get("MOLT_DIFF_TMPDIR") or os.environ.get("TMPDIR")
    tmpdir_base = (
        ext_tmp if ext_tmp and Path(ext_tmp).is_dir() else tempfile.gettempdir()
    )

    _log(f"Molt compiler fuzzer: {count} programs, seed={seed}, profile={profile}")
    _log(f"  timeout={timeout}s, tmpdir={tmpdir_base}")
    if output_dir:
        _log(f"  output_dir={output_dir}")
    _log("")

    with tempfile.TemporaryDirectory(prefix="molt_fuzz_", dir=tmpdir_base) as tmpdir:
        for i in range(count):
            # Each program gets its own deterministic sub-seed derived from the
            # master seed, so individual failures are reproducible by re-running
            # with --seed <master_seed> and the same --count.
            program_seed = seed + i
            rng = Random(program_seed)

            result = fuzz_one(
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
                    detail = result.error_detail[:200]
                    _log(f"         {detail}")
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


def _print_summary(summary: FuzzSummary) -> None:
    """Print the final summary report."""
    _log("")
    _log("=" * 60)
    _log("FUZZ SUMMARY")
    _log("=" * 60)
    _log(f"  Total programs:    {summary.total}")
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

    if summary.mismatches > 0:
        _log("")
        _log(
            f"  {summary.mismatches} MISMATCH(ES) FOUND - output differs between CPython and Molt"
        )
        for r in summary.failures:
            if r.status == "mismatch":
                _log(f"    seed={r.seed}")

    if summary.build_errors > 0:
        _log("")
        _log(f"  {summary.build_errors} BUILD ERROR(S)")
        for r in summary.failures:
            if r.status == "build_error":
                _log(f"    seed={r.seed}: {r.error_detail[:120]}")

    _log("=" * 60)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compiler fuzzer for Molt: generates random valid Python programs, "
        "compiles with Molt, runs against CPython, and reports mismatches.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            examples:
              python tools/fuzz_compiler.py --count 10 --seed 0 --verbose
              python tools/fuzz_compiler.py --count 500 --output-dir /tmp/fuzz_failures
              python tools/fuzz_compiler.py --count 100 --build-profile release
        """),
    )
    parser.add_argument(
        "--count",
        "-n",
        type=int,
        default=100,
        help="Number of random programs to generate (default: 100)",
    )
    parser.add_argument(
        "--seed",
        "-s",
        type=int,
        default=None,
        help="Random seed for reproducibility (default: derived from current time)",
    )
    parser.add_argument(
        "--output-dir",
        "-o",
        type=str,
        default=None,
        help="Directory to save failing programs and reports",
    )
    parser.add_argument(
        "--build-profile",
        type=str,
        default="dev",
        help="Molt build profile: dev, release, release-fast (default: dev)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=30.0,
        help="Timeout in seconds for each subprocess (default: 30)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Print detailed output for each program",
    )

    args = parser.parse_args()

    if args.seed is None:
        args.seed = int(time.time()) % (2**31)
        _log(f"Using auto-generated seed: {args.seed}")

    output_dir = Path(args.output_dir) if args.output_dir else None

    summary = run_fuzzer(
        count=args.count,
        seed=args.seed,
        output_dir=output_dir,
        profile=args.build_profile,
        timeout=args.timeout,
        verbose=args.verbose,
    )

    _print_summary(summary)

    # Exit non-zero if any mismatches or molt errors found
    if summary.mismatches > 0 or summary.molt_run_errors > 0:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
