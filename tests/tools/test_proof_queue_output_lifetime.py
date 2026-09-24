"""Explicit output lifetime crosses every queue admission boundary."""

from __future__ import annotations

import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3

import pytest

from tools.proof_queue_pkg import (
    cli,
    command_admission as admission,
    commands,
    pact,
    policy,
    runner,
    scheduling,
    state,
)


@pytest.mark.parametrize(
    "argv",
    [
        ["cargo", "check"],
        ["cargo", "+stable", "check", "--all-targets"],
        ["cargo", "test", "--lib"],
        ["cargo", "test", "--", "--exact", "a_test"],
        # --no-run here is a harness operand, not Cargo's compile-only option.
        ["cargo", "test", "--", "--skip", "--no-run"],
    ],
)
def test_only_explicit_check_and_actual_test_execution_are_disposable(argv):
    envelope = admission.envelope_for_command(
        argv, cargo_output_lifetime="terminal-success"
    )
    admission.validate_envelope(envelope, argv)
    assert admission.validated_cargo_output_lifetime(envelope) == "terminal-success"


@pytest.mark.parametrize(
    "argv",
    [
        ["cargo", "build"],
        ["cargo", "doc"],
        ["cargo", "run"],
        ["cargo", "rustc"],
        ["cargo", "bench"],
        ["cargo", "test", "--no-run"],
        ["cargo", "check", "--help"],
        ["cargo", "test", "--", "--list"],
        ["cargo", "--version"],
        ["cargo", "check", "--unit-graph"],
        ["cargo", "test", "--build-plan"],
        ["python", "-c", "print('cargo test')"],
    ],
)
def test_deferred_query_and_unknown_consumers_cannot_be_declared_disposable(argv):
    with pytest.raises(ValueError):
        admission.envelope_for_command(argv, cargo_output_lifetime="terminal-success")
    with pytest.raises(ValueError):
        admission.admission_envelope(argv, cargo_output_lifetime="terminal-success")


@pytest.mark.parametrize("value", [None, True, 1, [], {}, "", "auto", "failed"])
def test_lifetime_is_a_strict_declaration(value):
    with pytest.raises(ValueError, match="cargo_output_lifetime"):
        admission.envelope_for_command(["cargo", "test"], cargo_output_lifetime=value)


def test_default_envelopes_keep_historical_retaining_shape():
    command = ["cargo", "test"]
    envelope = admission.envelope_for_command(command)
    assert "cargo_output_lifetime" not in envelope
    assert admission.validated_cargo_output_lifetime(envelope) == "retain"
    admission.validate_envelope(envelope, command)


def test_canonical_guard_delegation_carries_outer_lifetime():
    command = policy._canonical_cargo_proof_command(["test", "--lib"])
    envelope = admission.envelope_for_command(
        command, cargo_output_lifetime="terminal-success"
    )
    assert envelope["delegated"]["argv"][:2] == ["cargo", "test"]
    assert "cargo_output_lifetime" not in envelope["delegated"]
    admission.validate_envelope(envelope, command)
    envelope["delegated"]["cargo_output_lifetime"] = "terminal-success"
    with pytest.raises(ValueError, match="does not match"):
        admission.validate_envelope(envelope, command)


@pytest.mark.parametrize("insert", [scheduling._insert_run, scheduling._admit_run])
def test_both_database_admission_paths_freeze_disposition(tmp_path, insert):
    db = tmp_path / "queue.sqlite3"
    command = policy._canonical_cargo_proof_command(["test", "--lib"])
    with closing(state._connect(db)) as conn:
        insert(
            conn,
            run_id="lifetime",
            logical_id="lifetime",
            reason="proof only",
            command=command,
            cwd=tmp_path,
            resource_family="rust",
            contention_key="rust",
            scopes=[],
            git_snapshot={},
            log_path=tmp_path / "proof.log",
            summary_json=tmp_path / "proof.guard.json",
            cargo_output_lifetime="terminal-success",
        )
        row = state._row_by_run_id(conn, "lifetime")
        envelope = json.loads(row["command_envelope_json"])
        assert envelope["cargo_output_lifetime"] == "terminal-success"
        admission.validate_envelope(envelope, command)
        request_path, _result, request_envelope, _nonce = (
            runner._write_execution_request(
                row=row,
                command=command,
                repo_root=tmp_path,
                resource_family="rust",
                run_id="lifetime",
                env_override_names=[],
                log_path=tmp_path / "proof.log",
                summary_path=tmp_path / "proof.guard.json",
                timeout_seconds=10.0,
            )
        )
        assert request_envelope == envelope
        assert json.loads(request_path.read_text())["envelope"] == envelope
        with pytest.raises(sqlite3.IntegrityError):
            conn.execute(
                "UPDATE proof_runs SET command_envelope_json = '{}' WHERE run_id = 'lifetime'"
            )


