"""The cleanup CLI consumes persisted proof custody, never loose target paths."""

from __future__ import annotations

import argparse
from contextlib import closing
import hashlib
import json
from pathlib import Path
import sqlite3

import pytest

from molt import disk_capacity
from tools.proof_queue_pkg import (
    cargo_cache_custody as cache,
    cargo_output_environment,
    command_admission,
    cli,
    commands,
    evidence,
    execution_environment,
    runner,
    scheduling,
    state,
    supervisor_custody,
)


@pytest.fixture
def generation(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, request):
    parameters = getattr(request, "param", (101, True))
    command_rc, source_eligible = parameters[:2]
    sealed = len(parameters) == 3 and parameters[2]
    nonce = "reclamation-fixture"
    nonce_hash = hashlib.sha256(nonce.encode()).hexdigest()
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda path: 100 * 1024**3
    )
    source = tmp_path / "source"
    source.mkdir()
    (source / "main.rs").write_text("fn main() {}", encoding="utf-8")
    monkeypatch.setattr(
        execution_environment,
        "_git_source_paths",
        lambda root, env, **kw: [source / "main.rs"],
    )
    result_root = tmp_path / "runs"
    source_content, _, _ = execution_environment.capture_source_content(
        source_root=source,
        env={},
        overlays=(),
        cas_root=result_root / "custody-cas",
        hash_workers=1,
    )
    lease = cache.acquire(
        result_root=result_root,
        source_root=source,
        toolchains={},
        command=["cargo", "test"],
        outputs=cargo_output_environment.CargoOutputEnvironment.for_envelope(
            command_admission.envelope_for_command(["cargo", "test"])
        ),
        env={},
        requested_target="unused",
        timeout_s=0.0,
        run_id="reclaim-unit",
        execution_nonce_sha256=nonce_hash,
        source_snapshot={"root": str(source), "commit": "fixture"},
        source_content=source_content,
    )
    (lease.target / "partial.rlib").write_bytes(b"partial artifact")
    if sealed:
        # This fixture tests the persisted CLI boundary, not process capture.
        # Keep real seal/manifest/CAS publication and terminal binding below.
        monkeypatch.setattr(cache, "_complete_custody", lambda result, **kwargs: True)
        lease.publish({})
    lease.close()
    args = argparse.Namespace(
        db=str(tmp_path / "queue.sqlite3"),
        logs_root=str(result_root),
        repo_root=str(source),
        run_id="reclaim-unit",
        apply=False,
    )
    with closing(state._connect(Path(args.db))) as conn:
        scheduling._insert_run(
            conn,
            run_id=args.run_id,
            logical_id="reclaim-unit",
            reason="fixture",
            command=["cargo", "test"],
            cwd=source,
            resource_family="cargo",
            contention_key="cargo",
            scopes=[],
            git_snapshot={},
            log_path=result_root / "unit.log",
            summary_json=result_root / "unit.guard.json",
        )
        row = state._row_by_run_id(conn, args.run_id)
        envelope = json.loads(row["command_envelope_json"])
        context = {
            "run_id": args.run_id,
            "execution_nonce_sha256": nonce_hash,
            "command_envelope": envelope,
            "command_envelope_sha256": hashlib.sha256(
                json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "derived_root_custody": {"prelaunch": [lease.provenance]},
            "guard_receipt": {"sha256": "c" * 64},
            "process_supervisor": {"receipt": {"state": "COMPLETE"}},
            "source_custody": {
                "identical": True,
                "evidence_eligible": source_eligible,
                "ineligible_reasons": []
                if source_eligible
                else ["source-dirty-prelaunch"],
            },
        }
        context["execution_custody_sha256"] = (
            supervisor_custody.execution_custody_sha256(
                context, run_id=args.run_id, returncode=command_rc
            )
        )
        outcome, context = runner._finalize_execution_receipt(
            execution_record={
                "phase": "complete",
                "command_started": True,
                "command_returncode": command_rc,
                "cargo_cache": lease.provenance,
                "cargo_cache_publication": lease.publication_outcome,
            },
            execution_path=result_root / "unit.execution.json",
            run_id=args.run_id,
            execution_nonce=nonce,
            receipt_context=context,
            process_cleanup_safe=True,
            status="passed" if command_rc == 0 else "failed",
            returncode=command_rc,
            execution_error=None,
        )
        state._update_run(
            conn,
            args.run_id,
            status=outcome["status"],
            returncode=outcome["returncode"],
            finished_at="2026-01-01T00:00:00Z",
            receipt_context_json=json.dumps(context),
        )
    return args, lease, context


@pytest.mark.parametrize(
    "generation,expected_status,expected_rc",
    [
        ((0, False), "non-evidence", 2),
        ((0, True), "passed", 0),
        ((101, True), "failed", 101),
        ((101, False), "failed", 101),
    ],
    indirect=["generation"],
)
def test_final_queue_outcome_binds_database_generation_and_retention(
    generation, expected_status, expected_rc, capsys
):
    args, lease, context = generation
    record = json.loads((Path(args.logs_root) / "unit.execution.json").read_text())
    assert record["receipt_context"] == context
    outcome = context["queue_terminal"]
    assert outcome["status"] == expected_status
    assert outcome["returncode"] == expected_rc
    assert outcome["command_returncode"] == record["command_returncode"]
    assert context[
        "execution_custody_sha256"
    ] == supervisor_custody.execution_custody_sha256(
        context, run_id=args.run_id, returncode=record["command_returncode"]
    )
    with closing(state._connect(Path(args.db))) as conn:
        row = state._row_by_run_id(conn, args.run_id)
        evidence._validate_terminal_evidence(row, context)
        provenance, projection = evidence._cargo_generation_terminal(
            row, context, required=True
        )
    terminal = cache.validate_terminal_receipt(
        result_root=Path(args.logs_root),
        provenance=provenance,
        projection=projection,
        run_id=args.run_id,
        execution_nonce_sha256=context["execution_nonce_sha256"],
    )
    assert terminal["queue_terminal"] == outcome
    assert terminal["command_returncode"] == record["command_returncode"]
    owner_before = lease.owner_path.read_bytes()
    assert commands._cmd_reclaim_cargo_generation(args) == 0
    assert json.loads(capsys.readouterr().out)["reclaim_eligible"] is True
    assert lease.owner_path.read_bytes() == owner_before
    args.apply = True
    assert commands._cmd_reclaim_cargo_generation(args) == 0
    assert json.loads(capsys.readouterr().out)["state"] == "reclaimed"
    assert not lease.target.exists()


def test_parser_requires_explicit_apply():
    parsed = cli._build_parser().parse_args(
        ["reclaim-cargo-generation", "--run-id", "unit"]
    )
    assert parsed.func is commands._cmd_reclaim_cargo_generation
    assert parsed.apply is False

    retire = cli._build_parser().parse_args(
        ["retire-terminal-sealed-generation", "--run-id", "unit"]
    )
    assert retire.func is commands._cmd_retire_terminal_sealed_generation
    assert retire.apply is False


@pytest.mark.parametrize("generation", [(101, True, True)], indirect=True)
def test_retirement_cli_preserves_terminal_receipt_and_records_disposition(
    generation, capsys
):
    args, lease, context = generation
    before = lease.owner_path.read_bytes()
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    assert json.loads(capsys.readouterr().out)["retirement_eligible"] is True
    assert lease.owner_path.read_bytes() == before
    args.apply = True
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["state"] == "retired-sealed"
    assert not lease.target.exists()
    assert json.loads(lease.owner_path.read_text())["lifecycle"] == "retired-sealed"
    assert json.loads(lease.pointer.read_text())["state"] == "retired-sealed"
    with closing(state._connect(Path(args.db))) as conn:
        row = state._row_by_run_id(conn, args.run_id)
        assert json.loads(row["receipt_context_json"]) == context
        evidence._validate_terminal_evidence(row, context)
        assert {row[0] for row in conn.execute("SELECT kind FROM proof_notes")} == {
            "decision",
            "finding",
        }


def test_default_inspection_preserves_artifacts_and_queue(generation, capsys):
    args, lease, _ = generation
    before = lease.owner_path.read_bytes()
    assert commands._cmd_reclaim_cargo_generation(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["reclaim_eligible"] is True
    assert lease.target.is_dir() and lease.owner_path.read_bytes() == before
    with closing(sqlite3.connect(args.db)) as conn:
        assert conn.execute("SELECT count(*) FROM proof_notes").fetchone()[0] == 0


def test_sealed_retirement_inspection_and_apply_reject_unsealed_generation(
    generation, capsys
):
    args, lease, _ = generation
    before = lease.owner_path.read_bytes()
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    inspection = json.loads(capsys.readouterr().out)
    assert inspection["retirement_eligible"] is False
    assert inspection["retirement_reason"] == "owner-lifecycle-not-retirable"
    assert lease.target.is_dir() and lease.owner_path.read_bytes() == before

    args.apply = True
    assert commands._cmd_retire_terminal_sealed_generation(args) == 2
    outcome = json.loads(capsys.readouterr().out)
    assert outcome["state"] == "not-retirable"
    assert lease.target.is_dir()
    with closing(sqlite3.connect(args.db)) as conn:
        assert conn.execute("SELECT count(*) FROM proof_notes").fetchone()[0] == 2


def test_apply_reclaims_fixture_and_retains_original_terminal_custody(
    generation, capsys
):
    args, lease, context = generation
    args.apply = True
    assert commands._cmd_reclaim_cargo_generation(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["state"] == "reclaimed" and not lease.target.exists()
    with closing(state._connect(Path(args.db))) as conn:
        row = state._row_by_run_id(conn, args.run_id)
        assert json.loads(row["receipt_context_json"]) == context
        evidence._validate_terminal_evidence(row, context)
        assert evidence._cargo_generation_terminal(row, context, required=True)
        assert conn.execute("SELECT count(*) FROM proof_notes").fetchone()[0] == 2


@pytest.mark.parametrize(
    "generation,command",
    [
        ((101, True), commands._cmd_reclaim_cargo_generation),
        ((101, True, True), commands._cmd_retire_terminal_sealed_generation),
    ],
    indirect=["generation"],
)
@pytest.mark.parametrize(
    "mutation",
    [
        "queued",
        "dispatched",
        "running",
        "stale",
        "blocked",
        "no-context",
        "no-digest",
        "wrong-digest",
        "wrong-custody",
        "wrong-generation",
        "legacy",
        "no-finished-at",
        "no-queue-outcome",
        "wrong-queue-status",
        "wrong-queue-returncode",
        "wrong-command-returncode",
    ],
)
def test_apply_rejects_ambiguous_or_substituted_custody(
    generation, command, capsys, mutation
):
    args, lease, context = generation
    updates = {}
    if mutation in {"queued", "dispatched", "running", "stale", "blocked"}:
        updates["status"] = mutation
    elif mutation == "no-finished-at":
        updates["finished_at"] = None
    elif mutation == "no-context":
        updates["receipt_context_json"] = None
    else:
        if mutation == "no-digest":
            context.pop("terminal_evidence_sha256")
        elif mutation == "wrong-digest":
            context["terminal_evidence_sha256"] = "d" * 64
        else:
            if mutation == "no-queue-outcome":
                context.pop("queue_terminal")
            elif mutation == "wrong-queue-status":
                context["queue_terminal"]["status"] = "non-evidence"
            elif mutation == "wrong-queue-returncode":
                context["queue_terminal"]["returncode"] = 2
            elif mutation == "wrong-command-returncode":
                context["queue_terminal"]["command_returncode"] = 0
            elif mutation == "wrong-custody":
                context["execution_custody_sha256"] = "d" * 64
            elif mutation == "wrong-generation":
                context["cargo_generation_lifecycle"]["generation_id"] = "d" * 16
            else:
                context.pop("cargo_generation_lifecycle")
                context.pop("derived_root_custody")
            context["terminal_evidence_sha256"] = (
                supervisor_custody.terminal_evidence_sha256(
                    context, run_id=args.run_id, returncode=101
                )
            )
        updates["receipt_context_json"] = json.dumps(context)
    with closing(state._connect(Path(args.db))) as conn:
        state._update_run(conn, args.run_id, **updates)
    args.apply = True
    before = lease.owner_path.read_bytes()
    assert command(args) == 2
    assert json.loads(capsys.readouterr().out)["state"] == "not-authorized"
    assert lease.target.is_dir() and lease.owner_path.read_bytes() == before


def test_missing_database_inspection_does_not_provision_state(tmp_path, capsys):
    db = tmp_path / "missing" / "queue.sqlite3"
    args = argparse.Namespace(
        db=str(db),
        logs_root=None,
        repo_root=str(tmp_path),
        run_id="missing",
        apply=False,
    )
    assert commands._cmd_reclaim_cargo_generation(args) == 2
    assert json.loads(capsys.readouterr().out)["state"] == "inspection-failed"
    assert not db.parent.exists()


@pytest.mark.parametrize(
    "generation,command,expected_state",
    [
        ((101, True), commands._cmd_reclaim_cargo_generation, "reclaimed"),
        (
            (101, True, True),
            commands._cmd_retire_terminal_sealed_generation,
            "retired-sealed",
        ),
    ],
    indirect=["generation"],
)
def test_post_delete_note_error_does_not_claim_artifacts_were_retained(
    generation, command, expected_state, monkeypatch, capsys
):
    args, lease, _ = generation
    insert = state._insert_note

    def fail_outcome(conn, **kwargs):
        if kwargs["kind"] == "finding":
            raise sqlite3.OperationalError("fixture note write failure")
        return insert(conn, **kwargs)

    monkeypatch.setattr(state, "_insert_note", fail_outcome)
    args.apply = True
    assert command(args) == 2
    result = json.loads(capsys.readouterr().out)
    assert result["state"] == expected_state and "error" in result
    assert not lease.target.exists()
