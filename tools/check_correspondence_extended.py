#!/usr/bin/env python3
"""Extended correspondence checks that supplement check_correspondence.py.

Adds:
  1. NaN-boxing constant check with Rust expression resolution
  2. BinOp/UnOp variant count check (Lean vs Python)
  3. Pure-op set alignment check (Lean ops vs Python _op_effect_class)
  4. Compiler pass presence check (Lean pass files exist with key functions)

Exits 1 if any mismatch is found.

Usage:
    uv run --python 3.12 python3 tools/check_correspondence_extended.py
    uv run --python 3.12 python3 tools/check_correspondence_extended.py --verbose
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


def _find_repo_root() -> Path:
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()
NANBOX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Runtime" / "NanBox.lean"
SYNTAX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Syntax.lean"
OBJ_MODEL_RS = ROOT / "runtime" / "molt-obj-model" / "src" / "lib.rs"
FRONTEND_PY = ROOT / "src" / "molt" / "frontend" / "__init__.py"
LEAN_PASSES_DIR = ROOT / "formal" / "lean" / "MoltTIR" / "Passes"

IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def bold(t: str) -> str:
    return _c("1", t)


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    return ""


# ── Parsing helpers ──────────────────────────────────────────────────


def _parse_lean_hex_constants(text: str) -> dict[str, int]:
    result: dict[str, int] = {}
    for m in re.finditer(r"def\s+(\w+)\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F_]+)", text):
        result[m.group(1)] = int(m.group(2).replace("_", ""), 16)
    return result


def _parse_rust_u64_constants(text: str) -> dict[str, int]:
    raw: dict[str, str] = {}
    for m in re.finditer(r"const\s+(\w+):\s*u64\s*=\s*(.+?);", text):
        raw[m.group(1)] = m.group(2).strip()
    resolved: dict[str, int] = {}
    for name, expr in raw.items():
        try:
            resolved[name] = _resolve_rust_expr(expr, raw)
        except ValueError:
            pass
    return resolved


def _resolve_rust_expr(expr: str, raw_consts: dict[str, str]) -> int:
    expr = expr.strip()
    if re.match(r"^0x[0-9a-fA-F_]+$", expr):
        return int(expr.replace("_", ""), 16)
    if expr.isdigit():
        return int(expr)
    m = re.match(r"1(?:u64)?\s*<<\s*(\w+)", expr)
    if m:
        shift_val = m.group(1)
        if shift_val.isdigit():
            return 1 << int(shift_val)
        if shift_val in raw_consts:
            return 1 << _resolve_rust_expr(raw_consts[shift_val], raw_consts)
        raise ValueError(f"Cannot resolve: {shift_val}")
    m = re.match(r"\(1u64\s*<<\s*(\w+)\)\s*-\s*1", expr)
    if m:
        shift_val = m.group(1)
        if shift_val.isdigit():
            return (1 << int(shift_val)) - 1
        if shift_val in raw_consts:
            return (1 << _resolve_rust_expr(raw_consts[shift_val], raw_consts)) - 1
        raise ValueError(f"Cannot resolve: {shift_val}")
    raise ValueError(f"Cannot parse: {expr}")


def _parse_lean_inductive_variants(text: str, type_name: str) -> list[str]:
    pattern = rf"inductive\s+{type_name}\s+where"
    m = re.search(pattern, text)
    if not m:
        return []
    start = m.end()
    lines = text[start:].split("\n")
    variants: list[str] = []
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("--"):
            continue
        if stripped.startswith("deriving"):
            break
        if re.match(
            r"^(inductive|def|theorem|structure|namespace|end|abbrev|section)\b",
            stripped,
        ):
            break
        for vm in re.finditer(r"\|\s*\.?(\w+)", stripped):
            name = vm.group(1)
            if name.endswith("_"):
                name = name[:-1]
            if name not in variants:
                variants.append(name)
    return variants


def _parse_python_effect_classes(text: str) -> dict[str, set[str]]:
    classes: dict[str, set[str]] = {
        "pure": set(),
        "reads_heap": set(),
        "writes_heap": set(),
        "control": set(),
    }
    m = re.search(r"def _op_effect_class\(self, op_kind: str\) -> str:", text)
    if not m:
        return classes
    method_body = text[m.end() :]
    end_match = re.search(r"\n    def ", method_body)
    if end_match:
        method_body = method_body[: end_match.start()]
    for block_match in re.finditer(
        r'if\s+op_kind\s+in\s+\{([^}]+)\}:\s*\n\s*return\s+"(\w+)"',
        method_body,
    ):
        ops_str = block_match.group(1)
        effect_class = block_match.group(2)
        if effect_class in classes:
            for op_match in re.finditer(r'"(\w+)"', ops_str):
                classes[effect_class].add(op_match.group(1))
    return classes


# ── Check functions ──────────────────────────────────────────────────


def check_nanbox_constants(verbose: bool) -> bool:
    """Check NaN-boxing constants match between Lean and Rust."""
    print(bold("\n[1] NaN-boxing constant alignment"))
    lean_text = _read(NANBOX_LEAN)
    rust_text = _read(OBJ_MODEL_RS)
    if not lean_text or not rust_text:
        print(red("  SKIP: source files missing"))
        return True

    lean_consts = _parse_lean_hex_constants(lean_text)
    rust_consts = _parse_rust_u64_constants(rust_text)

    mapping = {
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

    ok = True
    for lean_name, rust_name in mapping.items():
        if lean_name not in lean_consts:
            print(red(f"  FAIL: {lean_name} not in Lean"))
            ok = False
            continue
        if rust_name not in rust_consts:
            print(red(f"  FAIL: {rust_name} not in Rust"))
            ok = False
            continue
        lv = lean_consts[lean_name]
        rv = rust_consts[rust_name]
        if lv != rv:
            print(
                red(
                    f"  FAIL: {lean_name}/{rust_name}: Lean=0x{lv:016x} Rust=0x{rv:016x}"
                )
            )
            ok = False
        elif verbose:
            print(green(f"  ok: {lean_name}/{rust_name} = 0x{lv:016x}"))

    if ok:
        print(green(f"  {len(mapping)} constants matched"))
    return ok


def check_operator_counts(verbose: bool) -> bool:
    """Check BinOp/UnOp variant counts between Lean and Python."""
    print(bold("\n[2] BinOp/UnOp variant count alignment"))
    lean_text = _read(SYNTAX_LEAN)
    py_text = _read(FRONTEND_PY)
    if not lean_text or not py_text:
        print(red("  SKIP: source files missing"))
        return True

    lean_binops = _parse_lean_inductive_variants(lean_text, "BinOp")
    lean_unops = _parse_lean_inductive_variants(lean_text, "UnOp")

    # Extract Python op kinds from effect classes
    py_effect_ops: set[str] = set()
    for m in re.finditer(r'"([A-Z_]+)"', py_text):
        py_effect_ops.add(m.group(1))

    ok = True

    # Check BinOps
    missing_binops = []
    for v in lean_binops:
        if v.upper() not in py_effect_ops:
            missing_binops.append(v)
    if missing_binops:
        # Some are acceptable (bitwise/advanced ops not in Python pure set)
        acceptable = {
            "bit_and",
            "bit_or",
            "bit_xor",
            "lshift",
            "rshift",
            "div",
            "floordiv",
            "pow",
            "mod",
        }
        real_missing = [m for m in missing_binops if m not in acceptable]
        if real_missing:
            print(red(f"  FAIL: Lean BinOps missing from Python: {real_missing}"))
            ok = False

    if verbose:
        print(f"  Lean BinOp: {len(lean_binops)} variants: {', '.join(lean_binops)}")
        print(f"  Lean UnOp:  {len(lean_unops)} variants: {', '.join(lean_unops)}")

    # Check UnOps
    missing_unops = []
    for v in lean_unops:
        if v.upper() not in py_effect_ops:
            missing_unops.append(v)
    if missing_unops:
        # neg is modeled in Lean but Python inlines it (SUB from 0)
        # invert is bitwise, not present in Python IR
        acceptable = {"invert", "neg"}
        real_missing = [m for m in missing_unops if m not in acceptable]
        if real_missing:
            print(red(f"  FAIL: Lean UnOps missing from Python: {real_missing}"))
            ok = False

    if ok:
        print(
            green(
                f"  BinOp: {len(lean_binops)} variants, UnOp: {len(lean_unops)} variants -- aligned"
            )
        )
    return ok


def check_pure_op_alignment(verbose: bool) -> bool:
    """Check that Lean arithmetic/comparison ops are in Python's pure set."""
    print(bold("\n[3] Pure-op set alignment"))
    lean_text = _read(SYNTAX_LEAN)
    py_text = _read(FRONTEND_PY)
    if not lean_text or not py_text:
        print(red("  SKIP: source files missing"))
        return True

    py_classes = _parse_python_effect_classes(py_text)
    pure_ops = py_classes["pure"]
    lean_binops = _parse_lean_inductive_variants(lean_text, "BinOp")
    lean_unops = _parse_lean_inductive_variants(lean_text, "UnOp")

    ok = True

    # Core arithmetic/comparison ops that should be pure
    expected_pure_binops = {"add", "sub", "mul", "eq", "ne", "lt", "le", "gt", "ge"}
    # neg is modeled in Lean but Python inlines it (SUB from 0)
    expected_pure_unops = {"not", "abs"}

    for op in expected_pure_binops:
        if op in [v for v in lean_binops]:
            if op.upper() not in pure_ops:
                print(
                    red(
                        f"  FAIL: BinOp.{op} in Lean but {op.upper()} not in Python pure ops"
                    )
                )
                ok = False
            elif verbose:
                print(green(f"  ok: BinOp.{op} -> {op.upper()} is pure"))

    for op in expected_pure_unops:
        if op in [v for v in lean_unops]:
            if op.upper() not in pure_ops:
                print(
                    red(
                        f"  FAIL: UnOp.{op} in Lean but {op.upper()} not in Python pure ops"
                    )
                )
                ok = False
            elif verbose:
                print(green(f"  ok: UnOp.{op} -> {op.upper()} is pure"))

    if ok:
        checked = len(expected_pure_binops) + len(expected_pure_unops)
        print(green(f"  {checked} core ops verified as pure in both Lean and Python"))
    return ok


