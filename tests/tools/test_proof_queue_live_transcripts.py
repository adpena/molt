from __future__ import annotations

import json
from pathlib import Path
import subprocess
import time
from types import SimpleNamespace

import pytest

from tools.proof_queue_pkg import command_admission, command_identity
from tools.proof_queue_pkg import diagnostic_engine as engine
from tools.proof_queue_pkg import diagnostic_evidence as evidence
from tools.proof_queue_pkg import diagnostic_model
from tools.proof_queue_pkg import guarded_execution


def _execution(tmp_path: Path):
    log = tmp_path / "run.log"
    log.write_text("proof_queue running\n", encoding="utf-8")
    request_path, result_path = command_identity.execution_record_paths(log)
    command = ["owned-python", "owned-test.py"]
    envelope = {"schema": command_admission.ENVELOPE_SCHEMA, "purpose": "fixture"}
    row = {
        "status": "running",
        "run_id": "current-run",
        "log_path": str(log),
        "summary_json": str(tmp_path / "summary.json"),
        "receipt_context_json": "{}",
        "command_json": json.dumps(command),
        "command_envelope_json": json.dumps(envelope),
    }
    request = {
        "schema": command_admission.EXECUTION_SCHEMA,
        "run_id": row["run_id"],
        "execution_nonce": "a" * 64,
        "envelope": envelope,
        "command": command,
        "result_path": str(result_path),
    }
    opened = {}
    streams = command_identity.execution_transcript_paths(result_path)
    for name, path in streams.items():
        with path.open("xb") as stream:
            opened[name] = command_identity.opened_transcript_identity(path, stream)
    result = {
        "schema": command_admission.EXECUTION_SCHEMA,
        "run_id": row["run_id"],
        "execution_nonce": request["execution_nonce"],
        "envelope": envelope,
        "phase": "command",
        "command_started": True,
        "live_command_transcript": opened,
    }
    request_path.write_text(json.dumps(request), encoding="utf-8")
    result_path.write_text(json.dumps(result), encoding="utf-8")
    return row, request_path, result_path, request, result, streams


def test_live_streams_are_bounded_observations_not_terminal_receipts(
    tmp_path: Path,
) -> None:
    row, _request_path, _result_path, _request, _result, streams = _execution(tmp_path)
    limit = evidence.DIAGNOSTIC_LOG_TAIL_BYTES
    streams["stderr"].write_bytes(
        b"x" * (limit + 37) + b"\nerror[E0425]: missing list_len\n"
    )
    observed = evidence._live_command_evidence(row)
    assert observed.unavailable_reason is None
    assert "error[E0425]" in observed.text
    stderr = next(item for item in observed.observations if item["stream"] == "stderr")
    assert stderr["read_bytes"] == limit
    assert stderr["offset_bytes"] == streams["stderr"].stat().st_size - limit
    assert str(streams["stderr"]) in observed.artifacts
    assert row["status"] == "running"


@pytest.mark.parametrize(
    "field,value",
    [
        ("execution_nonce", "b" * 64),
        ("run_id", "old-run"),
        ("schema", "old-schema"),
        ("envelope", {"different": True}),
    ],
)
def test_stale_or_mismatched_execution_result_is_not_read(
    tmp_path: Path, field: str, value: object
) -> None:
    row, _rp, result_path, _request, result, streams = _execution(tmp_path)
    streams["stderr"].write_text(
        "error[E0133]: secret stale output\n", encoding="utf-8"
    )
    result[field] = value
    result_path.write_text(json.dumps(result), encoding="utf-8")
    observed = evidence._live_command_evidence(row)
    assert (
        observed.unavailable_reason == "execution request/result/row identity mismatch"
    )
    assert observed.text == ""


