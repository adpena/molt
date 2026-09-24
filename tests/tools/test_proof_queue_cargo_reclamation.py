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
    cargo_output_lifecycle,
    cargo_output_layout,
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
    sealed = len(parameters) >= 3 and parameters[2]
    lifetime = parameters[3] if len(parameters) >= 4 else "retain"
    external = len(parameters) >= 5 and parameters[4]
    external_root = tmp_path / "external-output"
    if external:
        external_root.mkdir()
    envelope = command_admission.envelope_for_command(
        ["cargo", "test"],
        cargo_output_lifetime=lifetime,
        cargo_output_root=str(external_root) if external else None,
    )
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
        cargo_output_lifetime=lifetime,
        cargo_output_root=envelope.get("cargo_output_root"),
        result_root=result_root,
        source_root=source,
        toolchains={},
        command=["cargo", "test"],
        outputs=cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope),
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
        allow_passed=False,
    )
    with closing(state._connect(Path(args.db))) as conn:
        scheduling._insert_run(
            conn,
            cargo_output_lifetime=lifetime,
            cargo_output_root=str(external_root) if external else None,
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


@pytest.mark.parametrize("invalid", ["false", "true", 0, 1, None])
def test_retirement_policy_rejects_truthy_nonboolean_opt_in(invalid):
    with pytest.raises(ValueError, match="must be boolean"):
        commands._cmd_retire_terminal_sealed_generation(
            argparse.Namespace(run_id="unused", apply=False, allow_passed=invalid)
        )


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
    assert retire.allow_passed is False

    admitted = cli._build_parser().parse_args(
        [
            "retire-terminal-sealed-generation",
            "--run-id",
            "unit",
            "--allow-passed",
        ]
    )
    assert admitted.allow_passed is True


@pytest.mark.parametrize("generation", [(101, True, True)], indirect=True)
def test_retirement_cli_preserves_terminal_receipt_and_records_disposition(
    generation, capsys
):
    args, lease, context = generation
    before = lease.owner_path.read_bytes()
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    inspection = json.loads(capsys.readouterr().out)
    assert inspection["retirement_eligible"] is True
    assert inspection["terminal_status"] == "failed"
    assert inspection["retirement_policy"] == {
        "allow_passed": False,
        "eligible_terminal_statuses": ["failed"],
    }
    assert lease.owner_path.read_bytes() == before
    args.apply = True
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["state"] == "retired-sealed"
    assert (
        isinstance(result["disposition_elapsed_s"], float)
        and result["disposition_elapsed_s"] >= 0
    )
    assert not lease.target.exists()
    assert json.loads(lease.owner_path.read_text())["lifecycle"] == "retired-sealed"
    assert json.loads(lease.pointer.read_text())["state"] == "retired-sealed"
    with closing(state._connect(Path(args.db))) as conn:
        row = state._row_by_run_id(conn, args.run_id)
        assert json.loads(row["receipt_context_json"]) == context
        evidence._validate_terminal_evidence(row, context)
        notes = list(
            conn.execute("SELECT kind, body FROM proof_notes ORDER BY note_id")
        )
        assert {row[0] for row in notes} == {
            "decision",
            "finding",
        }
        for _, body in notes:
            note = json.loads(body)
            assert note["terminal_status"] == "failed"
            assert note["retirement_policy"] == inspection["retirement_policy"]


@pytest.mark.parametrize("generation", [(0, True, True)], indirect=True)
def test_retirement_cli_requires_opt_in_and_records_passed_policy(generation, capsys):
    args, lease, _ = generation
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    denied = json.loads(capsys.readouterr().out)
    assert denied["retirement_eligible"] is False
    assert denied["retirement_reason"] == "terminal-run-not-failed"
    assert denied["terminal_status"] == "passed"
    assert lease.target.is_dir()

    args.allow_passed = True
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    admitted = json.loads(capsys.readouterr().out)
    policy = {
        "allow_passed": True,
        "eligible_terminal_statuses": ["failed", "passed"],
    }
    assert admitted["retirement_eligible"] is True
    assert admitted["retirement_policy"] == policy
    assert admitted["terminal_status"] == "passed"
    assert lease.target.is_dir()

    args.apply = True
    assert commands._cmd_retire_terminal_sealed_generation(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["state"] == "retired-sealed"
    assert result["retirement_policy"] == policy
    assert result["terminal_status"] == "passed"
    assert not lease.target.exists()
    with closing(state._connect(Path(args.db))) as conn:
        notes = list(
            conn.execute("SELECT kind, body FROM proof_notes ORDER BY note_id")
        )
    assert [kind for kind, _ in notes] == ["decision", "finding"]
    for _, body in notes:
        note = json.loads(body)
        assert note["retirement_policy"] == policy
        assert note["terminal_status"] == "passed"


def test_default_inspection_preserves_artifacts_and_queue(generation, capsys):
    args, lease, _ = generation
    before = lease.owner_path.read_bytes()
    assert commands._cmd_reclaim_cargo_generation(args) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["reclaim_eligible"] is True
    assert lease.target.is_dir() and lease.owner_path.read_bytes() == before
    with closing(sqlite3.connect(args.db)) as conn:
        assert conn.execute("SELECT count(*) FROM proof_notes").fetchone()[0] == 0


@pytest.mark.parametrize("generation", [(0, True)], indirect=True)
def test_passed_opt_in_inspection_and_apply_reject_unsealed_generation(
    generation, capsys
):
    args, lease, _ = generation
    args.allow_passed = True
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
        ((0, True, True), commands._cmd_retire_terminal_sealed_generation),
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
    if command is commands._cmd_retire_terminal_sealed_generation:
        args.allow_passed = True
    updates = {}
    with closing(state._connect(Path(args.db))) as conn:
        database_returncode = state._row_by_run_id(conn, args.run_id)["returncode"]
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
                original_returncode = context["queue_terminal"]["command_returncode"]
                context["queue_terminal"]["command_returncode"] = (
                    101 if original_returncode == 0 else 0
                )
            elif mutation == "wrong-custody":
                context["execution_custody_sha256"] = "d" * 64
            elif mutation == "wrong-generation":
                context["cargo_generation_lifecycle"]["generation_id"] = "d" * 16
            else:
                context.pop("cargo_generation_lifecycle")
                context.pop("derived_root_custody")
            context["terminal_evidence_sha256"] = (
                supervisor_custody.terminal_evidence_sha256(
                    context, run_id=args.run_id, returncode=database_returncode
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


def _persisted_terminal_bytes(args):
    with closing(state._connect(Path(args.db))) as conn:
        row = state._row_by_run_id(conn, args.run_id)
        return row["status"], row["returncode"], row["receipt_context_json"]


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_declared_success_retires_after_commit_without_rewriting_receipt(generation):
    args, lease, context = generation
    before = _persisted_terminal_bytes(args)
    receipt = (Path(args.logs_root) / "unit.execution.json").read_bytes()
    result = cargo_output_lifecycle.finalize_declared_success(
        Path(args.db), args.run_id
    )
    assert result["state"] == "retired-sealed"
    assert isinstance(result["disposition_elapsed_s"], float)
    assert result["disposition_elapsed_s"] >= 0
    assert not lease.target.exists()
    assert _persisted_terminal_bytes(args) == before
    assert (Path(args.logs_root) / "unit.execution.json").read_bytes() == receipt
    assert context["cargo_generation_lifecycle"]["state"] == "terminal-sealed-retained"
    with closing(state._connect(Path(args.db))) as conn:
        notes = state._notes_for_run_ids(conn, [args.run_id])[args.run_id]
        assert [json.loads(note["body"])["state"] for note in notes] == [
            "requested",
            "retired-sealed",
        ]
        assert json.loads(notes[-1]["body"])["disposition_elapsed_s"] >= 0
    assert (
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)[
            "state"
        ]
        == "retired-sealed"
    )
    # Completed history is not reopened on the next launch.
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert _persisted_terminal_bytes(args) == before


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success", True)], indirect=True
)
def test_external_retirement_preserves_metadata_siblings_and_offline_receipt(
    generation, monkeypatch
):
    args, lease, context = generation
    declaration = lease.provenance["cargo_output_root"]
    root = Path(declaration["path"])
    assert lease.target.is_relative_to(root)
    owner = Path(lease.provenance["generation_owner"])
    assert owner.is_relative_to(Path(args.logs_root))
    sibling = lease.target.parent / "retained-sibling"
    sibling.mkdir()
    (sibling / "keep.rlib").write_bytes(b"retained")
    before = _persisted_terminal_bytes(args)
    cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    assert not lease.target.exists()
    assert (sibling / "keep.rlib").read_bytes() == b"retained"
    assert owner.exists() and (owner.parent.parent / "target.lock").exists()
    assert (Path(args.logs_root) / "custody-cas").is_dir()
    assert _persisted_terminal_bytes(args) == before

    def absent(raw):
        raise OSError("volume offline")

    monkeypatch.setattr(cargo_output_layout, "declare_root", absent)
    with closing(state._connect(Path(args.db))) as conn:
        row = state._row_by_run_id(conn, args.run_id)
        evidence._validate_terminal_evidence(row, context)
        evidence._cargo_generation_terminal(row, context, required=True)


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success", True)], indirect=True
)
@pytest.mark.parametrize("lifecycle", ["terminal-sealed-retained", "retiring-sealed"])
@pytest.mark.parametrize("unavailable", [False, True])
def test_missing_or_replaced_external_root_never_authorizes_retirement(
    generation, monkeypatch, lifecycle, unavailable
):
    args, lease, _ = generation
    owner_path = Path(lease.provenance["generation_owner"])
    owner = json.loads(owner_path.read_text())
    owner["lifecycle"] = lifecycle
    owner_path.write_text(json.dumps(owner))
    original = cargo_output_layout.declare_root

    def replaced(raw):
        if unavailable:
            raise OSError("selected volume offline")
        value = original(raw)
        return {**value, "inode": value["inode"] + 1}

    monkeypatch.setattr(cargo_output_layout, "declare_root", replaced)
    monkeypatch.setattr(
        cache,
        "delete_path",
        lambda path: pytest.fail("unavailable root reached deletion"),
    )
    with pytest.raises(ValueError, match="unavailable|replaced or remounted"):
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    assert lease.target.exists()
    assert json.loads(owner_path.read_text())["lifecycle"] == lifecycle


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_caught_disposition_error_persists_elapsed_metric(generation, monkeypatch):
    args, lease, _ = generation

    def fail(**kwargs):
        raise OSError("fixture disposal error")

    monkeypatch.setattr(cache, "retire_terminal_sealed", fail)
    with pytest.raises(OSError, match="fixture disposal error"):
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    with closing(state._connect(Path(args.db))) as conn:
        notes = [
            json.loads(note["body"])
            for note in state._notes_for_run_ids(conn, [args.run_id])[args.run_id]
        ]
    finding = next(
        note for note in notes if note.get("state") == "retirement-indeterminate"
    )
    assert isinstance(finding["disposition_elapsed_s"], float)
    assert finding["disposition_elapsed_s"] >= 0
    assert lease.target.exists()


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success", True)], indirect=True
)
def test_external_root_is_revalidated_after_identity_lock_wait(generation, monkeypatch):
    args, lease, context = generation
    acquire_lock = cache._acquire_file_lock
    declare_root = cargo_output_layout.declare_root
    locked = False

    def acquire(*args, **kwargs):
        nonlocal locked
        handle = acquire_lock(*args, **kwargs)
        locked = True
        return handle

    def root(raw):
        value = declare_root(raw)
        return {**value, "inode": value["inode"] + 1} if locked else value

    monkeypatch.setattr(cache, "_acquire_file_lock", acquire)
    monkeypatch.setattr(cargo_output_layout, "declare_root", root)
    monkeypatch.setattr(
        cache, "delete_path", lambda path: pytest.fail("replaced root reached deletion")
    )
    with pytest.raises(ValueError, match="replaced or remounted"):
        cache.retire_terminal_sealed(
            result_root=Path(args.logs_root),
            provenance=lease.provenance,
            projection=context["cargo_generation_lifecycle"],
            allow_passed=True,
        )
    assert locked and lease.target.exists()


