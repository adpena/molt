from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools import guarded_exec

# Cargo runs here are fakes; the admission test patches the probe itself.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


def _install_fake_context(module, monkeypatch, result=None):
    captured: dict[str, object] = {}

    class FakeContext:
        @classmethod
        def from_env(cls, prefix, env, *, repo_root):
            captured["prefix"] = prefix
            captured["env"] = dict(env)
            captured["repo_root"] = repo_root
            return cls()

        def run(
            self,
            command,
            *,
            cwd,
            env,
            capture_output,
            timeout,
            cancellation_requested,
            closure_progress,
            running_summary_json,
            running_summary_environ,
        ):
            captured["command"] = list(command)
            captured["cwd"] = cwd
            captured["run_env"] = dict(env)
            captured["capture_output"] = capture_output
            captured["timeout"] = timeout
            captured["cancellation_requested"] = cancellation_requested
            captured["closure_progress"] = closure_progress
            captured["running_summary_json"] = running_summary_json
            captured["running_summary_environ"] = running_summary_environ
            return (
                result
                if result is not None
                else module.harness_memory_guard.GuardedCompletedProcess(
                    command,
                    0,
                    "",
                    "",
                    elapsed_s=0,
                    descendants_closed=True,
                    child_stderr="",
                )
            )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "HarnessExecutionContext",
        FakeContext,
        raising=True,
    )
    return captured


@pytest.mark.parametrize("closed", [True, False])
@pytest.mark.parametrize("worker", [False, True])
def test_guarded_exec_collects_after_published_standalone_result(
    tmp_path, monkeypatch, closed, worker
):
    from molt import dx

    result = guarded_exec.harness_memory_guard.GuardedCompletedProcess(
        ["fixture"],
        7,
        "",
        "",
        elapsed_s=0,
        descendants_closed=closed,
        child_stderr="",
    )
    _install_fake_context(guarded_exec, monkeypatch, result)
    metrics = tmp_path / "terminal.json"
    if worker:
        monkeypatch.setenv(
            "MOLT_MEMORY_GUARD_STARTUP_JSON", str(tmp_path / "startup.json")
        )
    else:
        monkeypatch.delenv("MOLT_MEMORY_GUARD_STARTUP_JSON", raising=False)
    monkeypatch.delenv("MOLT_PROOF_QUEUE", raising=False)
    observed = []

    def collect(_env, *, cwd):
        payload = json.loads(metrics.read_text(encoding="utf-8"))
        assert payload["returncode"] == 7
        assert payload["descendants_closed"] is closed
        assert cwd == guarded_exec.ROOT
        observed.append("published")

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    assert guarded_exec.main(["--metrics-json", str(metrics), "--", "fixture"]) == 7
    assert observed == ([] if worker else ["published"])


def test_guarded_exec_metrics_preserve_child_and_infrastructure_outcomes(
    tmp_path, monkeypatch
) -> None:
    module = guarded_exec
    guard = module.harness_memory_guard.memory_guard
    failure = guard.GuardInfrastructureFailure(
        phase="temporary_artifact_custody", details=("invalid index",)
    )
    artifacts = {"state": "reclaimed", "retention": {"errors": ["invalid index"]}}
    result = module.harness_memory_guard.GuardedCompletedProcess(
        ["fixture"],
        guard.INFRASTRUCTURE_RETURN_CODE,
        "",
        "",
        elapsed_s=0.25,
        child_returncode=0,
        infrastructure_failure=failure,
        temporary_artifacts=artifacts,
        child_stderr="",
    )
    _install_fake_context(module, monkeypatch, result)
    metrics = tmp_path / "metrics.json"
    rc = module.main(["--metrics-json", str(metrics), "--", "fixture"])
    assert rc == guard.INFRASTRUCTURE_RETURN_CODE
    payload = json.loads(metrics.read_text(encoding="utf-8"))
    assert payload["returncode"] == rc
    assert payload["child_returncode"] == 0
    assert payload["infrastructure_failure"] == guard.infrastructure_failure_payload(
        failure
    )
    assert payload["temporary_artifacts"] == artifacts


