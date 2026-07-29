from __future__ import annotations

import ast
import os
from pathlib import Path

from tests import conftest as pytest_conftest
from tests.wasm_linked_runner import wasm_test_build_env

ROOT = Path(__file__).resolve().parents[1]
WASM_BUILD_HELPERS = (
    "tests/test_array_slice_semantics.py",
    "tests/test_list_repeat_len_regression.py",
    "tests/test_wasm_browser_db_host.py",
    "tests/test_wasm_browser_embed.py",
    "tests/test_wasm_browser_gpu_host.py",
    "tests/test_wasm_determinism.py",
    "tests/test_wasm_freestanding.py",
    "tests/test_wasm_importlib_package_bootstrap.py",
    "tests/test_wasm_optimization.py",
    "tests/test_wasm_performance.py",
    "tests/test_wasm_pipeline_e2e.py",
    "tests/test_wasm_size_tracking.py",
    "tests/test_wasm_split_runtime.py",
    "tests/test_wasm_split_runtime_imported_module.py",
    "tests/test_wasm_vfs_e2e.py",
    "tests/wasm_linked_runner.py",
)
WASM_OPTIMIZER_TEST_CONSUMERS = (
    "tests/test_wasm_performance.py",
    "tests/test_wasm_pipeline_e2e.py",
)
PROCESS_CALL_NAMES = frozenset(
    {"_run_wasm_test_process", "call", "check_call", "check_output", "Popen", "run"}
)


def _tree(relative_path: str) -> ast.Module:
    path = ROOT / relative_path
    return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))


def _string_literals(tree: ast.AST) -> list[str]:
    return [
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
    ]


def _call_leaf_name(call: ast.Call) -> str | None:
    function = call.func
    if isinstance(function, ast.Name):
        return function.id
    if isinstance(function, ast.Attribute):
        return function.attr
    return None


def _is_canonical_optimizer_call(call: ast.Call) -> bool:
    function = call.func
    return (
        isinstance(function, ast.Attribute)
        and function.attr == "optimize"
        and isinstance(function.value, ast.Name)
        and function.value.id == "wasm_optimize"
    )


def _optimizer_consumer_policy_violations(tree: ast.AST) -> list[str]:
    violations: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            value = node.value
            if value in {"-O1", "-O2", "-O3", "-O4", "-Os", "-Oz"}:
                violations.append(f"line {node.lineno}: raw optimizer level {value}")
            if value.startswith(("--enable-", "--disable-")):
                violations.append(
                    f"line {node.lineno}: copied optimizer feature flag {value}"
                )
            if value == "--no-validation":
                violations.append(f"line {node.lineno}: optimizer validation bypass")
        if not isinstance(node, ast.Call):
            continue
        leaf_name = _call_leaf_name(node)
        if leaf_name == "which" and any(
            isinstance(arg, ast.Constant) and arg.value == "wasm-opt"
            for arg in node.args
        ):
            violations.append(f"line {node.lineno}: raw wasm-opt discovery")
        if leaf_name in PROCESS_CALL_NAMES and any(
            (isinstance(child, ast.Name) and child.id in {"wasm_opt", "wasm_opt_path"})
            or (
                isinstance(child, ast.Constant)
                and child.value in {"wasm-opt", "wasm-opt.exe"}
            )
            for child in ast.walk(node)
        ):
            violations.append(f"line {node.lineno}: raw wasm-opt process pipeline")
    return violations


def test_wasm_build_helpers_do_not_force_private_cargo_policy() -> None:
    violations: list[str] = []
    for relative_path in WASM_BUILD_HELPERS:
        tree = _tree(relative_path)
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and len(node.args) >= 2:
                function = node.func
                key = node.args[0]
                value = node.args[1]
                if (
                    isinstance(function, ast.Attribute)
                    and function.attr in {"setdefault", "__setitem__"}
                    and isinstance(key, ast.Constant)
                    and key.value == "CARGO_BUILD_JOBS"
                    and isinstance(value, ast.Constant)
                    and value.value == "1"
                ):
                    violations.append(
                        f"{relative_path}:{node.lineno}: forced serial Cargo jobs"
                    )
            if isinstance(node, ast.Assign):
                for target in node.targets:
                    if (
                        isinstance(target, ast.Subscript)
                        and isinstance(target.slice, ast.Constant)
                        and target.slice.value == "CARGO_BUILD_JOBS"
                        and isinstance(node.value, ast.Constant)
                        and node.value.value == "1"
                    ):
                        violations.append(
                            f"{relative_path}:{node.lineno}: forced serial Cargo jobs"
                        )
        for value in _string_literals(tree):
            if value.startswith("MOLT_WASM") and "SCCACHE" in value:
                violations.append(
                    f"{relative_path}: WASM-specific sccache policy {value!r}"
                )
    assert not violations, "\n".join(violations)


