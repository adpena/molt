#!/usr/bin/env python3
"""Correctness coverage dashboard for the Molt compiler (MOL-285).

Collects and reports correctness coverage across all verification layers:
  - Lean 4 formal proofs (sorry audit)
  - Quint TLA+ models (invariant coverage)
  - Differential, property-based, mutation, and fuzz tests
  - CI verification gates (tiered)
  - End-to-end proof chain status

Usage:
    uv run --python 3.12 python3 tools/correctness_dashboard.py
    uv run --python 3.12 python3 tools/correctness_dashboard.py --json
    uv run --python 3.12 python3 tools/correctness_dashboard.py --verbose
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

ROOT = Path(__file__).resolve().parents[1]
FORMAL_LEAN = ROOT / "formal" / "lean"
FORMAL_QUINT = ROOT / "formal" / "quint"
TESTS = ROOT / "tests"
TOOLS = ROOT / "tools"
CI_WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

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


def dim(t: str) -> str:
    return _c("2", t)


# ---------------------------------------------------------------------------
# 1. Lean Proof Coverage
# ---------------------------------------------------------------------------

META_EXCLUDE = {"SorryAudit.lean", "Completeness.lean"}

_SORRY_RE = re.compile(r"\bsorry\b")


def _count_sorrys_in_text(text: str) -> int:
    """Count sorry tactics in *text*, ignoring comments and string literals.

    Handles:
    - Block comments: ``/- ... -/`` (including nested)
    - Line comments: ``-- ...``
    - String literals: ``"..."``
    """
    # 1. Remove block comments (handle nesting via repeated passes).
    prev = None
    cleaned = text
    while prev != cleaned:
        prev = cleaned
        cleaned = re.sub(r"/\-(?:(?!/\-)(?:(?!\-/).|\n))*?\-/", " ", cleaned, flags=re.DOTALL)

    # 2. Process line by line: strip line comments, then string literals.
    total = 0
    for line in cleaned.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("--"):
            continue
        line = re.sub(r"--.*", "", line)
        line = re.sub(r'"(?:[^"\\]|\\.)*"', '""', line)
        total += len(_SORRY_RE.findall(line))
    return total


def _is_meta_file(path: Path) -> bool:
    """Return True if the file lives under a Meta/ directory or is excluded."""
    return "Meta" in path.parts or path.name in META_EXCLUDE


def collect_lean_coverage() -> dict[str, Any]:
    """Count Lean files and sorry occurrences."""
    if not FORMAL_LEAN.exists():
        return {
            "total_files": 0,
            "sorry_free_files": 0,
            "total_sorrys": 0,
            "sorry_pct_complete": 0.0,
            "files_with_sorrys": [],
        }

    lean_files: list[Path] = []
    for p in sorted(FORMAL_LEAN.rglob("*.lean")):
        if p.name == "lakefile.lean":
            continue
        if _is_meta_file(p):
            continue
        lean_files.append(p)

    total_sorrys = 0
    sorry_free = 0
    files_with_sorrys: list[dict[str, Any]] = []

    for lf in lean_files:
        try:
            text = lf.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        count = _count_sorrys_in_text(text)
        total_sorrys += count
        if count == 0:
            sorry_free += 1
        else:
            rel = str(lf.relative_to(FORMAL_LEAN))
            files_with_sorrys.append({"file": rel, "sorrys": count})

    files_with_sorrys.sort(key=lambda d: d["sorrys"], reverse=True)

    total = len(lean_files)
    pct = (sorry_free / total * 100) if total > 0 else 0.0

    return {
        "total_files": total,
        "sorry_free_files": sorry_free,
        "total_sorrys": total_sorrys,
        "sorry_pct_complete": round(pct, 1),
        "files_with_sorrys": files_with_sorrys,
    }


# ---------------------------------------------------------------------------
# 2. Quint Spec Coverage
# ---------------------------------------------------------------------------


def collect_quint_coverage() -> dict[str, Any]:
    """Count Quint models and invariant coverage."""
    if not FORMAL_QUINT.exists():
        return {"total_models": 0, "with_invariants": 0, "models": []}

    qnt_files = sorted(FORMAL_QUINT.glob("*.qnt"))
    models: list[dict[str, Any]] = []

    for qf in qnt_files:
        try:
            text = qf.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        # Look for invariant definitions: val Inv, def Inv, or --invariant=Inv
        has_inv = bool(re.search(r"\bInv\b", text))
        models.append({
            "file": qf.name,
            "has_invariant": has_inv,
        })

    with_inv = sum(1 for m in models if m["has_invariant"])

    return {
        "total_models": len(models),
        "with_invariants": with_inv,
        "models": models,
    }


# ---------------------------------------------------------------------------
# 3. Test Coverage
# ---------------------------------------------------------------------------


def _count_py_files(directory: Path) -> int:
    """Count .py files in a directory tree."""
    if not directory.exists():
        return 0
    return sum(1 for _ in directory.rglob("*.py"))


def _count_pattern_in_files(directory: Path, pattern: str) -> int:
    """Count regex matches across all .py files in a directory."""
    if not directory.exists():
        return 0
    total = 0
    for py in directory.rglob("*.py"):
        try:
            text = py.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        total += len(re.findall(pattern, text))
    return total


def _count_kani_harnesses() -> int:
    """Count Kani proof harnesses across all Rust crates."""
    count = 0
    runtime_dir = ROOT / "runtime"
    if not runtime_dir.exists():
        return 0
    for rs in runtime_dir.rglob("kani_*.rs"):
        try:
            text = rs.read_text(encoding="utf-8", errors="replace")
            count += len(re.findall(r"#\[kani::proof\]", text))
        except OSError:
            pass
    return count


def _count_effective_tests(text: str) -> int:
    """Count effective test targets, expanding class-level parametrize."""
    try:
        tree = __import__("ast").parse(text)
    except SyntaxError:
        return len(re.findall(r"def test_", text))

    import ast as _ast

    count = 0
    for node in _ast.walk(tree):
        if isinstance(node, _ast.ClassDef):
            # Check for class-level @pytest.mark.parametrize
            param_count = 1
            for dec in node.decorator_list:
                if (
                    isinstance(dec, _ast.Call)
                    and isinstance(dec.func, _ast.Attribute)
                    and dec.func.attr == "parametrize"
                    and len(dec.args) >= 2
                ):
                    # Second arg is the list of parameter values
                    arg = dec.args[1]
                    if isinstance(arg, (_ast.List, _ast.Tuple)):
                        param_count *= len(arg.elts)
            methods = sum(
                1 for item in _ast.walk(node)
                if isinstance(item, _ast.FunctionDef) and item.name.startswith("test_")
            )
            count += methods * param_count
        elif isinstance(node, _ast.FunctionDef) and node.name.startswith("test_"):
            # Only count top-level / module-level test functions (not in classes)
            pass  # handled below

    # Count module-level test functions (not inside classes)
    class_ranges: list[tuple[int, int]] = []
    for node in _ast.iter_child_nodes(tree):
        if isinstance(node, _ast.ClassDef):
            class_ranges.append((node.lineno, node.end_lineno or node.lineno))

    for node in _ast.iter_child_nodes(tree):
        if isinstance(node, _ast.FunctionDef) and node.name.startswith("test_"):
            count += 1

    return count


def collect_test_coverage() -> dict[str, Any]:
    """Collect test counts across all testing layers."""
    diff_dir = TESTS / "differential"
    prop_dir = TESTS / "property"
    fuzz_dir = TESTS / "fuzz"
    mutation_tool = TOOLS / "mutation_test.py"

    # Differential tests by category
    basic_count = _count_py_files(diff_dir / "basic")
    stdlib_count = _count_py_files(diff_dir / "stdlib")
    pyperformance_count = _count_py_files(diff_dir / "pyperformance")
    diff_total = _count_py_files(diff_dir)

    # Property-based test functions (count @given decorators)
    property_funcs = _count_pattern_in_files(prop_dir, r"@given")

    # Mutation test operators
    mutation_operators = 0
    if mutation_tool.exists():
        try:
            text = mutation_tool.read_text(encoding="utf-8", errors="replace")
            # Count entries in COMPILER_OPERATORS = [...] list
            match = re.search(
                r"COMPILER_OPERATORS\s*(?::[^=]+=|=)\s*\[\s*((?:\"[^\"]+\"\s*,?\s*)+)\]",
                text,
                re.DOTALL,
            )
            if match:
                mutation_operators = len(re.findall(r'"(\w+)"', match.group(1)))
        except OSError:
            pass

    # Fuzz targets (accounts for class-level @pytest.mark.parametrize)
    fuzz_targets = 0
    if fuzz_dir.exists():
        for py in fuzz_dir.rglob("*.py"):
            if py.name.startswith("test_"):
                try:
                    text = py.read_text(encoding="utf-8", errors="replace")
                    fuzz_targets += _count_effective_tests(text)
                except OSError:
                    pass

    return {
        "differential": {
            "total": diff_total,
            "basic": basic_count,
            "stdlib": stdlib_count,
            "pyperformance": pyperformance_count,
        },
        "property_based": property_funcs,
        "mutation_operators": mutation_operators,
        "fuzz_targets": fuzz_targets,
    }


# ---------------------------------------------------------------------------
# 4. Verification Pass Chain (CI gates)
# ---------------------------------------------------------------------------

# Classification heuristics for CI step names
_GATE_CLASSIFIERS: list[tuple[str, list[str]]] = [
    ("formal", ["formal", "lean", "quint", "proof", "lake"]),
    ("correctness", [
        "diff", "translation", "deterministic", "determinism",
        "reproducible", "correspondence", "verified", "ir-structure",
        "property", "mutation", "fuzz", "model-based", "gate",
        "molt-diff", "core-lane", "ir-probe", "coverage",
    ]),
    ("performance", ["perf", "bench", "throughput", "k6"]),
    ("security", ["secret", "audit", "capabilities", "trust"]),
]


def _classify_gate(step_name: str) -> str:
    """Classify a CI gate into a category."""
    name_lower = step_name.lower()
    for category, keywords in _GATE_CLASSIFIERS:
        if any(kw in name_lower for kw in keywords):
            return category
    return "other"


def collect_ci_gates() -> dict[str, Any]:
    """Parse CI workflow and classify gates."""
    if not CI_WORKFLOW.exists():
        return {"total": 0, "by_category": {}, "gates": []}

    try:
        text = CI_WORKFLOW.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return {"total": 0, "by_category": {}, "gates": []}

    # Extract step names (- name: ...)
    step_names = re.findall(r"^\s*-\s+name:\s+(.+)$", text, re.MULTILINE)

    # Filter to actual verification steps (not setup/install/upload steps)
    setup_keywords = [
        "install", "set up", "cache", "checkout", "upload",
        "ensure clang", "ensure zig", "rust cache", "create extension",
        "prepare external", "export external", "build extension",
    ]
    gates: list[dict[str, str]] = []
    for name in step_names:
        name_clean = name.strip().strip('"').strip("'")
        name_lower = name_clean.lower()
        if any(kw in name_lower for kw in setup_keywords):
            continue
        category = _classify_gate(name_clean)
        gates.append({"name": name_clean, "category": category})

    by_category: dict[str, int] = {}
    for g in gates:
        cat = g["category"]
        by_category[cat] = by_category.get(cat, 0) + 1

    # Also classify by tier based on ci_gate.py structure
    # Tier 1: checks in _build_checks with tier=1
    # We read ci_gate.py to count tiers
    tier_counts = _count_ci_gate_tiers()

    return {
        "total": len(gates),
        "by_category": by_category,
        "tier_counts": tier_counts,
        "gates": gates,
    }


def _count_ci_gate_tiers() -> dict[str, int]:
    """Count checks per tier from ci_gate.py."""
    ci_gate = TOOLS / "ci_gate.py"
    if not ci_gate.exists():
        return {}
    try:
        text = ci_gate.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return {}

    tiers: dict[str, int] = {}
    for match in re.finditer(r"tier=(\d+)", text):
        tier_key = f"tier_{match.group(1)}"
        tiers[tier_key] = tiers.get(tier_key, 0) + 1
    return tiers


# ---------------------------------------------------------------------------
# 5. Proof Chain Status
# ---------------------------------------------------------------------------

# Each entry: (label, lean_files_to_check, description_if_blocked)
_PROOF_CHAIN: list[tuple[str, list[str], str]] = [
    (
        "ConstFold expr correctness",
        ["MoltTIR/Passes/ConstFoldCorrect.lean"],
        "",
    ),
    (
        "SCCP expr correctness",
        ["MoltTIR/Passes/SCCPCorrect.lean"],
        "var-case definedness",
    ),
    (
        "DCE instr correctness",
        ["MoltTIR/Passes/DCECorrect.lean"],
        "simulation pending",
    ),
    (
        "Lowering preservation",
        ["MoltLowering/Correct.lean"],
        "PyExpr induction",
    ),
    (
        "E2E compilation",
        [
            "MoltTIR/EndToEnd.lean",
            "MoltTIR/Compilation/CompilationCorrectness.lean",
            "MoltTIR/Compilation/ForwardSimulation.lean",
        ],
        "blocked by lowering",
    ),
]


def collect_proof_chain() -> list[dict[str, Any]]:
    """Assess the status of each link in the proof chain."""
    chain: list[dict[str, Any]] = []

    for label, files, block_reason in _PROOF_CHAIN:
        total_sorrys = 0
        for rel in files:
            fp = FORMAL_LEAN / rel
            if not fp.exists():
                total_sorrys += -1  # signal missing
                continue
            try:
                text = fp.read_text(encoding="utf-8", errors="replace")
            except OSError:
                total_sorrys += -1
                continue
            total_sorrys += _count_sorrys_in_text(text)

        if total_sorrys < 0:
            status = "missing"
            icon = " "
        elif total_sorrys == 0:
            status = "complete"
            icon = "ok"
        else:
            status = "partial"
            icon = "~"

        detail = ""
        if status == "partial":
            detail = f"{total_sorrys} sorry{'s' if total_sorrys != 1 else ''}"
            if block_reason:
                detail += f": {block_reason}"
        elif status == "missing":
            detail = "files not found"
        elif status == "complete" and block_reason:
            detail = block_reason

        chain.append({
            "label": label,
            "status": status,
            "icon": icon,
            "sorrys": max(total_sorrys, 0),
            "detail": detail,
        })

    return chain


# ---------------------------------------------------------------------------
# Report rendering
# ---------------------------------------------------------------------------

_CHAIN_ICONS = {
    "complete": "[ok]",
    "partial": "[~ ]",
    "missing": "[  ]",
}

_CHAIN_ICONS_COLOR = {
    "complete": green("[ok]"),
    "partial": yellow("[~ ]"),
    "missing": red("[  ]"),
}


def render_human(
    lean: dict[str, Any],
    quint: dict[str, Any],
    tests: dict[str, Any],
    ci: dict[str, Any],
    chain: list[dict[str, Any]],
    verbose: bool = False,
) -> None:
    """Print the human-readable dashboard to stdout."""
    print()
    print(bold("=== Molt Correctness Coverage Dashboard ==="))

    # -- Formal Verification --
    print(f"\n{bold('Formal Verification:')}")
    print(
        f"  Lean 4 proofs:        {lean['total_files']} files, "
        f"{lean['total_sorrys']} sorrys "
        f"({lean['sorry_pct_complete']}% sorry-free)"
    )
    print(
        f"  Quint TLA+ models:    {quint['total_models']} models, "
        f"{quint['with_invariants']} with invariants"
    )
    kani_count = _count_kani_harnesses()
    print(f"  Kani harnesses:       {kani_count} harnesses")

    if verbose and lean["files_with_sorrys"]:
        print(f"\n  {dim('Top files with sorrys:')}")
        for entry in lean["files_with_sorrys"][:10]:
            print(f"    {dim(str(entry['sorrys']).rjust(3))}  {dim(entry['file'])}")

    # -- Testing --
    diff = tests["differential"]
    print(f"\n{bold('Testing:')}")
    print(
        f"  Differential tests:   {diff['total']} "
        f"(basic: {diff['basic']}, stdlib: {diff['stdlib']}, "
        f"pyperformance: {diff['pyperformance']})"
    )
    print(f"  Property-based:       {tests['property_based']} test functions")
    print(f"  Mutation operators:   {tests['mutation_operators']}")
    print(f"  Fuzz targets:         {tests['fuzz_targets']}")

    # -- CI Gates --
    print(f"\n{bold('CI Gates:')}")
    tiers = ci.get("tier_counts", {})
    t1 = tiers.get("tier_1", 0)
    t2 = tiers.get("tier_2", 0)
    t3 = tiers.get("tier_3", 0)
    print(f"  Tier 1 (per-commit):  {t1} gates")
    print(f"  Tier 2 (per-PR):      {t2} gates")
    print(f"  Tier 3 (nightly):     {t3} gates")

    by_cat = ci.get("by_category", {})
    if by_cat:
        parts = []
        for cat in ("formal", "correctness", "performance", "security", "other"):
            count = by_cat.get(cat, 0)
            if count > 0:
                parts.append(f"{count} {cat}")
        if parts:
            print(f"  Workflow steps:       {ci['total']} total ({', '.join(parts)})")

    # -- Proof Chain Status --
    print(f"\n{bold('Proof Chain Status:')}")
    for link in chain:
        icons = _CHAIN_ICONS_COLOR if IS_TTY else _CHAIN_ICONS
        icon = icons.get(link["status"], "[??]")
        detail = f" ({link['detail']})" if link["detail"] else ""
        if link["status"] == "complete":
            label = green(link["label"])
        elif link["status"] == "partial":
            label = yellow(link["label"])
        else:
            label = red(link["label"])
        print(f"  {icon} {label}{dim(detail)}")

    print()


def build_json_output(
    lean: dict[str, Any],
    quint: dict[str, Any],
    tests: dict[str, Any],
    ci: dict[str, Any],
    chain: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build the JSON-serializable output dict."""
    return {
        "formal_verification": {
            "lean": lean,
            "quint": quint,
            "kani_harnesses": _count_kani_harnesses(),
        },
        "testing": tests,
        "ci_gates": ci,
        "proof_chain": chain,
    }