def test_live_transcript_rejects_boolean_numeric_envelope_substitution(
    tmp_path: Path,
) -> None:
    row, request_path, result_path, request, result, streams = _execution(tmp_path)
    envelope = {"schema": command_admission.ENVELOPE_SCHEMA, "purpose": True}
    row["command_envelope_json"] = json.dumps(envelope)
    request["envelope"] = {**envelope, "purpose": 1}
    result["envelope"] = {**envelope, "purpose": 1}
    streams["stderr"].write_text("error[E0133]: rejected output\n", encoding="utf-8")
    request_path.write_text(json.dumps(request), encoding="utf-8")
    result_path.write_text(json.dumps(result), encoding="utf-8")

    observed = evidence._live_command_evidence(row)

    assert (
        observed.unavailable_reason == "execution request/result/row identity mismatch"
    )
    assert observed.text == ""


@pytest.mark.parametrize(
    "substitution", ["path", "inode", "missing", "command", "result_path"]
)
def test_transcript_admission_rejects_path_file_and_command_substitution(
    tmp_path: Path, substitution: str
) -> None:
    row, request_path, result_path, request, result, streams = _execution(tmp_path)
    streams["stderr"].write_text("error[E0425]: rejected output\n", encoding="utf-8")
    if substitution == "path":
        result["live_command_transcript"]["stderr"]["path"] = str(
            tmp_path / "other.bin"
        )
    elif substitution == "inode":
        result["live_command_transcript"]["stderr"]["inode"] += 1
    elif substitution == "missing":
        result.pop("live_command_transcript")
    elif substitution == "command":
        request["command"] = ["another-command"]
    else:
        request["result_path"] = str(tmp_path / "another.execution.json")
    result_path.write_text(json.dumps(result), encoding="utf-8")
    request_path.write_text(json.dumps(request), encoding="utf-8")
    observed = evidence._live_command_evidence(row)
    assert observed.unavailable_reason
    assert observed.text == ""