def test_guarded_exec_signal_metrics_drive_executor_failure_scope(
    tmp_path, monkeypatch
) -> None:
    from tools import proof_plan
    from tools.memory_guard_core import reporting

    module = guarded_exec
    for returncode, expected in ((128, "partition"), (143, "global")):
        result = module.harness_memory_guard.GuardedCompletedProcess(
            ["fixture"],
            returncode,
            "",
            "",
            elapsed_s=0,
            descendants_closed=True,
            child_stderr="",
        )
        _install_fake_context(module, monkeypatch, result)
        metrics = tmp_path / f"metrics-{returncode}.json"
        assert (
            module.main(["--metrics-json", str(metrics), "--", "fixture"]) == returncode
        )
        payload = json.loads(metrics.read_text(encoding="utf-8"))
        assert (
            proof_plan._guarded_failure_scope(
                payload,
                metrics_valid=True,
                returncode=returncode,
                cancel_requested=False,
            )[0]
            == expected
        )
    assert reporting.exit_signal_payload(0xC0000005, windows_process_model=True) == {
        "signal": None,
        "name": "NTSTATUS 0xC0000005",
        "conventional_shell_status": False,
    }


def test_guarded_exec_uses_family_timeout_env(monkeypatch) -> None:
    module = guarded_exec
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "123.5")

    rc = module.main(["--prefix", "MOLT_WASM_TEST", "--", "python3", "-c", "pass"])

    assert rc == 0
    assert captured["timeout"] == 123.5
    assert captured["command"] == ["python3", "-c", "pass"]


def test_guarded_exec_cli_timeout_overrides_family_env(monkeypatch) -> None:
    module = guarded_exec
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "123.5")

    rc = module.main(
        [
            "--prefix",
            "MOLT_WASM_TEST",
            "--timeout",
            "7",
            "--",
            "python3",
            "-c",
            "pass",
        ]
    )

    assert rc == 0
    assert captured["timeout"] == 7


def test_guarded_exec_timeout_env_remains_fallback(monkeypatch) -> None:
    module = guarded_exec
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.delenv("MOLT_WASM_TEST_TIMEOUT_SEC", raising=False)
    monkeypatch.delenv("MOLT_TEST_PROCESS_TIMEOUT_SEC", raising=False)
    monkeypatch.setenv("CUSTOM_TIMEOUT_SEC", "88")

    rc = module.main(
        [
            "--prefix",
            "MOLT_WASM_TEST",
            "--timeout-env",
            "CUSTOM_TIMEOUT_SEC",
            "--",
            "python3",
            "-c",
            "pass",
        ]
    )

    assert rc == 0
    assert captured["timeout"] == 88


def test_guarded_exec_family_timeout_can_disable_fallback(monkeypatch) -> None:
    module = guarded_exec
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "0")
    monkeypatch.setenv("CUSTOM_TIMEOUT_SEC", "88")

    rc = module.main(
        [
            "--prefix",
            "MOLT_WASM_TEST",
            "--timeout-env",
            "CUSTOM_TIMEOUT_SEC",
            "--",
            "python3",
            "-c",
            "pass",
        ]
    )

    assert rc == 0
    assert captured["timeout"] is None


@pytest.mark.parametrize(
    "command",
    [
        [
            "cargo",
            "test",
            "-p",
            "molt-backend",
            "--features",
            "native-backend llvm",
            "--lib",
        ],
        ["cargo", "clippy", "--workspace", "--all-features"],
        ["cargo.exe", "check", "--package=molt-backend", "--features=llvm"],
        ["cargo", "clippy", "-p", "molt-tir", "--all-features"],
    ],
)
@pytest.mark.parametrize("selected", [False, True])
def test_guarded_exec_preserves_caller_toolchains_without_sdk_discovery(
    monkeypatch: pytest.MonkeyPatch, command: list[str], selected: bool
) -> None:
    from molt import llvm_toolchain

    module = guarded_exec
    captured = _install_fake_context(module, monkeypatch)
    pin = llvm_toolchain.required_llvm_backend_pin(module.ROOT)
    assert pin is not None
    sdk_environment = {
        "MOLT_LLVM_PREFIX": "/selected SDK",
        pin.env_var: "/external config search root",
        "LLVM_CONFIG_PATH": "/external config search root/bin/llvm-config",
        llvm_toolchain.mlir_sys_prefix_env_var(pin.major): "/selected SDK",
        llvm_toolchain.tablegen_prefix_env_var(pin.major): "/selected SDK",
    }
    for name, value in sdk_environment.items():
        if selected:
            monkeypatch.setenv(name, value)
        else:
            monkeypatch.delenv(name, raising=False)

    def unexpected_discovery(*_args, **_kwargs):
        pytest.fail("the command guard must not discover or reselect a caller's SDK")

    monkeypatch.setattr(
        llvm_toolchain, "verify_available_llvm_toolchain", unexpected_discovery
    )
    assert module.main(["--prefix", "MOLT_TEST_SUITE", "--", *command]) == 0
    assert captured["command"] == command
    projected = captured["run_env"]
    assert {name: projected.get(name) for name in sdk_environment} == {
        name: value if selected else None for name, value in sdk_environment.items()
    }


