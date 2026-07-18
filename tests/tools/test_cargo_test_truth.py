from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "check_cargo_test_truth.py"
SPEC = importlib.util.spec_from_file_location("check_cargo_test_truth", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_cargo_test_topology_cannot_mask_or_skip_binaries() -> None:
    assert MODULE.violations() == []


def test_cargo_truth_runner_custody_is_proof_plan_owned(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan = tmp_path / "proof_plan.toml"
    plan.write_text(
        """
[[command]]
id = "rust.test.renamed"
argv = ["python3", "tools/run_cargo_test_truth.py"]
""".lstrip(),
        encoding="utf-8",
    )
    monkeypatch.setattr(MODULE, "PROOF_PLAN", plan)

    assert any("rust.test.default-truth" in failure for failure in MODULE.violations())


def test_multi_executable_proof_gate_requires_complete_failure_collection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan = tmp_path / "proof_plan.toml"
    plan.write_text(
        """
[[command]]
id = "rust.test.default-truth"
argv = ["python3", "tools/run_cargo_test_truth.py"]

[[rule]]
name = "unsafe-filtered-package-test"
gates = ["cargo test -p molt-ir a_test_filter"]
""".lstrip(),
        encoding="utf-8",
    )
    monkeypatch.setattr(MODULE, "PROOF_PLAN", plan)

    assert any("lacks --no-fail-fast" in failure for failure in MODULE.violations())


def test_truth_runner_accepts_only_the_exact_registered_set() -> None:
    runner_path = ROOT / "tools" / "run_cargo_test_truth.py"
    spec = importlib.util.spec_from_file_location("run_cargo_test_truth", runner_path)
    assert spec is not None and spec.loader is not None
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    context = {"platform": "windows", "target": "default"}
    registered = [entry["identity"] for entry in runner.check_suite_honesty.load_manifest()["execution_reds"]]
    output = "\n".join(f"test {identity} ... FAILED" for identity in registered)
    returncode = 101 if registered else 0
    assert runner.verdict(output, returncode, context) == []
    assert runner.verdict(output + "\ntest new_red ... FAILED", 101, context)
    if registered:
        assert runner.verdict(output.replace("FAILED", "ok", 1), 101, context)


def test_truth_runner_rejects_compile_failures_without_test_identity() -> None:
    runner_path = ROOT / "tools" / "run_cargo_test_truth.py"
    spec = importlib.util.spec_from_file_location("run_cargo_test_truth_compile", runner_path)
    assert spec is not None and spec.loader is not None
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    problems = runner.verdict(
        "error[E0004]: non-exhaustive patterns\nerror: could not compile `molt`",
        101,
        {"platform": "windows", "target": "default"},
    )
    assert any("compiler error" in problem for problem in problems)
