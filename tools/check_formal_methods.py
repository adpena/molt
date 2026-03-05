#!/usr/bin/env python3
"""Gate check: formal methods proofs and model verification.

Validates:
  - Lean 4 proofs compile (formal/lean/)
  - Quint models pass invariant checks (formal/quint/)
  - Formal methods file inventory matches expectations

Usage:
  python3 tools/check_formal_methods.py             # run all checks
  python3 tools/check_formal_methods.py --lean      # Lean proofs only
  python3 tools/check_formal_methods.py --quint     # Quint models only
  python3 tools/check_formal_methods.py --inventory # file inventory only
  python3 tools/check_formal_methods.py --json      # machine-readable output
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
FORMAL_DIR = ROOT / "formal"
LEAN_DIR = FORMAL_DIR / "lean"
QUINT_DIR = FORMAL_DIR / "quint"

# Expected Lean source files (relative to formal/lean/)
EXPECTED_LEAN_FILES = [
    "lakefile.lean",
    "lean-toolchain",
    # Core
    "MoltTIR/Basic.lean",
    "MoltTIR/Types.lean",
    "MoltTIR/Syntax.lean",
    "MoltTIR/WellFormed.lean",
    # Semantics
    "MoltTIR/Semantics/State.lean",
    "MoltTIR/Semantics/EvalExpr.lean",
    "MoltTIR/Semantics/ExecBlock.lean",
    "MoltTIR/Semantics/ExecFunc.lean",
    "MoltTIR/Semantics/Determinism.lean",
    # CFG
    "MoltTIR/CFG.lean",
    "MoltTIR/CFG/Loops.lean",
    # Passes
    "MoltTIR/Passes/Effects.lean",
    "MoltTIR/Passes/ConstFold.lean",
    "MoltTIR/Passes/ConstFoldCorrect.lean",
    "MoltTIR/Passes/DCE.lean",
    "MoltTIR/Passes/DCECorrect.lean",
    "MoltTIR/Passes/Lattice.lean",
    "MoltTIR/Passes/SCCP.lean",
    "MoltTIR/Passes/SCCPCorrect.lean",
    "MoltTIR/Passes/SCCPMulti.lean",
    "MoltTIR/Passes/SCCPMultiCorrect.lean",
    "MoltTIR/Passes/CSE.lean",
    "MoltTIR/Passes/CSECorrect.lean",
    "MoltTIR/Passes/LICM.lean",
    "MoltTIR/Passes/LICMCorrect.lean",
    "MoltTIR/Passes/Pipeline.lean",
    # Correctness proofs
    "MoltTIR/Semantics/BlockCorrect.lean",
    "MoltTIR/Semantics/FuncCorrect.lean",
    # Runtime verification
    "MoltTIR/Runtime/NanBox.lean",
    "MoltTIR/Runtime/Refcount.lean",
    "MoltTIR/Runtime/WasmNative.lean",
    # Tests
    "MoltTIR/Tests/Smoke.lean",
]

# Expected Quint model files (relative to formal/quint/)
EXPECTED_QUINT_FILES = [
    "molt_build_determinism.qnt",
    "molt_runtime_determinism.qnt",
    "molt_midend_pipeline.qnt",
]

# Quint models and their invariants to verify
QUINT_MODELS = [
    ("molt_build_determinism.qnt", "Inv"),
    ("molt_runtime_determinism.qnt", "Inv"),
    ("molt_midend_pipeline.qnt", "inv"),
]

JAVA_HOME_CANDIDATES = (
    Path("/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"),
    Path("/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home"),
    Path("/usr/local/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"),
    Path("/usr/local/opt/openjdk/libexec/openjdk.jdk/Contents/Home"),
    Path("/Library/Java/JavaVirtualMachines/openjdk-21.jdk/Contents/Home"),
    Path("/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"),
)


def _safe_run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    timeout: int,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
        check=False,
    )


def _detect_runtime_mismatch(output: str) -> bool:
    has_node = "Node.js v" in output
    has_esm_mismatch = (
        "require is not defined in ES module scope" in output
        or "ERR_REQUIRE_ESM" in output
    )
    return has_node and has_esm_mismatch


def _parse_node_major(version_text: str) -> int | None:
    match = re.search(r"v(\d+)\.", version_text)
    if not match:
        return None
    try:
        return int(match.group(1))
    except ValueError:
        return None


def _node_info() -> dict[str, Any]:
    node_path = shutil.which("node")
    if not node_path:
        return {"path": None, "version": None, "major": None}
    try:
        proc = _safe_run([node_path, "--version"], timeout=10)
    except Exception:
        return {"path": node_path, "version": None, "major": None}
    version = (proc.stdout or proc.stderr or "").strip()
    return {
        "path": node_path,
        "version": version or None,
        "major": _parse_node_major(version),
    }


def _resolve_quint_fallback_prefix() -> list[str] | None:
    raw = os.environ.get("MOLT_QUINT_NODE_FALLBACK", "").strip()
    if raw:
        parts = [part for part in shlex.split(raw) if part]
        return parts if parts else None
    for candidate in (
        Path("/opt/homebrew/opt/node@22/bin/node"),
        Path("/usr/local/opt/node@22/bin/node"),
    ):
        if candidate.exists():
            return [str(candidate)]
    if not shutil.which("npx"):
        return None
    return ["npx", "-y", "node@22"]


def _resolve_java_home() -> str | None:
    for env_key in ("JAVA_HOME", "MOLT_JAVA_HOME"):
        raw = os.environ.get(env_key, "").strip()
        if raw and (Path(raw) / "bin" / "java").exists():
            return raw
    for candidate in JAVA_HOME_CANDIDATES:
        if (candidate / "bin" / "java").exists():
            return str(candidate)
    return None


def _resolve_java_bin(java_home: str | None) -> str | None:
    if java_home:
        java_path = Path(java_home) / "bin" / "java"
        if java_path.exists():
            return str(java_path)
    discovered = shutil.which("java")
    if discovered:
        return discovered
    return None


def _build_quint_env(java_home: str | None) -> dict[str, str] | None:
    if not java_home:
        return None
    env = dict(os.environ)
    env["JAVA_HOME"] = java_home
    java_bin_dir = str(Path(java_home) / "bin")
    current_path = env.get("PATH", "")
    path_parts = [part for part in current_path.split(os.pathsep) if part]
    if java_bin_dir not in path_parts:
        env["PATH"] = (
            f"{java_bin_dir}{os.pathsep}{current_path}"
            if current_path
            else java_bin_dir
        )
    return env


def _detect_missing_java_runtime(output: str) -> bool:
    text = output.lower()
    return any(
        marker in text
        for marker in (
            "unable to locate a java runtime",
            "no java runtime present",
            "java: command not found",
            "could not find java",
            "java_home is not defined correctly",
        )
    )


def _resolve_apalache_work_dir() -> Path:
    candidates: list[Path] = []
    for env_key in ("MOLT_APALACHE_WORK_DIR", "MOLT_DIFF_TMPDIR", "TMPDIR"):
        raw = os.environ.get(env_key, "").strip()
        if raw:
            candidates.append(Path(raw).expanduser())
    candidates.append(Path("/Volumes/APDataStore/Molt/tmp/apalache"))
    candidates.append(Path("/tmp/molt_apalache"))
    for candidate in candidates:
        try:
            candidate.mkdir(parents=True, exist_ok=True)
            return candidate.resolve()
        except OSError:
            continue
    return ROOT


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


def check_lean(*, verbose: bool = True) -> list[str]:
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

    if verbose:
        print("  Running: lake build (formal/lean/) ...")
    result = _safe_run([lake, "build"], cwd=LEAN_DIR, timeout=600)

    if result.returncode != 0:
        errors.append("Lean proofs failed to build")
        # Show last 20 lines of output for diagnostics
        output = (result.stdout + result.stderr).strip()
        for line in output.splitlines()[-20:]:
            errors.append(f"  {line}")

    return errors


def check_quint(*, verbose: bool = True) -> tuple[list[str], dict[str, Any]]:
    """Run Quint model verification with Node toolchain diagnostics/fallback."""
    errors: list[str] = []
    diagnostics: dict[str, Any] = {
        "node": _node_info(),
        "java": {
            "home": None,
            "path": None,
            "version": None,
        },
        "java_runtime_missing": False,
        "quint_path": None,
        "fallback_prefix": None,
        "fallback_used": False,
        "fallback_attempts": 0,
        "runtime_mismatch_detected": False,
        "models": [],
    }

    quint = shutil.which("quint")
    if quint is None:
        errors.append(
            "quint not found; install via: npm install -g @informalsystems/quint"
        )
        return errors, diagnostics

    diagnostics["quint_path"] = quint
    diagnostics["fallback_prefix"] = _resolve_quint_fallback_prefix()
    work_dir = _resolve_apalache_work_dir()
    diagnostics["apalache_work_dir"] = str(work_dir)
    java_home = _resolve_java_home()
    java_bin = _resolve_java_bin(java_home)
    diagnostics["java"]["home"] = java_home
    diagnostics["java"]["path"] = java_bin
    if java_bin:
        try:
            java_probe = _safe_run([java_bin, "-version"], timeout=10)
            diagnostics["java"]["version"] = (
                java_probe.stderr or java_probe.stdout or ""
            ).strip() or None
        except Exception:
            diagnostics["java"]["version"] = None
    quint_env = _build_quint_env(java_home)

    if not QUINT_DIR.exists():
        errors.append("formal/quint/ directory does not exist")
        return errors, diagnostics

    for model_file, invariant in QUINT_MODELS:
        model_path = QUINT_DIR / model_file
        model_diag: dict[str, Any] = {
            "model": model_file,
            "invariant": invariant,
            "attempted_commands": [],
            "returncode": None,
            "fallback_used": False,
            "runtime_mismatch": False,
            "java_runtime_missing": False,
        }
        diagnostics["models"].append(model_diag)

        if not model_path.exists():
            errors.append(f"missing Quint model: {model_file}")
            model_diag["error"] = "missing_model"
            continue

        args = [
            "run",
            str(model_path),
            f"--invariant={invariant}",
            "--max-steps=12",
            "--max-samples=200",
        ]
        primary_cmd = [quint, *args]
        model_diag["attempted_commands"].append(primary_cmd)
        if verbose:
            print(f"  Running: quint run {model_file} --invariant={invariant} ...")
        primary = _safe_run(primary_cmd, timeout=120, env=quint_env, cwd=work_dir)

        final_result = primary
        output = (primary.stdout or "") + (primary.stderr or "")
        node_major = diagnostics["node"].get("major")
        mismatch = (
            primary.returncode != 0
            and isinstance(node_major, int)
            and node_major >= 25
            and _detect_runtime_mismatch(output)
        )

        if mismatch:
            diagnostics["runtime_mismatch_detected"] = True
            model_diag["runtime_mismatch"] = True
            fallback_prefix = diagnostics.get("fallback_prefix")
            if isinstance(fallback_prefix, list) and fallback_prefix:
                fallback_cmd = [*fallback_prefix, quint, *args]
                model_diag["attempted_commands"].append(fallback_cmd)
                diagnostics["fallback_attempts"] = (
                    int(diagnostics.get("fallback_attempts", 0)) + 1
                )
                fallback = _safe_run(
                    fallback_cmd,
                    timeout=240,
                    env=quint_env,
                    cwd=work_dir,
                )
                final_result = fallback
                model_diag["fallback_used"] = True
                diagnostics["fallback_used"] = True

        model_diag["returncode"] = int(final_result.returncode)
        final_output = (final_result.stdout or "") + (final_result.stderr or "")
        java_runtime_missing = _detect_missing_java_runtime(final_output)
        model_diag["java_runtime_missing"] = java_runtime_missing
        if java_runtime_missing:
            diagnostics["java_runtime_missing"] = True

        if final_result.returncode != 0:
            errors.append(f"Quint model {model_file} invariant {invariant} violated")
            output = final_output.strip()
            for line in output.splitlines()[-10:]:
                errors.append(f"  {line}")

    if diagnostics["runtime_mismatch_detected"] and errors:
        errors.append(
            "quint_runtime_toolchain_mismatch: Node >=25 with current quint may fail; "
            "set MOLT_QUINT_NODE_FALLBACK='npx -y node@22' or install a compatible Node/quint pair"
        )
    if diagnostics["java_runtime_missing"]:
        errors.append(
            "quint_java_runtime_missing: Java runtime not detected for Quint/Apalache; "
            "install OpenJDK 21 and set JAVA_HOME (for example "
            "/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home)"
        )

    return errors, diagnostics


def run_gate(
    *,
    run_inventory: bool,
    run_lean: bool,
    run_quint: bool,
    verbose: bool = True,
) -> dict[str, Any]:
    report: dict[str, Any] = {
        "ok": True,
        "checks": {},
        "errors": [],
    }

    if run_inventory:
        inv_errors = check_inventory()
        report["checks"]["inventory"] = {
            "ok": not bool(inv_errors),
            "errors": inv_errors,
        }
        report["errors"].extend(inv_errors)

    if run_lean:
        lean_errors = check_lean(verbose=verbose)
        report["checks"]["lean"] = {
            "ok": not bool(lean_errors),
            "errors": lean_errors,
        }
        report["errors"].extend(lean_errors)

    if run_quint:
        quint_errors, quint_diag = check_quint(verbose=verbose)
        report["checks"]["quint"] = {
            "ok": not bool(quint_errors),
            "errors": quint_errors,
            "diagnostics": quint_diag,
        }
        report["errors"].extend(quint_errors)

    report["ok"] = not bool(report["errors"])
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description="Formal methods gate check")
    parser.add_argument("--lean", action="store_true", help="Check Lean proofs only")
    parser.add_argument("--quint", action="store_true", help="Check Quint models only")
    parser.add_argument(
        "--inventory", action="store_true", help="Check file inventory only"
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON report to stdout",
    )
    parser.add_argument(
        "--json-only",
        action="store_true",
        help="Emit only machine-readable JSON report to stdout",
    )
    args = parser.parse_args()

    run_all = not (args.lean or args.quint or args.inventory)
    run_inventory = bool(run_all or args.inventory)
    run_lean = bool(run_all or args.lean)
    run_quint = bool(run_all or args.quint)

    log_enabled = not bool(args.json_only)
    if log_enabled and run_inventory:
        print("[formal-methods] Checking file inventory ...")
    if log_enabled and run_lean:
        print("[formal-methods] Checking Lean proofs ...")
    if log_enabled and run_quint:
        print("[formal-methods] Checking Quint models ...")

    report = run_gate(
        run_inventory=run_inventory,
        run_lean=run_lean,
        run_quint=run_quint,
        verbose=log_enabled,
    )

    if log_enabled and report["errors"]:
        print("\nformal-methods gate FAILED:")
        for err in report["errors"]:
            print(f"  - {err}")
    elif log_enabled:
        print("\nformal-methods gate: ok")

    if args.json or args.json_only:
        print(json.dumps(report, indent=2, sort_keys=True))

    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
