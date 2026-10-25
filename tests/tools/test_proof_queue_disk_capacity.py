"""Build-capacity refusal must precede work and retain actionable evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import disk_capacity
from molt import dx
from molt.cargo_execution_policy import CARGO_WRAPPER_ENV_NAMES
from tools import disk_guard
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    diagnostic_build_rules,
    diagnostic_engine,
    evidence,
    policy,
    runner,
    scheduling,
    state,
    supervisor_generation,
)


def test_cleanup_and_build_admission_share_exact_threshold() -> None:
    env = {disk_capacity.DISK_GUARD_HIGH_WATER_ENV: "0.000000001"}
    assert disk_guard.GuardConfig.from_env(env).high_water_bytes == 2
    assert disk_capacity.minimum_headroom_bytes(env) == 2


@pytest.mark.parametrize("raw", ["0", "nan", "inf", "bad", ""])
def test_cleanup_cannot_silently_replace_invalid_admission_policy(raw: str) -> None:
    with pytest.raises(disk_capacity.DiskCapacityError):
        disk_guard.GuardConfig.from_env({disk_capacity.DISK_GUARD_HIGH_WATER_ENV: raw})


def test_queue_refuses_low_capacity_before_provisioning_and_retains_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, request: pytest.FixtureRequest
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    output_root = tmp_path
    if os.name == "nt":
        from molt.temporary_artifacts import OwnedTemporaryDirectory

        # Path admission precedes disk admission. Keep the real Windows Cargo
        # descendant budget while giving this disk test its own short output.
        temporary = OwnedTemporaryDirectory(prefix="q")
        request.addfinalizer(temporary.cleanup)
        output_root = Path(temporary.name)
    args = argparse.Namespace(
        db=str(output_root / "queue.sqlite3"),
        logs_root=str(output_root / "runs"),
        repo_root=str(source),
    )
    envelope = {"toolchains": ["cargo"]}
    monkeypatch.setattr(
        command_admission, "admission_envelope", lambda command, **kwargs: envelope
    )
    monkeypatch.setattr(
        command_admission, "envelope_for_command", lambda command, **kwargs: envelope
    )
    monkeypatch.setattr(policy, "_proof_command_policy_error", lambda command: None)
    monkeypatch.setattr(
        scheduling,
        "_lane_maturity_admission",
        lambda **kwargs: SimpleNamespace(allow=True),
    )
    monkeypatch.setattr(
        evidence, "_try_write_marimo_notebook", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 0)
    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", lambda path: 0)
    monkeypatch.delenv(disk_capacity.DISK_GUARD_HIGH_WATER_ENV, raising=False)

    def never_provision(*args: object, **kwargs: object) -> None:
        pytest.fail("disk-rejected build reached environment or process provisioning")

    monkeypatch.setattr(runner, "development_artifact_env", never_provision)
    monkeypatch.setattr(runner, "_write_execution_request", never_provision)
    command = ["cargo", "test"]
    log = Path(args.logs_root) / "capacity.log"
    summary = Path(args.logs_root) / "capacity.memory_guard.json"
    conn = state._connect(Path(args.db))
    scheduling._insert_run(
        conn,
        run_id="capacity-unit",
        logical_id="capacity-unit",
        reason="unit-test capacity admission",
        command=command,
        cwd=source,
        resource_family="cargo",
        contention_key="compiler-build",
        scopes=[],
        git_snapshot={},
        log_path=log,
        summary_json=summary,
    )
    rc = runner._run_one(
        args,
        logical_id="capacity-unit",
        reason="unit-test capacity admission",
        command=command,
        resource_family="cargo",
        contention_key="compiler-build",
        scopes=[],
        env_overrides={},
        timeout=30,
        existing_run_id="capacity-unit",
        existing_log_path=log,
        existing_summary_json=summary,
    )
    row = state._row_by_run_id(conn, "capacity-unit")
    assert row is not None
    assert rc == 2 and row["status"] == "failed" and row["elapsed_s"] == 0
    assert row["guard_pid"] is None
    context = json.loads(row["receipt_context_json"])
    assert context["status"] == "not-executed"
    assert "disk_capacity_admission" in context, context
    admission = context["disk_capacity_admission"]
    assert admission["status"] == "rejected"
    assert admission["probes"][0]["requested_path"] == str(
        Path(args.logs_root).resolve()
    )
    assert admission["probes"][0]["free_bytes"] == 0
    assert not summary.exists()
    diagnostics = diagnostic_engine._run_diagnostics(row)
    assert [item["signal_id"] for item in diagnostics] == ["build-disk-capacity"]
    conn.close()


def test_run_one_derives_dx_environment_from_requested_overrides_before_launch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, request: pytest.FixtureRequest
) -> None:
    repo_root = Path(__file__).resolve().parents[2]
    ambient_root = tmp_path / "ambient-artifacts"
    requested_root = tmp_path / "requested-artifacts"
    if os.name == "nt":
        from molt.temporary_artifacts import OwnedTemporaryDirectory

        # Exercise the real Cargo path-budget admission, independently of
        # pytest's long descriptive case directory. Keep ownership and cleanup.
        temporary = OwnedTemporaryDirectory(prefix="q")
        request.addfinalizer(temporary.cleanup)
        requested_root = Path(temporary.name)
    requested_tools = requested_root / "toolchains"
    resource_family = "cargo"
    contention_key = "compiler-build"
    session_id = state._proof_session_id(resource_family, contention_key)

    for name in (
        *dx.DX_ENV_KEYS,
        *CARGO_WRAPPER_ENV_NAMES,
        "MOLT_SESSION_ID_GENERATED",
        "VIRTUAL_ENV",
        "MOLT_VENV",
        "MOLT_UV_PROJECT_PURPOSE",
        "MOLT_UV_PROJECT_PYTHON",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ambient_root))
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(ambient_root / "toolchains"))
    monkeypatch.setenv("MOLT_SESSION_ID", "ambient-session")
    monkeypatch.setenv("MOLT_USE_SCCACHE", "1")

    overrides = {
        "MOLT_EXT_ROOT": str(requested_root),
        "MOLT_TARGET_ROOT": str(requested_tools),
        "MOLT_SESSION_ID": session_id,
        "MOLT_USE_SCCACHE": "0",
    }
    observed: dict[str, object] = {}
    original_resolver = runner.development_artifact_env
    original_validate_environment = (
        runner.cargo_output_layout.CargoOutputLayout.validate_environment
    )
    original_require_build_capacity = disk_capacity.require_build_capacity

    def observe_output_validation(layout, env):
        observed["validation_environment"] = dict(env)
        return original_validate_environment(layout, env)

    def observe_capacity(paths, *, env=None, **kwargs):
        observed["capacity_environment"] = dict(env or {})
        return original_require_build_capacity(paths, env=env, **kwargs)

    def observe_resolver(base_root, base_env, **kwargs):
        observed["derivation_input"] = dict(base_env)
        observed["session_id_argument"] = kwargs.get("session_id")
        derived = original_resolver(base_root, base_env, **kwargs)
        observed["derived"] = dict(derived)
        return derived

    monkeypatch.setattr(runner, "development_artifact_env", observe_resolver)
    monkeypatch.setattr(
        runner.cargo_output_layout.CargoOutputLayout,
        "validate_environment",
        observe_output_validation,
    )
    monkeypatch.setattr(disk_capacity, "require_build_capacity", observe_capacity)
    discoveries: list[dict[str, str]] = []

    def observe_sccache_discovery(env):
        discoveries.append(dict(env))
        return str(tmp_path / "tools" / "sccache")

    monkeypatch.setattr(dx, "pinned_sccache", observe_sccache_discovery)
    monkeypatch.setenv(disk_capacity.DISK_GUARD_HIGH_WATER_ENV, "0.000000001")

    args = argparse.Namespace(
        db=str(requested_root / "queue.sqlite3"),
        logs_root=str(requested_root / "runs"),
        repo_root=str(repo_root),
    )
    envelope = {"toolchains": ["cargo"]}
    monkeypatch.setattr(
        command_admission, "admission_envelope", lambda command, **kwargs: envelope
    )
    monkeypatch.setattr(
        command_admission, "envelope_for_command", lambda command, **kwargs: envelope
    )
    monkeypatch.setattr(policy, "_proof_command_policy_error", lambda command: None)
    monkeypatch.setattr(
        scheduling,
        "_lane_maturity_admission",
        lambda **kwargs: SimpleNamespace(allow=True),
    )
    monkeypatch.setattr(
        evidence, "_try_write_marimo_notebook", lambda *args, **kwargs: None
    )

    launch_observations: list[dict[str, str]] = []

    def stop_before_child(command, *, cwd, env, stdout):
        launch_observations.append(dict(env))
        raise RuntimeError("test stopped before child launch")

    monkeypatch.setattr(runner.custody, "_launch_queued_command", stop_before_child)

    command = ["cargo", "test"]
    log = Path(args.logs_root) / "requested-env.log"
    summary = Path(args.logs_root) / "requested-env.memory_guard.json"
    conn = state._connect(Path(args.db))
    scheduling._insert_run(
        conn,
        run_id="requested-env-unit",
        logical_id="requested-env-unit",
        reason="unit-test requested execution environment",
        command=command,
        cwd=repo_root,
        resource_family=resource_family,
        contention_key=contention_key,
        scopes=[],
        env_overrides=overrides,
        git_snapshot={},
        log_path=log,
        summary_json=summary,
    )
    rc = runner._run_one(
        args,
        logical_id="requested-env-unit",
        reason="unit-test requested execution environment",
        command=command,
        resource_family=resource_family,
        contention_key=contention_key,
        scopes=[],
        env_overrides=overrides,
        timeout=30,
        existing_run_id="requested-env-unit",
        existing_log_path=log,
        existing_summary_json=summary,
    )

    assert rc == 2
    assert len(launch_observations) == 1, log.read_text(encoding="utf-8")
    derivation_input = observed["derivation_input"]
    assert isinstance(derivation_input, dict)
    assert {name: derivation_input.get(name) for name in overrides} == overrides
    for phase in ("validation_environment", "capacity_environment"):
        phase_environment = observed[phase]
        assert isinstance(phase_environment, dict)
        assert {name: phase_environment.get(name) for name in overrides} == overrides
    assert observed["session_id_argument"] == session_id
    assert discoveries == []

    derived = observed["derived"]
    assert isinstance(derived, dict)
    assert derived["MOLT_EXT_ROOT"] == str(requested_root)
    assert derived["MOLT_TARGET_ROOT"] == str(requested_tools)
    assert derived["MOLT_SESSION_ID"] == session_id
    assert "RUSTC_WRAPPER" not in derived
    # Cargo owns no Python selection. DX's ordinary scratch policy must reach
    # launch without inventing an immutable Python image for this command.
    assert "UV_PROJECT_ENVIRONMENT" not in derived
    output_names = (
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "UV_CACHE_DIR",
        "PIP_CACHE_DIR",
        "RUFF_CACHE_DIR",
        "SCCACHE_DIR",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "PYTHONPYCACHEPREFIX",
        "TMPDIR",
        "TMP",
        "TEMP",
    )
    for name in output_names:
        assert Path(derived[name]).is_relative_to(requested_root), (name, derived[name])

    launch_env = launch_observations[0]
    # IPC directories follow the short host-socket policy, independently of
    # artifact placement. The queue must preserve that selected path.
    socket_dir = Path(derived["MOLT_BACKEND_DAEMON_SOCKET_DIR"])
    assert socket_dir.is_absolute() and not socket_dir.is_relative_to(repo_root)
    assert launch_env["MOLT_BACKEND_DAEMON_SOCKET_DIR"] == str(socket_dir)
    if os.name != "nt":
        assert len(os.fsencode(socket_dir / "moltbd.ffffffffffffffff.sock")) < 104
    assert launch_env["MOLT_EXT_ROOT"] == str(requested_root)
    assert launch_env["MOLT_TARGET_ROOT"] == str(requested_tools)
    assert launch_env["MOLT_SESSION_ID"] == session_id
    assert launch_env["MOLT_USE_SCCACHE"] == "0"
    assert "RUSTC_WRAPPER" not in launch_env
    assert "UV_PROJECT_ENVIRONMENT" not in launch_env
    assert launch_env["PYTHONPYCACHEPREFIX"] == derived["PYTHONPYCACHEPREFIX"]
    for name in output_names:
        assert Path(launch_env[name]).is_relative_to(requested_root), (
            name,
            launch_env[name],
        )
    row = state._row_by_run_id(conn, "requested-env-unit")
    assert row is not None and row["status"] == "failed" and row["guard_pid"] is None
    conn.close()


def test_typed_python_policy_uses_captured_prefix_without_uv_selection(
    tmp_path,
) -> None:
    selected = tmp_path / "managed-image" / ("a" * 64)
    environment = {
        "PYTHONPYCACHEPREFIX": str(tmp_path / "foreign-cache"),
        "PYTHONDONTWRITEBYTECODE": "0",
    }
    command, updates = command_admission._supervised_execution_command(
        {"python": {"kind": "direct"}},
        [sys.executable, "-I", "-c", "pass"],
        {
            "python": {
                "location": {"prefix": str(selected)},
                "executable": sys.executable,
                "base_executable": sys.executable,
            }
        },
        environment=environment,
        cwd=tmp_path,
    )
    assert command[1:5] == [
        "-B",
        "-X",
        f"pycache_prefix={selected / 'bytecode'}",
        "-I",
    ]
    assert updates["PYTHONPYCACHEPREFIX"] == str(selected / "bytecode")
    assert updates["PYTHONDONTWRITEBYTECODE"] == "1"
    assert "UV_PROJECT_ENVIRONMENT" not in updates
    assert environment["PYTHONPYCACHEPREFIX"] == str(tmp_path / "foreign-cache")


@pytest.mark.parametrize(
    "located",
    [{}, {"python": {}}, {"python": {"location": {"prefix": "relative-image"}}}],
)
def test_typed_python_policy_refuses_missing_captured_selection(
    tmp_path, located
) -> None:
    with pytest.raises(
        ValueError, match="no (located runtime|captured environment prefix)"
    ):
        command_admission._supervised_execution_command(
            {"python": {"kind": "direct"}},
            [sys.executable, "-c", "pass"],
            located,
            environment={"PYTHONPYCACHEPREFIX": str(tmp_path / "scratch")},
            cwd=tmp_path,
        )


@pytest.mark.parametrize(
    "failure",
    [
        "LINK : fatal error LNK1180: insufficient disk space",
        "error: could not write output: No space left on device (os error 28)",
        "OSError: [Errno 28] No space left on device",
        "OSError: [WinError 112] There is not enough space on the disk",
        "error: output write failed (os error 112)",
        "DiskCapacityError: build capacity admission rejected diagnostic={}",
    ],
)
def test_disk_failure_is_classified_as_infrastructure(failure: str) -> None:
    row = {"status": "failed", "log_path": "run.log", "summary_json": "guard.json"}
    diagnostic = diagnostic_build_rules._disk_capacity_diagnostic(row, failure)
    assert diagnostic is not None
    assert diagnostic["signal_id"] == "build-disk-capacity"
    assert diagnostic["severity"] == "infra"
    assert "compiler code" in diagnostic["next_action"]


def test_capacity_observation_does_not_terminalize_a_live_command() -> None:
    row = {"status": "running", "log_path": "run.log", "summary_json": "guard.json"}
    diagnostic = diagnostic_build_rules._disk_capacity_diagnostic(row, "error: ENOSPC")
    assert diagnostic is not None
    assert "remains running" in diagnostic["summary"]
    assert "does not authorize cancellation" in diagnostic["next_action"]


@pytest.mark.parametrize(
    "text",
    [
        "error[E0308]: mismatched types",
        "error: out of memory",
        "LINK: LNK1180 insufficient memory",
    ],
)
def test_other_compile_failures_are_not_reclassified_as_disk_capacity(
    text: str,
) -> None:
    row = {"status": "failed", "log_path": "run.log", "summary_json": "guard.json"}
    assert diagnostic_build_rules._disk_capacity_diagnostic(row, text) is None


def _supervisor_inputs(source_root: Path) -> dict[str, object]:
    return {
        "source_root": str(source_root),
        "command": ["cargo", "build"],
        "cargo_inputs": {},
        "target": None,
        "profile": "debug",
    }


def test_supervisor_build_refuses_capacity_before_starting_cargo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "supervisor-target"
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 0)
    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", lambda path: 0)

    def never_run(*args: object, **kwargs: object) -> None:
        pytest.fail("supervisor capacity rejection started Cargo")

    monkeypatch.setattr(command_identity, "_run_captured", never_run)
    with pytest.raises(disk_capacity.DiskCapacityError):
        supervisor_generation.build_cargo(
            inputs=_supervisor_inputs(tmp_path),
            environment=supervisor_generation.effective_cargo_environment(
                _supervisor_inputs(tmp_path), {"CARGO_TARGET_DIR": str(target)}
            ),
        )
    assert not target.exists()


def test_supervisor_build_probes_separate_build_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    crate = (tmp_path / "tools" / "proof_supervisor").resolve()
    crate.mkdir(parents=True)
    observed: list[Path] = []

    def admit(paths, **kwargs):
        observed.extend(paths)

    def run(command, **kwargs):
        assert command[-2:] == ["--target-dir", str(crate / "output")]
        assert kwargs["cwd"] == crate
        output = crate / "output" / "debug"
        output.mkdir(parents=True)
        suffix = ".exe" if os.name == "nt" else ""
        (output / f"molt-proof-supervisor{suffix}").write_bytes(b"fixture")
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(disk_capacity, "require_build_capacity", admit)
    monkeypatch.setattr(command_identity, "_run_captured", run)
    binary, completed = supervisor_generation.build_cargo(
        inputs=_supervisor_inputs(crate.parents[1]),
        environment=supervisor_generation.effective_cargo_environment(
            _supervisor_inputs(crate.parents[1]),
            {"CARGO_TARGET_DIR": "output", "CARGO_BUILD_BUILD_DIR": "build"},
        ),
    )
    assert completed.returncode == 0
    assert binary.parent == crate / "output" / "debug"
    assert observed == [crate / "output", crate / "build"]


@pytest.mark.parametrize("safe", [False, True])
def test_generation_terminal_projection_binds_only_validated_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, safe: bool
) -> None:
    nonce = "a" * 64
    nonce_hash = hashlib.sha256(nonce.encode()).hexdigest()
    provenance = {
        "generation_run_id": "unit",
        "execution_nonce_sha256": nonce_hash,
        "input_sha256": "b" * 64,
        "generation_id": "c" * 16,
        "path": str(tmp_path / "target"),
    }
    record = {
        "cargo_cache": provenance,
        "cargo_cache_publication": {"state": "unsealed", "reason": "unit"},
        "command_started": True,
        "command_returncode": 101,
    }
    context = {
        "derived_root_custody": {"prelaunch": [provenance]},
        "execution_custody_sha256": "d" * 64,
        "guard_receipt": {"sha256": "e" * 64},
        "process_supervisor": {"receipt": {"state": "COMPLETE"}},
    }
    captured = []

    def record_terminal(**kwargs: object) -> dict[str, object]:
        captured.append(kwargs)
        return {"state": "recorded"}

    monkeypatch.setattr(
        runner.cargo_cache_custody, "record_terminal_receipt", record_terminal
    )
    projection = runner._record_cargo_generation_terminal(
        execution_record=record,
        execution_path=tmp_path / "run.execution.json",
        run_id="unit",
        execution_nonce=nonce,
        receipt_context=context if safe else None,
        process_cleanup_safe=safe,
    )
    assert projection == {"state": "recorded"}
    terminal = captured[0]["terminal_receipt"]
    assert terminal["execution_nonce_sha256"] == nonce_hash
    assert terminal["process_cleanup_safe"] is safe
    assert terminal["cargo_cache_publication"] == record["cargo_cache_publication"]
    assert terminal["process_supervisor"] == (
        context["process_supervisor"] if safe else None
    )
    if safe:
        context["derived_root_custody"]["prelaunch"] = []
        with pytest.raises(ValueError, match="validated derived output"):
            runner._record_cargo_generation_terminal(
                execution_record=record,
                execution_path=tmp_path / "run.execution.json",
                run_id="unit",
                execution_nonce=nonce,
                receipt_context=context,
                process_cleanup_safe=True,
            )
        assert len(captured) == 1
