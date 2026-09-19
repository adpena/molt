from __future__ import annotations

import importlib
import inspect
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from molt import disk_capacity as DISK_CAPACITY
from molt.cli.models import _RuntimeArtifactState
from molt.cargo_execution_policy import CARGO_WRAPPER_ENV_NAMES
from molt.disk_capacity import DEFAULT_MINIMUM_HEADROOM_BYTES, DiskCapacityError
from tests.runtime_build_identity_helper import RuntimeFixtureRoot, runtime_cargo_plan


CARGO = importlib.import_module("molt.cli.cargo_execution")
RUNTIME = importlib.import_module("molt.cli.runtime_native_build")
RUNTIME_WASM_SUPPORT = importlib.import_module("molt.cli.runtime_wasm_build_support")


@pytest.fixture(autouse=True)
def _admit_test_cargo_capacity(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        DISK_CAPACITY,
        "_default_measure_free_bytes",
        lambda _path: DEFAULT_MINIMUM_HEADROOM_BYTES + 1,
    )


def _cargo_env(target_root: Path, **values: str) -> dict[str, str]:
    return {"CARGO_TARGET_DIR": str(target_root), **values}


def _completed(
    command: list[str],
    returncode: int,
    *,
    stdout: str | bytes = "",
    stderr: str | bytes = "",
    elapsed_s: float = 0.01,
    peak_process_kb: int = 32,
    peak_tree_kb: int = 64,
) -> subprocess.CompletedProcess[object]:
    result: subprocess.CompletedProcess[object] = subprocess.CompletedProcess(
        command, returncode, stdout, stderr
    )
    result.elapsed_s = elapsed_s  # type: ignore[attr-defined]
    result.peak = SimpleNamespace(rss_kb=peak_process_kb)  # type: ignore[attr-defined]
    result.peak_total = SimpleNamespace(rss_kb=peak_tree_kb)  # type: ignore[attr-defined]
    result.timed_out = False  # type: ignore[attr-defined]
    result.guard_signal = None  # type: ignore[attr-defined]
    return result


@pytest.mark.parametrize(
    "wrapper",
    [
        "sccache",
        "/opt/cache/sccache",
        r"C:\tools\sccache.exe",
        '"C:\\Program Files\\sccache.exe"',
    ],
)
def test_canonical_cargo_environment_forces_incremental_off_with_sccache(
    monkeypatch: pytest.MonkeyPatch,
    wrapper: str,
) -> None:
    monkeypatch.setenv("RUSTC_WRAPPER", wrapper)
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_USE_EXTERNAL_ARTIFACTS", raising=False)

    env = CARGO._cargo_build_env()

    assert env["RUSTC_WRAPPER"] == wrapper
    assert env["CARGO_INCREMENTAL"] == "0"


@pytest.mark.parametrize("explicit", [None, "0", "1"])
def test_canonical_cargo_environment_uses_normal_policy_without_sccache(
    monkeypatch: pytest.MonkeyPatch,
    explicit: str | None,
) -> None:
    monkeypatch.delenv("RUSTC_WRAPPER", raising=False)
    if explicit is None:
        monkeypatch.delenv("CARGO_INCREMENTAL", raising=False)
    else:
        monkeypatch.setenv("CARGO_INCREMENTAL", explicit)
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_USE_EXTERNAL_ARTIFACTS", raising=False)

    env = CARGO._cargo_build_env()

    assert env["CARGO_INCREMENTAL"] == ("1" if explicit is None else explicit)