def test_error_classification_consumes_live_transcript_without_terminal_claim(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    row, _rp, _xp, _request, _result, streams = _execution(tmp_path)
    streams["stderr"].write_text(
        "error[E0425]: cannot find function list_len\n", encoding="utf-8"
    )
    for name in (
        "_running_guard_timeout_diagnostic",
        "_running_pytest_failures_observed_diagnostic",
        "_running_pytest_current_test_missing_diagnostic",
        "_running_child_missing_diagnostic",
    ):
        monkeypatch.setattr(engine, name, lambda _row: None)
    diagnostics = engine._run_diagnostics(row)
    compiler = next(
        item for item in diagnostics if item["signal_id"] == "rust-compiler-error"
    )
    assert "E0425" in compiler["summary"]
    assert "running" in compiler["summary"]
    assert compiler["observation_state"] == "running"
    assert str(streams["stderr"]) in compiler["artifacts"]
    assert compiler["transcript_observations"]
    assert not diagnostic_model._diagnostics_have_terminal_stale_signal(diagnostics)
    assert row["status"] == "running"
    assert "E0425" not in Path(row["log_path"]).read_text(encoding="utf-8")


def test_terminal_rows_do_not_consume_mutable_live_transcripts(tmp_path: Path) -> None:
    row, _rp, _xp, _request, _result, streams = _execution(tmp_path)
    streams["stderr"].write_text("error[E0425]: mutable output\n", encoding="utf-8")
    row["status"] = "passed"
    assert evidence._live_command_evidence(row) == evidence.LiveCommandEvidence()


def test_replaced_result_during_observation_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    row, _rp, result_path, _request, result, streams = _execution(tmp_path)
    original = command_identity.opened_transcript_identity

    def replace_generation(path, handle):
        identity = original(path, handle)
        result["execution_nonce"] = "b" * 64
        result_path.write_text(json.dumps(result), encoding="utf-8")
        return identity

    monkeypatch.setattr(
        command_identity, "opened_transcript_identity", replace_generation
    )
    observed = evidence._live_command_evidence(row)
    assert observed.text == ""
    assert (
        observed.unavailable_reason
        == "execution authority changed during transcript observation"
    )


def test_read_log_tail_never_reads_past_its_observed_byte_budget(
    tmp_path: Path,
) -> None:
    path = tmp_path / "large.log"
    path.write_bytes(b"prefix" + b"tail")
    assert evidence._read_log_tail(path, limit=4) == "tail"


@pytest.mark.parametrize(
    "failure",
    [None, "stdout-open", "stderr-open", "start", "started-publication", "deadline"],
)
def test_producer_publishes_only_opened_streams_before_start_and_keeps_owned_wait(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: str | None
) -> None:
    row, _request_path, result_path, _request, result, streams = _execution(tmp_path)
    for path in streams.values():
        path.unlink()
    result.pop("live_command_transcript")
    result.update(phase="identity", command_started=False)
    result_path.write_text(json.dumps(result), encoding="utf-8")
    if failure in {"stdout-open", "stderr-open"}:
        streams[failure.removesuffix("-open")].write_bytes(b"prior output")

    events = []
    opened_handles = []
    process = object()
    original_publish = guarded_execution.supervisor._atomic_json

    def publish(path, payload):
        assert path == result_path
        assert all(stream.is_file() for stream in streams.values())
        events.append(("publish", payload["command_started"]))
        if failure == "started-publication" and payload["command_started"]:
            raise OSError("publication failure")
        original_publish(path, payload)

    def start(command, *, cwd, env, stdout, stderr):
        assert list(command) == json.loads(row["command_json"])
        assert cwd == tmp_path and env == {}
        published = json.loads(result_path.read_text(encoding="utf-8"))
        assert published["phase"] == "command"
        assert published["command_started"] is False
        for name, handle in (("stdout", stdout), ("stderr", stderr)):
            assert not handle.closed
            assert published["live_command_transcript"][name] == (
                command_identity.opened_transcript_identity(streams[name], handle)
            )
            opened_handles.append(handle)
        events.append(("start", False))
        if failure == "start":
            raise OSError("launch failure")
        stderr.write(b"error[E0425]: observed at supervisor launch\n")
        stderr.flush()
        observed = evidence._live_command_evidence(row)
        assert observed.unavailable_reason is None
        assert "E0425" in observed.text
        return process

    def wait(child, *, timeout, terminate_timeout):
        assert child is process
        assert 0 < timeout <= 30 and terminate_timeout == 1
        assert all(not handle.closed for handle in opened_handles)
        events.append(("wait", result["command_started"]))
        return 0

    monkeypatch.setattr(guarded_execution.supervisor, "_atomic_json", publish)
    monkeypatch.setattr(
        guarded_execution.admission,
        "_COMMANDS",
        SimpleNamespace(start_owned=start, wait_owned=wait),
    )
    arguments = dict(
        cwd=tmp_path,
        env={},
        result_path=result_path,
        result=result,
        execution_deadline=time.monotonic() + (-1 if failure == "deadline" else 30),
        timeout_seconds=30,
        shutdown_reserve=1,
    )
    if failure is None:
        assert (
            guarded_execution._run_supervisor_with_transcripts(
                json.loads(row["command_json"]), **arguments
            )
            == 0
        )
    else:
        error = subprocess.TimeoutExpired if failure == "deadline" else OSError
        with pytest.raises(error):
            guarded_execution._run_supervisor_with_transcripts(
                json.loads(row["command_json"]), **arguments
            )

    assert all(handle.closed for handle in opened_handles)
    if failure in {"stdout-open", "stderr-open"}:
        assert events == []
        assert "live_command_transcript" not in result
        assert (
            json.loads(result_path.read_text(encoding="utf-8"))["phase"] == "identity"
        )
    elif failure == "start":
        assert events == [("publish", False), ("start", False)]
        assert result["command_started"] is False
    elif failure == "deadline":
        assert events == [("publish", False)]
        assert result["command_started"] is False
    else:
        assert events == [
            ("publish", False),
            ("start", False),
            ("publish", True),
            ("wait", True),
        ]
