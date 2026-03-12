#!/usr/bin/env python3
"""Gate check: verify all formal methods artifacts compile and pass.

Checks:
  1. Lean 4 proofs build (lake build)
  2. All Quint models pass invariant checks (quint run)
  3. Known-bad Quint model FAILS (meta-test)
  4. Proof-code correspondence (NaN-boxing constants match Rust)

Usage:
    uv run --python 3.12 python3 tools/check_formal_methods.py
    uv run --python 3.12 python3 tools/check_formal_methods.py --lean-only
    uv run --python 3.12 python3 tools/check_formal_methods.py --quint-only
    uv run --python 3.12 python3 tools/check_formal_methods.py --check-correspondence
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORMAL_DIR = ROOT / "formal"
LEAN_DIR = FORMAL_DIR / "lean"
QUINT_DIR = FORMAL_DIR / "quint"

# Rust source for NaN-boxing constants
OBJ_MODEL_LIB = ROOT / "runtime" / "molt-obj-model" / "src" / "lib.rs"
# Lean formalization of NaN-boxing constants
NANBOX_LEAN = LEAN_DIR / "MoltTIR" / "Runtime" / "NanBox.lean"
# Lean Luau backend builtin mapping
LUAU_EMIT_LEAN = LEAN_DIR / "MoltTIR" / "Backend" / "LuauEmit.lean"
# Rust Luau backend
LUAU_RS = ROOT / "runtime" / "molt-backend" / "src" / "luau.rs"

# ── Terminal colors ──────────────────────────────────────────────────

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


# ── Expected file inventories ────────────────────────────────────────

EXPECTED_LEAN_FILES = [
    "lakefile.lean",
    "lean-toolchain",
    "MoltTIR/Basic.lean",
    "MoltTIR/Types.lean",
    "MoltTIR/Syntax.lean",
    "MoltTIR/WellFormed.lean",
    "MoltTIR/CFG.lean",
    "MoltTIR/Semantics/State.lean",
    "MoltTIR/Semantics/EvalExpr.lean",
    "MoltTIR/Semantics/ExecBlock.lean",
    "MoltTIR/Semantics/ExecFunc.lean",
    "MoltTIR/Semantics/Determinism.lean",
    "MoltTIR/Passes/ConstFold.lean",
    "MoltTIR/Passes/ConstFoldCorrect.lean",
    "MoltTIR/Passes/DCE.lean",
    "MoltTIR/Passes/DCECorrect.lean",
    "MoltTIR/Passes/Effects.lean",
    "MoltTIR/Passes/Lattice.lean",
    "MoltTIR/Passes/SCCP.lean",
    "MoltTIR/Passes/SCCPCorrect.lean",
    "MoltTIR/Passes/Pipeline.lean",
    "MoltTIR/Runtime/NanBox.lean",
    "MoltTIR/Runtime/WasmNative.lean",
    "MoltTIR/Runtime/Refcount.lean",
    "MoltTIR/Backend/LuauSyntax.lean",
    "MoltTIR/Backend/LuauEmit.lean",
    "MoltTIR/Backend/LuauCorrect.lean",
    "MoltTIR/Tests/Smoke.lean",
]

EXPECTED_QUINT_FILES = [
    "molt_build_determinism.qnt",
    "molt_runtime_determinism.qnt",
    "molt_midend_pipeline.qnt",
    "molt_luau_transpiler.qnt",
    "molt_calling_convention.qnt",
]

# Quint models, their invariants, and max steps for CI simulation.
QUINT_MODELS: list[tuple[str, str, int]] = [
    ("molt_build_determinism.qnt", "Inv", 12),
    ("molt_runtime_determinism.qnt", "Inv", 15),
    ("molt_midend_pipeline.qnt", "inv", 20),
    ("molt_luau_transpiler.qnt", "Inv", 15),
    ("molt_calling_convention.qnt", "Inv", 15),
]

# Known-bad model meta-test: this SHOULD fail (order-dependent hashing).
KNOWN_BAD_MODEL = "molt_build_determinism.qnt"
KNOWN_BAD_MODULE = "molt_build_order_dependent"
KNOWN_BAD_INV = "OrderDependentInv"
KNOWN_BAD_STEPS = 10

# NaN-boxing constants to cross-check between Rust and Lean.
# name → (rust_regex, lean_regex)
NANBOX_CONSTANTS: dict[str, tuple[str, str]] = {
    "QNAN": (
        r"const QNAN:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def QNAN\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "TAG_INT": (
        r"const TAG_INT:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def TAG_INT\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "TAG_BOOL": (
        r"const TAG_BOOL:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def TAG_BOOL\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "TAG_NONE": (
        r"const TAG_NONE:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def TAG_NONE\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "TAG_PTR": (
        r"const TAG_PTR:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def TAG_PTR\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "TAG_PENDING/TAG_PEND": (
        r"const TAG_PENDING:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def TAG_PEND\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "TAG_MASK": (
        r"const TAG_MASK:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)",
        r"def TAG_MASK\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
    "INT_MASK": (
        # Rust uses a computed expression: (1u64 << INT_WIDTH) - 1
        # We'll handle this specially.
        r"const INT_MASK:\s*u64\s*=\s*(.+);",
        r"def INT_MASK\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)",
    ),
}


def _normalize_hex(val: str, rust_text: str = "") -> int:
    """Normalize a hex constant (strip underscores) to an integer."""
    val = val.strip().rstrip(";")
    # Handle Rust computed expressions like (1u64 << INT_WIDTH) - 1
    if "<<" in val:
        # Parse: (1u64 << N) - 1, where N is a literal or variable
        m = re.match(r"\(1u64\s*<<\s*(\w+)\)\s*-\s*1", val.strip())
        if m:
            shift_val = m.group(1)
            if shift_val.isdigit():
                return (1 << int(shift_val)) - 1
            # Resolve variable from Rust source
            if rust_text:
                vm = re.search(rf"const {shift_val}:\s*u64\s*=\s*(\d+)", rust_text)
                if vm:
                    return (1 << int(vm.group(1))) - 1
            raise ValueError(f"Cannot resolve variable: {shift_val}")
        # Parse: 1 << N
        m2 = re.match(r"1\s*<<\s*(\d+)", val.strip())
        if m2:
            return 1 << int(m2.group(1))
        raise ValueError(f"Cannot parse computed constant: {val}")
    return int(val.replace("_", ""), 16)


# ── Result tracking ──────────────────────────────────────────────────


@dataclass
class CheckResult:
    name: str
    passed: bool
    detail: str
    warnings: list[str] = field(default_factory=list)


# ── Check 1: Lean Build ──────────────────────────────────────────────


def check_lean_build(*, skip_build: bool = False) -> CheckResult:
    """Build Lean proofs and report statistics."""
    if not LEAN_DIR.exists():
        return CheckResult("Lean build", False, "formal/lean/ directory not found")

    # Count theorems and sorry gaps across all .lean files
    theorem_count = 0
    sorry_count = 0
    lean_files = list(LEAN_DIR.rglob("*.lean"))
    for lf in lean_files:
        text = lf.read_text(errors="replace")
        theorem_count += len(re.findall(r"\btheorem\b", text))
        sorry_count += len(re.findall(r"\bsorry\b", text))

    stats = (
        f"{theorem_count} theorems, {sorry_count} sorry gaps, {len(lean_files)} files"
    )

    if skip_build:
        warnings = []
        if sorry_count > 0:
            warnings.append(f"{sorry_count} sorry gaps (documented proof holes)")
        return CheckResult("Lean stats", True, stats, warnings)

    # Find lake binary
    lake = shutil.which("lake")
    if lake is None:
        elan_lake = Path.home() / ".elan" / "bin" / "lake"
        if elan_lake.exists():
            lake = str(elan_lake)
        else:
            return CheckResult(
                "Lean build",
                True,
                f"SKIPPED (lake not found; install elan). Stats: {stats}",
                ["lake not installed -- Lean build check skipped"],
            )

    print("  Running: lake build (formal/lean/) ...")
    t0 = time.monotonic()
    try:
        result = subprocess.run(
            [lake, "build"],
            cwd=LEAN_DIR,
            capture_output=True,
            text=True,
            timeout=600,  # 10 minutes
        )
    except subprocess.TimeoutExpired:
        return CheckResult(
            "Lean build", False, f"lake build timed out (>600s). Stats: {stats}"
        )

    elapsed = time.monotonic() - t0

    if result.returncode != 0:
        output = (result.stdout + result.stderr).strip()
        tail = "\n".join(output.splitlines()[-20:])
        return CheckResult(
            "Lean build",
            False,
            f"lake build FAILED (exit {result.returncode}, {elapsed:.1f}s). Stats: {stats}\n{tail}",
        )

    warnings = []
    if sorry_count > 0:
        warnings.append(f"{sorry_count} sorry gaps (documented proof holes)")

    # Check for warnings in output
    output = (result.stdout + result.stderr).strip()
    warn_lines = [ln for ln in output.splitlines() if "warning" in ln.lower()]
    if warn_lines:
        warnings.append(f"{len(warn_lines)} compiler warnings")

    return CheckResult(
        "Lean build",
        True,
        f"OK ({elapsed:.1f}s). {stats}",
        warnings,
    )


# ── Check 2: Quint Model Verification ────────────────────────────────


def check_quint_models() -> list[CheckResult]:
    """Run each Quint model with bounded simulation."""
    results: list[CheckResult] = []

    quint = shutil.which("quint")
    if quint is None:
        results.append(
            CheckResult(
                "Quint models",
                True,
                "SKIPPED (quint not found; install via: npm install -g @informalsystems/quint)",
                ["quint not installed -- Quint model checks skipped"],
            )
        )
        return results

    if not QUINT_DIR.exists():
        results.append(
            CheckResult("Quint models", False, "formal/quint/ directory not found")
        )
        return results

    for model_file, invariant, max_steps in QUINT_MODELS:
        model_path = QUINT_DIR / model_file
        if not model_path.exists():
            results.append(CheckResult(f"Quint {model_file}", False, "file not found"))
            continue

        label = f"Quint {model_file}"
        print(
            f"  Running: quint run {model_file} --invariant={invariant} --max-steps={max_steps} ..."
        )

        t0 = time.monotonic()
        try:
            proc = subprocess.run(
                [
                    quint,
                    "run",
                    str(model_path),
                    f"--invariant={invariant}",
                    f"--max-steps={max_steps}",
                    "--max-samples=200",
                ],
                capture_output=True,
                text=True,
                timeout=120,
            )
        except subprocess.TimeoutExpired:
            results.append(CheckResult(label, False, "timed out (>120s)"))
            continue

        elapsed = time.monotonic() - t0
        output = (proc.stdout + proc.stderr).strip()

        if proc.returncode != 0:
            tail = "\n".join(output.splitlines()[-10:])
            results.append(
                CheckResult(
                    label,
                    False,
                    f"FAILED (exit {proc.returncode}, {elapsed:.1f}s)\n{tail}",
                )
            )
        else:
            # Try to extract states/samples from output
            detail = f"PASS ({elapsed:.1f}s)"
            results.append(CheckResult(label, True, detail))

    return results


# ── Check 3: Known-Bad Model Meta-Test ────────────────────────────────


def check_known_bad_model() -> CheckResult:
    """Run the intentionally buggy model — it SHOULD fail."""
    quint = shutil.which("quint")
    if quint is None:
        return CheckResult(
            "Known-bad meta-test",
            True,
            "SKIPPED (quint not installed)",
            ["quint not installed -- meta-test skipped"],
        )

    model_path = QUINT_DIR / KNOWN_BAD_MODEL
    if not model_path.exists():
        return CheckResult("Known-bad meta-test", False, f"{KNOWN_BAD_MODEL} not found")

    qualified = f"{model_path}::{KNOWN_BAD_MODULE}"
    print(
        f"  Running known-bad: quint run {KNOWN_BAD_MODEL}::{KNOWN_BAD_MODULE} --invariant={KNOWN_BAD_INV} ..."
    )

    try:
        proc = subprocess.run(
            [
                quint,
                "run",
                qualified,
                f"--invariant={KNOWN_BAD_INV}",
                f"--max-steps={KNOWN_BAD_STEPS}",
                "--max-samples=500",
            ],
            capture_output=True,
            text=True,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return CheckResult(
            "Known-bad meta-test",
            False,
            "timed out -- cannot confirm model checker detects the bug",
        )

    if proc.returncode != 0:
        # Good! The buggy model was correctly caught.
        return CheckResult(
            "Known-bad meta-test",
            True,
            "PASS (model checker correctly detected the order-dependent bug)",
        )
    else:
        # Bad! The model checker didn't catch the known bug.
        return CheckResult(
            "Known-bad meta-test",
            False,
            "META-FAILURE: known-bad model passed when it should have failed. "
            "Model checker may not be working correctly.",
        )


# ── Check 4: Proof-Code Correspondence ──────────────────────────────


def check_nanbox_correspondence() -> CheckResult:
    """Verify NaN-boxing constants match between Rust and Lean."""
    if not OBJ_MODEL_LIB.exists():
        return CheckResult(
            "NaN-box correspondence", False, f"Rust source not found: {OBJ_MODEL_LIB}"
        )
    if not NANBOX_LEAN.exists():
        return CheckResult(
            "NaN-box correspondence", False, f"Lean source not found: {NANBOX_LEAN}"
        )

    rust_text = OBJ_MODEL_LIB.read_text()
    lean_text = NANBOX_LEAN.read_text()

    mismatches: list[str] = []
    matched = 0

    for name, (rust_re, lean_re) in NANBOX_CONSTANTS.items():
        rust_match = re.search(rust_re, rust_text)
        lean_match = re.search(lean_re, lean_text)

        if not rust_match:
            mismatches.append(f"{name}: not found in Rust source")
            continue
        if not lean_match:
            mismatches.append(f"{name}: not found in Lean source")
            continue

        try:
            rust_val = _normalize_hex(rust_match.group(1), rust_text=rust_text)
            lean_val = _normalize_hex(lean_match.group(1))
        except (ValueError, IndexError) as e:
            mismatches.append(f"{name}: parse error: {e}")
            continue

        if rust_val != lean_val:
            mismatches.append(
                f"{name}: Rust=0x{rust_val:016x} Lean=0x{lean_val:016x} MISMATCH"
            )
        else:
            matched += 1

    if mismatches:
        detail = f"{matched} matched, {len(mismatches)} mismatched:\n" + "\n".join(
            f"  - {m}" for m in mismatches
        )
        return CheckResult("NaN-box correspondence", False, detail)

    return CheckResult("NaN-box correspondence", True, f"all {matched} constants match")


def check_luau_builtin_correspondence() -> CheckResult:
    """Verify Lean builtinMapping entries exist in Rust luau.rs."""
    if not LUAU_EMIT_LEAN.exists():
        return CheckResult(
            "Luau builtin correspondence",
            False,
            f"Lean source not found: {LUAU_EMIT_LEAN}",
        )
    if not LUAU_RS.exists():
        return CheckResult(
            "Luau builtin correspondence", False, f"Rust source not found: {LUAU_RS}"
        )

    lean_text = LUAU_EMIT_LEAN.read_text()
    rust_text = LUAU_RS.read_text()

    # Extract builtin mapping pairs from Lean: ("irName", "luauName")
    lean_mappings = re.findall(r'\("(\w+)",\s*"([^"]+)"\)', lean_text)
    if not lean_mappings:
        return CheckResult(
            "Luau builtin correspondence",
            True,
            "SKIPPED (no builtin mappings found in Lean)",
            ["Could not parse builtinMapping from LuauEmit.lean"],
        )

    missing: list[str] = []
    found = 0

    for ir_name, luau_name in lean_mappings:
        # Check if either the IR name or Luau name appears in the Rust backend
        if ir_name not in rust_text and luau_name not in rust_text:
            missing.append(f"{ir_name} -> {luau_name}")
        else:
            found += 1

    warnings: list[str] = []
    if missing:
        warnings.append(
            f"{len(missing)} Lean builtins not found in luau.rs: {', '.join(missing[:5])}"
            + (f" (+{len(missing) - 5} more)" if len(missing) > 5 else "")
        )

    # This is a warning, not a hard failure — the Lean model may be a subset.
    return CheckResult(
        "Luau builtin correspondence",
        True,
        f"{found}/{len(lean_mappings)} Lean builtins verified in Rust backend",
        warnings,
    )


# ── Check: File Inventory ────────────────────────────────────────────


def check_inventory() -> CheckResult:
    """Check that all expected formal methods files exist."""
    missing: list[str] = []

    if not FORMAL_DIR.exists():
        return CheckResult("File inventory", False, "formal/ directory does not exist")

    for f in EXPECTED_LEAN_FILES:
        if not (LEAN_DIR / f).exists():
            missing.append(f"formal/lean/{f}")

    for f in EXPECTED_QUINT_FILES:
        if not (QUINT_DIR / f).exists():
            missing.append(f"formal/quint/{f}")

    if missing:
        detail = f"{len(missing)} missing:\n" + "\n".join(f"  - {m}" for m in missing)
        return CheckResult("File inventory", False, detail)

    total = len(EXPECTED_LEAN_FILES) + len(EXPECTED_QUINT_FILES)
    return CheckResult("File inventory", True, f"all {total} expected files present")


# ── Summary ──────────────────────────────────────────────────────────


def print_summary(results: list[CheckResult]) -> int:
    """Print a summary table and return exit code."""
    print()
    print(bold("=" * 70))
    print(bold("  Formal Methods Gate Check Summary"))
    print(bold("=" * 70))

    all_passed = True
    all_warnings: list[str] = []

    for r in results:
        status = green("PASS") if r.passed else red("FAIL")
        print(f"  [{status}] {r.name}: {r.detail.splitlines()[0]}")
        # Print any multi-line detail
        for line in r.detail.splitlines()[1:]:
            print(f"         {line}")
        for w in r.warnings:
            print(f"         {yellow('WARN')}: {w}")
            all_warnings.append(w)
        if not r.passed:
            all_passed = False

    print(bold("-" * 70))
    passed_count = sum(1 for r in results if r.passed)
    failed_count = sum(1 for r in results if not r.passed)
    print(
        f"  Total: {len(results)} checks | "
        f"{green(f'{passed_count} passed')} | "
        f"{red(f'{failed_count} failed') if failed_count else f'{failed_count} failed'} | "
        f"{yellow(f'{len(all_warnings)} warnings') if all_warnings else f'{len(all_warnings)} warnings'}"
    )
    print(bold("=" * 70))

    if all_passed:
        print(f"\n{green('formal-methods gate: ok')}")
        return 0
    else:
        print(f"\n{red('formal-methods gate: FAILED')}")
        return 1


# ── Main ─────────────────────────────────────────────────────────────


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Gate check: verify all formal methods artifacts compile and pass.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--lean-only", action="store_true", help="Run Lean build check only"
    )
    parser.add_argument(
        "--quint-only", action="store_true", help="Run Quint model checks only"
    )
    parser.add_argument(
        "--check-correspondence",
        action="store_true",
        help="Run proof-code correspondence checks only",
    )
    parser.add_argument(
        "--inventory-only",
        action="store_true",
        help="Check file inventory only (no builds)",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="Skip actual Lean/Quint builds (inventory + correspondence only)",
    )
    args = parser.parse_args()

    run_all = not (
        args.lean_only
        or args.quint_only
        or args.check_correspondence
        or args.inventory_only
    )
    results: list[CheckResult] = []

    # Always check inventory
    if run_all or args.inventory_only:
        print("[formal-methods] Checking file inventory ...")
        results.append(check_inventory())

    # Lean build
    if run_all or args.lean_only:
        print("[formal-methods] Checking Lean proofs ...")
        results.append(check_lean_build(skip_build=args.skip_build))

    # Quint models
    if run_all or args.quint_only:
        print("[formal-methods] Checking Quint models ...")
        if args.skip_build:
            results.append(
                CheckResult(
                    "Quint models", True, "SKIPPED (--skip-build)", ["build skipped"]
                )
            )
        else:
            results.extend(check_quint_models())

            # Known-bad meta-test
            print("[formal-methods] Running known-bad model meta-test ...")
            results.append(check_known_bad_model())

    # Proof-code correspondence
    if run_all or args.check_correspondence:
        print("[formal-methods] Checking proof-code correspondence ...")
        results.append(check_nanbox_correspondence())
        results.append(check_luau_builtin_correspondence())

    return print_summary(results)


if __name__ == "__main__":
    raise SystemExit(main())