def test_existing_sccache_wrapper_is_normalized_before_nested_build(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    env = {"RUSTC_WRAPPER": r"C:\tools\sccache.exe", "CARGO_INCREMENTAL": "1"}

    CARGO._maybe_enable_sccache(env)

    assert env["CARGO_INCREMENTAL"] == "0"


@pytest.mark.parametrize("wrapper_env", CARGO_WRAPPER_ENV_NAMES)
def test_every_cargo_wrapper_alias_disables_incremental(
    monkeypatch: pytest.MonkeyPatch,
    wrapper_env: str,
) -> None:
    for name in CARGO_WRAPPER_ENV_NAMES:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv(wrapper_env, r"C:\tools\sccache.exe")
    monkeypatch.setenv("CARGO_INCREMENTAL", "1")

    env = CARGO._cargo_build_env()

    assert env[wrapper_env] == r"C:\tools\sccache.exe"
    assert env["CARGO_INCREMENTAL"] == "0"


def test_real_rustc_failure_with_sccache_command_is_never_retry_authority(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "rustc"]
    calls: list[dict[str, str]] = []
    stderr = "\n".join(
        (
            "error: could not compile `molt-runtime` (lib)",
            "Caused by:",
            "  process didn't exit successfully: `/usr/bin/sccache /usr/bin/rustc "
            "--crate-name molt_runtime` (signal: 9, SIGKILL: kill)",
        )
    )

    def run(_cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[object]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        return _completed(command, 101, stderr=stderr)

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env=_cargo_env(
            tmp_path / "cargo-target",
            RUSTC_WRAPPER="/usr/bin/sccache",
        ),
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 101
    assert len(calls) == 1
    assert result.retry_reason is None
    evidence = CARGO.cargo_execution_evidence(result)
    assert evidence["attempt_count"] == 1
    assert evidence["signal"] == {
        "number": 9,
        "name": "SIGKILL",
        "source": "cargo-diagnostic",
    }


def test_empty_or_untyped_attempts_fall_back_to_one_typed_terminal_record() -> None:
    result = _completed(
        ["cargo", "check"],
        101,
        stderr="error: terminal cargo failure",
        elapsed_s=0.25,
        peak_process_kb=12,
        peak_tree_kb=24,
    )
    result.attempts = ()  # type: ignore[attr-defined]

    evidence = CARGO.cargo_execution_evidence(result)

    assert evidence["attempt_count"] == 1
    assert evidence["duration_seconds"] == pytest.approx(0.25)
    assert evidence["peak_process_rss_bytes"] == 12 * 1024
    assert evidence["peak_tree_rss_bytes"] == 24 * 1024
    attempts = evidence["attempts"]
    assert isinstance(attempts, list)
    assert attempts[0]["stderr"] == "error: terminal cargo failure"


@pytest.mark.parametrize("child_returncode", [0, 7])
@pytest.mark.parametrize("json_output", [False, True])
def test_guard_infrastructure_is_not_wrapper_retry_or_cargo_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    child_returncode: int,
    json_output: bool,
) -> None:
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

    failure = GuardInfrastructureFailure(
        "temporary_artifact_custody", ("receipt unavailable",)
    )
    command = ["cargo", "rustc"]
    terminal = _completed(
        command,
        125 if child_returncode == 0 else child_returncode,
        stderr="sccache: error: cache unavailable\nsignal: 9, SIGKILL",
    )
    terminal.child_returncode = child_returncode
    terminal.infrastructure_failure = failure
    calls = []

    def run(_cmd, **kwargs):
        calls.append(kwargs)
        return terminal

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    monkeypatch.setattr(
        CARGO,
        "_attest_sccache_stats",
        lambda *_args: pytest.fail(
            "infrastructure failure must not launch cache probes"
        ),
    )
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env=_cargo_env(tmp_path / "cargo-target", RUSTC_WRAPPER="sccache"),
        timeout=1,
        json_output=json_output,
        label="Runtime build",
    )
    assert len(calls) == 1
    assert result.retry_reason is None
    assert result.child_returncode == child_returncode
    assert result.infrastructure_failure is failure
    for candidate in (terminal, result):
        evidence = CARGO.cargo_execution_evidence(candidate)
        assert evidence["child_returncode"] == child_returncode
        assert evidence["infrastructure_failure"] == failure.json_payload()
        assert evidence["signal"] is None
        [attempt] = evidence["attempts"]
        assert attempt["failure_kind"] == "infrastructure_error"
        assert attempt["child_returncode"] == child_returncode
        assert attempt["infrastructure_failure"] == failure.json_payload()


