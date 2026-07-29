from __future__ import annotations

import importlib.util
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
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
            "failure_identities": raw_registered,
            "test_results": [
                {"identity": identity, "status": "fail"}
                for identity in raw_registered
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
            {"identity": identity, "status": "pass"}
            for identity in raw_registered
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


def test_truth_runner_prefetches_every_workspace_lock_used_by_nested_tests(
    tmp_path: Path,
) -> None:
    runner = _load_tool("run_cargo_test_truth_prefetch", "run_cargo_test_truth.py")

    assert runner.LOCKED_WORKSPACES == (
        ("root", ROOT / "Cargo.toml"),
        ("runtime", ROOT / "runtime" / "Cargo.toml"),
    )
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


def test_truth_runner_executes_every_locked_workspace_with_exact_manifest_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = _load_tool(
        "run_cargo_test_truth_workspace_execution", "run_cargo_test_truth.py"
    )
    monkeypatch.setattr(runner, "RUNS_ROOT", tmp_path / "runs")
    monkeypatch.setattr(runner, "RECEIPT", tmp_path / "latest.json")
    monkeypatch.setattr(runner, "run_identity", lambda _started: "workspace-matrix")
    monkeypatch.setattr(runner, "host_target", lambda: "x86_64-pc-windows-msvc")
    source_identity = {"schema": "molt.git-source.v1", "head": "exact"}
    monkeypatch.setattr(runner, "git_source_identity", lambda: source_identity)
    commands: list[tuple[str, ...]] = []

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
        output = json.dumps({"packages": []}) if command[1] == "metadata" else ""
        evidence_path.parent.mkdir(parents=True, exist_ok=True)
        evidence_path.write_text(output, encoding="utf-8")
        return runner.StreamedCommandResult(
            returncode=0,
            retained_output=output,
            cargo_test_artifacts=(),
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

    # Empty synthetic Cargo JSON intentionally fails coverage, after the full
    # command matrix has executed and published terminal evidence.
    assert runner.main() == 1
    for workspace, manifest in runner.LOCKED_WORKSPACES:
        matching = [command for command in commands if str(manifest) in command]
        assert [command[1] for command in matching] == ["fetch", "metadata", "test"]
        for command in matching:
            assert command[command.index("--manifest-path") + 1] == str(manifest)
        [test_command] = [command for command in matching if command[1] == "test"]
        config = test_command[test_command.index("--config") + 1]
        assert '"--run-id","workspace-matrix"' in config
        assert "molt.git-source.v1" in config
        runner_argv = json.loads(config.split("=", 1)[1])
        receipt_index = runner_argv.index("--receipt-dir") + 1
        assert runner_argv[receipt_index] == str(
            tmp_path / "runs" / "workspace-matrix" / "binaries" / workspace
        )


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
        "path+file:///molt/runtime/molt-cpython-abi#"
        "molt-lang-cpython-abi@0.1.0"
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
    assert any("structural attribution only" in problem for problem in problems)


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
    runner = _load_tool("run_cargo_test_truth_terminal_failure", "run_cargo_test_truth.py")
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
    runner = _load_tool("run_cargo_test_truth_compile_checkpoint", "run_cargo_test_truth.py")
    monkeypatch.setattr(runner, "RUNS_ROOT", tmp_path / "runs")
    monkeypatch.setattr(runner, "RECEIPT", tmp_path / "latest.json")
    monkeypatch.setattr(
        runner, "LOCKED_WORKSPACES", (("root", ROOT / "Cargo.toml"),)
    )
    monkeypatch.setattr(runner, "run_identity", lambda _started: "run-compile-failure")
    monkeypatch.setattr(runner, "host_target", lambda: "x86_64-pc-windows-msvc")
    commands = iter(
        [
            (0, "prefetch complete\n"),
            (0, json.dumps({"packages": []})),
            (101, "error[E0004]: compiler diagnostics\nerror: could not compile `molt`\n"),
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
    runner = _load_tool("run_cargo_test_truth_bounded_stream", "run_cargo_test_truth.py")
    artifact = json.dumps(
        {
            "reason": "compiler-artifact",
            "profile": {"test": True},
            "executable": "target/test-binary",
        }
    ) + "\n"
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
    assert result.evidence["sha256"] == hashlib.sha256(
        (artifact + diagnostic).encode()
    ).hexdigest()
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
            stdout=f"test {identity} ...\n" if identity == "limit_one" else "",
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
        stdout="test candidate ...\n",
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
    assert diagnosis["structural_failures"] == [
        {"identity": "candidate", "termination": {"kind": "timeout", "returncode": 124}}
    ]
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
    assert binary_runner.termination_payload(0xC00000FD, timed_out=False)[
        "name"
    ] == "STATUS_STACK_OVERFLOW"
    unknown = binary_runner.termination_payload(0xC1234567, timed_out=False)
    assert unknown["kind"] == "windows-exception"
    assert unknown["code"] == "0xC1234567"
    assert unknown["raw_code"] == 0xC1234567


def test_binary_runner_exact_diagnosis_canonicalizes_inherited_libtest_controls() -> None:
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
        argv=startup_crash.argv,
        returncode=-6,
        stdout="test candidate ...\n",
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
            stdout="test module::tests::ordinary_failure ... FAILED\n",
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
    monkeypatch.setattr(binary_runner, "execute_binary", lambda _argv, _timeout: baseline)
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
            stdout="test same::identity ... ok\n",
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
    receipts = [json.loads(path.read_text(encoding="utf-8")) for path in tmp_path.glob("*.json")]
    assert len(receipts) == 2
    assert len({receipt["invocation_id"] for receipt in receipts}) == 2
    assert all(receipt["test_results"] == [{"identity": "same::identity", "status": "pass"}] for receipt in receipts)


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
            return result(argv, -6, "test abort_test ...\n")
        skipped = {
            argv[index + 1]
            for index, value in enumerate(argv[:-1])
            if value == "--skip"
        }
        if skipped:
            return result(argv, -6 if "safe_test" in skipped else 0)
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
            return result(argv, 0, f"test {culprit} ... ok\n")
        assert "--test-threads=1" in argv
        return result(argv, -6, f"test {culprit} ... ")

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
                "schema": "molt.cargo-test-binary.v1",
                "invocation_id": "invocation-one",
                "executable": "one",
            }
        ),
        encoding="utf-8",
    )

    assert runner.load_binary_receipts(tmp_path) == [
        {
            "schema": "molt.cargo-test-binary.v1",
            "invocation_id": "invocation-one",
            "executable": "one",
        }
    ]

    receipt.write_text(json.dumps({"schema": "wrong"}), encoding="utf-8")
    with pytest.raises(RuntimeError, match="invalid Cargo test binary receipt schema"):
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
                "schema": "molt.cargo-test-binary.v1",
                "invocation_id": "invocation-one",
                "run_id": "exact-run",
                "executable_resolved": str(executable),
                "executable_size": size,
                "executable_sha256": digest,
            }
        ),
        encoding="utf-8",
    )

    assert len(
        runner.load_binary_receipts(tmp_path, expected_run_id="exact-run")
    ) == 1
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
                "schema": "molt.cargo-test-binary.v1",
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
            stdout="test exact::source ... ok\n",
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