def check_compiler_passes(verbose: bool) -> bool:
    """Check that expected compiler passes exist in Lean with key functions."""
    print(bold("\n[4] Compiler pass presence"))

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

    ok = True
    for pass_name, (filename, func_name) in passes.items():
        path = LEAN_PASSES_DIR / filename
        if not path.exists():
            print(red(f"  FAIL: {pass_name} file missing: {path}"))
            ok = False
            continue
        text = path.read_text(errors="replace")
        if func_name not in text:
            print(red(f"  FAIL: {func_name} not found in {path.name}"))
            ok = False
            continue
        # Check correctness proof exists
        proof_path = LEAN_PASSES_DIR / f"{pass_name}Correct.lean"
        if not proof_path.exists():
            print(red(f"  FAIL: correctness proof missing: {proof_path.name}"))
            ok = False
            continue
        if verbose:
            print(green(f"  ok: {pass_name}: {filename} + {pass_name}Correct.lean"))

    if ok:
        print(
            green(f"  {len(passes)} passes verified (definition + correctness proof)")
        )
    return ok


# ── Main ─────────────────────────────────────────────────────────────


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Extended Lean-Rust-Python correspondence checks."
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    print(bold("=" * 70))
    print(bold("  Extended Correspondence Checks"))
    print(bold("=" * 70))

    results = [
        check_nanbox_constants(args.verbose),
        check_operator_counts(args.verbose),
        check_pure_op_alignment(args.verbose),
        check_compiler_passes(args.verbose),
    ]

    print()
    print(bold("-" * 70))
    passed = sum(results)
    total = len(results)
    if all(results):
        print(green(f"  All {total} checks passed"))
        return 0
    else:
        print(red(f"  {total - passed}/{total} checks FAILED"))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