@pytest.mark.parametrize(
    "generation",
    [
        (0, True, True),
        (101, True, True, "terminal-success"),
        (0, False, True, "terminal-success"),
    ],
    indirect=True,
)
def test_defaults_failures_and_non_evidence_are_retained(generation):
    args, lease, _ = generation
    before = _persisted_terminal_bytes(args)
    assert (
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
        is None
    )
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert lease.target.exists()
    assert _persisted_terminal_bytes(args) == before


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_interruption_before_finalization_is_resumed_from_committed_run(generation):
    args, lease, _ = generation
    before = _persisted_terminal_bytes(args)
    assert lease.target.exists()
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert not lease.target.exists()
    assert _persisted_terminal_bytes(args) == before


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_manual_policy_refusal_does_not_consume_declared_finalization(
    generation, capsys
):
    args, lease, _ = generation
    before = _persisted_terminal_bytes(args)
    args.apply = True
    args.allow_passed = False
    commands._cmd_terminal_cargo_disposition(args, retire=True)
    refused = json.loads(capsys.readouterr().out)
    assert refused["state"] == "not-retirable"
    assert lease.target.exists()
    with closing(state._connect(Path(args.db))) as conn:
        assert cargo_output_lifecycle.unresolved_dispositions(conn) == []
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert not lease.target.exists()
    assert _persisted_terminal_bytes(args) == before


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_finalizer_never_deletes_before_terminal_commit(generation):
    args, lease, _ = generation
    with closing(state._connect(Path(args.db))) as conn:
        state._update_run(
            conn, args.run_id, status="running", returncode=None, finished_at=None
        )
        conn.execute(
            "UPDATE proof_runs SET status='passed', returncode=0, finished_at='2026-01-01T00:00:00Z' WHERE run_id=?",
            (args.run_id,),
        )
        # A separate persisted-authority reader cannot observe an uncommitted
        # success. A failed commit/rollback must never authorize deletion.
        assert (
            cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
            is None
        )
        conn.rollback()
    assert (
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
        is None
    )
    assert lease.target.exists()


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_disposition_failure_is_loud_separate_and_never_automatically_retried(
    generation, monkeypatch
):
    args, lease, _ = generation
    before = _persisted_terminal_bytes(args)
    attempts = []

    def refuse(path):
        attempts.append(path)
        return False, "fixture target in use"

    monkeypatch.setattr(cache, "delete_path", refuse)
    with pytest.raises(RuntimeError, match="finalization failed"):
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    assert len(attempts) == 1 and lease.target.exists()
    assert _persisted_terminal_bytes(args) == before
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert len(attempts) == 1
    with closing(state._connect(Path(args.db))) as conn:
        unresolved = cargo_output_lifecycle.unresolved_dispositions(conn)
    assert unresolved[0]["state"] == "finalization-unresolved"


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
@pytest.mark.parametrize("lifecycle", ["retire-blocked", "retiring-sealed"])
def test_pending_recovery_records_but_does_not_retry_partial_retirement(
    generation, lifecycle, monkeypatch, capsys
):
    args, lease, _ = generation
    owner_path = Path(lease.provenance["generation_owner"])
    owner = json.loads(owner_path.read_text())
    owner["lifecycle"] = lifecycle
    cache._write_owner(owner_path, owner)
    monkeypatch.setattr(
        cache, "delete_path", lambda path: pytest.fail("partial deletion retried")
    )
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert "finalization unresolved" in capsys.readouterr().err
    assert lease.target.exists()
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert not capsys.readouterr().err


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_retired_tombstone_closes_interrupted_append_only_outcome(generation):
    args, lease, context = generation
    cache.retire_terminal_sealed(
        result_root=Path(args.logs_root),
        provenance=lease.provenance,
        projection=context["cargo_generation_lifecycle"],
        allow_passed=True,
    )
    assert not lease.target.exists()
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    with closing(state._connect(Path(args.db))) as conn:
        notes = state._notes_for_run_ids(conn, [args.run_id])[args.run_id]
    assert json.loads(notes[-1]["body"])["state"] == "retired-sealed"


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_owner_lifetime_substitution_cannot_authorize_or_hide_disposal(generation):
    args, lease, _ = generation
    owner_path = Path(lease.provenance["generation_owner"])
    owner = json.loads(owner_path.read_text())
    owner["cargo_output_lifetime"] = "retain"
    cache._write_owner(owner_path, owner)
    with pytest.raises(ValueError, match="lifetime mismatch"):
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    assert lease.target.exists()


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
def test_recovery_batch_can_be_bounded_without_reading_completed_artifacts(
    generation, monkeypatch
):
    args, lease, _ = generation
    with pytest.raises(ValueError, match="batch limit"):
        cargo_output_lifecycle.resume_declared_successes(Path(args.db), limit=0)
    assert lease.target.exists()
    cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    monkeypatch.setattr(
        cargo_output_lifecycle,
        "finalize_declared_success",
        lambda *args: pytest.fail("completed artifact reread"),
    )
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))