# ---------------------------------------------------------------------------
# MOL-214: Delta protocol integration
# ---------------------------------------------------------------------------

def _import_sibling(module_name: str, filename: str) -> Any:
    """Import a sibling script from the tools/ directory, returning the module or None."""
    import importlib.util
    path = TOOLS / filename
    if not path.exists():
        return None
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        return None
    mod = importlib.util.module_from_spec(spec)
    # Register in sys.modules so dataclass introspection works.
    sys.modules[module_name] = mod
    try:
        spec.loader.exec_module(mod)
    except Exception:
        sys.modules.pop(module_name, None)
        return None
    return mod


def _try_import_delta() -> Any:
    """Import the delta tracker, returning the class or None."""
    try:
        from tools.dashboard_delta import DeltaTracker
        return DeltaTracker
    except ImportError:
        mod = _import_sibling("dashboard_delta", "dashboard_delta.py")
        return getattr(mod, "DeltaTracker", None) if mod else None


def _try_import_slo() -> Any:
    """Import the specialization SLO evaluator, returning the class or None."""
    try:
        from tools.specialization_slo import SpecializationSLO
        return SpecializationSLO
    except ImportError:
        mod = _import_sibling("specialization_slo", "specialization_slo.py")
        return getattr(mod, "SpecializationSLO", None) if mod else None


