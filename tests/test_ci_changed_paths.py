from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "tools" / "ci_changed_paths.py"
SPEC = importlib.util.spec_from_file_location("ci_changed_paths", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
ci_changed_paths = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci_changed_paths
SPEC.loader.exec_module(ci_changed_paths)


def _classes(*paths: str) -> dict[str, bool]:
    return ci_changed_paths.classify_paths(paths)


def test_docs_only_pr_skips_expensive_workflows() -> None:
    classes = _classes("AGENTS.md", "docs/agent/AGENTS.full.md")

    assert classes == {
        "python_tooling": False,
        "rust": False,
        "llvm": False,
        "python_security": False,
        "rust_security": False,
    }


def test_python_source_change_runs_python_smoke_only() -> None:
    classes = _classes("src/molt/cli/runtime_wasm_cache.py")

    assert classes["python_tooling"] is True
    assert classes["rust"] is False
    assert classes["llvm"] is False
    assert classes["python_security"] is False
    assert classes["rust_security"] is False


def test_runtime_text_leaf_change_runs_rust_without_llvm() -> None:
    classes = _classes("runtime/molt-stdlib-text/src/tokenize.rs")

    assert classes["python_tooling"] is False
    assert classes["rust"] is True
    assert classes["llvm"] is False


def test_midend_change_runs_llvm_stack() -> None:
    classes = _classes("runtime/molt-passes/src/tir/value_range.rs")

    assert classes["rust"] is True
    assert classes["llvm"] is True


def test_lockfiles_select_security_and_build_classes() -> None:
    cargo = _classes("Cargo.lock")
    uv = _classes("uv.lock")

    assert cargo["rust"] is True
    assert cargo["llvm"] is True
    assert cargo["rust_security"] is True
    assert cargo["python_tooling"] is False

    assert uv["python_tooling"] is True
    assert uv["python_security"] is True
    assert uv["rust"] is False
    assert uv["rust_security"] is False


def test_workflow_authority_changes_run_their_owned_jobs() -> None:
    ci = _classes(".github/workflows/ci.yml")
    security = _classes(".github/workflows/security_hardening.yml")

    assert ci["python_tooling"] is True
    assert ci["rust"] is True
    assert ci["llvm"] is True

    assert security["python_security"] is True
    assert security["rust_security"] is True
    assert security["python_tooling"] is False


def test_classifier_changes_force_all_classes() -> None:
    classes = _classes("tools/ci_changed_paths.py")

    assert classes == ci_changed_paths.all_true()
