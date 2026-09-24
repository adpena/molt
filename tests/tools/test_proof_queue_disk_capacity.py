"""Build-capacity refusal must precede work and retain actionable evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import disk_capacity
from tools import disk_guard
from tools.proof_supervisor import build as supervisor_build
from tools.proof_queue_pkg import (
    command_admission,
    diagnostic_build_rules,
    diagnostic_engine,
    evidence,
    policy,
    runner,
    scheduling,
    state,
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
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    args = argparse.Namespace(
        db=str(tmp_path / "queue.sqlite3"),
        logs_root=str(tmp_path / "runs"),
        repo_root=str(tmp_path),
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
        cwd=tmp_path,
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


def test_supervisor_build_refuses_capacity_before_starting_cargo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "supervisor-target"))
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", lambda path: 0)
    monkeypatch.setattr(supervisor_build.sys, "argv", ["build.py", "--release"])

    def never_run(*args: object, **kwargs: object) -> None:
        pytest.fail("supervisor capacity rejection started Cargo")

    monkeypatch.setattr(supervisor_build, "_COMMANDS", SimpleNamespace(run=never_run))
    with pytest.raises(disk_capacity.DiskCapacityError):
        supervisor_build.main()
    assert not (tmp_path / "supervisor-target").exists()


def test_supervisor_build_probes_separate_build_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(supervisor_build, "ROOT", tmp_path)
    monkeypatch.setenv("CARGO_TARGET_DIR", "output")
    monkeypatch.setenv("CARGO_BUILD_BUILD_DIR", "build")
    monkeypatch.setattr(supervisor_build.sys, "argv", ["build.py"])
    observed = []

    def admit(paths, **kwargs):
        observed.extend(paths)

    def run(command, **kwargs):
        assert command[-2:] == ["--target-dir", str(tmp_path / "output")]
        assert kwargs["cwd"] == tmp_path
        output = tmp_path / "output" / "debug"
        output.mkdir(parents=True)
        suffix = ".exe" if supervisor_build.os.name == "nt" else ""
        (output / f"molt-proof-supervisor{suffix}").write_bytes(b"fixture")

    monkeypatch.setattr(supervisor_build, "require_build_capacity", admit)
    monkeypatch.setattr(supervisor_build, "_COMMANDS", SimpleNamespace(run=run))
    assert supervisor_build.main() == 0
    assert observed == [tmp_path / "output", tmp_path / "build"]


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