def flatten_dashboard_state(
    lean: dict[str, Any],
    quint: dict[str, Any],
    tests: dict[str, Any],
    ci: dict[str, Any],
    chain: list[dict[str, Any]],
) -> dict[str, Any]:
    """Flatten the dashboard output into a flat key-value map suitable for
    delta tracking."""
    flat: dict[str, Any] = {}
    # Lean
    flat["lean_total_files"] = lean.get("total_files", 0)
    flat["lean_sorry_free"] = lean.get("sorry_free_files", 0)
    flat["lean_total_sorrys"] = lean.get("total_sorrys", 0)
    flat["lean_pct_complete"] = lean.get("sorry_pct_complete", 0.0)
    # Quint
    flat["quint_total_models"] = quint.get("total_models", 0)
    flat["quint_with_invariants"] = quint.get("with_invariants", 0)
    # Tests
    diff = tests.get("differential", {})
    flat["test_differential_total"] = diff.get("total", 0) if isinstance(diff, dict) else 0
    flat["test_property_funcs"] = tests.get("property_based", 0)
    flat["test_mutation_operators"] = tests.get("mutation_operators", 0)
    flat["test_fuzz_targets"] = tests.get("fuzz_targets", 0)
    flat["test_kani_harnesses"] = tests.get("kani_harnesses", 0)
    # CI
    flat["ci_gate_total"] = ci.get("total", 0)
    # Chain completeness
    complete = sum(1 for link in chain if link.get("status") == "complete")
    flat["proof_chain_complete"] = complete
    flat["proof_chain_total"] = len(chain)
    return flat


