#!/usr/bin/env python3
"""theorem_to_test.py -- Generate Python differential tests from Lean 4 theorems.

Parses theorem statements from formal/lean/MoltTIR/ and generates concrete
Python test files under tests/differential/basic/generated_from_proofs/ that
exercise the same properties the theorems prove.

Usage:
    uv run python3 tools/theorem_to_test.py [--dry-run]
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
    LEAN_DIR / "Passes" / "ConstFoldCorrect.lean",
    LEAN_DIR / "Backend" / "WasmCorrect.lean",
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
    if not init_path.exists() or not init_path.read_text(encoding="utf-8").strip():
        init_path.write_text("# MOLT_META: source_role=fixture\n", encoding="utf-8")

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