def test_cargo_build_env_is_the_single_sccache_admission_authority() -> None:
    cargo_tree = _tree("src/molt/cli/cargo_execution.py")
    build_env = next(
        node
        for node in cargo_tree.body
        if isinstance(node, ast.FunctionDef) and node.name == "_cargo_build_env"
    )
    calls = [
        node
        for node in ast.walk(build_env)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "_maybe_enable_sccache"
    ]
    assert len(calls) == 1

    consumers: list[str] = []
    for path in (ROOT / "src" / "molt" / "cli").glob("*.py"):
        if path.name == "cargo_execution.py":
            continue
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        if any(
            isinstance(node, ast.Name) and node.id == "_maybe_enable_sccache"
            for node in ast.walk(tree)
        ):
            consumers.append(str(path.relative_to(ROOT)))
    assert consumers == []


def test_wasm_optimizer_features_have_one_source_authority() -> None:
    authority = "src/molt/wasm_optimization.py"
    forbidden_prefixes = ("--enable-", "--disable-")
    violations: list[str] = []
    for root in (ROOT / "src", ROOT / "tools"):
        for path in root.rglob("*.py"):
            relative = path.relative_to(ROOT).as_posix()
            if relative == authority:
                continue
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            for value in _string_literals(tree):
                if value.startswith(forbidden_prefixes) and (
                    "gc" in value
                    or "simd" in value
                    or "reference" in value
                    or "descriptor" in value
                    or "bulk-memory" in value
                    or "multivalue" in value
                ):
                    violations.append(
                        f"{relative}: duplicate optimizer feature {value}"
                    )
    assert not violations, "\n".join(violations)


def test_wasm_optimizer_test_consumers_use_canonical_authority() -> None:
    violations: list[str] = []
    for relative_path in WASM_OPTIMIZER_TEST_CONSUMERS:
        tree = _tree(relative_path)
        violations.extend(
            f"{relative_path}:{violation}"
            for violation in _optimizer_consumer_policy_violations(tree)
        )
        canonical_calls = [
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.Call) and _is_canonical_optimizer_call(node)
        ]
        if not canonical_calls:
            violations.append(f"{relative_path}: no canonical optimizer call")
    assert not violations, "\n".join(violations)


def test_wasm_optimizer_test_consumer_guard_rejects_raw_authority() -> None:
    tree = ast.parse(
        """
wasm_opt = shutil.which("wasm-opt")
_run_wasm_test_process(
    [wasm_opt, "-Oz", "--enable-simd", "--no-validation", "in.wasm"]
)
"""
    )

    violations = _optimizer_consumer_policy_violations(tree)

    assert any("raw wasm-opt discovery" in violation for violation in violations)
    assert any("raw wasm-opt process pipeline" in violation for violation in violations)
    assert any("raw optimizer level" in violation for violation in violations)
    assert any("copied optimizer feature flag" in violation for violation in violations)
    assert any("optimizer validation bypass" in violation for violation in violations)


def test_pytest_session_scope_marks_only_generated_session_ids(
    monkeypatch,
) -> None:
    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.delenv("MOLT_SESSION_ID_GENERATED", raising=False)
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)

    pytest_conftest._ensure_pytest_process_scope()

    assert os.environ["MOLT_SESSION_ID"].startswith("pytest-")
    assert os.environ["MOLT_SESSION_ID_GENERATED"] == "1"

    monkeypatch.setenv("MOLT_SESSION_ID", "operator-session")
    monkeypatch.delenv("MOLT_SESSION_ID_GENERATED", raising=False)
    pytest_conftest._ensure_pytest_process_scope()

    assert os.environ["MOLT_SESSION_ID"] == "operator-session"
    assert "MOLT_SESSION_ID_GENERATED" not in os.environ

    monkeypatch.setenv("PYTEST_XDIST_WORKER", "gw3")
    pytest_conftest._ensure_pytest_process_scope()

    assert os.environ["MOLT_SESSION_ID"] == "pytest-xdist-gw3"
    assert os.environ["MOLT_SESSION_ID_GENERATED"] == "1"


def test_wasm_build_env_replaces_generated_pytest_session_with_stable_lane() -> None:
    env = wasm_test_build_env(ROOT, create_dirs=False)

    assert env["MOLT_SESSION_ID"] == "test-wasm-local"
    assert env.get("MOLT_SESSION_ID_GENERATED") is None