@pytest.mark.parametrize(
    ("stderr", "reason"),
    [
        ("sccache: error: cache server unavailable", "explicit-sccache-error"),
        (
            "error: failed to execute process `/opt/bin/sccache /usr/bin/rustc -vV`",
            "sccache-launch-failure",
        ),
    ],
)
def test_explicit_wrapper_failure_retries_once_and_retains_both_attempts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    stderr: str,
    reason: str,
) -> None:
    command = ["cargo", "build"]
    calls: list[dict[str, str]] = []

    def run(_cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[object]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        if len(calls) == 1:
            return _completed(
                command,
                2,
                stderr=stderr,
                elapsed_s=0.2,
                peak_process_kb=10,
                peak_tree_kb=20,
            )
        return _completed(
            command,
            101,
            stderr="error[E0425]: retry reached rustc",
            elapsed_s=0.3,
            peak_process_kb=30,
            peak_tree_kb=40,
        )

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env=_cargo_env(
            tmp_path / "cargo-target",
            RUSTC_WRAPPER="C:/tools/sccache.exe",
        ),
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 101
    assert result.retry_reason == reason
    assert ["RUSTC_WRAPPER" in env for env in calls] == [True, False]
    evidence = CARGO.cargo_execution_evidence(result)
    assert evidence["attempt_count"] == 2
    assert evidence["duration_seconds"] == pytest.approx(0.5)
    assert evidence["peak_process_rss_bytes"] == 30 * 1024
    assert evidence["peak_tree_rss_bytes"] == 40 * 1024
    attempts = evidence["attempts"]
    assert isinstance(attempts, list)
    assert attempts[0]["failure_kind"] == reason
    assert stderr in attempts[0]["stderr"]
    assert "retry reached rustc" in attempts[1]["stderr"]


def test_sccache_retry_removes_every_sccache_wrapper_alias(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "build"]
    calls: list[dict[str, str]] = []

    def run(_cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[object]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        if len(calls) == 1:
            return _completed(command, 2, stderr="sccache: error: transport reset")
        return _completed(command, 0)

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env=_cargo_env(
            tmp_path / "cargo-target",
            **{name: "sccache" for name in CARGO_WRAPPER_ENV_NAMES},
        ),
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 0
    assert all(name in calls[0] for name in CARGO_WRAPPER_ENV_NAMES)
    assert all(name not in calls[1] for name in CARGO_WRAPPER_ENV_NAMES)


def test_tempfile_cargo_path_uses_same_retry_and_evidence_authority(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "rustc"]
    calls: list[dict[str, str]] = []

    def fail_pipe_runner(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("tempfile cargo path used the pipe runner")

    def tempfile_runner(
        _cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        if len(calls) == 1:
            return _completed(  # type: ignore[return-value]
                command, 1, stderr=b"sccache: error: transport reset"
            )
        return _completed(command, 0, stdout=b"cargo-json\n")  # type: ignore[return-value]

    monkeypatch.setattr(CARGO, "_run_completed_command", fail_pipe_runner)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env=_cargo_env(
            tmp_path / "cargo-target",
            RUSTC_WRAPPER="/usr/bin/sccache",
        ),
        timeout=1.0,
        json_output=True,
        label="Runtime wasm build",
        tempfile_runner=tempfile_runner,
        progress_label=None,
    )

    assert result.returncode == 0
    assert result.stdout == "cargo-json\n"
    assert len(result.attempts) == 2


def test_guard_timeout_is_preserved_in_execution_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "build"]
    timed_out = _completed(command, 124, stderr="memory_guard: timeout")
    timed_out.timed_out = True  # type: ignore[attr-defined]
    monkeypatch.setattr(
        CARGO, "_run_completed_command", lambda *_args, **_kwargs: timed_out
    )

    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env=_cargo_env(tmp_path / "cargo-target"),
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    evidence = CARGO.cargo_execution_evidence(result)
    assert evidence["timed_out"] is True
    assert evidence["attempts"][0]["timed_out"] is True


def test_cargo_capacity_rejection_has_probe_evidence_and_never_spawns(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executed: list[list[str]] = []
    target = tmp_path / "cargo-target"
    monkeypatch.setattr(
        DISK_CAPACITY,
        "_default_measure_free_bytes",
        lambda _path: DEFAULT_MINIMUM_HEADROOM_BYTES - 1,
    )
    monkeypatch.setattr(
        CARGO,
        "_run_completed_command",
        lambda command, **_kwargs: executed.append(command),
    )

    with pytest.raises(DiskCapacityError) as caught:
        CARGO._run_cargo_with_sccache_retry(
            ["cargo", "build"],
            cwd=tmp_path,
            env=_cargo_env(target),
            timeout=1.0,
            json_output=True,
            label="Capacity rejection",
        )

    assert executed == []
    assert caught.value.diagnostic["status"] == "rejected"
    assert caught.value.diagnostic["probes"] == [
        {
            "requested_path": str(target.resolve()),
            "measured_path": str(tmp_path.resolve()),
            "free_bytes": DEFAULT_MINIMUM_HEADROOM_BYTES - 1,
            "required_bytes": DEFAULT_MINIMUM_HEADROOM_BYTES,
            "error": None,
        }
    ]
    assert not target.exists()


def test_cargo_requires_explicit_target_declaration_before_spawn(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executed: list[list[str]] = []
    monkeypatch.setattr(
        CARGO,
        "_run_completed_command",
        lambda command, **_kwargs: executed.append(command),
    )

    with pytest.raises(ValueError, match="explicit CARGO_TARGET_DIR"):
        CARGO._run_cargo_with_sccache_retry(
            ["cargo", "build", "--target-dir", "command-only-target"],
            cwd=tmp_path,
            env={},
            timeout=1.0,
            json_output=True,
            label="Missing target declaration",
        )

    assert executed == []


def test_cargo_rejects_conflicting_command_target_before_spawn(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executed: list[list[str]] = []
    monkeypatch.setattr(
        CARGO,
        "_run_completed_command",
        lambda command, **_kwargs: executed.append(command),
    )

    with pytest.raises(ValueError, match="--target-dir conflicts"):
        CARGO._run_cargo_with_sccache_retry(
            ["cargo", "build", "--target-dir=other-target"],
            cwd=tmp_path,
            env=_cargo_env(tmp_path / "cargo-target"),
            timeout=1.0,
            json_output=True,
            label="Conflicting target declaration",
        )

    assert executed == []


def test_cargo_admits_explicit_build_dir_as_a_second_output_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target = tmp_path / "a-target"
    build_dir = tmp_path / "z-build-dir"
    target.mkdir()
    build_dir.mkdir()
    free_by_path = {
        target.resolve(): DEFAULT_MINIMUM_HEADROOM_BYTES + 1,
        build_dir.resolve(): DEFAULT_MINIMUM_HEADROOM_BYTES - 1,
    }
    executed: list[list[str]] = []
    monkeypatch.setattr(
        DISK_CAPACITY,
        "_default_measure_free_bytes",
        lambda path: free_by_path[path],
    )
    monkeypatch.setattr(
        CARGO,
        "_run_completed_command",
        lambda command, **_kwargs: executed.append(command),
    )

    with pytest.raises(DiskCapacityError) as caught:
        CARGO._run_cargo_with_sccache_retry(
            ["cargo", "build"],
            cwd=tmp_path,
            env=_cargo_env(
                target,
                CARGO_BUILD_BUILD_DIR=str(build_dir),
            ),
            timeout=1.0,
            json_output=True,
            label="Build directory capacity rejection",
        )

    assert executed == []
    probes = caught.value.diagnostic["probes"]
    assert [probe["requested_path"] for probe in probes] == [
        str(target.resolve()),
        str(build_dir.resolve()),
    ]
    assert [probe["free_bytes"] for probe in probes] == [
        DEFAULT_MINIMUM_HEADROOM_BYTES + 1,
        DEFAULT_MINIMUM_HEADROOM_BYTES - 1,
    ]


def test_terminal_cargo_summary_preserves_signal_after_long_command() -> None:
    command = "/usr/bin/sccache /usr/bin/rustc " + ("--extern dependency " * 500)
    summary = RUNTIME._native_runtime_first_error(
        cargo_stdout="",
        cargo_stderr=(
            "error: could not compile `molt-runtime` (lib)\n"
            f"process didn't exit successfully: `{command}` (signal: 9, SIGKILL: kill)\n"
        ),
        fallback="Cargo exited with code 101",
    )

    assert "could not compile `molt-runtime`" in summary
    assert "signal: 9, SIGKILL" in summary
    assert "Cargo exited with code 101" in summary
    assert len(summary) <= RUNTIME._NATIVE_RUNTIME_SUMMARY_LIMIT


def test_native_failure_receipt_carries_attempts_signal_timing_and_rss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    command = ["cargo", "rustc"]
    results = iter(
        (
            _completed(
                command,
                2,
                stderr="sccache: error: reset",
                elapsed_s=1.25,
                peak_process_kb=100,
                peak_tree_kb=200,
            ),
            _completed(
                command,
                101,
                stderr=(
                    "error: could not compile `molt-runtime`\n"
                    "process didn't exit successfully (signal: 9, SIGKILL: kill)"
                ),
                elapsed_s=2.5,
                peak_process_kb=300,
                peak_tree_kb=400,
            ),
        )
    )
    monkeypatch.setattr(
        CARGO, "_run_completed_command", lambda *_args, **_kwargs: next(results)
    )
    cargo_result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=tmp_path,
        env=_cargo_env(
            tmp_path / "cargo-target",
            RUSTC_WRAPPER="/usr/bin/sccache",
        ),
        timeout=10.0,
        json_output=True,
        label="Runtime build",
    )
    monkeypatch.setattr(RUNTIME, "_build_state_root", lambda _root: tmp_path / "state")
    state = _RuntimeArtifactState()

    assert not RUNTIME._record_native_runtime_failure(
        state,
        project_root=tmp_path,
        stage="cargo",
        summary="release runtime compile failed",
        command=command,
        cargo_stdout=cargo_result.stdout,
        cargo_stderr=cargo_result.stderr,
        returncode=cargo_result.returncode,
        cargo_result=cargo_result,
    )
    failure = state.native_runtime_build_failure
    assert failure is not None and failure.evidence_path is not None
    payload = json.loads(failure.evidence_path.read_text(encoding="utf-8"))
    assert payload["schema"] == "molt.native-runtime-build-failure.v2"
    assert payload["schema_version"] == 2
    execution = payload["cargo_execution"]
    assert execution["schema"] == "molt.cargo-execution.v1"
    assert execution["attempt_count"] == 2
    assert execution["retry_reason"] == "explicit-sccache-error"
    assert payload["duration_seconds"] == pytest.approx(3.75)
    assert payload["peak_process_rss_bytes"] == 300 * 1024
    assert payload["peak_tree_rss_bytes"] == 400 * 1024
    assert payload["signal"]["name"] == "SIGKILL"
    assert execution["attempts"][0]["schema"] == "molt.cargo-attempt.v1"
    assert "sccache: error" in execution["attempts"][0]["stderr"]
    assert "could not compile" in execution["attempts"][1]["stderr"]
    assert failure.json_payload()["attempt_count"] == 2


def test_resolved_runtime_plan_never_changes_environment_or_retries(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    plan = runtime_cargo_plan(
        tmp_path,
        fixture_root=runtime_fixture_root,
        env=_cargo_env(
            tmp_path / "cargo-target",
            RUSTC_WRAPPER="sccache",
            CARGO_INCREMENTAL="0",
        ),
        cargo_command=("cargo", "rustc"),
    )
    calls: list[dict[str, object]] = []

    def run(command: list[str], **kwargs: object):
        calls.append(kwargs)
        assert command == list(plan.command)
        assert kwargs["env"] == dict(plan.environment)
        return _completed(
            command, 2, stderr="sccache: error: failed to execute compile"
        )

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    monkeypatch.setattr(
        CARGO,
        "normalize_cargo_environment",
        lambda _env: pytest.fail("captured plan must not be renormalized"),
    )
    result = CARGO._run_resolved_cargo_plan(
        plan, timeout=1.0, json_output=True, label="Exact runtime build"
    )
    assert len(calls) == 1
    assert result.returncode == 2
    assert result.retry_reason is None
    assert len(result.attempts) == 1
    assert result.attempts[0].failure_kind == "explicit-sccache-error"


def test_resolved_plan_drift_preserves_guarded_execution_evidence(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    plan = runtime_cargo_plan(
        tmp_path,
        fixture_root=runtime_fixture_root,
        env=_cargo_env(tmp_path / "cargo-target"),
        cargo_command=("cargo", "rustc"),
    )

    def run(command: list[str], **_kwargs: object):
        config = tmp_path / ".cargo" / "config.toml"
        config.parent.mkdir()
        config.write_text("[build]\njobs = 1\n", encoding="utf-8")
        return _completed(
            command, 0, stdout="captured build diagnostics", peak_tree_kb=123
        )

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    with pytest.raises(
        CARGO.CargoPlanExecutionError, match="changed during execution"
    ) as caught:
        CARGO._run_resolved_cargo_plan(
            plan, timeout=1.0, json_output=True, label="Exact runtime build"
        )
    evidence = CARGO.cargo_execution_evidence(caught.value.cargo_result)
    assert evidence["attempt_count"] == 1
    assert evidence["peak_tree_rss_bytes"] == 123 * 1024
    assert evidence["attempts"][0]["stdout"] == "captured build diagnostics"


def test_runtime_builds_have_no_private_sccache_retry_lane() -> None:
    source = "\n".join(
        inspect.getsource(module) for module in (RUNTIME, RUNTIME_WASM_SUPPORT)
    )
    assert "retry_env = env.copy()" not in source
    assert "retry_env = build_env.copy()" not in source
    assert 'Path(wrapper).name == "sccache"' not in source
    assert "_run_cargo_with_sccache_retry(" not in source
    assert "_run_resolved_cargo_plan(" in source
