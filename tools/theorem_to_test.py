#!/usr/bin/env python3
"""theorem_to_test.py -- Generate Python differential tests from Lean 4 theorems.

Parses theorem statements from formal/lean/MoltTIR/ and generates concrete
Python test files under tests/differential/basic/generated_from_proofs/ that
exercise the same properties the theorems prove.

Usage:
    uv run --python 3.12 python3 tools/theorem_to_test.py [--dry-run]
"""

from __future__ import annotations

import argparse
import re
import sys
import textwrap
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
LEAN_DIR = REPO_ROOT / "formal" / "lean" / "MoltTIR"
OUTPUT_DIR = REPO_ROOT / "tests" / "differential" / "basic" / "generated_from_proofs"

LEAN_FILES = [
    LEAN_DIR / "Backend" / "LuauCorrect.lean",
    LEAN_DIR / "Passes" / "ConstFoldCorrect.lean",
    LEAN_DIR / "Backend" / "LuauSemantics.lean",
]

# Regex to extract theorem name from Lean 4 source.
# Matches: theorem <name> ...
THEOREM_RE = re.compile(r"^theorem\s+(\w+)", re.MULTILINE)


@dataclass
class TestCase:
    """A generated Python test case linked to a Lean theorem."""

    theorem_name: str
    lean_file: str
    description: str
    code: str


@dataclass
class TheoremMapper:
    """Maps Lean theorem names to Python test generators."""

    mappings: dict[str, callable] = field(default_factory=dict)

    def register(self, pattern: str):
        """Decorator: register a generator for theorems matching pattern."""

        def decorator(func):
            self.mappings[pattern] = func
            return func

        return decorator

    def match(self, theorem_name: str, lean_file: str) -> TestCase | None:
        """Return a TestCase if the theorem name matches a known pattern."""
        for pattern, generator in self.mappings.items():
            if re.match(pattern, theorem_name):
                return generator(theorem_name, lean_file)
        return None


mapper = TheoremMapper()


# ======================================================================
# Binary operator theorems
# ======================================================================