def test_guarded_exec_metrics_preserve_deadline_and_cargo_ownership(
    tmp_path, monkeypatch
) -> None:
    module = guarded_exec
    guard = module.harness_memory_guard.memory_guard
    quarantine = guard.CargoIncrementalQuarantine(
        reason="timeout",
        recorded_at="2026-10-03T00:00:00Z",
        target_dir=str(tmp_path),
        quarantine_dir=None,
        command=("cargo", "test"),
        cwd=str(tmp_path),
        ownership_status="deferred",
        errors=("compiler ownership unavailable",),
    )
    result = module.harness_memory_guard.GuardedCompletedProcess(
        ["fixture"],
        124,
        "",
        "",
        elapsed_s=1.0,
        timed_out=True,
        guard_signal=15,
        cargo_incremental_quarantine=quarantine,
        child_stderr="",
    )
    _install_fake_context(module, monkeypatch, result)
    output = tmp_path / "metrics.json"
    assert module.main(["--metrics-json", str(output), "--", "fixture"]) == 124
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert payload["timed_out"] is True
    assert payload["guard_signal"] == 15
    assert payload["cargo_incremental_quarantine"]["ownership_status"] == "deferred"
    assert payload["cargo_incremental_quarantine"]["errors"] == [
        "compiler ownership unavailable"
    ]
    assert payload["termination_reports"] == []


def test_guarded_exec_forwards_sticky_cancel_and_worker_custody(tmp_path, monkeypatch):
    module = guarded_exec
    guard = module.harness_memory_guard.memory_guard
    cancel = tmp_path / "cancel"
    startup = tmp_path / "startup.json"
    command = ["fixture"]
    worker_env = guard._worker_env(
        {}, command, launch_id="a" * 32, startup_json=str(startup)
    )
    for key, value in worker_env.items():
        monkeypatch.setenv(key, value)
    progress = tmp_path / "progress"
    captured = _install_fake_context(module, monkeypatch)
    assert (
        module.main(["--cancel-file", str(cancel), "--progress-file", str(progress)])
        == 0
    )
    callback = captured["cancellation_requested"]
    assert callback() is False
    cancel.write_text("cancel", encoding="utf-8")
    assert callback() is True
    # The guard's closure notices reach the owner's progress file.
    captured["closure_progress"]("tree_signalled")
    assert progress.read_text(encoding="utf-8").endswith(" tree_signalled\n")
    assert guard._load_internal_command(captured["run_env"]) is None
    assert guard._load_internal_command(captured["running_summary_environ"]) == command


def test_guarded_exec_refuses_a_cargo_compile_below_the_capacity_floor(
    tmp_path, monkeypatch, capsys
) -> None:
    from molt import disk_capacity

    module = guarded_exec
    captured = _install_fake_context(module, monkeypatch)
    measured: list[Path] = []

    def measure(path):
        measured.append(path)
        return disk_capacity.DEFAULT_MINIMUM_HEADROOM_BYTES - 1

    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", measure)
    monkeypatch.setattr(
        disk_capacity,
        "_darwin_important_usage_bytes",
        lambda path: disk_capacity.DEFAULT_MINIMUM_HEADROOM_BYTES - 1,
    )
    monkeypatch.delenv(disk_capacity.DISK_GUARD_HIGH_WATER_ENV, raising=False)
    target = tmp_path / "proof-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))

    rc = module.main(["--", "cargo", "test", "--locked", "-p", "molt-ir", "--lib"])

    assert rc == 2
    assert "command" not in captured  # no Cargo process started
    assert "build capacity admission rejected" in capsys.readouterr().err
    assert tmp_path in measured

    # A Cargo command that does not compile needs no build capacity.
    assert module.main(["--", "cargo", "metadata", "--no-deps"]) == 0
    assert captured["command"] == ["cargo", "metadata", "--no-deps"]
