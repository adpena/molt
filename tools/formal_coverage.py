#!/usr/bin/env python3
"""Formal methods coverage quantification.

Reports how much of the real Molt compiler is covered by formal proofs:
  - Opcode coverage: Lean Expr/BinOp/UnOp vs real MoltOp kinds
  - Pass coverage: which midend passes have correctness proofs
  - CFG pattern coverage: which control-flow constructs are modeled

Usage:
  python3 tools/formal_coverage.py           # full report
  python3 tools/formal_coverage.py --json    # machine-readable JSON
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LEAN_DIR = ROOT / "formal" / "lean" / "MoltTIR"

# ── Real compiler opcodes (from MoltOp.kind values) ──────────────
# Arithmetic / comparison / bitwise
REAL_BINOPS = {
    "add",
    "sub",
    "mul",
    "div",
    "floordiv",
    "mod",
    "pow",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "bit_and",
    "bit_or",
    "bit_xor",
    "lshift",
    "rshift",
    "matmul",
    "truediv",
}

REAL_UNOPS = {
    "neg",
    "not",
    "abs",
    "invert",
    "pos",
    "bool_cast",
    "int_cast",
    "float_cast",
    "str_cast",
}

REAL_EXPR_KINDS = {
    "const_int",
    "const_float",
    "const_bool",
    "const_str",
    "const_none",
    "load_var",
    "store_var",
    "binop",
    "unop",
    "call_func",
    "call_method",
    "get_attr",
    "set_attr",
    "get_item",
    "set_item",
    "del_item",
    "build_list",
    "build_tuple",
    "build_set",
    "build_dict",
    "build_slice",
    "build_string",
    "import_name",
    "import_from",
    "unpack_sequence",
    "unpack_ex",
    "format_value",
    "contains_op",
}

REAL_CONTROL_FLOW = {
    "if_branch",
    "else_branch",
    "end_if",
    "for_loop",
    "while_loop",
    "end_loop",
    "break",
    "continue",
    "return",
    "try",
    "except",
    "finally",
    "end_try",
    "raise",
    "reraise",
    "with_enter",
    "with_exit",
    "yield",
    "yield_from",
    "await",
}

# Midend passes in real compiler
REAL_PASSES = {
    "const_fold": "Constant folding",
    "dce": "Dead code elimination",
    "sccp": "Sparse conditional constant propagation",
    "cse": "Common subexpression elimination",
    "simplify": "Expression simplification",
    "join_canonicalize": "Join point canonicalization",
    "guard_hoist": "Guard hoisting",
    "prune": "Unreachable block pruning",
    "verifier": "IR well-formedness verifier",
    "licm": "Loop invariant code motion",
}


def parse_lean_binops() -> set[str]:
    """Extract BinOp constructors from Syntax.lean."""
    syntax = LEAN_DIR / "Syntax.lean"
    if not syntax.exists():
        return set()
    text = syntax.read_text()
    ops: set[str] = set()
    in_binop = False
    for line in text.splitlines():
        if "inductive BinOp" in line:
            in_binop = True
            continue
        if in_binop:
            if line.strip().startswith("deriving") or (
                line.strip().startswith("inductive") and "BinOp" not in line
            ):
                break
            matches = re.findall(r"\|\s+(\w+)", line)
            matches = [name[:-1] if name.endswith("_") else name for name in matches]
            ops.update(matches)
    return ops


def parse_lean_unops() -> set[str]:
    """Extract UnOp constructors from Syntax.lean."""
    syntax = LEAN_DIR / "Syntax.lean"
    if not syntax.exists():
        return set()
    text = syntax.read_text()
    ops: set[str] = set()
    in_unop = False
    for line in text.splitlines():
        if "inductive UnOp" in line:
            in_unop = True
            continue
        if in_unop:
            if line.strip().startswith("deriving") or (
                line.strip().startswith("inductive") and "UnOp" not in line
            ):
                break
            matches = re.findall(r"\|\s+(\w+)", line)
            matches = [name[:-1] if name.endswith("_") else name for name in matches]
            ops.update(matches)
    return ops


def parse_lean_expr_kinds() -> set[str]:
    """Extract Expr constructors from Syntax.lean."""
    syntax = LEAN_DIR / "Syntax.lean"
    if not syntax.exists():
        return set()
    text = syntax.read_text()
    kinds: set[str] = set()
    in_expr = False
    for line in text.splitlines():
        if "inductive Expr" in line:
            in_expr = True
            continue
        if in_expr:
            if line.strip().startswith("deriving") or (
                line.strip().startswith("inductive") and "Expr" not in line
            ):
                break
            matches = re.findall(r"\|\s+(\w+)", line)
            kinds.update(matches)
    return kinds


def count_theorems(path: Path) -> int:
    """Count theorem/lemma declarations in a Lean file."""
    if not path.exists():
        return 0
    text = path.read_text()
    return len(
        re.findall(
            r"^(theorem|lemma|private theorem|private lemma)\s+", text, re.MULTILINE
        )
    )


def count_sorry(path: Path) -> int:
    """Count sorry occurrences (actual usage, not comments) in a Lean file."""
    if not path.exists():
        return 0
    text = path.read_text()
    return len(re.findall(r"^\s*sorry\b", text, re.MULTILINE))


def scan_passes() -> dict[str, dict]:
    """Scan which passes have formal proofs."""
    passes_dir = LEAN_DIR / "Passes"
    results = {}
    for pass_name, desc in REAL_PASSES.items():
        lean_name = {
            "const_fold": "ConstFold",
            "dce": "DCE",
            "sccp": "SCCP",
            "cse": "CSE",
            "simplify": None,
            "join_canonicalize": None,
            "guard_hoist": None,
            "prune": None,
            "verifier": None,
            "licm": "LICM",
        }.get(pass_name)

        if lean_name is None:
            results[pass_name] = {
                "description": desc,
                "formalized": False,
                "correctness_proof": False,
                "theorems": 0,
                "sorry_count": 0,
            }
            continue

        pass_file = passes_dir / f"{lean_name}.lean"
        correct_file = passes_dir / f"{lean_name}Correct.lean"

        formalized = pass_file.exists()
        has_proof = correct_file.exists()
        theorems = count_theorems(pass_file) + count_theorems(correct_file)
        sorry = count_sorry(pass_file) + count_sorry(correct_file)

        # Check multi-block variant
        multi_file = passes_dir / f"{lean_name}Multi.lean"
        multi_correct = passes_dir / f"{lean_name}MultiCorrect.lean"
        if multi_file.exists():
            theorems += count_theorems(multi_file) + count_theorems(multi_correct)
            sorry += count_sorry(multi_file) + count_sorry(multi_correct)

        results[pass_name] = {
            "description": desc,
            "formalized": formalized,
            "correctness_proof": has_proof,
            "theorems": theorems,
            "sorry_count": sorry,
        }
    return results


def scan_all_theorems() -> dict[str, int]:
    """Count theorems across all Lean files."""
    results = {}
    for lean_file in sorted(LEAN_DIR.rglob("*.lean")):
        rel = lean_file.relative_to(LEAN_DIR)
        n = count_theorems(lean_file)
        if n > 0:
            results[str(rel)] = n
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description="Formal methods coverage report")
    parser.add_argument("--json", action="store_true", help="JSON output")
    args = parser.parse_args()

    lean_binops = parse_lean_binops()
    lean_unops = parse_lean_unops()
    _lean_exprs = parse_lean_expr_kinds()

    # Opcode coverage
    binop_covered = lean_binops & REAL_BINOPS
    binop_missing = REAL_BINOPS - lean_binops
    unop_covered = lean_unops & REAL_UNOPS
    unop_missing = REAL_UNOPS - lean_unops

    # Expr kind mapping (Lean uses val/var/bin/un; real uses many more)
    lean_expr_to_real = {
        "val": {"const_int", "const_float", "const_bool", "const_str", "const_none"},
        "var": {"load_var"},
        "bin": {"binop"},
        "un": {"unop"},
    }
    real_covered_by_expr = set()
    for real_set in lean_expr_to_real.values():
        real_covered_by_expr.update(real_set)
    expr_missing = REAL_EXPR_KINDS - real_covered_by_expr

    # Control flow coverage
    lean_cf = {"ret", "jmp", "br"}  # from Terminator
    real_cf_covered = {"return", "if_branch", "else_branch"}  # mapped from Terminator
    cf_missing = REAL_CONTROL_FLOW - real_cf_covered

    # Pass coverage
    pass_results = scan_passes()
    passes_proven = sum(1 for p in pass_results.values() if p["correctness_proof"])
    passes_formalized = sum(1 for p in pass_results.values() if p["formalized"])

    # Theorem counts
    all_theorems = scan_all_theorems()
    total_theorems = sum(all_theorems.values())

    # Total sorry
    total_sorry = 0
    for lean_file in LEAN_DIR.rglob("*.lean"):
        total_sorry += count_sorry(lean_file)

    # Total lean files
    lean_files = list(LEAN_DIR.rglob("*.lean"))

    report = {
        "opcode_coverage": {
            "binop": {
                "modeled": sorted(binop_covered),
                "missing": sorted(binop_missing),
                "coverage_pct": round(
                    len(binop_covered) / max(len(REAL_BINOPS), 1) * 100, 1
                ),
            },
            "unop": {
                "modeled": sorted(unop_covered),
                "missing": sorted(unop_missing),
                "coverage_pct": round(
                    len(unop_covered) / max(len(REAL_UNOPS), 1) * 100, 1
                ),
            },
            "expr_kinds": {
                "modeled": sorted(real_covered_by_expr),
                "missing": sorted(expr_missing),
                "coverage_pct": round(
                    len(real_covered_by_expr) / max(len(REAL_EXPR_KINDS), 1) * 100, 1
                ),
            },
            "control_flow": {
                "modeled_terminators": sorted(lean_cf),
                "real_covered": sorted(real_cf_covered),
                "missing": sorted(cf_missing),
            },
        },
        "pass_coverage": {
            "passes": pass_results,
            "formalized_count": passes_formalized,
            "proven_count": passes_proven,
            "total_count": len(REAL_PASSES),
            "coverage_pct": round(passes_proven / max(len(REAL_PASSES), 1) * 100, 1),
        },
        "proof_metrics": {
            "lean_files": len(lean_files),
            "total_theorems": total_theorems,
            "total_sorry": total_sorry,
            "theorems_per_file": all_theorems,
        },
    }

    if args.json:
        print(json.dumps(report, indent=2))
        return 0

    # Human-readable report
    print("=" * 70)
    print("Molt Formal Methods Coverage Report")
    print("=" * 70)

    print(f"\n  Lean files:     {len(lean_files)}")
    print(f"  Total theorems: {total_theorems}")
    print(f"  Total sorry:    {total_sorry}")

    print("\n--- Opcode Coverage ---")
    bc = report["opcode_coverage"]["binop"]
    print(f"  BinOp:  {len(bc['modeled'])}/{len(REAL_BINOPS)}  ({bc['coverage_pct']}%)")
    if bc["missing"]:
        print(f"    missing: {', '.join(bc['missing'])}")

    uc = report["opcode_coverage"]["unop"]
    print(f"  UnOp:   {len(uc['modeled'])}/{len(REAL_UNOPS)}  ({uc['coverage_pct']}%)")
    if uc["missing"]:
        print(f"    missing: {', '.join(uc['missing'])}")

    ec = report["opcode_coverage"]["expr_kinds"]
    print(
        f"  Expr:   {len(ec['modeled'])}/{len(REAL_EXPR_KINDS)}"
        f"  ({ec['coverage_pct']}%)"
    )

    print("\n--- Pass Coverage ---")
    for name, info in pass_results.items():
        status = (
            "proven"
            if info["correctness_proof"]
            else ("modeled" if info["formalized"] else "---")
        )
        sorry_tag = f" ({info['sorry_count']} sorry)" if info["sorry_count"] else ""
        print(f"  {name:24s} {status:8s}  {info['theorems']:2d} theorems{sorry_tag}")
    pc = report["pass_coverage"]
    print(
        f"\n  Proven: {pc['proven_count']}/{pc['total_count']}  ({pc['coverage_pct']}%)"
    )

    print("\n--- Theorem Distribution ---")
    for fname, count in sorted(all_theorems.items()):
        print(f"  {fname:50s} {count:3d}")

    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