@mapper.register(r"^emitBinOp_correct_add$")
def gen_add(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer addition correctness (emitBinOp_correct_add)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitBinOp_correct_add
            # Source: {lean_file}
            # Property: a + b on integers produces the same result in Molt as in Python.

            # Basic cases
            print(0 + 0)
            print(1 + 2)
            print(-1 + 1)
            print(-1 + (-1))

            # Identity
            print(42 + 0)
            print(0 + 42)

            # Large integers
            print(10**18 + 10**18)
            print(-(10**18) + 10**18)

            # Boundary values
            print(2**31 - 1 + 1)
            print(-(2**31) + (-1))
            print(2**63 - 1 + 1)
            print(-(2**63) + (-1))

            # Commutativity (verified by printing both orders)
            a, b = 17, 53
            print(a + b)
            print(b + a)
            print(a + b == b + a)

            # Associativity
            a, b, c = 11, 22, 33
            print((a + b) + c)
            print(a + (b + c))
            print((a + b) + c == a + (b + c))
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitBinOp_correct_sub$")
def gen_sub(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer subtraction correctness (emitBinOp_correct_sub)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitBinOp_correct_sub
            # Source: {lean_file}
            # Property: a - b on integers produces the same result in Molt as in Python.

            # Basic cases
            print(0 - 0)
            print(3 - 1)
            print(1 - 3)
            print(-1 - (-1))

            # Identity
            print(42 - 0)

            # Self-subtraction
            print(99 - 99)
            print(-99 - (-99))

            # Large integers
            print(10**18 - 1)
            print(1 - 10**18)
            print(-(10**18) - 10**18)

            # Boundary values
            print(2**31 - 1)
            print(-(2**31) - 1)
            print(2**63 - 1)
            print(-(2**63) - 1)

            # Non-commutativity
            a, b = 17, 53
            print(a - b)
            print(b - a)
            print(a - b == -(b - a))
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitBinOp_correct_mul$")
def gen_mul(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer multiplication correctness (emitBinOp_correct_mul)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitBinOp_correct_mul
            # Source: {lean_file}
            # Property: a * b on integers produces the same result in Molt as in Python.

            # Basic cases
            print(0 * 0)
            print(1 * 1)
            print(2 * 3)
            print(-2 * 3)
            print(-2 * -3)

            # Identity and zero
            print(42 * 1)
            print(1 * 42)
            print(42 * 0)
            print(0 * 42)

            # Negation via multiplication
            print(5 * -1)
            print(-1 * 5)

            # Large integers
            print(10**9 * 10**9)
            print(-(10**9) * 10**9)

            # Commutativity
            a, b = 17, 53
            print(a * b)
            print(b * a)
            print(a * b == b * a)

            # Associativity
            a, b, c = 3, 7, 11
            print((a * b) * c)
            print(a * (b * c))
            print((a * b) * c == a * (b * c))

            # Distributivity
            a, b, c = 5, 3, 7
            print(a * (b + c))
            print(a * b + a * c)
            print(a * (b + c) == a * b + a * c)
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitBinOp_correct_mod$")
def gen_mod(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer modulo correctness (emitBinOp_correct_mod)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitBinOp_correct_mod
            # Source: {lean_file}
            # Property: a % b on integers produces the same result in Molt as in Python.

            # Basic cases
            print(10 % 3)
            print(10 % 5)
            print(0 % 1)
            print(0 % 7)
            print(1 % 1)

            # Negative dividend (Python uses floored division for mod)
            print(-10 % 3)
            print(-1 % 3)
            print(-7 % 4)

            # Negative divisor
            print(10 % -3)
            print(1 % -3)
            print(7 % -4)

            # Both negative
            print(-10 % -3)
            print(-7 % -4)

            # Large values
            print(10**18 % 7)
            print(-(10**18) % 7)
            print(10**18 % (10**9 + 7))

            # Division by zero
            try:
                print(1 % 0)
            except ZeroDivisionError as e:
                print(f"ZeroDivisionError: {{e}}")

            # Invariant: a == (a // b) * b + (a % b)
            for a in [10, -10, 7, -7, 0, 100]:
                for b in [3, -3, 7, -7, 1, -1]:
                    print(a == (a // b) * b + (a % b))
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitBinOp_correct_eq$")
def gen_eq(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer equality comparison (emitBinOp_correct_eq)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitBinOp_correct_eq
            # Source: {lean_file}
            # Property: a == b on integers produces correct boolean result.

            # Basic cases
            print(0 == 0)
            print(1 == 1)
            print(1 == 2)
            print(-1 == -1)
            print(-1 == 1)

            # Identity
            x = 42
            print(x == x)
            print(x == 42)

            # Large integers
            print(10**18 == 10**18)
            print(10**18 == 10**18 + 1)
            print(-(10**18) == -(10**18))

            # Boundary values
            print(2**31 - 1 == 2**31 - 1)
            print(2**31 == 2**31)
            print(2**63 - 1 == 2**63 - 1)
            print(2**63 == 2**63)

            # Reflexivity
            for v in [0, 1, -1, 42, -42, 10**18, -(10**18)]:
                print(v == v)

            # Symmetry
            a, b = 17, 53
            print((a == b) == (b == a))
            a2, b2 = 42, 42
            print((a2 == b2) == (b2 == a2))

            # Negation
            print(not (1 == 2))
            print(not (1 == 1))
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitBinOp_correct_lt$")
def gen_lt(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer less-than comparison (emitBinOp_correct_lt)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitBinOp_correct_lt
            # Source: {lean_file}
            # Property: a < b on integers produces correct boolean result.

            # Basic cases
            print(0 < 1)
            print(1 < 0)
            print(0 < 0)
            print(-1 < 0)
            print(0 < -1)
            print(-1 < 1)
            print(1 < -1)

            # Irreflexivity
            for v in [0, 1, -1, 42, -42]:
                print(v < v)

            # Transitivity
            print(1 < 2 and 2 < 3)
            print(1 < 3)

            # Asymmetry
            a, b = 3, 7
            print(a < b)
            print(b < a)
            print(not (a < b and b < a))

            # Large integers
            print(10**18 < 10**18 + 1)
            print(10**18 + 1 < 10**18)
            print(-(10**18) < 10**18)
            print(10**18 < -(10**18))

            # Boundary values
            print(2**31 - 1 < 2**31)
            print(2**63 - 1 < 2**63)
            print(-(2**31) < 2**31 - 1)

            # Trichotomy: exactly one of a<b, a==b, a>b
            for a in [-5, 0, 5, 42]:
                for b in [-5, 0, 5, 42]:
                    lt = a < b
                    eq = a == b
                    gt = a > b
                    print(int(lt) + int(eq) + int(gt) == 1)
        """).format(lean_file=lean_file),
    )


# ======================================================================
# Unary operator theorems
# ======================================================================


@mapper.register(r"^emitUnOp_correct_neg$")
def gen_neg(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer negation correctness (emitUnOp_correct_neg)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitUnOp_correct_neg
            # Source: {lean_file}
            # Property: -a on integers produces correct result.

            # Basic cases
            print(-0)
            print(-1)
            print(-(-1))
            print(-42)
            print(-(-42))

            # Double negation is identity
            for v in [0, 1, -1, 42, -42, 10**18, -(10**18)]:
                print(-(-v) == v)

            # Negation inverts sign
            print(-5 < 0)
            print(-(-5) > 0)
            print(-0 == 0)

            # Large integers
            print(-(10**18))
            print(-(-(10**18)))
            print(-(2**63))
            print(-(-(2**63)))

            # Negation and addition: a + (-a) == 0
            for v in [1, -1, 42, -42, 10**18, -(10**18)]:
                print(v + (-v) == 0)

            # Negation distributes over addition
            a, b = 17, 53
            print(-(a + b) == (-a) + (-b))
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitUnOp_correct_not$")
def gen_not(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Boolean not correctness (emitUnOp_correct_not)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitUnOp_correct_not
            # Source: {lean_file}
            # Property: not b on booleans produces correct result.

            # Truth table
            print(not True)
            print(not False)

            # Double negation
            print(not not True)
            print(not not False)
            print(not not True == True)
            print(not not False == False)

            # Involution
            for b in [True, False]:
                print(not not b == b)

            # De Morgan's laws
            for a in [True, False]:
                for b in [True, False]:
                    print(not (a and b) == (not a or not b))
                    print(not (a or b) == (not a and not b))

            # not on truthy/falsy integer values
            print(not 0)
            print(not 1)
            print(not -1)
            print(not 42)
        """).format(lean_file=lean_file),
    )


# ======================================================================
# Index adjustment theorem
# ======================================================================


@mapper.register(r"^index_adjust_correct$")
def gen_index_adjust(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="0-to-1 based index adjustment (index_adjust_correct)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: index_adjust_correct
            # Source: {lean_file}
            # Property: 0-based index n maps to 1-based index n+1.
            # In Python this manifests as correct 0-based indexing on sequences.

            # List indexing (0-based)
            lst = [10, 20, 30, 40, 50]
            for i in range(len(lst)):
                print(f"lst[{{i}}] = {{lst[i]}}")

            # Negative indexing
            print(lst[-1])
            print(lst[-2])
            print(lst[-len(lst)])

            # Tuple indexing
            tup = (100, 200, 300)
            for i in range(len(tup)):
                print(f"tup[{{i}}] = {{tup[i]}}")

            # String indexing
            s = "hello"
            for i in range(len(s)):
                print(f"s[{{i}}] = {{s[i]}}")

            # Boundary: empty sequence
            empty: list[int] = []
            print(len(empty))
            try:
                print(empty[0])
            except IndexError as e:
                print(f"IndexError: {{e}}")

            # Boundary: single element
            single = [99]
            print(single[0])
            print(single[-1])

            # Index out of range
            try:
                print(lst[5])
            except IndexError as e:
                print(f"IndexError: {{e}}")

            try:
                print(lst[-6])
            except IndexError as e:
                print(f"IndexError: {{e}}")

            # Verify index arithmetic: element at i equals element at i - len for negative
            for i in range(len(lst)):
                print(lst[i] == lst[i - len(lst)])
        """).format(lean_file=lean_file),
    )


# ======================================================================
# evalLuauExpr_* theorems (literal evaluation)
# ======================================================================


@mapper.register(r"^evalLuauExpr_intLit$")
def gen_int_literal(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Integer literal evaluation (evalLuauExpr_intLit)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: evalLuauExpr_intLit
            # Source: {lean_file}
            # Property: Integer literals evaluate to their expected values.

            print(0)
            print(1)
            print(-1)
            print(42)
            print(-42)
            print(2**31 - 1)
            print(2**31)
            print(-(2**31))
            print(2**63 - 1)
            print(2**63)
            print(-(2**63))
            print(10**18)
            print(-(10**18))

            # Verify type
            print(type(0).__name__)
            print(type(42).__name__)
            print(type(-1).__name__)
            print(type(10**18).__name__)
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^evalLuauExpr_boolLit$")
def gen_bool_literal(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Boolean literal evaluation (evalLuauExpr_boolLit)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: evalLuauExpr_boolLit
            # Source: {lean_file}
            # Property: Boolean literals evaluate to their expected values.

            print(True)
            print(False)
            print(type(True).__name__)
            print(type(False).__name__)

            # Bool is a subclass of int
            print(isinstance(True, int))
            print(isinstance(False, int))
            print(True == 1)
            print(False == 0)
            print(True + True)
            print(False + False)
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^evalLuauExpr_strLit$")
def gen_str_literal(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="String literal evaluation (evalLuauExpr_strLit)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: evalLuauExpr_strLit
            # Source: {lean_file}
            # Property: String literals evaluate to their expected values.

            print("")
            print("hello")
            print("world")
            print("hello world")
            print(type("").__name__)
            print(type("hello").__name__)

            # Empty string is falsy
            print(bool(""))
            print(bool("x"))

            # Length
            print(len(""))
            print(len("hello"))
            print(len("abc"))

            # Escape sequences
            print("a\\nb")
            print("a\\tb")
            print("a\\\\b")
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^evalLuauExpr_nil$")
def gen_none_literal(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="None literal evaluation (evalLuauExpr_nil)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: evalLuauExpr_nil
            # Source: {lean_file}
            # Property: None literal evaluates correctly.

            print(None)
            print(type(None).__name__)
            print(None is None)
            print(None == None)
            print(bool(None))

            # None is a singleton
            a = None
            b = None
            print(a is b)

            # None is falsy
            if not None:
                print("None is falsy")
            else:
                print("None is truthy")

            # repr and str
            print(repr(None))
            print(str(None))
        """).format(lean_file=lean_file),
    )


# ======================================================================
# Constant folding theorem
# ======================================================================


@mapper.register(r"^constFoldExpr_correct$")
def gen_const_fold(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Constant folding preserves semantics (constFoldExpr_correct)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: constFoldExpr_correct
            # Source: {lean_file}
            # Property: Constant folding does not change expression results.
            # These expressions should produce identical results whether or not
            # the compiler constant-folds them.

            # Arithmetic constant expressions
            print(2 + 3)
            print(10 - 4)
            print(6 * 7)
            print(10 % 3)

            # Nested constant expressions
            print((2 + 3) * (4 + 5))
            print((10 - 3) * 2 + 1)
            print(((1 + 2) + 3) + 4)

            # Unary constant expressions
            print(-5)
            print(-(-5))
            print(not True)
            print(not False)
            print(not not True)

            # Mixed with variables (should not be folded but still correct)
            x = 10
            print(x + 5)
            print(x * 2 + 3)
            print(x - x)
            print(x + 0)
            print(x * 1)

            # Boolean constant expressions
            print(True and True)
            print(True and False)
            print(False or True)
            print(False or False)

            # Comparison constant expressions
            print(1 == 1)
            print(1 == 2)
            print(1 < 2)
            print(2 < 1)
            print(3 < 3)

            # Expressions that a constant folder might simplify
            print(0 + 42)
            print(42 + 0)
            print(42 * 1)
            print(1 * 42)
            print(42 - 0)
            print(0 * 999)
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^constFoldInstr_correct$")
def gen_const_fold_instr(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Constant folding on instructions preserves semantics (constFoldInstr_correct)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: constFoldInstr_correct
            # Source: {lean_file}
            # Property: Instruction-level constant folding preserves semantics.
            # Variable assignments with foldable RHS should produce the same values.

            # Simple foldable assignments
            a = 2 + 3
            print(a)
            b = 10 * 5
            print(b)
            c = 100 - 1
            print(c)
            d = 17 % 5
            print(d)

            # Nested foldable
            e = (2 + 3) * (4 + 5)
            print(e)

            # Unary foldable
            f = -42
            print(f)
            g = not True
            print(g)

            # Use folded results in subsequent operations
            h = a + b
            print(h)
            i = a * b - c
            print(i)
        """).format(lean_file=lean_file),
    )


# ======================================================================
# Expression emission correctness (structural)
# ======================================================================


@mapper.register(r"^emitExpr_correct_val$")
def gen_expr_val(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Value literal expression correctness (emitExpr_correct_val)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitExpr_correct_val
            # Source: {lean_file}
            # Property: Value literals in expressions evaluate correctly.

            # Integer values in expressions
            print(1 + 0)
            print(0 + 0)
            print(42 + 0)

            # Boolean values in expressions
            print(True and True)
            print(False or False)

            # String values in expressions
            print("hello" + "")
            print("" + "world")

            # None in expressions
            print(None is None)
            print(None == None)
        """).format(lean_file=lean_file),
    )


@mapper.register(r"^emitExpr_correct$")
def gen_expr_full(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Full expression emission correctness (emitExpr_correct)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: emitExpr_correct
            # Source: {lean_file}
            # Property: Full structural induction -- all expression forms evaluate correctly.

            # Literal expressions
            print(42)
            print(True)
            print("hello")
            print(None)

            # Variable reference expressions
            x = 10
            y = 20
            print(x)
            print(y)

            # Binary expressions (nested)
            print(x + y)
            print(x - y)
            print(x * y)
            print((x + y) * (x - y))
            print(x + y + 1)

            # Unary expressions
            print(-x)
            print(not True)
            print(-(-x))

            # Mixed nesting
            a = 3
            b = 4
            c = 5
            print(a + b * c)
            print((a + b) * c)
            print(-(a + b))
            print(a * b + c * (a - b))

            # Deeply nested
            print(((1 + 2) * 3 + 4) * 5)
            print(-(((1 + 2) * 3)))
        """).format(lean_file=lean_file),
    )


# ======================================================================
# Index adjustment semantic theorem
# ======================================================================


@mapper.register(r"^index_adjust_semantic$")
def gen_index_adjust_semantic(name: str, lean_file: str) -> TestCase:
    return TestCase(
        theorem_name=name,
        lean_file=lean_file,
        description="Semantic index adjustment evaluation (index_adjust_semantic)",
        code=textwrap.dedent("""\
            # Generated from Lean theorem: index_adjust_semantic
            # Source: {lean_file}
            # Property: adjustIndex(intLit(n)) evaluates to n+1 (semantic evaluation).
            # Validates that 0-based to 1-based index conversion works at evaluation time.

            # For Python (0-based), element at index i in a list of size n
            # corresponds to 1-based index i+1. Test that list[i] works correctly.
            lst = list(range(1, 11))  # [1, 2, ..., 10]

            # Verify each 0-based index accesses the correct element
            for i in range(10):
                # Element at 0-based index i should be i+1 (by construction)
                print(lst[i] == i + 1)

            # Verify slice semantics preserve index arithmetic
            print(lst[0:3])
            print(lst[3:7])
            print(lst[7:10])

            # Verify that index i gives the (i+1)-th element (1-based)
            for i in range(len(lst)):
                print(f"0-based index {{i}} -> value {{lst[i]}} (1-based position {{i + 1}})")
        """).format(lean_file=lean_file),
    )


# ======================================================================
# Main logic
# ======================================================================


def extract_theorems(lean_file: Path) -> list[str]:
    """Extract all theorem names from a Lean 4 file."""
    if not lean_file.exists():
        return []
    text = lean_file.read_text(encoding="utf-8")
    return THEOREM_RE.findall(text)


def generate_all_tests() -> list[TestCase]:
    """Parse Lean files and generate test cases for matched theorems."""
    tests: list[TestCase] = []
    seen: set[str] = set()

    for lean_file in LEAN_FILES:
        if not lean_file.exists():
            print(f"WARNING: Lean file not found: {lean_file}", file=sys.stderr)
            continue

        rel_path = lean_file.relative_to(REPO_ROOT)
        theorem_names = extract_theorems(lean_file)
        print(f"Found {len(theorem_names)} theorems in {rel_path}", file=sys.stderr)

        for thm_name in theorem_names:
            if thm_name in seen:
                continue
            seen.add(thm_name)

            test_case = mapper.match(thm_name, str(rel_path))
            if test_case is not None:
                tests.append(test_case)
                print(f"  -> Generated test for: {thm_name}", file=sys.stderr)
            else:
                print(f"  -- Skipped (no mapping): {thm_name}", file=sys.stderr)

    return tests


def write_tests(tests: list[TestCase], dry_run: bool = False) -> list[Path]:
    """Write generated test files. Returns list of written paths."""
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    # Ensure __init__.py exists
    init_path = OUTPUT_DIR / "__init__.py"
    if not init_path.exists():
        init_path.write_text("", encoding="utf-8")

    written: list[Path] = []
    for test in tests:
        filename = f"test_{test.theorem_name}.py"
        filepath = OUTPUT_DIR / filename

        if dry_run:
            print(f"Would write: {filepath}")
            print(f"  Description: {test.description}")
            continue

        filepath.write_text(test.code, encoding="utf-8")
        written.append(filepath)
        print(f"Wrote: {filepath.relative_to(REPO_ROOT)}", file=sys.stderr)

    return written


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate Python differential tests from Lean 4 theorems."
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print what would be generated without writing files.",
    )
    args = parser.parse_args()

    tests = generate_all_tests()
    if not tests:
        print("No test cases generated.", file=sys.stderr)
        sys.exit(1)

    written = write_tests(tests, dry_run=args.dry_run)

    print(f"\nGenerated {len(tests)} test files from Lean theorems.", file=sys.stderr)
    if not args.dry_run:
        print(f"Output directory: {OUTPUT_DIR.relative_to(REPO_ROOT)}", file=sys.stderr)
        for path in written:
            print(f"  {path.relative_to(REPO_ROOT)}")


if __name__ == "__main__":
    main()
