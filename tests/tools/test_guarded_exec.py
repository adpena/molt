from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace


REPO_ROOT = Path(__file__).resolve().parents[2]
GUARDED_EXEC = REPO_ROOT / "tools" / "guarded_exec.py"


def _load_guarded_exec():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_guarded_exec", GUARDED_EXEC
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _install_fake_context(module, monkeypatch, result=None):
    captured: dict[str, object] = {}

    class FakeContext:
        @classmethod
        def from_env(cls, prefix, env, *, repo_root):
            captured["prefix"] = prefix
            captured["env"] = dict(env)
            captured["repo_root"] = repo_root
            return cls()

        def run(self, command, *, cwd, env, capture_output, timeout):
            captured["command"] = list(command)
            captured["cwd"] = cwd
            captured["run_env"] = dict(env)
            captured["capture_output"] = capture_output
            captured["timeout"] = timeout
            return (
                result
                if result is not None
                else SimpleNamespace(returncode=0, stderr="")
            )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "HarnessExecutionContext",
        FakeContext,
        raising=True,
    )
    return captured


def test_guarded_exec_metrics_preserve_child_and_infrastructure_outcomes(
    tmp_path, monkeypatch
) -> None:
    module = _load_guarded_exec()
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
    )
    _install_fake_context(module, monkeypatch, result)
    metrics = tmp_path / "metrics.json"
    rc = module.main(["--metrics-json", str(metrics), "--", "fixture"])
    assert rc == guard.INFRASTRUCTURE_RETURN_CODE
    payload = json.loads(metrics.read_text())
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

    module = _load_guarded_exec()
    for returncode, expected in ((128, "partition"), (143, "global")):
        result = SimpleNamespace(returncode=returncode, stderr="")
        _install_fake_context(module, monkeypatch, result)
        metrics = tmp_path / f"metrics-{returncode}.json"
        assert (
            module.main(["--metrics-json", str(metrics), "--", "fixture"]) == returncode
        )
        payload = json.loads(metrics.read_text(encoding="utf-8"))
        assert (
            proof_plan._guarded_failure_scope(
                payload, metrics_valid=True, returncode=returncode, cancelled=False
            )[0]
            == expected
        )
    assert reporting.exit_signal_payload(0xC0000005, windows_process_model=True) == {
        "signal": None,
        "name": "NTSTATUS 0xC0000005",
        "conventional_shell_status": False,
    }


def test_guarded_exec_uses_family_timeout_env(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "123.5")

    rc = module.main(["--prefix", "MOLT_WASM_TEST", "--", "python3", "-c", "pass"])

    assert rc == 0
    assert captured["timeout"] == 123.5
    assert captured["command"] == ["python3", "-c", "pass"]


def test_guarded_exec_cli_timeout_overrides_family_env(monkeypatch) -> None:
    module = _load_guarded_exec()
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
    module = _load_guarded_exec()
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
    module = _load_guarded_exec()
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


def test_guarded_exec_preflights_backend_llvm_toolchain(monkeypatch, capsys) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setattr(
        module,
        "mlir_toolchain_environment",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            module.LlvmToolchainConfigError("missing SDK")
        ),
        raising=True,
    )

    rc = module.main(
        [
            "--prefix",
            "MOLT_TEST_SUITE",
            "--",
            "cargo",
            "test",
            "-p",
            "molt-backend",
            "--features",
            "native-backend llvm",
            "--lib",
        ]
    )

    assert rc == 2
    assert "command" not in captured
    err = capsys.readouterr().err
    assert "guarded_exec preflight" in err
    assert "missing SDK" in err


def test_guarded_exec_projects_verified_llvm_environment_into_cargo(
    monkeypatch,
) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)

    def project(_root, *, environ):
        return dict(environ) | {
            "LLVM_SYS_221_PREFIX": "/usr",
            "LLVM_CONFIG_PATH": "/usr/bin/llvm-config-22",
            "MOLT_LLVM_PREFIX": "/usr/lib/llvm-22",
        }

    monkeypatch.setattr(module, "mlir_toolchain_environment", project, raising=True)

    rc = module.main(
        [
            "--prefix",
            "MOLT_TEST_SUITE",
            "--",
            "cargo",
            "check",
            "-p",
            "molt-backend",
            "--features",
            "llvm",
        ]
    )

    assert rc == 0
    assert captured["run_env"]["LLVM_SYS_221_PREFIX"] == "/usr"
    assert captured["run_env"]["LLVM_CONFIG_PATH"] == "/usr/bin/llvm-config-22"
    assert captured["run_env"]["MOLT_LLVM_PREFIX"] == "/usr/lib/llvm-22"


def test_guarded_exec_does_not_preflight_tir_all_features(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    rc = module.main(
        [
            "--prefix",
            "MOLT_TEST_SUITE",
            "--",
            "cargo",
            "clippy",
            "-p",
            "molt-tir",
            "--all-features",
        ]
    )

    assert rc == 0
    assert captured["command"] == [
        "cargo",
        "clippy",
        "-p",
        "molt-tir",
        "--all-features",
    ]


def test_guarded_exec_metrics_preserve_deadline_and_cargo_ownership(
    tmp_path, monkeypatch
) -> None:
    module = _load_guarded_exec()
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
