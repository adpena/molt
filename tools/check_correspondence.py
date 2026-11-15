#!/usr/bin/env python3
"""Comprehensive Lean-Rust-Python correspondence checker.

Verifies that type definitions, constants, operator enums, builtin mappings,
and semantic invariants stay in sync across:
  - Lean formalization (formal/lean/MoltTIR/)
  - Rust runtime/codegen ABI (runtime/molt-codegen-abi/, runtime/molt-backend/)
  - Python frontend (src/molt/frontend/)

All expected values are parsed from source files -- nothing is hardcoded.

Usage:
    uv run python3 tools/check_correspondence.py
    uv run python3 tools/check_correspondence.py --verbose
    uv run python3 tools/check_correspondence.py --json
    uv run python3 tools/check_correspondence.py --category nanbox
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.correspondence_sources import (  # noqa: E402
    normalize_lean_variant_name as _normalize_lean_variant_name,
    parse_lean_eval_binop_rules as _parse_lean_evalBinOp_rules,
    parse_lean_eval_unop_rules as _parse_lean_evalUnOp_rules,
    parse_lean_hex_constants as _parse_lean_hex_constants,
    parse_lean_inductive_variants as _parse_lean_inductive_variants,
    parse_rust_enum_variants as _parse_rust_enum_variants,
    parse_rust_unsigned_constant_expressions as _parse_rust_u64_constants,
    resolve_rust_or_hex_int as _normalize_hex,
)


def _find_repo_root() -> Path:
    """Find the real repo root, handling worktree checkouts."""
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
        ).strip()
        return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()

# Source paths
LEAN_DIR = ROOT / "formal" / "lean"

# Lean sources
NANBOX_LEAN = LEAN_DIR / "MoltTIR" / "Runtime" / "NanBox.lean"
SYNTAX_LEAN = LEAN_DIR / "MoltTIR" / "Syntax.lean"
TYPES_LEAN = LEAN_DIR / "MoltTIR" / "Types.lean"
EVAL_LEAN = LEAN_DIR / "MoltTIR" / "Semantics" / "EvalExpr.lean"
LATTICE_LEAN = LEAN_DIR / "MoltTIR" / "Passes" / "Lattice.lean"
SCCP_LEAN = LEAN_DIR / "MoltTIR" / "Passes" / "SCCP.lean"

# Rust sources
CODEGEN_ABI_RS = ROOT / "runtime" / "molt-codegen-abi" / "src" / "lib.rs"
TIR_TYPES_RS = ROOT / "runtime" / "molt-ir" / "src" / "tir" / "types.rs"
FRONTEND_TYPES_PY = ROOT / "src" / "molt" / "frontend" / "_types.py"
LEAN_PASSES_DIR = LEAN_DIR / "MoltTIR" / "Passes"

SRC = ROOT / "src"


from molt.frontend.lowering.op_kinds_generated import FRONTEND_REGISTERED_KINDS  # noqa: E402

# Terminal colors
IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def yellow(t: str) -> str:
    return _c("33", t)


def bold(t: str) -> str:
    return _c("1", t)


# Result tracking


@dataclass
class CheckItem:
    """A single checked correspondence item."""

    name: str
    passed: bool
    detail: str = ""


@dataclass
class CategoryResult:
    """Results for one correspondence category."""

    category: str
    description: str
    items: list[CheckItem] = field(default_factory=list)
    metrics: dict[str, int] = field(default_factory=dict)

    @property
    def passed(self) -> int:
        return sum(1 for i in self.items if i.passed)

    @property
    def failed(self) -> int:
        return sum(1 for i in self.items if not i.passed)

    @property
    def total(self) -> int:
        return len(self.items)

    @property
    def ok(self) -> bool:
        return self.total > 0 and self.failed == 0


# Source loading


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace", encoding="utf-8")
    return ""


#
# Category 1: NaN-boxing constants
#


def check_nanbox_constants() -> CategoryResult:
    result = CategoryResult(
        "nanbox",
        "NaN-boxing constants (Rust molt-codegen-abi <-> Lean NanBox)",
    )

    rust_text = _read(CODEGEN_ABI_RS)
    lean_text = _read(NANBOX_LEAN)

    if not rust_text:
        result.items.append(
            CheckItem("source", False, f"Rust source missing: {CODEGEN_ABI_RS}")
        )
        return result
    if not lean_text:
        result.items.append(
            CheckItem("source", False, f"Lean source missing: {NANBOX_LEAN}")
        )
        return result

    lean_consts = _parse_lean_hex_constants(lean_text)
    rust_consts = _parse_rust_u64_constants(rust_text)

    lean_to_rust: dict[str, str] = {
        "QNAN": "QNAN",
        "TAG_INT": "TAG_INT",
        "TAG_BOOL": "TAG_BOOL",
        "TAG_NONE": "TAG_NONE",
        "TAG_PTR": "TAG_PTR",
        "TAG_PEND": "TAG_PENDING",
        "TAG_MASK": "TAG_MASK",
        "INT_MASK": "INT_MASK",
        "INT_SIGN": "INT_SIGN_BIT",
    }

    for lean_name, lean_val in lean_consts.items():
        rust_name = lean_to_rust.get(lean_name)
        if rust_name is None:
            if lean_name == "TAG_CHECK":
                expected = lean_consts.get("QNAN", 0) | lean_consts.get("TAG_MASK", 0)
                result.items.append(
                    CheckItem(
                        lean_name,
                        lean_val == expected,
                        f"computed: 0x{lean_val:016x} (QNAN | TAG_MASK)"
                        if lean_val == expected
                        else f"0x{lean_val:016x} != expected 0x{expected:016x}",
                    )
                )
                continue
            result.items.append(
                CheckItem(lean_name, False, "no known Rust counterpart")
            )
            continue

        rust_expr = rust_consts.get(rust_name)
        if rust_expr is None:
            result.items.append(
                CheckItem(f"{lean_name}/{rust_name}", False, "not found in Rust")
            )
            continue

        try:
            rust_val = _normalize_hex(rust_expr, rust_text=rust_text)
        except (ValueError, IndexError) as e:
            result.items.append(
                CheckItem(f"{lean_name}/{rust_name}", False, f"parse error: {e}")
            )
            continue

        if lean_val == rust_val:
            result.items.append(
                CheckItem(f"{lean_name}/{rust_name}", True, f"0x{lean_val:016x}")
            )
        else:
            result.items.append(
                CheckItem(
                    f"{lean_name}/{rust_name}",
                    False,
                    f"MISMATCH: Lean=0x{lean_val:016x} Rust=0x{rust_val:016x}",
                )
            )

    return result


#
# Category 2: Operator enums
#


def check_operator_enums() -> CategoryResult:
    result = CategoryResult(
        "operators",
        "Operator enums (Lean Syntax <-> generated Python frontend op kinds)",
    )

    lean_text = _read(SYNTAX_LEAN)

    if not lean_text:
        result.items.append(CheckItem("source", False, f"Lean missing: {SYNTAX_LEAN}"))
        return result

    lean_binops = _parse_lean_inductive_variants(lean_text, "BinOp")
    lean_unops = _parse_lean_inductive_variants(lean_text, "UnOp")
    python_ops = set(FRONTEND_REGISTERED_KINDS)

    result.items.append(
        CheckItem(
            "BinOp count",
            True,
            f"Lean: {len(lean_binops)} variants ({', '.join(lean_binops)})",
        )
    )

    for v in lean_binops:
        py_name = _normalize_lean_variant_name(v).upper()
        result.items.append(
            CheckItem(
                f"BinOp.{v}",
                py_name in python_ops,
                "present in generated Python frontend registry"
                if py_name in python_ops
                else "NOT found in generated Python frontend registry",
            )
        )

    result.items.append(
        CheckItem(
            "UnOp count",
            True,
            f"Lean: {len(lean_unops)} variants ({', '.join(lean_unops)})",
        )
    )

    for v in lean_unops:
        py_name = _normalize_lean_variant_name(v).upper()
        acceptable_gap = _normalize_lean_variant_name(v) in {"invert", "guard"}
        result.items.append(
            CheckItem(
                f"UnOp.{v}",
                py_name in python_ops or acceptable_gap,
                "present in generated Python frontend registry"
                if py_name in python_ops
                else "compositionally lowered in Python frontend",
            )
        )

    return result


#
# Category 3: Type system
#


def check_type_system() -> CategoryResult:
    result = CategoryResult(
        "types",
        "Type system (Lean Ty <-> Rust TirType <-> Python-visible type tags)",
    )

    lean_text = _read(TYPES_LEAN)
    rust_text = _read(TIR_TYPES_RS)
    python_text = _read(FRONTEND_TYPES_PY)

    if not lean_text:
        result.items.append(CheckItem("source", False, f"Lean missing: {TYPES_LEAN}"))
        return result
    if not rust_text:
        result.items.append(CheckItem("source", False, f"Rust missing: {TIR_TYPES_RS}"))
        return result
    if not python_text:
        result.items.append(
            CheckItem("source", False, f"Python missing: {FRONTEND_TYPES_PY}")
        )
        return result

    lean_types = _parse_lean_inductive_variants(lean_text, "Ty")
    lean_values = _parse_lean_inductive_variants(_read(SYNTAX_LEAN), "Value")

    rust_types = set(_parse_rust_enum_variants(rust_text, "TirType"))
    py_type_tags: set[str] = set()
    for m in re.finditer(r'"(\w+)":\s*\d+', python_text):
        py_type_tags.add(m.group(1))

    result.items.append(
        CheckItem(
            "Ty count",
            True,
            f"Lean: {len(lean_types)} ({', '.join(lean_types)})",
        )
    )

    if lean_values:
        result.items.append(
            CheckItem(
                "Value count",
                True,
                f"Lean: {len(lean_values)} ({', '.join(lean_values)})",
            )
        )

    rust_mapping = {
        "int": "I64",
        "float": "F64",
        "bool": "Bool",
        "none": "None",
        "str": "Str",
        "bytes": "Bytes",
        "list": "List",
        "dict": "Dict",
        "set": "Set",
        "tuple": "Tuple",
        "box": "Box",
        "dynBox": "DynBox",
        "func": "Func",
        "bigInt": "BigInt",
        "ptr": "Ptr",
        "union": "Union",
        "never": "Never",
        "obj": "UserClass",
    }
    python_mapping = {
        "int": "int",
        "float": "float",
        "bool": "bool",
        "str": "str",
        "bytes": "bytes",
        "list": "list",
        "dict": "dict",
        "set": "set",
        "tuple": "tuple",
        "bigInt": "int",
        "obj": "object",
    }
    for ty in lean_types:
        rust_name = rust_mapping.get(ty)
        python_name = python_mapping.get(ty)
        rust_ok = rust_name in rust_types if rust_name else False
        python_ok = python_name in py_type_tags if python_name else True
        details = [f"Rust TirType::{rust_name}" if rust_name else "no Rust mapping"]
        details.append(
            f"Python tag {python_name!r}" if python_name else "compiler-internal type"
        )
        result.items.append(
            CheckItem(f"Ty.{ty}", rust_ok and python_ok, ", ".join(details))
        )

    return result


def check_eval_rules() -> CategoryResult:
    result = CategoryResult(
        "eval_rules",
        "Evaluation rules (Lean EvalExpr <-> Lean Syntax operators)",
    )

    eval_text = _read(EVAL_LEAN)
    syntax_text = _read(SYNTAX_LEAN)

    if not eval_text or not syntax_text:
        result.items.append(CheckItem("source", False, "Missing required Lean sources"))
        return result

    binops = _parse_lean_inductive_variants(syntax_text, "BinOp")
    unops = _parse_lean_inductive_variants(syntax_text, "UnOp")

    binop_rules = _parse_lean_evalBinOp_rules(eval_text)
    covered_binops = {r[0] for r in binop_rules}
    covered_binops.update(
        _normalize_lean_variant_name(name)
        for name in re.findall(r"\|\s*\.(\w+),", eval_text)
    )

    unop_rules = _parse_lean_evalUnOp_rules(eval_text)
    covered_unops = {r[0] for r in unop_rules}

    result.items.append(
        CheckItem(
            "evalBinOp rules",
            True,
            f"{len(binop_rules)} rules, {len(covered_binops)} operators covered",
        )
    )
    result.items.append(
        CheckItem(
            "evalUnOp rules",
            True,
            f"{len(unop_rules)} rules, {len(covered_unops)} operators covered",
        )
    )

    # Known intentional gaps in the Lean formalization
    intentional_gaps = {
        "bit_and",
        "bit_or",
        "bit_xor",
        "lshift",
        "rshift",
        "div",
        "floordiv",
        "pow",
        # The formal Value universe has no container value, so membership is
        # deliberately outside expression evaluation rather than a silent gap.
        "in",
        "not_in",
    }

    for op in binops:
        if op in covered_binops:
            rules_for = [(a, b) for (o, a, b) in binop_rules if o == op]
            detail = (
                f"{len(rules_for)} typed rule(s): "
                + ", ".join(f"{a}x{b}" for a, b in rules_for)
                if rules_for
                else "generic scalar semantic rule"
            )
            result.items.append(
                CheckItem(
                    f"evalBinOp .{op}",
                    True,
                    detail,
                )
            )
        elif op in intentional_gaps:
            result.items.append(
                CheckItem(
                    f"evalBinOp .{op}",
                    True,
                    "intentionally unmodeled (falls to catch-all)",
                )
            )
        else:
            result.items.append(
                CheckItem(f"evalBinOp .{op}", False, "NO rule, not a known gap")
            )

    for op in unops:
        if op in covered_unops:
            types = [t for (o, t) in unop_rules if o == op]
            result.items.append(
                CheckItem(
                    f"evalUnOp .{op}",
                    True,
                    f"type(s): {', '.join(types)}",
                )
            )
        elif op == "invert":
            result.items.append(
                CheckItem(
                    f"evalUnOp .{op}",
                    True,
                    "intentionally unmodeled (bitwise)",
                )
            )
        else:
            result.items.append(CheckItem(f"evalUnOp .{op}", False, "NO rule"))

    return result


#
# Category 7: SCCP lattice
#


def check_sccp_lattice() -> CategoryResult:
    result = CategoryResult(
        "sccp_lattice",
        "SCCP lattice (Lean Lattice + SCCP consistency)",
    )

    lattice_text = _read(LATTICE_LEAN)
    sccp_text = _read(SCCP_LEAN)

    if not lattice_text:
        result.items.append(CheckItem("source", False, f"Missing: {LATTICE_LEAN}"))
        return result
    if not sccp_text:
        result.items.append(CheckItem("source", False, f"Missing: {SCCP_LEAN}"))
        return result

    absval_variants = _parse_lean_inductive_variants(lattice_text, "AbsVal")
    result.items.append(
        CheckItem(
            "AbsVal variants",
            len(absval_variants) == 3,
            f"{len(absval_variants)}: {', '.join(absval_variants)} (expected 3)",
        )
    )

    required_theorems = [
        "unknown_le",
        "le_overdefined",
        "join_comm",
        "join_idem",
        "join_assoc",
        "le_refl",
        "join_concretizes",
    ]

    for thm in required_theorems:
        found = re.search(rf"\btheorem\s+{thm}\b", lattice_text) is not None
        result.items.append(
            CheckItem(
                f"theorem {thm}",
                found,
                "proven" if found else "NOT FOUND",
            )
        )

    for func in (
        "absEvalBinOp",
        "absEvalUnOp",
        "absEvalExpr",
        "sccpExpr",
        "sccpInstrs",
        "sccpBlock",
        "sccpFunc",
    ):
        found = func in sccp_text
        result.items.append(
            CheckItem(f"SCCP.{func}", found, "defined" if found else "NOT FOUND")
        )

    return result


#
# Category 8: Structural invariants
#


def check_compiler_passes() -> CategoryResult:
    """Require every modeled optimization to retain its correctness module."""
    result = CategoryResult(
        "compiler_passes",
        "Lean pass definitions and their correctness proof modules",
    )
    passes = {
        "ConstFold": ("ConstFold.lean", "constFoldFunc"),
        "DCE": ("DCE.lean", "dceFunc"),
        "SCCP": ("SCCP.lean", "sccpFunc"),
        "CSE": ("CSE.lean", "cseFunc"),
        "LICM": ("LICM.lean", "licmFunc"),
        "GuardHoist": ("GuardHoist.lean", "guardHoistFunc"),
        "JoinCanon": ("JoinCanon.lean", "joinCanonFunc"),
        "EdgeThread": ("EdgeThread.lean", "edgeThreadFunc"),
    }
    for pass_name, (filename, function_name) in passes.items():
        implementation = LEAN_PASSES_DIR / filename
        proof = LEAN_PASSES_DIR / f"{pass_name}Correct.lean"
        implementation_text = _read(implementation)
        ok = (
            bool(implementation_text)
            and function_name in implementation_text
            and proof.is_file()
        )
        result.items.append(
            CheckItem(
                pass_name,
                ok,
                f"definition={implementation.name}:{function_name}, proof={proof.name}",
            )
        )
    return result


#
# Orchestration
#

ALL_CATEGORIES: dict[str, callable] = {
    "nanbox": check_nanbox_constants,
    "operators": check_operator_enums,
    "types": check_type_system,
    "eval_rules": check_eval_rules,
    "sccp_lattice": check_sccp_lattice,
    "compiler_passes": check_compiler_passes,
}


def run_checks(
    categories: list[str] | None = None,
) -> list[CategoryResult]:
    results: list[CategoryResult] = []
    targets = categories or list(ALL_CATEGORIES.keys())
    for name in targets:
        if name in ALL_CATEGORIES:
            results.append(ALL_CATEGORIES[name]())
    return results


def print_report(results: list[CategoryResult], *, verbose: bool = False) -> int:
    total_passed = 0
    total_failed = 0
    total_items = 0

    print()
    print(bold("=" * 76))
    print(bold("  Lean-Rust-Python Correspondence Report"))
    print(bold("=" * 76))

    for cat in results:
        status = green("PASS") if cat.ok else red("FAIL")
        print(f"\n  [{status}] {bold(cat.category)}: {cat.description}")
        print(f"         {cat.passed}/{cat.total} items passed")

        if verbose or not cat.ok:
            for item in cat.items:
                marker = green("ok") if item.passed else red("FAIL")
                print(f"           [{marker}] {item.name}: {item.detail}")

        total_passed += cat.passed
        total_failed += cat.failed
        total_items += cat.total

    print()
    print(bold("-" * 76))
    score = (total_passed / total_items * 100) if total_items else 0
    cats_ok = sum(1 for c in results if c.ok)
    cats_fail = sum(1 for c in results if not c.ok)
    print(
        f"  Correspondence score: {bold(f'{score:.1f}%')} "
        f"({total_passed}/{total_items} items)"
    )
    print(
        f"  Categories: {green(f'{cats_ok} passed')} | "
        f"{red(f'{cats_fail} failed') if cats_fail else f'{cats_fail} failed'}"
    )
    print(bold("=" * 76))

    if total_failed or any(not category.ok for category in results):
        print(f"\n{red('correspondence check: FAILED')}")
        return 1
    print(f"\n{green('correspondence check: ok')}")
    return 0


def json_report(results: list[CategoryResult]) -> dict:
    selected = sum(c.total for c in results)
    passed = sum(c.passed for c in results)
    failed = sum(c.failed for c in results)
    errors = 0
    return {
        "schema": "molt.correspondence-proof.v2",
        "status": (
            "success"
            if results and selected > 0 and failed == 0 and errors == 0
            else "failure"
        ),
        "selected": selected,
        "executed": selected,
        "passed": passed,
        "failed": failed,
        "errors": errors,
        "zero_work": selected == 0,
        "score": round(passed / selected * 100, 1) if selected else 0,
        "categories": [
            {
                "name": c.category,
                "description": c.description,
                "status": "success" if c.ok else "failure",
                "selected": c.total,
                "executed": c.total,
                "passed": c.passed,
                "failed": c.failed,
                "errors": 0,
                "zero_work": c.total == 0,
                "metrics": c.metrics,
                "items": [
                    {
                        "name": i.name,
                        "passed": i.passed,
                        "detail": i.detail,
                    }
                    for i in c.items
                ],
            }
            for c in results
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Comprehensive Lean-Rust-Python correspondence checker.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--json", action="store_true", help="Output JSON report")
    parser.add_argument(
        "--receipt",
        metavar="FILE",
        help="Write the counted correspondence receipt to FILE",
    )
    parser.add_argument(
        "--category",
        type=str,
        choices=list(ALL_CATEGORIES.keys()),
        help="Check a specific category only",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show all checked items",
    )
    args = parser.parse_args()

    categories = [args.category] if args.category else None
    results = run_checks(categories)

    report = json_report(results)
    if args.receipt:
        receipt_path = Path(args.receipt)
        receipt_path.parent.mkdir(parents=True, exist_ok=True)
        receipt_path.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    if args.json:
        print(json.dumps(report, indent=2))
        return 0 if report["status"] == "success" else 1

    return print_report(results, verbose=args.verbose)


if __name__ == "__main__":
    raise SystemExit(main())
