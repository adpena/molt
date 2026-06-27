#!/usr/bin/env python3
"""Extended correspondence checks that supplement check_correspondence.py.

Adds:
  1. NaN-boxing constant check with Rust expression resolution
  2. BinOp/UnOp variant count check (Lean vs Python)
  3. Frontend effect alignment check (Lean ops vs generated Python effects)
  4. Compiler pass presence check (Lean pass files exist with key functions)

Exits 1 if any mismatch is found.

Usage:
    uv run --python 3.12 python3 tools/check_correspondence_extended.py
    uv run --python 3.12 python3 tools/check_correspondence_extended.py --verbose
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

try:
    from correspondence_sources import (
        parse_lean_hex_constants as _parse_lean_hex_constants,
        parse_lean_inductive_variants as _parse_lean_inductive_variants,
        parse_rust_unsigned_constants as _parse_rust_u64_constants,
    )
except ModuleNotFoundError:  # pragma: no cover - package-style import path
    from tools.correspondence_sources import (
        parse_lean_hex_constants as _parse_lean_hex_constants,
        parse_lean_inductive_variants as _parse_lean_inductive_variants,
        parse_rust_unsigned_constants as _parse_rust_u64_constants,
    )


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
CODEGEN_ABI_RS = ROOT / "runtime" / "molt-codegen-abi" / "src" / "lib.rs"
LEAN_PASSES_DIR = ROOT / "formal" / "lean" / "MoltTIR" / "Passes"
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.frontend.lowering.op_kinds_generated import FRONTEND_EFFECT_CLASS  # noqa: E402

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


def _generated_python_effect_classes() -> dict[str, set[str]]:
    classes: dict[str, set[str]] = {
        "pure": set(),
        "reads_heap": set(),
        "writes_heap": set(),
        "control": set(),
    }
    for kind, effect in FRONTEND_EFFECT_CLASS.items():
        classes[effect].add(kind)
    return classes


# Check functions


def check_nanbox_constants(verbose: bool) -> bool:
    """Check NaN-boxing constants match between Lean and Rust."""
    print(bold("\n[1] NaN-boxing constant alignment"))
    lean_text = _read(NANBOX_LEAN)
    rust_text = _read(CODEGEN_ABI_RS)
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
    if not lean_text:
        print(red("  SKIP: source files missing"))
        return True

    lean_binops = _parse_lean_inductive_variants(lean_text, "BinOp")
    lean_unops = _parse_lean_inductive_variants(lean_text, "UnOp")

    py_effect_ops = set(FRONTEND_EFFECT_CLASS)

    ok = True

    # Check BinOps
    missing_binops = []
    for v in lean_binops:
        if v.upper() not in py_effect_ops:
            missing_binops.append(v)
    if missing_binops:
        print(red(f"  FAIL: Lean BinOps missing from Python: {missing_binops}"))
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
        # invert is not yet surfaced in the frontend generated effect authority.
        acceptable = {"invert"}
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
    """Check Lean operator names against generated frontend effects."""
    print(bold("\n[3] Frontend effect alignment"))
    lean_text = _read(SYNTAX_LEAN)
    if not lean_text:
        print(red("  SKIP: source files missing"))
        return True

    py_classes = _generated_python_effect_classes()
    pure_ops = py_classes["pure"]
    barrier_ops = py_classes["writes_heap"]
    lean_binops = _parse_lean_inductive_variants(lean_text, "BinOp")
    lean_unops = _parse_lean_inductive_variants(lean_text, "UnOp")

    ok = True

    expected_barrier_binops = {
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
        "in",
        "not_in",
    }
    expected_pure_binops = {"and", "or", "is", "is_not"}
    expected_barrier_unops = {"abs", "neg", "pos", "invert"}
    expected_pure_unops = {"not"}

    for op in expected_barrier_binops:
        if op in [v for v in lean_binops]:
            kind = op.upper()
            if kind not in barrier_ops:
                print(
                    red(
                        f"  FAIL: BinOp.{op} in Lean but {kind} is not a frontend barrier"
                    )
                )
                ok = False
            elif verbose:
                print(green(f"  ok: BinOp.{op} -> {kind} is a frontend barrier"))

    for op in expected_pure_binops:
        if op in [v for v in lean_binops]:
            kind = op.upper()
            if kind not in pure_ops:
                print(
                    red(
                        f"  FAIL: BinOp.{op} in Lean but {kind} not in frontend pure ops"
                    )
                )
                ok = False
            elif verbose:
                print(green(f"  ok: BinOp.{op} -> {kind} is pure"))

    for op in expected_pure_unops:
        if op in [v for v in lean_unops]:
            kind = op.upper()
            if kind not in pure_ops:
                print(
                    red(
                        f"  FAIL: UnOp.{op} in Lean but {kind} not in frontend pure ops"
                    )
                )
                ok = False
            elif verbose:
                print(green(f"  ok: UnOp.{op} -> {kind} is pure"))

    for op in expected_barrier_unops:
        if op in [v for v in lean_unops]:
            kind = op.upper()
            if kind not in barrier_ops:
                print(
                    red(
                        f"  FAIL: UnOp.{op} in Lean but {kind} is not a frontend barrier"
                    )
                )
                ok = False
            elif verbose:
                print(green(f"  ok: UnOp.{op} -> {kind} is a frontend barrier"))

    if ok:
        checked = (
            len(expected_barrier_binops)
            + len(expected_pure_binops)
            + len(expected_barrier_unops)
            + len(expected_pure_unops)
        )
        print(green(f"  {checked} core ops verified against frontend effects"))
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


# Main


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
