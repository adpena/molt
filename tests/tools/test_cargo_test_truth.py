from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace

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


def test_every_cargo_compilation_gate_requires_locked_dependency_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan = tmp_path / "proof_plan.toml"
    plan.write_text(
        """
[[command]]
id = "rust.test.default-truth"
argv = ["python3", "tools/run_cargo_test_truth.py"]

[[rule]]
name = "unlocked-package-test"
gates = ["cargo check -p molt-ir"]
""".lstrip(),
        encoding="utf-8",
    )
    monkeypatch.setattr(MODULE, "PROOF_PLAN", plan)

    assert any("lacks --locked" in failure for failure in MODULE.violations())


def test_truth_runner_accepts_only_the_exact_registered_set() -> None:
    runner_path = ROOT / "tools" / "run_cargo_test_truth.py"
    spec = importlib.util.spec_from_file_location("run_cargo_test_truth", runner_path)
    assert spec is not None and spec.loader is not None
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    context = {"platform": "windows", "target": "default"}
    registered = [
        entry["identity"]
        for entry in runner.check_suite_honesty.load_manifest()["execution_reds"]
    ]
    output = "\n".join(f"test {identity} ... FAILED" for identity in registered)
    returncode = 101 if registered else 0
    assert runner.verdict(output, returncode, context) == []
    assert runner.verdict(output + "\ntest new_red ... FAILED", 101, context)
    if registered:
        assert runner.verdict(output.replace("FAILED", "ok", 1), 101, context)


def test_truth_runner_rejects_compile_failures_without_test_identity() -> None:
    runner_path = ROOT / "tools" / "run_cargo_test_truth.py"
    spec = importlib.util.spec_from_file_location(
        "run_cargo_test_truth_compile", runner_path
    )
    assert spec is not None and spec.loader is not None
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    problems = runner.verdict(
        "error[E0004]: non-exhaustive patterns\nerror: could not compile `molt`",
        101,
        {"platform": "windows", "target": "default"},
    )
    assert any("compiler error" in problem for problem in problems)


def _load_tool(name: str, filename: str):
    path = ROOT / "tools" / filename
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_truth_runner_prefetches_every_workspace_lock_used_by_nested_tests() -> None:
    runner = _load_tool("run_cargo_test_truth_prefetch", "run_cargo_test_truth.py")

    assert runner.LOCKED_WORKSPACES == (
        ROOT / "Cargo.toml",
        ROOT / "runtime" / "Cargo.toml",
    )
    config = runner.target_runner_config("x86_64-unknown-linux-gnu")
    assert config.startswith("target.x86_64-unknown-linux-gnu.runner=[")
    assert "cargo_test_binary_runner.py" in config


def test_resource_binary_runner_isolates_each_test_and_continues_after_failure(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_isolation", "cargo_test_binary_runner.py"
    )
    calls: list[list[str]] = []

    def fake_run(argv, **kwargs):
        calls.append(list(argv))
        if "--list" in argv:
            return SimpleNamespace(
                returncode=0,
                stdout="limit_one: test\nlimit_two: test\n",
                stderr="",
            )
        identity = argv[argv.index("--exact") + 1]
        return SimpleNamespace(returncode=6 if identity == "limit_one" else 0)

    monkeypatch.setattr(binary_runner.subprocess, "run", fake_run)

    assert binary_runner.run_resource_tests("resource_enforcement-hash", []) == 1
    assert [call[call.index("--exact") + 1] for call in calls[1:]] == [
        "limit_one",
        "limit_two",
    ]
    assert all("--test-threads=1" in call for call in calls[1:])
    assert "test limit_one ... FAILED" in capsys.readouterr().out


def test_resource_binary_detection_does_not_capture_sibling_targets() -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_detection", "cargo_test_binary_runner.py"
    )

    assert binary_runner.is_resource_test_binary(
        "/tmp/deps/resource_enforcement-a1b2c3"
    )
    assert binary_runner.is_resource_test_binary("resource_enforcement.exe")
    assert not binary_runner.is_resource_test_binary(
        "/tmp/deps/resource_accounting-a1b2c3"
    )


def test_isolated_signal_failure_is_captured_as_exact_receipt_identity() -> None:
    runner = _load_tool("run_cargo_test_truth_signal", "run_cargo_test_truth.py")
    rows = runner.parse_test_results(
        "test env_var_init_installs_tracker ... FAILED\n"
        "isolated resource test process exited with -6\n",
        {"platform": "linux", "target": "default"},
    )

    assert rows == [
        {
            "identity": "env_var_init_installs_tracker",
            "status": "fail",
            "context": {"platform": "linux", "target": "default"},
        }
    ]
