#!/usr/bin/env python3
"""Gate check: formal methods proofs and model verification.

Validates:
  - Lean 4 proofs compile (formal/lean/)
  - Quint models pass invariant checks (formal/quint/)
  - Formal methods file inventory matches expectations

Usage:
  python3 tools/check_formal_methods.py           # run all checks
  python3 tools/check_formal_methods.py --lean     # Lean proofs only
  python3 tools/check_formal_methods.py --quint    # Quint models only
  python3 tools/check_formal_methods.py --inventory  # file inventory only
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORMAL_DIR = ROOT / "formal"
LEAN_DIR = FORMAL_DIR / "lean"
QUINT_DIR = FORMAL_DIR / "quint"

# Expected Lean source files (relative to formal/lean/)
EXPECTED_LEAN_FILES = [
    "lakefile.lean",
    "lean-toolchain",
    "MoltTIR/Basic.lean",
    "MoltTIR/Types.lean",
    "MoltTIR/Syntax.lean",
    "MoltTIR/WellFormed.lean",
    "MoltTIR/Semantics/State.lean",
    "MoltTIR/Semantics/EvalExpr.lean",
    "MoltTIR/Semantics/ExecBlock.lean",
    "MoltTIR/Semantics/ExecFunc.lean",
    "MoltTIR/Semantics/Determinism.lean",
    "MoltTIR/Passes/ConstFold.lean",
    "MoltTIR/Passes/ConstFoldCorrect.lean",
    "MoltTIR/Tests/Smoke.lean",
]

# Expected Quint model files (relative to formal/quint/)
EXPECTED_QUINT_FILES = [
    "molt_build_determinism.qnt",
    "molt_runtime_determinism.qnt",
]

# Quint models and their invariants to verify
QUINT_MODELS = [
    ("molt_build_determinism.qnt", "Inv"),
    ("molt_runtime_determinism.qnt", "Inv"),
]


def check_inventory() -> list[str]:
    """Check that all expected formal methods files exist."""
    errors: list[str] = []

    if not FORMAL_DIR.exists():
        errors.append("formal/ directory does not exist")
        return errors

    for f in EXPECTED_LEAN_FILES:
        path = LEAN_DIR / f
        if not path.exists():
            errors.append(f"missing Lean file: formal/lean/{f}")

    for f in EXPECTED_QUINT_FILES:
        path = QUINT_DIR / f
        if not path.exists():
            errors.append(f"missing Quint file: formal/quint/{f}")

    return errors


def check_lean() -> list[str]:
    """Build Lean proofs via `lake build`."""
    errors: list[str] = []

    lake = shutil.which("lake")
    if lake is None:
        # Check elan-managed path
        elan_lake = Path.home() / ".elan" / "bin" / "lake"
        if elan_lake.exists():
            lake = str(elan_lake)
        else:
            errors.append(
                "lake not found; install elan (https://github.com/leanprover/elan)"
            )
            return errors

    if not LEAN_DIR.exists():
        errors.append("formal/lean/ directory does not exist")
        return errors

    print("  Running: lake build (formal/lean/) ...")
    result = subprocess.run(
        [lake, "build"],
        cwd=LEAN_DIR,
        capture_output=True,
        text=True,
        timeout=600,
    )

    if result.returncode != 0:
        errors.append("Lean proofs failed to build")
        # Show last 20 lines of output for diagnostics
        output = (result.stdout + result.stderr).strip()
        for line in output.splitlines()[-20:]:
            errors.append(f"  {line}")

    return errors


def check_quint() -> list[str]:
    """Run Quint model verification."""
    errors: list[str] = []

    quint = shutil.which("quint")
    if quint is None:
        errors.append(
            "quint not found; install via: npm install -g @informalsystems/quint"
        )
        return errors

    if not QUINT_DIR.exists():
        errors.append("formal/quint/ directory does not exist")
        return errors

    for model_file, invariant in QUINT_MODELS:
        model_path = QUINT_DIR / model_file
        if not model_path.exists():
            errors.append(f"missing Quint model: {model_file}")
            continue

        # Use simulation (run) with bounded steps for CI speed.
        # Full `quint verify` can be run separately for exhaustive checking.
        print(f"  Running: quint run {model_file} --invariant={invariant} ...")
        result = subprocess.run(
            [
                quint,
                "run",
                str(model_path),
                f"--invariant={invariant}",
                "--max-steps=12",
                "--max-samples=200",
            ],
            capture_output=True,
            text=True,
            timeout=120,
        )

        if result.returncode != 0:
            errors.append(f"Quint model {model_file} invariant {invariant} violated")
            output = (result.stdout + result.stderr).strip()
            for line in output.splitlines()[-10:]:
                errors.append(f"  {line}")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description="Formal methods gate check")
    parser.add_argument("--lean", action="store_true", help="Check Lean proofs only")
    parser.add_argument("--quint", action="store_true", help="Check Quint models only")
    parser.add_argument(
        "--inventory", action="store_true", help="Check file inventory only"
    )
    args = parser.parse_args()

    run_all = not (args.lean or args.quint or args.inventory)
    errors: list[str] = []

    if run_all or args.inventory:
        print("[formal-methods] Checking file inventory ...")
        errors.extend(check_inventory())

    if run_all or args.lean:
        print("[formal-methods] Checking Lean proofs ...")
        errors.extend(check_lean())

    if run_all or args.quint:
        print("[formal-methods] Checking Quint models ...")
        errors.extend(check_quint())

    if errors:
        print("\nformal-methods gate FAILED:")
        for err in errors:
            print(f"  - {err}")
        return 1

    print("\nformal-methods gate: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