@pytest.mark.parametrize(
    "generation", [(0, True, True, "terminal-success")], indirect=True
)
@pytest.mark.parametrize(
    "extra",
    [
        {},
        {"schema": "unrelated"},
        {
            "schema": "molt.proof-cargo-generation-sealed-retirement.v1",
            "run_id": "someone-else",
        },
    ],
)
def test_unrelated_findings_do_not_suppress_pending_cleanup_or_pollute_status(
    generation, extra
):
    args, lease, _ = generation
    with closing(state._connect(Path(args.db))) as conn:
        state._insert_note(
            conn,
            run_id=args.run_id,
            kind="finding",
            body=json.dumps(
                {
                    "cargo_output_lifetime": "terminal-success",
                    "state": "finalization-unresolved",
                    **extra,
                }
            ),
        )
        state._insert_note(
            conn,
            run_id=args.run_id,
            kind="finding",
            body="ordinary non-JSON observation",
        )
        assert cargo_output_lifecycle.unresolved_dispositions(conn) == []
    cargo_output_lifecycle.resume_declared_successes(Path(args.db))
    assert not lease.target.exists()


@pytest.mark.parametrize("limit", [-1, 0, True, 101, 1.5, "8"])
def test_recovery_and_status_limits_cannot_become_unbounded(tmp_path, limit):
    with pytest.raises(ValueError, match="batch limit"):
        cargo_output_lifecycle.resume_declared_successes(
            tmp_path / "missing.sqlite3", limit=limit
        )
    with closing(sqlite3.connect(":memory:")) as conn:
        with pytest.raises(ValueError, match="batch limit"):
            cargo_output_lifecycle.unresolved_dispositions(conn, limit=limit)


@pytest.mark.parametrize("generation", [(0, True, True)], indirect=True)
def test_non_opted_in_failure_does_not_fabricate_disposition_notes(
    generation, monkeypatch
):
    args, lease, _ = generation

    def fail(*args):
        raise ValueError("fixture malformed retained receipt")

    monkeypatch.setattr(cargo_output_lifecycle, "_finalize_declared_success", fail)
    with pytest.raises(ValueError, match="fixture malformed"):
        cargo_output_lifecycle.finalize_declared_success(Path(args.db), args.run_id)
    with closing(state._connect(Path(args.db))) as conn:
        assert state._notes_for_run_ids(conn, [args.run_id]).get(args.run_id, []) == []
    assert lease.target.exists()