@pytest.mark.parametrize("detach", [False, True])
@pytest.mark.parametrize("subcommand", ["cargo", "exec"])
def test_cli_inline_and_detached_propagate_lifetime(monkeypatch, detach, subcommand):
    recorded = {}

    def capture(args, **kwargs):
        recorded.update(kwargs)
        return (2, None) if detach else 0

    monkeypatch.setattr(runner, "_queue_one" if detach else "_run_one", capture)
    argv = [
        subcommand,
        "--id",
        "unit",
        "--reason",
        "proof only",
        "--cargo-output-lifetime",
        "terminal-success",
    ]
    if detach:
        argv.append("--detach")
    argv += [
        "--",
        *(
            ["test", "--lib"]
            if subcommand == "cargo"
            else policy._canonical_cargo_proof_command(["test", "--lib"])
        ),
    ]
    cli.main(argv)
    assert recorded["cargo_output_lifetime"] == "terminal-success"


def test_toml_submit_persists_lifetime_and_does_not_admit_raw_cargo(
    tmp_path, monkeypatch
):
    command = policy._canonical_cargo_proof_command(["check"])
    monkeypatch.setattr(
        commands,
        "_load_specs",
        lambda path: [
            {
                "id": "unit",
                "command": command,
                "cargo_output_lifetime": "terminal-success",
            }
        ],
    )
    monkeypatch.setattr(
        commands.evidence, "_try_write_marimo_notebook", lambda *args, **kwargs: None
    )
    args = argparse.Namespace(
        db=str(tmp_path / "queue.sqlite3"),
        logs_root=str(tmp_path / "runs"),
        repo_root=str(tmp_path),
        dsl="unused",
    )
    assert commands._cmd_submit(args) == 0
    with closing(state._connect(Path(args.db))) as conn:
        raw = conn.execute("SELECT command_envelope_json FROM proof_runs").fetchone()[0]
    assert json.loads(raw)["cargo_output_lifetime"] == "terminal-success"
    monkeypatch.setattr(
        commands,
        "_load_specs",
        lambda path: [
            {
                "id": "raw",
                "command": ["cargo", "check"],
                "cargo_output_lifetime": "terminal-success",
            }
        ],
    )
    with pytest.raises(SystemExit):
        commands._cmd_submit(args)


def test_named_print_spec_keeps_explicit_declaration(capsys):
    command = policy._canonical_cargo_proof_command(["check"])
    spec = {
        "logical_id": "unit",
        "reason": "proof only",
        "command": command,
        "resource_family": "rust",
        "contention_key": "rust",
        "scopes": [],
        "env_overrides": {},
        "notes": [],
        "timeout": 10.0,
        "cargo_output_lifetime": "terminal-success",
    }
    assert pact._run_named_spec(argparse.Namespace(env=[], print_spec=True), spec) == 0
    assert (
        json.loads(capsys.readouterr().out)["cargo_output_lifetime"]
        == "terminal-success"
    )


@pytest.mark.parametrize("queue", [False, True])
def test_invalid_disposition_is_rejected_before_database_or_dispatch(tmp_path, queue):
    args = argparse.Namespace(db=str(tmp_path / "must-not-exist.sqlite3"))
    kwargs = dict(
        logical_id="bad",
        reason="bad",
        command=policy._canonical_cargo_proof_command(["test", "--no-run"]),
        resource_family="rust",
        contention_key="rust",
        scopes=[],
        env_overrides={},
        cargo_output_lifetime="terminal-success",
    )
    result = (
        runner._queue_one(args, **kwargs)
        if queue
        else runner._run_one(args, timeout=10, **kwargs)
    )
    assert result == ((2, None) if queue else 2)
    assert not Path(args.db).exists()