def publish_to_delta_tracker(
    lean: dict[str, Any],
    quint: dict[str, Any],
    tests: dict[str, Any],
    ci: dict[str, Any],
    chain: list[dict[str, Any]],
) -> Any:
    """Push the current dashboard state into a DeltaTracker instance.

    Returns the tracker (or None if the delta module is not available).
    """
    DeltaTracker = _try_import_delta()
    if DeltaTracker is None:
        return None

    tracker = DeltaTracker()
    flat = flatten_dashboard_state(lean, quint, tests, ci, chain)

    # Also merge specialization SLO metrics if available.
    SLOClass = _try_import_slo()
    if SLOClass is not None:
        slo = SLOClass.from_env()
        report = slo.evaluate()
        flat.update(report.dashboard_metrics())

    tracker.batch_update(flat)
    return tracker


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Molt correctness coverage dashboard.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output results as JSON (for CI consumption)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show additional detail (top sorry files, gate list)",
    )
    parser.add_argument(
        "--delta-snapshot",
        action="store_true",
        help="Output delta-protocol snapshot (flat key-value state) as JSON",
    )
    args = parser.parse_args()

    lean = collect_lean_coverage()
    quint = collect_quint_coverage()
    tests = collect_test_coverage()
    ci = collect_ci_gates()
    chain = collect_proof_chain()

    if args.delta_snapshot:
        flat = flatten_dashboard_state(lean, quint, tests, ci, chain)
        # Merge SLO metrics.
        SLOClass = _try_import_slo()
        if SLOClass is not None:
            slo = SLOClass.from_env()
            report = slo.evaluate()
            flat.update(report.dashboard_metrics())
        print(json.dumps(flat, indent=2))
        return

    if args.json:
        output = build_json_output(lean, quint, tests, ci, chain)
        # Append SLO status to JSON output.
        SLOClass = _try_import_slo()
        if SLOClass is not None:
            slo = SLOClass.from_env()
            report = slo.evaluate()
            output["specialization_slo"] = report.to_dict()
        print(json.dumps(output, indent=2))
    else:
        render_human(lean, quint, tests, ci, chain, verbose=args.verbose)

    # Publish to delta tracker (fire-and-forget for wired consumers).
    publish_to_delta_tracker(lean, quint, tests, ci, chain)


if __name__ == "__main__":
    main()
