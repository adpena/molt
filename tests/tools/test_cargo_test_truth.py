from __future__ import annotations

import importlib.util
import hashlib
from io import StringIO
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from types import SimpleNamespace

import pytest

from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "check_cargo_test_truth.py"
SPEC = importlib.util.spec_from_file_location("check_cargo_test_truth", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def _accounting(count: int, *, complete: bool = True) -> dict:
    return {
        "schema": "molt.libtest-accounting.v1",
        "complete": complete,
        "observed_results": count,
        "declared_results": count if complete else None,
        "issues": [],
    }


def _libtest_output(identity: str, status: str = "ok") -> str:
    return (
        f"running 1 test\ntest {identity} ... {status}\n"
        f"test result: {'FAILED' if status == 'FAILED' else 'ok'}. "
        f"{int(status == 'ok')} passed; {int(status == 'FAILED')} failed; "
        f"{int(status == 'ignored')} ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
    )


def test_cargo_test_topology_cannot_mask_or_skip_binaries() -> None:
    assert MODULE.violations() == []


def test_truth_runner_direct_import_uses_canonical_tool_modules(tmp_path: Path) -> None:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    code = """
import runpy
import sys
from pathlib import Path

sys.path.insert(0, str(Path(sys.argv[1]).parent))
runpy.run_path(sys.argv[1])
assert "tools.check_suite_honesty" in sys.modules
assert "tools.command_execution" in sys.modules
assert "check_suite_honesty" not in sys.modules
assert "command_execution" not in sys.modules
"""
    completed = run_guarded_test_process(
        [sys.executable, "-c", code, str(ROOT / "tools" / "run_cargo_test_truth.py")],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


def test_cargo_truth_runner_custody_is_proof_plan_owned(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plan = tmp_path / "proof_plan.toml"
    plan.write_text(
        """
[[command]]
id = "rust.test.renamed"
argv = ["uv", "run", "--frozen", "python3", "tools/run_cargo_test_truth.py"]
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
argv = ["uv", "run", "--frozen", "python3", "tools/run_cargo_test_truth.py"]

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
argv = ["uv", "run", "--frozen", "python3", "tools/run_cargo_test_truth.py"]

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
    executable = str((ROOT / "target" / "deps" / "molt-runtime-test").resolve())
    metadata = {
        runner._executable_key(executable): {
            "package": "molt-runtime@0.1.0",
            "target_name": "molt_runtime",
            "target_kind": "lib",
            "executable": executable,
        }
    }
    raw_registered = [identity.rsplit("::", 1)[-1] for identity in registered]
    receipts = [
        {
            "executable_resolved": executable,
            "status": "failed" if registered else "success",
            "schema": "molt.cargo-test-binary.v2",
            "result_accounting": _accounting(len(raw_registered)),
            "failure_identities": raw_registered,
            "test_results": [
                {"identity": identity, "status": "fail"} for identity in raw_registered
            ],
        }
    ]
    output = ""
    returncode = 101 if registered else 0
    assert (
        runner.verdict(
            output,
            returncode,
            context,
            binary_receipts=receipts,
            expected_binaries=metadata,
        )
        == []
    )
    receipts[0]["status"] = "failed"
    receipts[0]["failure_identities"] = [*raw_registered, "new_red"]
    receipts[0]["test_results"] = [
        *receipts[0]["test_results"],
        {"identity": "new_red", "status": "fail"},
    ]
    assert runner.verdict(
        output,
        101,
        context,
        binary_receipts=receipts,
        expected_binaries=metadata,
    )
    if registered:
        receipts[0]["failure_identities"] = []
        receipts[0]["test_results"] = [
            {"identity": identity, "status": "pass"} for identity in raw_registered
        ]
        assert runner.verdict(
            output,
            101,
            context,
            binary_receipts=receipts,
            expected_binaries=metadata,
        )


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
        binary_receipts=[],
        expected_binaries={},
    )
    assert any("compiler error" in problem for problem in problems)


def _load_tool(name: str, filename: str):
    path = ROOT / "tools" / filename
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_truth_runner_owns_only_the_root_workspace_and_binary_runner_custody(
    tmp_path: Path,
) -> None:
    runner = _load_tool("run_cargo_test_truth_prefetch", "run_cargo_test_truth.py")

    assert runner.WORKSPACE_MANIFEST == ROOT / "Cargo.toml"
    config = runner.target_runner_config("x86_64-unknown-linux-gnu", tmp_path)
    assert config.startswith("target.x86_64-unknown-linux-gnu.runner=[")
    assert "cargo_test_binary_runner.py" in config
    assert "--timeout-seconds" in config
    assert str(runner.BINARY_TIMEOUT_SECONDS) in config
    assert "--receipt-dir" in config

    config = runner.target_runner_config(
        "x86_64-unknown-linux-gnu",
        tmp_path,
        "run-123",
        {"schema": "molt.git-source.v1", "head": "abc123"},
    )
    assert "--run-id" in config
    assert "run-123" in config
    assert "--source-identity-json" in config
    assert "molt.git-source.v1" in config


@pytest.mark.parametrize("publish_binary", [False, True])
def test_truth_runner_traverses_the_root_workspace_exactly_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, publish_binary: bool
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_workspace_execution", "run_cargo_test_truth.py"
    )
    monkeypatch.setattr(runner, "RUNS_ROOT", tmp_path / "runs")
    monkeypatch.setattr(runner, "RECEIPT", tmp_path / "latest.json")
    monkeypatch.setattr(runner, "run_identity", lambda _started: "root-workspace")
    monkeypatch.setattr(runner, "host_target", lambda: "x86_64-pc-windows-msvc")
    source_identity = {"schema": "molt.git-source.v1", "head": "exact"}
    monkeypatch.setattr(runner, "git_source_identity", lambda: source_identity)
    commands: list[tuple[str, ...]] = []
    package_id = "path+file:///molt/runtime/molt-runtime#0.1.0"

    def fake_streamed(
        command,
        *,
        evidence_path: Path,
        retain_cargo_artifacts=False,
        timeout_seconds=None,
    ):
        del retain_cargo_artifacts, timeout_seconds
        command = tuple(command)
        commands.append(command)
        output = (
            json.dumps(
                {
                    "packages": [
                        {"id": package_id, "name": "molt-runtime", "version": "0.1.0"}
                    ]
                }
            )
            if command[1] == "metadata"
            else ""
        )
        artifacts = ()
        if command[1] == "test" and publish_binary:
            executable = tmp_path / "runtime-test"
            executable.write_bytes(b"exact executable")
            size, digest = runner._file_identity(executable)
            artifacts = (
                {
                    "executable": str(executable),
                    "target": {"name": "molt_runtime", "kind": ["lib"]},
                    "package_id": package_id,
                },
            )
            receipt_dir = tmp_path / "runs" / "root-workspace" / "binaries" / "root"
            (receipt_dir / "one.json").write_text(
                json.dumps(
                    {
                        "schema": "molt.cargo-test-binary.v2",
                        "result_accounting": _accounting(1),
                        "invocation_id": "one",
                        "run_id": "root-workspace",
                        "source_identity": source_identity,
                        "executable_resolved": str(executable),
                        "executable_size": size,
                        "executable_sha256": digest,
                        "status": "success",
                        "test_results": [{"identity": "test_one", "status": "pass"}],
                        "failure_identities": [],
                    }
                ),
                encoding="utf-8",
            )
        evidence_path.parent.mkdir(parents=True, exist_ok=True)
        evidence_path.write_text(output, encoding="utf-8")
        return runner.StreamedCommandResult(
            returncode=0,
            retained_output=output,
            cargo_test_artifacts=artifacts,
            evidence={
                "path": str(evidence_path),
                "bytes": len(output),
                "sha256": hashlib.sha256(output.encode()).hexdigest(),
                "tail": output,
                "contains_compiler_error": False,
                "controller_errors": [],
            },
        )

    monkeypatch.setattr(runner, "run_streamed", fake_streamed)
    monkeypatch.setattr(runner, "verdict", lambda *_args, **_kwargs: [])

    # Missing synthetic artifacts must fail coverage after the same complete
    # root traversal used for a fully attributed binary receipt.
    assert runner.main() == (0 if publish_binary else 1)
    assert [command[1] for command in commands] == ["fetch", "metadata", "test"]
    for command in commands:
        assert command[command.index("--manifest-path") + 1] == str(ROOT / "Cargo.toml")
        assert "--locked" in command
    test_command = commands[-1]
    assert test_command[: len(runner.CANONICAL_COMMAND)] == runner.CANONICAL_COMMAND
    config = test_command[test_command.index("--config") + 1]
    assert '"--run-id","root-workspace"' in config
    assert "molt.git-source.v1" in config
    runner_argv = json.loads(config.split("=", 1)[1])
    receipt_index = runner_argv.index("--receipt-dir") + 1
    run_dir = tmp_path / "runs" / "root-workspace"
    assert runner_argv[receipt_index] == str(run_dir / "binaries" / "root")
    assert [path.name for path in (run_dir / "binaries").iterdir()] == ["root"]
    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["status"] == ("success" if publish_binary else "failed")
    assert manifest["source_identity"] == source_identity
    assert [phase["kind"] for phase in manifest["phases"]] == [
        "dependency-prefetch",
        "package-metadata",
        "workspace-test",
    ]
    assert all(phase["workspace"] == "root" for phase in manifest["phases"])
    assert all(phase["manifest"] == "Cargo.toml" for phase in manifest["phases"])
    if publish_binary:
        assert manifest["problems"] == []
        assert manifest["observed_test_count"] == 1
        [binary] = manifest["test_binaries"]
        [expected] = manifest["expected_test_binaries"]
        assert binary["workspace"] == expected["workspace"] == "root"
        assert expected["package"] == "molt-runtime@0.1.0"
        assert manifest["phases"][-1]["binary_receipt_count"] == 1
        assert manifest["phases"][-1]["expected_binary_count"] == 1
    else:
        assert "Cargo JSON reported zero expected test binaries" in manifest["problems"]


@pytest.mark.parametrize(
    ("failure", "expected_commands", "termination", "problem"),
    [
        (
            "fetch",
            ["fetch"],
            {"kind": "exit", "returncode": 101},
            "Cargo dependency prefetch failed; diagnostics retained in terminal phase",
        ),
        (
            "metadata",
            ["fetch", "metadata"],
            {"kind": "exit", "returncode": 101},
            "Cargo package metadata failed; diagnostics retained in terminal phase",
        ),
        (
            "invalid-metadata",
            ["fetch", "metadata"],
            {"kind": "metadata-validation", "returncode": 2},
            "Cargo package identity metadata was invalid",
        ),
    ],
)
def test_truth_runner_preflight_failure_is_terminal_without_starting_tests(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
    expected_commands: list[str],
    termination: dict,
    problem: str,
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_preflight_failure", "run_cargo_test_truth.py"
    )
    monkeypatch.setattr(runner, "RUNS_ROOT", tmp_path / "runs")
    monkeypatch.setattr(runner, "RECEIPT", tmp_path / "latest.json")
    monkeypatch.setattr(runner, "run_identity", lambda _started: failure)
    source_identity = {"schema": "molt.git-source.v1", "head": "exact"}
    monkeypatch.setattr(runner, "git_source_identity", lambda: source_identity)
    commands: list[str] = []

    def forbidden(*_args, **_kwargs):
        pytest.fail(
            "preflight failure must not start tests or replace its terminal verdict"
        )

    monkeypatch.setattr(runner, "host_target", forbidden)
    monkeypatch.setattr(runner, "verdict", forbidden)

    def fake_streamed(command, *, evidence_path, **_kwargs):
        commands.append(command[1])
        assert command[command.index("--manifest-path") + 1] == str(ROOT / "Cargo.toml")
        output = "preflight diagnostics\n"
        return runner.StreamedCommandResult(
            returncode=101 if command[1] == failure else 0,
            retained_output=output,
            cargo_test_artifacts=(),
            evidence={"path": str(evidence_path), "tail": output},
        )

    monkeypatch.setattr(runner, "run_streamed", fake_streamed)
    assert runner.main() == 1
    assert commands == expected_commands
    receipt = json.loads(runner.RECEIPT.read_text(encoding="utf-8"))
    assert receipt["status"] == "failed"
    assert receipt["source_identity"] == source_identity
    assert receipt["problems"] == [problem]
    assert receipt["phases"][-1]["termination"] == termination
    assert receipt["phases"][-1]["evidence"]["tail"] == "preflight diagnostics\n"
    assert all(phase["workspace"] == "root" for phase in receipt["phases"])
    assert all(phase["manifest"] == "Cargo.toml" for phase in receipt["phases"])
    assert (
        json.loads(
            (runner.RUNS_ROOT / failure / "manifest.json").read_text(encoding="utf-8")
        )
        == receipt
    )
    assert runner._ACTIVE_RUN_TERMINALIZER is None


def test_truth_runner_derives_complete_binary_coverage_from_cargo_json() -> None:
    runner = _load_tool("run_cargo_test_truth_coverage", "run_cargo_test_truth.py")
    first = str((ROOT / "target" / "deps" / "one-test").resolve())
    second = str((ROOT / "target" / "deps" / "two-test").resolve())
    output = "\n".join(
        [
            json.dumps(
                {
                    "reason": "compiler-artifact",
                    "profile": {"test": True},
                    "executable": first,
                    "package_id": "path+file:///molt#one@0.1.0",
                    "target": {"name": "one", "kind": ["lib"]},
                }
            ),
            json.dumps(
                {
                    "reason": "compiler-artifact",
                    "profile": {"test": False},
                    "executable": str(ROOT / "target" / "deps" / "dependency"),
                    "package_id": "path+file:///molt#dependency@0.1.0",
                    "target": {"name": "dependency", "kind": ["lib"]},
                }
            ),
            json.dumps(
                {
                    "reason": "compiler-artifact",
                    "profile": {"test": True},
                    "executable": second,
                    "package_id": "path+file:///molt#two@0.1.0",
                    "target": {"name": "two", "kind": ["test"]},
                }
            ),
        ]
    )
    expected = runner.expected_test_binaries(
        output,
        {
            "path+file:///molt#one@0.1.0": "one@0.1.0",
            "path+file:///molt#dependency@0.1.0": "dependency@0.1.0",
            "path+file:///molt#two@0.1.0": "two@0.1.0",
        },
    )
    receipts = [
        {"executable_resolved": first},
        {"executable_resolved": second},
    ]

    assert len(expected) == 2
    assert runner.binary_coverage_problems(expected, receipts) == []
    assert any(
        "zero receipt-producing" in problem
        for problem in runner.binary_coverage_problems(expected, [])
    )
    assert any(
        "coverage incomplete" in problem
        for problem in runner.binary_coverage_problems(expected, receipts[:1])
    )


def test_truth_runner_uses_metadata_for_version_only_and_renamed_path_ids() -> None:
    runner = _load_tool("run_cargo_test_truth_metadata_ids", "run_cargo_test_truth.py")
    ordinary_id = "path+file:///molt/runtime/molt-runtime#0.1.0"
    renamed_id = (
        "path+file:///molt/runtime/molt-cpython-abi#molt-lang-cpython-abi@0.1.0"
    )
    metadata = json.dumps(
        {
            "packages": [
                {"id": ordinary_id, "name": "molt-runtime", "version": "0.1.0"},
                {
                    "id": renamed_id,
                    "name": "molt-lang-cpython-abi",
                    "version": "0.1.0",
                },
            ]
        }
    )
    identities = runner.package_identities_from_metadata(metadata)
    first = str((ROOT / "target" / "deps" / "runtime-test").resolve())
    second = str((ROOT / "target" / "deps" / "abi-test").resolve())
    artifacts = "\n".join(
        json.dumps(
            {
                "reason": "compiler-artifact",
                "profile": {"test": True},
                "executable": executable,
                "package_id": package_id,
                "target": {"name": target, "kind": ["lib"]},
            }
        )
        for executable, package_id, target in (
            (first, ordinary_id, "molt_runtime"),
            (second, renamed_id, "molt_cpython_abi"),
        )
    )

    expected = runner.expected_test_binaries(artifacts, identities)
    assert expected[runner._executable_key(first)]["package"] == "molt-runtime@0.1.0"
    assert (
        expected[runner._executable_key(second)]["package"]
        == "molt-lang-cpython-abi@0.1.0"
    )


def test_truth_verdict_namespaces_same_test_by_package_target_and_executable() -> None:
    runner = _load_tool("run_cargo_test_truth_namespace", "run_cargo_test_truth.py")
    first = str((ROOT / "target" / "deps" / "one-test").resolve())
    second = str((ROOT / "target" / "deps" / "two-test").resolve())
    expected = {
        runner._executable_key(first): {
            "package": "one@0.1.0",
            "target_name": "one_target",
            "target_kind": "lib",
            "executable": first,
        },
        runner._executable_key(second): {
            "package": "two@0.1.0",
            "target_name": "two_target",
            "target_kind": "test",
            "executable": second,
        },
    }
    receipts = [
        {
            "executable_resolved": executable,
            "status": "success",
            "failure_identities": [],
            "test_results": [{"identity": "tests::same", "status": "pass"}],
            "schema": "molt.cargo-test-binary.v2",
            "result_accounting": _accounting(1),
        }
        for executable in (first, second)
    ]

    rows, problems = runner.receipt_test_rows(
        receipts,
        expected,
        {"platform": "windows", "target": "default"},
    )
    assert problems == []
    assert len(rows) == 2
    assert rows[0]["identity"] != rows[1]["identity"]
    assert all(row["identity"].endswith("::tests::same") for row in rows)
    assert rows[0]["identity"] == "one@0.1.0::lib:one_target::tests::same"
    assert rows[1]["identity"] == "two@0.1.0::test:two_target::tests::same"
    assert all("file:///" not in row["identity"] for row in rows)
    assert {row["context"]["cargo_target"] for row in rows} == {
        "one_target",
        "two_target",
    }

    contradictory = [
        {
            "executable_resolved": first,
            "status": "failed",
            "schema": "molt.cargo-test-binary.v2",
            "result_accounting": _accounting(1),
            "failure_identities": ["tests::same"],
            "test_results": [{"identity": "tests::same", "status": "pass"}],
        }
    ]
    contradicted_rows, contradicted_problems = runner.receipt_test_rows(
        contradictory,
        expected,
        {"platform": "windows", "target": "default"},
    )
    assert contradicted_rows[0]["status"] == "pass"
    assert any("contradicted pass" in problem for problem in contradicted_problems)


@pytest.mark.parametrize(
    "kind",
    [
        "prior-state-interaction",
        "parallel-or-order-interaction",
        "diagnostic-timeout",
        "budget-exhausted",
    ],
)
def test_truth_structural_candidate_sets_are_red_but_never_known_red_eligible(
    kind: str,
) -> None:
    runner = _load_tool(
        f"run_cargo_test_truth_structural_{kind}", "run_cargo_test_truth.py"
    )
    executable = str((ROOT / "target" / "deps" / "runtime-test").resolve())
    expected = {
        runner._executable_key(executable): {
            "package": "molt-runtime@0.1.0",
            "target_name": "molt_runtime",
            "target_kind": "lib",
            "executable": executable,
        }
    }
    receipts = [
        {
            "executable_resolved": executable,
            "status": "failed",
            "failure_identities": [],
            "test_results": [],
            "schema": "molt.cargo-test-binary.v2",
            "result_accounting": _accounting(0, complete=False),
            "diagnosis": {
                "kind": kind,
                "identity": "not_confirmed",
                "candidate_tests": ["not_confirmed", "sibling"],
            },
        }
    ]

    rows, problems = runner.receipt_test_rows(
        receipts,
        expected,
        {"platform": "windows", "target": "default"},
    )
    assert rows == []
    assert len(problems) == 1
    assert "not semantic or known-red evidence" in problems[0]
    assert "complete libtest result accounting" in problems[0]


@pytest.mark.parametrize(
    "location", ["status", "diagnosis", "execution", "malformed", "missing"]
)
def test_truth_excludes_entire_infrastructure_receipt_from_semantic_evidence(location):
    runner = _load_tool(
        "run_cargo_test_truth_infrastructure", "run_cargo_test_truth.py"
    )
    executable = str((ROOT / "target" / "deps" / "runtime-test").resolve())
    expected = {
        runner._executable_key(executable): {
            "package": "molt-runtime@0.1.0",
            "target_name": "molt_runtime",
            "target_kind": "lib",
            "executable": executable,
        }
    }
    failure = {
        "phase": "temporary_artifact_custody",
        "details": ["receipt unavailable"],
    }
    receipt = {
        "executable_resolved": executable,
        "status": "success",
        "failure_identities": ["tests::red"],
        "test_results": [
            {"identity": "tests::green", "status": "pass"},
            {"identity": "tests::red", "status": "fail"},
        ],
    }
    if location in {"status", "missing"}:
        receipt["status"] = "infrastructure_error"
    if location in {"status", "diagnosis"}:
        receipt["diagnosis"] = {
            "kind": "infrastructure-error",
            "child_returncode": 0,
            "infrastructure_failure": failure,
        }
    if location in {"execution", "malformed"}:
        receipt["executions"] = [
            {"infrastructure_failure": failure if location == "execution" else {}}
        ]

    rows, problems = runner.receipt_test_rows(
        [receipt], expected, {"platform": "windows", "target": "default"}
    )
    assert rows == []
    assert len(problems) == 1
    assert "not semantic or known-red evidence" in problems[0]
    assert "infrastructure" in problems[0]


def test_truth_does_not_infer_infrastructure_from_exit_125():
    runner = _load_tool("run_cargo_test_truth_exit125", "run_cargo_test_truth.py")
    assert (
        runner._receipt_infrastructure_problem(
            {
                "status": "failed",
                "returncode": 125,
                "executions": [{"returncode": 125, "infrastructure_failure": None}],
            }
        )
        is None
    )


def test_truth_runner_retains_explicit_run_identity_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = _load_tool("run_cargo_test_truth_run_identity", "run_cargo_test_truth.py")
    monkeypatch.setattr(runner, "RUNS_ROOT", tmp_path / "runs")

    first = runner.prepare_run_directory("run-123")
    runner.write_receipt(
        first / "manifest.json",
        {"schema": "molt.cargo-test-truth.v2", "run_id": "run-123"},
    )
    (first / "preserved-binary.json").write_text("{}", encoding="utf-8")

    with pytest.raises(RuntimeError, match="already exists and is immutable"):
        runner.prepare_run_directory("run-123")
    assert (first / "preserved-binary.json").exists()


def test_truth_runner_terminal_failure_retains_diagnostics_and_termination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_terminal_failure", "run_cargo_test_truth.py"
    )
    monkeypatch.setattr(runner, "RECEIPT", tmp_path / "latest.json")
    started = runner.datetime.now(runner.timezone.utc)
    phase = {
        "kind": "workspace-test",
        "argv": ["cargo", "test", "--workspace"],
        "returncode": 101,
        "termination": runner.command_termination(101),
        "evidence": {
            "path": str(tmp_path / "run" / "phases" / "workspace-test.log"),
            "bytes": 38,
            "sha256": "exact-diagnostic-hash",
            "tail": "error[E0004]: compile diagnostics\n",
            "contains_compiler_error": True,
        },
    }
    run_manifest = tmp_path / "run" / "manifest.json"

    runner.publish_terminal_failure(
        run_manifest,
        identity="run-compile-failure",
        run_dir=run_manifest.parent,
        started=started,
        context={"platform": "windows", "target": "default"},
        phases=[phase],
        problem="canonical Cargo workspace test command failed",
    )

    receipt = json.loads(run_manifest.read_text(encoding="utf-8"))
    assert receipt["status"] == "failed"
    assert receipt["phases"][-1]["termination"] == {
        "kind": "exit",
        "returncode": 101,
    }
    assert "error[E0004]" in receipt["phases"][-1]["evidence"]["tail"]
    assert json.loads(runner.RECEIPT.read_text(encoding="utf-8")) == receipt


def test_truth_runner_main_finalizes_compile_failure_before_attribution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_compile_checkpoint", "run_cargo_test_truth.py"
    )
    monkeypatch.setattr(runner, "RUNS_ROOT", tmp_path / "runs")
    monkeypatch.setattr(runner, "RECEIPT", tmp_path / "latest.json")
    monkeypatch.setattr(runner, "run_identity", lambda _started: "run-compile-failure")
    monkeypatch.setattr(runner, "host_target", lambda: "x86_64-pc-windows-msvc")
    commands = iter(
        [
            (0, "prefetch complete\n"),
            (0, json.dumps({"packages": []})),
            (
                101,
                "error[E0004]: compiler diagnostics\nerror: could not compile `molt`\n",
            ),
        ]
    )

    def fake_streamed(
        _command,
        *,
        evidence_path: Path,
        retain_cargo_artifacts=False,
        timeout_seconds=None,
    ):
        del timeout_seconds
        returncode, output = next(commands)
        evidence_path.parent.mkdir(parents=True, exist_ok=True)
        evidence_path.write_text(output, encoding="utf-8")
        return runner.StreamedCommandResult(
            returncode=returncode,
            retained_output="" if retain_cargo_artifacts else output,
            cargo_test_artifacts=(),
            evidence={
                "path": str(evidence_path),
                "bytes": len(output.encode()),
                "sha256": "test-hash",
                "tail": output[-16_384:],
                "contains_compiler_error": "could not compile" in output,
            },
        )

    monkeypatch.setattr(runner, "run_streamed", fake_streamed)

    assert runner.main() == 1

    manifest = json.loads(
        (runner.RUNS_ROOT / "run-compile-failure" / "manifest.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["status"] == "failed"
    workspace_phase = manifest["phases"][-1]
    assert workspace_phase["kind"] == "workspace-test"
    assert workspace_phase["termination"] == {"kind": "exit", "returncode": 101}
    assert "could not compile" in workspace_phase["evidence"]["tail"]


def test_streamed_workspace_output_is_exact_on_disk_and_bounded_in_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_bounded_stream", "run_cargo_test_truth.py"
    )
    artifact = (
        json.dumps(
            {
                "reason": "compiler-artifact",
                "profile": {"test": True},
                "executable": "target/test-binary",
            }
        )
        + "\n"
    )
    diagnostic = "error[E0004]: " + ("x" * 1_000_000) + "\n"

    class FakeProcess:
        stdout = iter((artifact, diagnostic))

        @staticmethod
        def wait() -> int:
            return 101

    monkeypatch.setattr(
        runner,
        "_COMMANDS",
        SimpleNamespace(start_guarded=lambda *_args, **_kwargs: FakeProcess()),
    )
    evidence_path = tmp_path / "phase.log"

    result = runner.run_streamed(
        ("cargo", "test"),
        evidence_path=evidence_path,
        retain_cargo_artifacts=True,
    )

    assert result.returncode == 101
    assert result.retained_output == ""
    assert result.cargo_test_artifacts == (
        {
            "executable": "target/test-binary",
            "package_id": None,
            "target": None,
        },
    )
    assert evidence_path.read_text(encoding="utf-8") == artifact + diagnostic
    assert result.evidence["bytes"] == len((artifact + diagnostic).encode())
    assert (
        result.evidence["sha256"]
        == hashlib.sha256((artifact + diagnostic).encode()).hexdigest()
    )
    assert len(result.evidence["tail"].encode()) <= 16_384
    assert result.evidence["contains_compiler_error"] is True
    assert len(json.dumps(result.evidence).encode()) < 20_000

    receipt = runner.terminal_failure_receipt(
        identity="large-output",
        run_dir=tmp_path,
        started=runner.datetime.now(runner.timezone.utc),
        context={"platform": "windows", "target": "default"},
        phases=[{"kind": "workspace-test", "evidence": result.evidence}],
        problem="synthetic compile failure",
    )
    assert len(json.dumps(receipt).encode()) < 20_000


def test_streamed_workspace_failure_still_waits_closes_and_publishes_partial_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_partial_stream", "run_cargo_test_truth.py"
    )
    lifecycle: list[str] = []

    class BrokenStream:
        def __iter__(self):
            yield "first complete line\n"
            raise OSError("synthetic stream failure")

        def close(self) -> None:
            lifecycle.append("close")

    class FakeProcess:
        stdout = BrokenStream()

        @staticmethod
        def wait() -> int:
            lifecycle.append("wait")
            return 2

    monkeypatch.setattr(
        runner,
        "_COMMANDS",
        SimpleNamespace(start_guarded=lambda *_args, **_kwargs: FakeProcess()),
    )
    evidence_path = tmp_path / "partial.log"
    result = runner.run_streamed(("cargo", "test"), evidence_path=evidence_path)

    assert lifecycle == ["wait", "close"]
    assert evidence_path.read_text(encoding="utf-8") == "first complete line\n"
    assert result.returncode == 2
    assert result.evidence["controller_errors"] == [
        {
            "stage": "stream",
            "type": "OSError",
            "message": "synthetic stream failure",
        }
    ]


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
        return SimpleNamespace(
            returncode=-6 if identity == "limit_one" else 0,
            stdout=(
                f"running 1 test\ntest {identity} ...\n"
                if identity == "limit_one"
                else _libtest_output(identity)
            ),
            stderr="",
        )

    monkeypatch.setattr(
        binary_runner,
        "_COMMANDS",
        SimpleNamespace(run=fake_run),
    )

    returncode, diagnosis, _executions = binary_runner.run_resource_tests(
        "resource_enforcement-hash",
        [],
        total_timeout_seconds=60.0,
        deadline=time.monotonic() + 60.0,
    )
    assert returncode == 1
    assert diagnosis["kind"] == "resource-process-isolation"
    assert [call[call.index("--exact") + 1] for call in calls[1:]] == [
        "limit_one",
        "limit_two",
    ]
    assert all("--test-threads=1" in call for call in calls[1:])
    assert "test limit_one ... FAILED" in capsys.readouterr().out


def test_resource_timeout_remains_structural_and_not_known_red(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_resource_structural", "cargo_test_binary_runner.py"
    )
    discovery = binary_runner.BinaryExecution(
        argv=("resource_enforcement", "--list"),
        returncode=0,
        stdout="candidate: test\n",
        stderr="",
        elapsed_seconds=0.01,
        timed_out=False,
        peak_process_rss_kb=1024,
        peak_tree_rss_kb=2048,
    )
    timed_out = binary_runner.BinaryExecution(
        argv=("resource_enforcement", "--exact", "candidate"),
        returncode=124,
        stdout="running 1 test\ntest candidate ...\n",
        stderr="",
        elapsed_seconds=10.0,
        timed_out=True,
        peak_process_rss_kb=1024,
        peak_tree_rss_kb=2048,
    )
    monkeypatch.setattr(
        binary_runner,
        "listed_tests",
        lambda *_args, **_kwargs: (["candidate"], discovery),
    )
    monkeypatch.setattr(
        binary_runner,
        "execute_binary",
        lambda argv, _timeout: binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=timed_out.returncode,
            stdout=timed_out.stdout,
            stderr=timed_out.stderr,
            elapsed_seconds=timed_out.elapsed_seconds,
            timed_out=timed_out.timed_out,
            peak_process_rss_kb=timed_out.peak_process_rss_kb,
            peak_tree_rss_kb=timed_out.peak_tree_rss_kb,
        ),
    )

    returncode, diagnosis, _executions = binary_runner.run_resource_tests(
        "resource_enforcement",
        ["stale_filter", "--skip", "candidate", "--test-threads=8"],
        total_timeout_seconds=30.0,
        deadline=time.monotonic() + 30.0,
    )
    assert returncode == 1
    assert diagnosis["failed_tests"] == []
    [failure] = diagnosis["structural_failures"]
    assert failure["identity"] == "candidate"
    assert failure["termination"] == {"kind": "timeout", "returncode": 124}
    assert failure["libtest"]["pending_tests"] == ["candidate"]
    assert failure["libtest"]["complete"] is False
    assert failure["libtest"]["observations"] == []
    assert "test candidate ... FAILED" not in capsys.readouterr().out
    exact_argv = list(_executions[-1].argv)
    assert "stale_filter" not in exact_argv
    assert "--skip" not in exact_argv
    assert exact_argv.count("--test-threads=1") == 1


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


def test_libtest_failure_is_normalized_by_shared_authority() -> None:
    from tools.libtest_results import parse_libtest

    report = parse_libtest(
        StringIO(
            _libtest_output("env_var_init_installs_tracker - should panic", "FAILED")
        ),
        ("fixture",),
    )
    assert report.complete
    assert report.rows() == [
        {"identity": "env_var_init_installs_tracker", "status": "fail"}
    ]


def test_binary_runner_preserves_posix_signal_and_windows_fast_fail() -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_termination", "cargo_test_binary_runner.py"
    )

    assert binary_runner.termination_payload(-6, timed_out=False) == {
        "kind": "signal",
        "returncode": -6,
        "signal": 6,
        "name": "SIGABRT",
    }
    assert binary_runner.termination_payload(0xC0000409, timed_out=False) == {
        "kind": "windows-exception",
        "returncode": 0xC0000409,
        "code": "0xC0000409",
        "raw_code": 0xC0000409,
        "name": "STATUS_STACK_BUFFER_OVERRUN_OR_FAST_FAIL",
        "severity": "error",
        "facility": 0,
    }
    assert (
        binary_runner.termination_payload(0xC00000FD, timed_out=False)["name"]
        == "STATUS_STACK_OVERFLOW"
    )
    unknown = binary_runner.termination_payload(0xC1234567, timed_out=False)
    assert unknown["kind"] == "windows-exception"
    assert unknown["code"] == "0xC1234567"
    assert unknown["raw_code"] == 0xC1234567


def test_binary_runner_exact_diagnosis_canonicalizes_inherited_libtest_controls() -> (
    None
):
    binary_runner = _load_tool(
        "cargo_test_binary_runner_exact_args", "cargo_test_binary_runner.py"
    )
    argv = binary_runner._exact_argv(
        "runtime-test",
        [
            "--exact",
            "old_test",
            "stale_positional_filter",
            "--skip",
            "new_test",
            "--skip=another_test",
            "--ignored",
            "--shuffle",
            "--shuffle-seed",
            "42",
            "--test-threads=8",
            "--nocapture",
            "--format",
            "terse",
        ],
        "new_test",
        allowed_tests={"new_test"},
    )

    assert argv == [
        "runtime-test",
        "--format",
        "terse",
        "--ignored",
        "--exact",
        "new_test",
        "--test-threads=1",
        "--nocapture",
    ]
    default_argv = binary_runner._exact_argv(
        "runtime-test",
        ["default_filter", "--skip", "other"],
        "new_test",
        allowed_tests={"new_test"},
    )
    assert "--ignored" not in default_argv
    assert "--include-ignored" not in default_argv
    assert "default_filter" not in default_argv
    assert "other" not in default_argv

    list_argv = binary_runner._canonical_list_args(
        ["filter", "--format", "pretty", "--test-threads", "8", "--ignored"]
    )
    assert list_argv == ["--ignored", "filter"]
    filtered = ["module::selected", "--skip", "module::selected::excluded"]
    subset = binary_runner._subset_argv(
        "runtime-test",
        filtered,
        ["module::selected::one", "module::selected::two"],
        ["module::selected::one"],
    )
    assert subset is not None
    assert "module::selected" in subset
    assert "--skip" in subset
    serial = binary_runner._serial_argv("runtime-test", filtered)
    assert "module::selected" in serial
    with pytest.raises(RuntimeError, match="escaped listed selection domain"):
        binary_runner._exact_argv(
            "runtime-test",
            filtered,
            "outside::candidate",
            allowed_tests={"module::selected::one"},
        )


def test_binary_runner_harness_error_is_not_an_isolated_failure() -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_harness_error", "cargo_test_binary_runner.py"
    )
    harness_error = binary_runner.BinaryExecution(
        argv=("runtime-test", "--exact", "candidate"),
        returncode=101,
        stdout="",
        stderr="Option test-threads given more than once\n",
        elapsed_seconds=0.01,
        timed_out=False,
        peak_process_rss_kb=1024,
        peak_tree_rss_kb=2048,
    )

    assert binary_runner._exact_reproduction_kind(harness_error, "candidate") is None
    assert (
        binary_runner._confirmed_failure_identities(
            [], {"kind": "exact-runner-failure", "identity": "candidate"}
        )
        == set()
    )
    startup_crash = binary_runner.BinaryExecution(
        argv=("runtime-test", "--exact", "candidate"),
        returncode=-6,
        stdout="static initialization failed\n",
        stderr="",
        elapsed_seconds=0.01,
        timed_out=False,
        peak_process_rss_kb=1024,
        peak_tree_rss_kb=2048,
    )
    assert binary_runner._exact_reproduction_kind(startup_crash, "candidate") is None
    started_crash = binary_runner.BinaryExecution(
        argv=(*startup_crash.argv, "--test-threads=1"),
        returncode=-6,
        stdout="running 1 test\ntest candidate ...\n",
        stderr="",
        elapsed_seconds=0.01,
        timed_out=False,
        peak_process_rss_kb=1024,
        peak_tree_rss_kb=2048,
    )
    assert (
        binary_runner._exact_reproduction_kind(started_crash, "candidate")
        == "isolated-test"
    )


def test_binary_runner_timeout_is_bounded_and_keeps_captured_output(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_timeout", "cargo_test_binary_runner.py"
    )

    def fake_run(argv, **kwargs):
        error = subprocess.TimeoutExpired(
            argv,
            kwargs["timeout"],
            output="started\n",
            stderr="blocked\n",
        )
        setattr(
            error,
            "guarded_result",
            SimpleNamespace(
                elapsed_s=0.01,
                peak=SimpleNamespace(rss_kb=1024),
                peak_total=SimpleNamespace(rss_kb=2048),
            ),
        )
        raise error

    monkeypatch.setattr(binary_runner, "_COMMANDS", SimpleNamespace(run=fake_run))
    execution = binary_runner.execute_binary(["hung-test"], 0.01)

    assert execution.timed_out
    assert execution.returncode == 124
    assert execution.stdout == "started\n"
    assert execution.stderr == "blocked\n"
    assert execution.peak_process_rss_kb == 1024
    assert execution.peak_tree_rss_kb == 2048
    assert execution.termination == {"kind": "timeout", "returncode": 124}


def test_binary_runner_records_normal_reported_failure_identities(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_reported_failure", "cargo_test_binary_runner.py"
    )

    def fake_execute(argv: list[str], _timeout: float):
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=101,
            stdout=_libtest_output("module::tests::ordinary_failure", "FAILED"),
            stderr="",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    monkeypatch.setattr(binary_runner, "execute_binary", fake_execute)
    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "30",
                "--receipt-dir",
                str(tmp_path),
                "--run-id",
                "run-123",
                "--",
                "molt_runtime-hash",
            ]
        )
        == 1
    )
    [receipt_path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["run_id"] == "run-123"
    assert receipt["reported_failures"] == ["module::tests::ordinary_failure"]
    assert receipt["failure_identities"] == ["module::tests::ordinary_failure"]


def test_binary_runner_keeps_structural_candidates_out_of_known_red_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_structural_candidate",
        "cargo_test_binary_runner.py",
    )
    baseline = binary_runner.BinaryExecution(
        argv=("molt_runtime-hash",),
        returncode=-6,
        stdout="unattributed abort\n",
        stderr="",
        elapsed_seconds=0.01,
        timed_out=False,
        peak_process_rss_kb=1024,
        peak_tree_rss_kb=2048,
    )
    monkeypatch.setattr(
        binary_runner, "execute_binary", lambda _argv, _timeout: baseline
    )
    monkeypatch.setattr(
        binary_runner,
        "diagnose_abnormal_exit",
        lambda *_args, **_kwargs: (
            {
                "kind": "prior-state-interaction",
                "identity": "candidate_only",
                "candidate_tests": ["candidate_only", "sibling"],
            },
            [],
        ),
    )

    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "30",
                "--receipt-dir",
                str(tmp_path),
                "--",
                "molt_runtime-hash",
            ]
        )
        == 1
    )
    assert "test candidate_only ... FAILED" not in capsys.readouterr().out
    [receipt_path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["failure_identities"] == []
    assert receipt["diagnosis"]["candidate_tests"] == [
        "candidate_only",
        "sibling",
    ]


def test_binary_runner_receipts_are_append_only_per_invocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_append_only", "cargo_test_binary_runner.py"
    )

    def success(argv: list[str], _timeout: float):
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=0,
            stdout=_libtest_output("same::identity"),
            stderr="",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    monkeypatch.setattr(binary_runner, "execute_binary", success)
    args = [
        "--timeout-seconds",
        "30",
        "--receipt-dir",
        str(tmp_path),
        "--run-id",
        "run-append",
        "--",
        "molt_runtime-hash",
    ]
    assert binary_runner.main(args) == 0
    assert binary_runner.main(args) == 0
    receipts = [
        json.loads(path.read_text(encoding="utf-8")) for path in tmp_path.glob("*.json")
    ]
    assert len(receipts) == 2
    assert len({receipt["invocation_id"] for receipt in receipts}) == 2
    assert all(
        receipt["test_results"] == [{"identity": "same::identity", "status": "pass"}]
        for receipt in receipts
    )


def test_binary_runner_immutable_publish_refuses_collision(tmp_path: Path) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_immutable_collision", "cargo_test_binary_runner.py"
    )
    path = tmp_path / "receipt.json"
    binary_runner.write_receipt(path, {"generation": 1})
    with pytest.raises(FileExistsError):
        binary_runner.write_receipt(path, {"generation": 2})
    assert json.loads(path.read_text(encoding="utf-8")) == {"generation": 1}


def test_binary_runner_reduces_abort_to_exact_test_and_writes_receipt(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_reduction", "cargo_test_binary_runner.py"
    )

    def result(argv: list[str], returncode: int, stdout: str = ""):
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=returncode,
            stdout=stdout,
            stderr="fatal output\n" if returncode else "",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    def fake_execute(argv: list[str], _timeout: float):
        if "--list" in argv:
            return result(argv, 0, "safe_test: test\nabort_test: test\n")
        if "--exact" in argv:
            assert argv[argv.index("--exact") + 1] == "abort_test"
            return result(argv, -6, "running 1 test\ntest abort_test ...\n")
        skipped = {
            argv[index + 1]
            for index, value in enumerate(argv[:-1])
            if value == "--skip"
        }
        if skipped:
            return (
                result(argv, -6)
                if "safe_test" in skipped
                else result(argv, 0, _libtest_output("safe_test"))
            )
        return result(argv, -6, "unattributed abort\n")

    monkeypatch.setattr(binary_runner, "execute_binary", fake_execute)
    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "30",
                "--receipt-dir",
                str(tmp_path),
                "--",
                "molt_runtime-hash",
            ]
        )
        == 1
    )

    output = capsys.readouterr().out
    assert "test abort_test ... FAILED" in output
    [receipt_path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["diagnosis"]["kind"] == "isolated-test"
    assert receipt["diagnosis"]["identity"] == "abort_test"
    assert receipt["executions"][0]["termination"] == {
        "kind": "signal",
        "returncode": -6,
        "signal": 6,
        "name": "SIGABRT",
    }
    assert receipt["baseline_termination"] == receipt["executions"][0]["termination"]
    assert receipt["executions"][0]["stdout_tail"] == "unattributed abort\n"
    assert receipt["executions"][0]["stderr_tail"] == "fatal output\n"
    assert "stdout" not in receipt["executions"][0]
    assert "stderr" not in receipt["executions"][0]


@pytest.mark.parametrize(
    "stage", ["baseline", "discovery", "partition", "serial", "exact", "resource"]
)
def test_binary_runner_stops_every_diagnostic_phase_on_guard_infrastructure(
    tmp_path, monkeypatch, capsys, stage
):
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure
    from tools.proof_queue_pkg import diagnostic_engine

    binary_runner = _load_tool(
        "cargo_test_binary_runner_infrastructure", "cargo_test_binary_runner.py"
    )
    failure = GuardInfrastructureFailure(
        "temporary_artifact_custody", ("receipt unavailable",)
    )
    observed = []
    injected = False

    def execute(argv, _timeout):
        nonlocal injected
        assert not injected, (
            "infrastructure failure must stop diagnosis without another execution"
        )
        if "--list" in argv:
            current = "discovery"
            stdout = "first: test\n" + ("second: test\n" if stage != "exact" else "")
        elif "--exact" in argv:
            current = "resource" if stage == "resource" else "exact"
            stdout = _libtest_output("first", "FAILED")
        elif "--skip" in argv:
            current = "partition"
            stdout = ""
        elif observed:
            current = "serial"
            stdout = "running 1 test\ntest first ...\n"
        else:
            current = "baseline"
            stdout = ""
        observed.append(current)
        injected = current == stage
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=125 if injected else 0 if current == "discovery" else -6,
            stdout=stdout,
            stderr="",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1,
            peak_tree_rss_kb=1,
            child_returncode=0 if injected else None,
            infrastructure_failure=failure if injected else None,
        )

    monkeypatch.setattr(binary_runner, "execute_binary", execute)
    if stage == "serial":
        monkeypatch.setattr(binary_runner, "_subset_argv", lambda *_args: None)
    executable = (
        "resource_enforcement-hash" if stage == "resource" else "molt_runtime-hash"
    )
    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "30",
                "--receipt-dir",
                str(tmp_path),
                "--",
                executable,
            ]
        )
        == 2
    )
    assert injected and observed[-1] == stage
    [path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(path.read_text(encoding="utf-8"))
    assert receipt["status"] == "infrastructure_error"
    assert receipt["diagnosis"]["kind"] == "infrastructure-error"
    assert receipt["reported_failures"] == receipt["failure_identities"] == []
    assert receipt["executions"][-1]["child_returncode"] == 0
    assert receipt["executions"][-1]["infrastructure_failure"] == failure.json_payload()
    output = capsys.readouterr().out
    prefix = "cargo-test-binary-runner: infrastructure-outcome="
    reports = [
        line[len(prefix) :] for line in output.splitlines() if line.startswith(prefix)
    ]
    assert len(reports) == 1
    assert json.loads(reports[0]) == receipt["diagnosis"]
    if stage == "resource":
        assert "test first ... FAILED" in output
    log = tmp_path / "queue.log"
    log.write_text(
        output + "error: test failed, to rerun pass `--lib`\n", encoding="utf-8"
    )
    summary = tmp_path / "healthy-outer-guard.json"
    summary.write_text(json.dumps({"infrastructure_failure": None}), encoding="utf-8")
    diagnostics = diagnostic_engine._run_diagnostics(
        {
            "status": "failed",
            "summary_json": str(summary) if stage == "baseline" else None,
            "log_path": str(log),
        }
    )
    assert len(diagnostics) == 1
    assert diagnostics[0]["signal_id"] == "guard-infrastructure-error"
    assert diagnostics[0]["observation_state"] == "reported"
    assert "reported" in diagnostics[0]["summary"]
    assert "not authenticated" in diagnostics[0]["next_action"]


@pytest.mark.parametrize(
    "output",
    [
        "exit_code=125\n",
        "memory_guard: infrastructure_error during temporary_artifact_custody\n",
        '{"kind":"infrastructure-error","child_returncode":0}\n',
    ],
)
def test_queue_never_infers_nested_infrastructure_from_generic_output(tmp_path, output):
    from tools.proof_queue_pkg.diagnostic_evidence import (
        _guard_infrastructure_diagnostic,
    )

    log = tmp_path / "queue.log"
    log.write_text(output, encoding="utf-8")
    assert (
        _guard_infrastructure_diagnostic(
            {"status": "failed", "summary_json": None, "log_path": str(log)},
            output,
        )
        is None
    )


@pytest.mark.parametrize("invalid", ["phase", "child", "duplicate", "missing"])
def test_queue_rejects_malformed_typed_nested_infrastructure_reports(tmp_path, invalid):
    from tools.proof_queue_pkg.diagnostic_evidence import (
        _guard_infrastructure_diagnostic,
    )

    payload = {
        "kind": "infrastructure-error",
        "child_returncode": 0,
        "infrastructure_failure": {
            "phase": "temporary_artifact_custody",
            "details": ["receipt unavailable"],
        },
    }
    if invalid == "phase":
        payload["infrastructure_failure"]["phase"] = "unknown"
    elif invalid == "child":
        payload["child_returncode"] = True
    elif invalid == "missing":
        payload["infrastructure_failure"] = None
    encoded = json.dumps(payload)
    if invalid == "duplicate":
        encoded = encoded[:-1] + ', "child_returncode": 7}'
    log = tmp_path / "queue.log"
    log.write_text(
        "cargo-test-binary-runner: infrastructure-outcome=" + encoded + "\n",
        encoding="utf-8",
    )
    diagnostic = _guard_infrastructure_diagnostic(
        {"status": "failed", "summary_json": None, "log_path": str(log)},
        log.read_text(encoding="utf-8"),
    )
    assert diagnostic is not None
    assert diagnostic["observation_state"] == "reported"
    assert "Malformed reported Cargo infrastructure outcome" in diagnostic["evidence"]


def test_binary_runner_preserves_guard_result_without_reclassifying_child(
    tmp_path, monkeypatch
):
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure
    from types import SimpleNamespace

    binary_runner = _load_tool(
        "cargo_test_binary_runner_guard_fields", "cargo_test_binary_runner.py"
    )
    failure = GuardInfrastructureFailure(
        "temporary_artifact_custody", ("receipt unavailable",)
    )
    monkeypatch.setattr(binary_runner, "_ACTIVE_EVIDENCE_DIR", tmp_path)
    monkeypatch.setattr(
        binary_runner,
        "_COMMANDS",
        SimpleNamespace(
            run=lambda *_args, **_kwargs: SimpleNamespace(
                returncode=125,
                stdout="ok",
                stderr="",
                child_returncode=0,
                infrastructure_failure=failure,
            )
        ),
    )
    result = binary_runner.execute_binary(["fixture"], 1)
    assert result.child_returncode == 0
    assert result.infrastructure_failure is failure
    assert not result.succeeded
    assert result.receipt()["infrastructure_failure"] == failure.json_payload()


def test_binary_runner_uses_bounded_serial_attribution_when_skip_argv_is_too_long(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_command_limit", "cargo_test_binary_runner.py"
    )
    culprit = "lifecycle::tests::shutdown_culprit"
    tests = [f"module_{index}::tests::{('x' * 80)}" for index in range(4)]
    tests.append(culprit)
    calls: list[list[str]] = []
    monkeypatch.setattr(binary_runner, "MAX_DIAGNOSTIC_COMMAND_CHARS", 120)

    def result(argv: list[str], returncode: int, stdout: str = ""):
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=returncode,
            stdout=stdout,
            stderr="fatal output\n" if returncode else "",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    def fake_execute(argv: list[str], _timeout: float):
        calls.append(list(argv))
        if "--list" in argv:
            return result(argv, 0, "".join(f"{identity}: test\n" for identity in tests))
        if "--exact" in argv:
            assert argv[argv.index("--exact") + 1] == culprit
            return result(argv, 0, _libtest_output(culprit))
        assert "--test-threads=1" in argv
        return result(argv, -6, f"running 1 test\ntest {culprit} ... ")

    monkeypatch.setattr(binary_runner, "execute_binary", fake_execute)
    diagnosis, executions = binary_runner.diagnose_abnormal_exit(
        "molt_runtime-hash",
        [],
        total_timeout_seconds=30.0,
        deadline=time.monotonic() + 30.0,
    )

    assert diagnosis["kind"] == "prior-state-interaction"
    assert diagnosis["identity"] == culprit
    assert len(executions) == 3
    assert not any("--skip" in call for call in calls)


def test_binary_runner_diagnostics_share_one_absolute_binary_deadline(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_deadline", "cargo_test_binary_runner.py"
    )
    tests = [f"module::tests::test_{index:02d}" for index in range(64)]
    clock = [0.0]
    requested_timeouts: list[float] = []

    def result(argv: list[str], returncode: int, stdout: str = ""):
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=returncode,
            stdout=stdout,
            stderr="fatal output\n" if returncode else "",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    def fake_execute(argv: list[str], timeout: float):
        requested_timeouts.append(timeout)
        clock[0] += timeout
        if "--list" in argv:
            return result(argv, 0, "".join(f"{identity}: test\n" for identity in tests))
        return result(argv, -6)

    monkeypatch.setattr(binary_runner.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(binary_runner, "execute_binary", fake_execute)
    diagnosis, executions = binary_runner.diagnose_abnormal_exit(
        "molt_runtime-hash",
        [],
        total_timeout_seconds=8.0,
        deadline=8.0,
    )

    assert diagnosis["kind"] == "budget-exhausted"
    assert len(executions) == 7
    assert sum(requested_timeouts) <= 8.0


def test_binary_runner_partition_timeout_remains_unattributed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_partition_timeout", "cargo_test_binary_runner.py"
    )
    tests = ["module::tests::slow", "module::tests::other"]

    def result(argv: list[str], *, timed_out: bool, stdout: str = ""):
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=124 if timed_out else 0,
            stdout=stdout,
            stderr="",
            elapsed_seconds=5.0 if timed_out else 0.01,
            timed_out=timed_out,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    def fake_execute(argv: list[str], _timeout: float):
        if "--list" in argv:
            return result(
                argv,
                timed_out=False,
                stdout="".join(f"{identity}: test\n" for identity in tests),
            )
        return result(argv, timed_out=True)

    monkeypatch.setattr(binary_runner, "execute_binary", fake_execute)
    diagnosis, executions = binary_runner.diagnose_abnormal_exit(
        "molt_runtime-hash",
        [],
        total_timeout_seconds=60.0,
        deadline=time.monotonic() + 60.0,
    )

    assert diagnosis["kind"] == "diagnostic-timeout"
    assert "identity" not in diagnosis
    assert len(executions) == 2


def test_binary_runner_reserves_attribution_inside_one_binary_deadline(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_baseline_reserve", "cargo_test_binary_runner.py"
    )
    clock = [0.0]
    baseline_timeouts: list[float] = []

    def fake_execute(argv: list[str], timeout: float):
        baseline_timeouts.append(timeout)
        clock[0] += timeout
        return binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=124,
            stdout="started\n",
            stderr="blocked\n",
            elapsed_seconds=timeout,
            timed_out=True,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        )

    def fake_diagnose(
        executable: str,
        inherited_args: list[str],
        *,
        total_timeout_seconds: float,
        deadline: float,
    ):
        assert executable == "molt-runtime-test"
        assert inherited_args == []
        assert total_timeout_seconds == 10.0
        assert deadline == 10.0
        assert binary_runner._remaining(deadline) == pytest.approx(2.0)
        return {"kind": "budget-reserved"}, []

    monkeypatch.setattr(binary_runner.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(binary_runner, "execute_binary", fake_execute)
    monkeypatch.setattr(binary_runner, "diagnose_abnormal_exit", fake_diagnose)

    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "10",
                "--receipt-dir",
                str(tmp_path),
                "--",
                "molt-runtime-test",
            ]
        )
        == 1
    )
    assert baseline_timeouts == [pytest.approx(8.0)]
    [receipt_path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["timeout_seconds"] == 10.0
    assert receipt["baseline_timeout_seconds"] == pytest.approx(8.0)
    assert receipt["diagnostic_reserve_seconds"] == pytest.approx(2.0)


def test_truth_runner_loads_only_typed_atomic_binary_receipts(tmp_path: Path) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_binary_receipts", "run_cargo_test_truth.py"
    )
    receipt = tmp_path / "one.json"
    receipt.write_text(
        json.dumps(
            {
                "schema": "molt.cargo-test-binary.v2",
                "invocation_id": "invocation-one",
                "executable": "one",
            }
        ),
        encoding="utf-8",
    )

    assert runner.load_binary_receipts(tmp_path) == [
        {
            "schema": "molt.cargo-test-binary.v2",
            "invocation_id": "invocation-one",
            "executable": "one",
        }
    ]

    receipt.write_text(json.dumps({"schema": "wrong"}), encoding="utf-8")
    with pytest.raises(RuntimeError, match="invalid Cargo test binary receipt schema"):
        runner.load_binary_receipts(tmp_path)


def test_truth_runner_cannot_erase_infrastructure_status_with_duplicate_json_keys(
    tmp_path,
):
    runner = _load_tool("run_cargo_test_truth_exact_outcome", "run_cargo_test_truth.py")
    (tmp_path / "one.json").write_text(
        '{"schema":"molt.cargo-test-binary.v2","invocation_id":"one",'
        '"status":"infrastructure_error","status":"success"}',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="duplicate JSON key"):
        runner.load_binary_receipts(tmp_path)


def test_truth_runner_revalidates_exact_executable_bytes_at_collection(
    tmp_path: Path,
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_executable_identity", "run_cargo_test_truth.py"
    )
    executable = tmp_path / "molt-runtime-test"
    executable.write_bytes(b"original executable bytes")
    size, digest = runner._file_identity(executable)
    receipt = tmp_path / "one.json"
    receipt.write_text(
        json.dumps(
            {
                "schema": "molt.cargo-test-binary.v2",
                "invocation_id": "invocation-one",
                "run_id": "exact-run",
                "executable_resolved": str(executable),
                "executable_size": size,
                "executable_sha256": digest,
            }
        ),
        encoding="utf-8",
    )

    assert len(runner.load_binary_receipts(tmp_path, expected_run_id="exact-run")) == 1
    executable.write_bytes(b"tampered executable bytes")
    with pytest.raises(RuntimeError, match="changed after receipt publication"):
        runner.load_binary_receipts(tmp_path, expected_run_id="exact-run")


def test_truth_runner_rejects_binary_receipt_from_different_source_snapshot(
    tmp_path: Path,
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_source_identity", "run_cargo_test_truth.py"
    )
    executable = tmp_path / "molt-runtime-test"
    executable.write_bytes(b"executable")
    size, digest = runner._file_identity(executable)
    expected = {"schema": "molt.git-source.v1", "head": "expected"}
    (tmp_path / "one.json").write_text(
        json.dumps(
            {
                "schema": "molt.cargo-test-binary.v2",
                "invocation_id": "invocation-one",
                "run_id": "exact-run",
                "source_identity": {
                    "schema": "molt.git-source.v1",
                    "head": "different",
                },
                "executable_resolved": str(executable),
                "executable_size": size,
                "executable_sha256": digest,
            }
        ),
        encoding="utf-8",
    )

    with pytest.raises(RuntimeError, match="escaped source custody"):
        runner.load_binary_receipts(
            tmp_path,
            expected_run_id="exact-run",
            expected_source_identity=expected,
        )


def test_binary_runner_carries_exact_source_identity_into_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary_runner = _load_tool(
        "cargo_test_binary_runner_source_identity", "cargo_test_binary_runner.py"
    )
    source_identity = {"schema": "molt.git-source.v1", "head": "abc123"}
    monkeypatch.setattr(
        binary_runner,
        "execute_binary",
        lambda argv, _timeout: binary_runner.BinaryExecution(
            argv=tuple(argv),
            returncode=0,
            stdout=_libtest_output("exact::source"),
            stderr="",
            elapsed_seconds=0.01,
            timed_out=False,
            peak_process_rss_kb=1024,
            peak_tree_rss_kb=2048,
        ),
    )
    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "30",
                "--receipt-dir",
                str(tmp_path),
                "--run-id",
                "exact-run",
                "--source-identity-json",
                json.dumps(source_identity),
                "--",
                "molt_runtime-hash",
            ]
        )
        == 0
    )
    [receipt_path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["source_identity"] == source_identity


@pytest.mark.parametrize("threads", [("--test-threads=1",), ("--test-threads", "1")])
def test_libtest_serial_split_stdout_accounts_for_every_test(threads):
    from tools.libtest_results import parse_libtest

    output = (
        "running 10 tests\n"
        + "".join(
            f'test kernel::{index} ... {{"accounting_valid":true}}\n{{"receipt":{index}}}\nok\n'
            if index < 7
            else f"test kernel::{index} ... ok\n"
            for index in range(10)
        )
        + "test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 27.59s\n"
    )
    report = parse_libtest(
        StringIO(output), ("kernel_closure", "--nocapture", *threads)
    )
    assert report.complete and report.issues == () and report.pending == ()
    assert report.rows() == [
        {"identity": f"kernel::{i}", "status": "pass"} for i in range(10)
    ]


def test_libtest_ignored_filtering_and_should_panic_have_one_identity_authority():
    from tools.libtest_results import parse_libtest

    output = (
        "running 3 tests\ntest first - should panic ... panic diagnostic\nok\n"
        "test skipped ... ignored, requires network\ntest last ... ok\n"
        "test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 8 filtered out; finished in 0.01s\n"
    )
    report = parse_libtest(StringIO(output), ("fixture", "--test-threads=1"))
    assert report.complete
    assert report.rows() == [
        {"identity": "first", "status": "pass"},
        {"identity": "skipped", "status": "ignored"},
        {"identity": "last", "status": "pass"},
    ]
    assert report.summary["filtered_out"] == 8


@pytest.mark.parametrize(
    "argv",
    [
        ("fixture",),
        ("fixture", "--test-threads=2"),
        ("fixture", "--test-threads=1", "--test-threads=1"),
        ("fixture", "--skip", "--test-threads=1"),
    ],
)
def test_libtest_does_not_guess_parallel_or_ambiguous_serial_output(argv):
    from tools.libtest_results import parse_libtest

    output = _libtest_output("split").replace("... ok", '... {"data":1}\nok')
    report = parse_libtest(StringIO(output), argv)
    assert not report.complete and report.issues and report.rows() == []


@pytest.mark.parametrize(
    "noise",
    [
        "log says ok\n",
        "prefix test fake ... FAILED\n",
        '{"message":"test fake ... ok"}\n',
        "ok is not a result\n",
    ],
)
def test_libtest_arbitrary_non_protocol_output_is_not_a_result(noise):
    from tools.libtest_results import parse_libtest

    output = _libtest_output("real").replace("... ok", "... " + noise + "ok")
    report = parse_libtest(StringIO(output), ("fixture", "--test-threads=1"))
    assert report.complete
    assert report.rows() == [{"identity": "real", "status": "pass"}]


@pytest.mark.parametrize(
    "replacement",
    [
        "... ok\nok",
        "... output\nok\nFAILED",
        "... output\ntest forged ... ok\nok",
        "... ok trailing junk",
    ],
)
def test_libtest_result_like_noise_never_silently_becomes_success(replacement):
    from tools.libtest_results import parse_libtest

    report = parse_libtest(
        StringIO(_libtest_output("real").replace("... ok", replacement)),
        ("fixture", "--test-threads=1"),
    )
    assert not report.complete and report.issues and report.rows() == []


@pytest.mark.parametrize(
    "old,new",
    [
        ("1 passed", "2 passed"),
        ("0 failed", "1 failed"),
        ("0 ignored", "1 ignored"),
        ("running 1 test", "running 2 tests"),
        ("0 measured", "1 measured"),
        ("test result: ok", "test result: FAILED"),
    ],
)
def test_libtest_summary_disagreement_is_not_semantic_evidence(old, new):
    from tools.libtest_results import parse_libtest

    report = parse_libtest(
        StringIO(_libtest_output("real").replace(old, new)), ("fixture",)
    )
    assert not report.complete and report.issues and report.rows() == []


def test_libtest_empty_run_is_complete_but_missing_summary_is_not():
    from tools.libtest_results import parse_libtest

    output = "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.00s\n"
    assert parse_libtest(StringIO(output), ("fixture",)).complete
    partial = parse_libtest(
        StringIO('running 2 tests\ntest done ... ok\ntest blocked ... {"live":true}\n'),
        ("fixture", "--test-threads=1"),
    )
    assert not partial.complete and not partial.issues
    assert partial.pending == ("blocked",)
    assert partial.rows() == [{"identity": "done", "status": "pass"}]


def test_libtest_bounded_reader_rejects_oversized_noise_without_full_line_reads():
    from tools.libtest_results import MAX_LINE_CHARS, parse_libtest

    class BoundedReader(StringIO):
        def readline(self, size=-1):
            assert 0 < size <= MAX_LINE_CHARS + 1
            return super().readline(size)

    output = _libtest_output("real").replace(
        "... ok", "... " + "x" * (3 * MAX_LINE_CHARS) + "\nok"
    )
    report = parse_libtest(BoundedReader(output), ("fixture", "--test-threads=1"))
    assert "line-limit-exceeded" in report.issues and report.rows() == []


def test_binary_runner_reads_complete_stdout_not_tail_or_stderr(tmp_path):
    binary_runner = _load_tool(
        "cargo_test_binary_runner_stdout_authority", "cargo_test_binary_runner.py"
    )
    stdout = tmp_path / "stdout.log"
    stderr = tmp_path / "stderr.log"
    stdout.write_text(_libtest_output("real"), encoding="utf-8")
    stderr.write_text(_libtest_output("fake", "FAILED"), encoding="utf-8")
    execution = binary_runner.BinaryExecution(
        ("fixture",),
        0,
        "truncated tail",
        "test fake ... FAILED",
        0.01,
        False,
        None,
        None,
        stdout,
        stderr,
    )
    assert binary_runner._structured_results_for_execution(execution) == [
        {"identity": "real", "status": "pass"}
    ]
    assert binary_runner._test_results_for_execution(execution, "FAILED") == []
    assert execution.receipt()["libtest"]["complete"]


@pytest.mark.parametrize(
    "output",
    [
        "",
        "test missing_banner ... ok\n",
        "running 1 test\ntest missing_summary ... ok\n",
        _libtest_output("one").replace("1 passed", "2 passed"),
    ],
)
def test_binary_runner_exit_zero_does_not_bless_missing_accounting(
    tmp_path, monkeypatch, output
):
    binary_runner = _load_tool(
        "cargo_test_binary_runner_incomplete", "cargo_test_binary_runner.py"
    )
    monkeypatch.setattr(
        binary_runner,
        "execute_binary",
        lambda argv, _timeout: binary_runner.BinaryExecution(
            tuple(argv), 0, output, "", 0.01, False, None, None
        ),
    )
    monkeypatch.setattr(
        binary_runner,
        "diagnose_abnormal_exit",
        lambda *a, **k: pytest.fail("accounting is not an abnormal-exit retry request"),
    )
    assert (
        binary_runner.main(
            ["--timeout-seconds", "30", "--receipt-dir", str(tmp_path), "--", "fixture"]
        )
        == 2
    )
    [path] = list(tmp_path.glob("*.json"))
    receipt = json.loads(path.read_text(encoding="utf-8"))
    assert receipt["schema"] == "molt.cargo-test-binary.v2"
    assert (
        receipt["status"] == "failed" and not receipt["result_accounting"]["complete"]
    )
    assert receipt["diagnosis"]["kind"] == "libtest-accounting-error"


def test_resource_exit_zero_without_exact_result_is_structural(monkeypatch):
    binary_runner = _load_tool(
        "cargo_test_binary_runner_resource_accounting", "cargo_test_binary_runner.py"
    )
    discovery = binary_runner.BinaryExecution(
        ("resource_enforcement", "--list"),
        0,
        "wanted: test\n",
        "",
        0.01,
        False,
        None,
        None,
    )
    monkeypatch.setattr(
        binary_runner, "listed_tests", lambda *a, **k: (["wanted"], discovery)
    )
    monkeypatch.setattr(
        binary_runner,
        "execute_binary",
        lambda argv, _timeout: binary_runner.BinaryExecution(
            tuple(argv), 0, "", "", 0.01, False, None, None
        ),
    )
    code, diagnosis, executions = binary_runner.run_resource_tests(
        "resource_enforcement",
        [],
        total_timeout_seconds=30,
        deadline=time.monotonic() + 30,
    )
    assert code == 1 and diagnosis["failed_tests"] == []
    assert diagnosis["structural_failures"][0]["identity"] == "wanted"
    assert (
        binary_runner._canonical_test_results(executions, resource_isolation=True) == []
    )


@pytest.mark.parametrize(
    "mutation",
    [
        "old-schema",
        "missing",
        "incomplete",
        "wrong-count",
        "wrong-declared",
        "ambiguous",
    ],
)
def test_truth_rejects_success_without_complete_v2_accounting(mutation):
    runner = _load_tool("run_cargo_test_truth_accounting", "run_cargo_test_truth.py")
    executable = str((ROOT / "target" / "fixture").resolve())
    expected = {
        runner._executable_key(executable): dict(
            package="fixture",
            target_name="fixture",
            target_kind="test",
            executable=executable,
        )
    }
    receipt = dict(
        schema="molt.cargo-test-binary.v2",
        executable=executable,
        status="success",
        test_results=[dict(identity="one", status="pass")],
        failure_identities=[],
        result_accounting=_accounting(1),
    )
    if mutation == "old-schema":
        receipt["schema"] = "molt.cargo-test-binary.v1"
    elif mutation == "missing":
        del receipt["result_accounting"]
    elif mutation == "incomplete":
        receipt["result_accounting"]["complete"] = False
    elif mutation == "wrong-count":
        receipt["result_accounting"]["observed_results"] = 2
    elif mutation == "wrong-declared":
        receipt["result_accounting"]["declared_results"] = 2
    else:
        receipt["result_accounting"]["issues"] = ["ambiguous-standalone-result"]
    rows, problems = runner.receipt_test_rows([receipt], expected, {})
    assert not rows and len(problems) == 1
    assert "not semantic or known-red evidence" in problems[0]


def test_truth_keeps_ignored_in_receipt_but_not_execution_reality():
    runner = _load_tool("run_cargo_test_truth_ignored", "run_cargo_test_truth.py")
    executable = str((ROOT / "target" / "fixture").resolve())
    expected = {
        runner._executable_key(executable): dict(
            package="fixture",
            target_name="fixture",
            target_kind="test",
            executable=executable,
        )
    }
    receipt = dict(
        schema="molt.cargo-test-binary.v2",
        executable=executable,
        status="success",
        test_results=[dict(identity="skip", status="ignored")],
        failure_identities=[],
        result_accounting=_accounting(1),
    )
    assert runner.receipt_test_rows([receipt], expected, {}) == ([], [])


@pytest.mark.parametrize("extra", ["test real ... ok\n", "running 1 test\n"])
def test_libtest_duplicate_identity_or_run_is_ambiguous(extra):
    from tools.libtest_results import parse_libtest

    output = _libtest_output("real").replace("test result:", extra + "test result:")
    report = parse_libtest(StringIO(output), ("fixture", "--test-threads=1"))
    assert report.issues and not report.complete and report.rows() == []


def test_truth_loader_rejects_old_schema_instead_of_upgrading_evidence(tmp_path):
    runner = _load_tool("run_cargo_test_truth_old_schema", "run_cargo_test_truth.py")
    historical = dict(
        schema="molt.cargo-test-binary.v1", invocation_id="old", status="success"
    )
    path = tmp_path / "old.json"
    path.write_text(json.dumps(historical), encoding="utf-8")
    before = path.read_bytes()
    with pytest.raises(RuntimeError, match="invalid Cargo test binary receipt schema"):
        runner.load_binary_receipts(tmp_path)
    assert path.read_bytes() == before


@pytest.mark.parametrize(
    "rows",
    [
        [
            {"identity": "same", "status": "pass"},
            {"identity": "same", "status": "pass"},
        ],
        [
            {"identity": "same", "status": "pass"},
            {"identity": "same", "status": "fail"},
        ],
        [{"identity": "", "status": "pass"}],
        [{"identity": "  ", "status": "pass"}],
        [{"identity": "one", "status": "unknown"}],
        ["not a result object"],
    ],
)
def test_truth_rejects_duplicate_empty_or_malformed_v2_rows(rows):
    runner = _load_tool("run_cargo_test_truth_invalid_rows", "run_cargo_test_truth.py")
    executable = str((ROOT / "target" / "fixture").resolve())
    expected = {
        runner._executable_key(executable): dict(
            package="fixture",
            target_name="fixture",
            target_kind="test",
            executable=executable,
        )
    }
    receipt = dict(
        schema="molt.cargo-test-binary.v2",
        executable=executable,
        status="success",
        test_results=rows,
        failure_identities=[],
        result_accounting=_accounting(len(rows)),
    )
    published, problems = runner.receipt_test_rows([receipt], expected, {})
    assert not published and len(problems) == 1
    assert "not semantic or known-red evidence" in problems[0]


@pytest.mark.parametrize("isolated", [False, True])
@pytest.mark.parametrize("termination", ["timeout", "signal", "unexecuted"])
def test_known_failure_cannot_mask_incomplete_binary_cohort(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, isolated: bool, termination: str
) -> None:
    binary_runner = _load_tool("binary_mixed_failure", "cargo_test_binary_runner.py")
    runner = _load_tool("truth_mixed_failure", "run_cargo_test_truth.py")
    executable = str(tmp_path / ("resource_enforcement" if isolated else "fixture"))

    def execution(argv, output, code, *, timed_out=False):
        return binary_runner.BinaryExecution(
            tuple(argv), code, output, "", 0.01, timed_out, None, None
        )

    if isolated and termination == "unexecuted":
        discovery = execution([executable, "--list"], "known: test\nblocked: test\n", 0)
        known = execution(
            [executable, "--exact", "known", "--test-threads=1"],
            _libtest_output("known", "FAILED"),
            101,
        )
        monkeypatch.setattr(
            binary_runner,
            "run_resource_tests",
            lambda *args, **kwargs: (
                1,
                {
                    "kind": "resource-isolation-timeout",
                    "failed_tests": ["known"],
                    "unexecuted_tests": ["blocked"],
                },
                [discovery, known],
            ),
        )
    else:

        def execute(argv, _timeout):
            if "--list" in argv:
                return execution(argv, "known: test\nblocked: test\n", 0)
            if "--exact" in argv and argv[argv.index("--exact") + 1] == "known":
                return execution(argv, _libtest_output("known", "FAILED"), 101)
            output = (
                "running 1 test\ntest blocked ...\n"
                if isolated
                else "running 2 tests\ntest known ... FAILED\ntest blocked ...\n"
            )
            return execution(
                argv,
                output,
                124 if termination == "timeout" else -6,
                timed_out=termination == "timeout",
            )

        monkeypatch.setattr(binary_runner, "execute_binary", execute)

    receipt_dir = tmp_path / "receipts"
    assert (
        binary_runner.main(
            [
                "--timeout-seconds",
                "30",
                "--receipt-dir",
                str(receipt_dir),
                "--",
                executable,
                "--nocapture",
                "--test-threads=1",
            ]
        )
        != 0
    )
    [path] = list(receipt_dir.glob("*.json"))
    receipt = json.loads(path.read_text(encoding="utf-8"))
    assert "known" in receipt["failure_identities"]
    assert receipt["result_accounting"]["complete"] is False
    expected = {
        runner._executable_key(executable): dict(
            package="fixture",
            target_name="fixture",
            target_kind="test",
            executable=executable,
        )
    }
    rows, problems = runner.receipt_test_rows([receipt], expected, {})
    assert rows == []
    assert len(problems) == 1 and "not semantic or known-red evidence" in problems[0]
