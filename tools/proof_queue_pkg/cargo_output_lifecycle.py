"""Persisted queue authority for manual and declared Cargo output disposition.

Proof receipts are immutable. Output disposition has its own append-only queue
notes and generation tombstone, and never changes the successful proof result.
"""

from __future__ import annotations

from contextlib import closing
import json
from pathlib import Path
import sqlite3
import sys
import time
from typing import Mapping

from molt.exact_json import loads_exact
from tools.proof_queue_pkg import (
    cargo_cache_custody,
    command_admission,
    evidence,
    state,
)


def load_terminal_generation(
    db: Path, run_id: str
) -> tuple[sqlite3.Row, dict[str, object], dict[str, object], dict[str, object], Path]:
    db = db.resolve()
    with closing(sqlite3.connect(f"{db.as_uri()}?mode=ro", uri=True)) as conn:
        row = state._row_by_run_id(conn, run_id)
    if row is None:
        raise ValueError(f"unknown proof run {run_id!r}")
    context = loads_exact(str(state._row_value(row, "receipt_context_json")))
    if not isinstance(context, dict):
        raise ValueError("proof run has no persisted terminal receipt context")
    evidence._validate_terminal_evidence(row, context)
    generation = evidence._cargo_generation_terminal(row, context, required=True)
    assert generation is not None
    provenance, projection = generation
    return row, context, provenance, projection, Path(str(row["log_path"])).parent


def apply_disposition(
    *,
    db: Path,
    run_id: str,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    result_root: Path,
    payload: dict[str, object],
    retire: bool,
    allow_passed: bool = False,
) -> None:
    """One append-only intent/outcome boundary around the locked authority."""
    action = "retirement" if retire else "reclamation"
    apply = (
        cargo_cache_custody.retire_terminal_sealed
        if retire
        else cargo_cache_custody.reclaim_terminal_unsealed
    )
    if retire:
        intent = cargo_cache_custody.inspect_terminal_sealed_retirement(
            result_root=result_root,
            provenance=provenance,
            projection=projection,
            allow_passed=allow_passed,
        )
        payload["retirement_policy"] = intent["retirement_policy"]
        payload["terminal_status"] = intent["terminal_status"]
    with closing(state._connect(db)) as conn:
        state._insert_note(
            conn,
            run_id=run_id,
            kind="decision",
            body=json.dumps(
                {**payload, "state": "requested", "generation": projection},
                sort_keys=True,
            ),
        )
        payload["state"] = f"{action}-in-progress"
        started = time.perf_counter()
        try:
            payload.update(
                apply(
                    result_root=result_root,
                    provenance=provenance,
                    projection=projection,
                    **({"allow_passed": allow_passed} if retire else {}),
                )
            )
        except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
            payload.update(
                state=f"{action}-indeterminate",
                error=f"{type(exc).__name__}: {exc}",
                disposition_elapsed_s=max(0.0, time.perf_counter() - started),
            )
            state._insert_note(
                conn,
                run_id=run_id,
                kind="finding",
                body=json.dumps(payload, sort_keys=True),
            )
            raise
        payload["disposition_elapsed_s"] = max(0.0, time.perf_counter() - started)
        state._insert_note(
            conn,
            run_id=run_id,
            kind="finding",
            body=json.dumps(payload, sort_keys=True),
        )


def _finalize_declared_success(db: Path, run_id: str) -> dict[str, object] | None:
    """Finalize only explicit, committed, independently validated successful runs."""
    with closing(sqlite3.connect(f"{db.resolve().as_uri()}?mode=ro", uri=True)) as conn:
        row = state._row_by_run_id(conn, run_id)
    if row is None:
        raise ValueError(f"unknown proof run {run_id!r}")
    envelope = loads_exact(str(row["command_envelope_json"]))
    if row["status"] != "passed":
        return None
    if not isinstance(envelope, dict):
        raise ValueError("persisted command envelope must be an object")
    lifetime = command_admission.validated_cargo_output_lifetime(envelope)
    if lifetime != "terminal-success":
        return None
    command = loads_exact(str(row["command_json"]))
    if not isinstance(command, list) or not all(
        isinstance(value, str) for value in command
    ):
        raise ValueError("persisted command must be an argv list")
    command_admission.validate_envelope(envelope, command)
    row, context, provenance, projection, result_root = load_terminal_generation(
        db, run_id
    )
    source = context.get("source_custody")
    if (
        not isinstance(source, dict)
        or source.get("evidence_eligible") is not True
        or source.get("identical") is not True
    ):
        raise ValueError(
            "declared Cargo output finalization requires completed source custody"
        )
    if provenance.get("cargo_output_lifetime", "retain") != lifetime:
        raise ValueError(
            "Cargo generation output lifetime differs from immutable admission"
        )
    finding = cargo_cache_custody.inspect_terminal_sealed_retirement(
        result_root=result_root,
        provenance=provenance,
        projection=projection,
        allow_passed=True,
    )
    payload = {
        "schema": "molt.proof-cargo-generation-sealed-retirement.v1",
        "run_id": run_id,
        "apply": True,
        "cargo_output_lifetime": lifetime,
        **finding,
    }
    if finding["state"] == "retired-sealed":
        # Close a crash after locked retirement but before its finding.
        with closing(state._connect(db)) as conn:
            if not _has_declared_outcome(conn, run_id):
                state._insert_note(
                    conn,
                    run_id=run_id,
                    kind="finding",
                    body=json.dumps(payload, sort_keys=True),
                )
        return payload
    # Recovery may start an untouched finalization, never repeat a partial or
    # blocked deletion. Those states require explicit operator investigation.
    if (
        finding["state"] != "terminal-sealed-retained"
        or finding["retirement_eligible"] is not True
    ):
        raise RuntimeError(
            f"declared Cargo output finalization unresolved: {json.dumps(payload, sort_keys=True)}"
        )
    apply_disposition(
        db=db,
        run_id=run_id,
        provenance=provenance,
        projection=projection,
        result_root=result_root,
        payload=payload,
        retire=True,
        allow_passed=True,
    )
    if payload.get("state") != "retired-sealed":
        raise RuntimeError(
            f"declared Cargo output finalization failed: {json.dumps(payload, sort_keys=True)}"
        )
    return payload


_DECLARED_FINDING = """
    n.kind = 'finding' AND CASE WHEN json_valid(n.body)
    THEN json_extract(n.body, '$.cargo_output_lifetime') = 'terminal-success'
        AND json_extract(n.body, '$.schema') = 'molt.proof-cargo-generation-sealed-retirement.v1'
        AND json_extract(n.body, '$.run_id') = n.run_id
    ELSE 0 END
"""


def _has_declared_outcome(conn: sqlite3.Connection, run_id: str) -> bool:
    return (
        conn.execute(
            f"SELECT 1 FROM proof_notes n WHERE n.run_id = ? AND {_DECLARED_FINDING} AND CASE WHEN json_valid(n.body) THEN json_extract(n.body, '$.state') = 'retired-sealed' ELSE 0 END LIMIT 1",
            (run_id,),
        ).fetchone()
        is not None
    )


def finalize_declared_success(db: Path, run_id: str) -> dict[str, object] | None:
    # Do not fabricate a declaration in failure notes for unrelated/legacy rows.
    with closing(sqlite3.connect(f"{db.resolve().as_uri()}?mode=ro", uri=True)) as conn:
        row = state._row_by_run_id(conn, run_id)
    if row is None:
        raise ValueError(f"unknown proof run {run_id!r}")
    envelope = loads_exact(str(row["command_envelope_json"]))
    if not isinstance(envelope, dict):
        raise ValueError("persisted command envelope must be an object")
    declared = (
        envelope.get("cargo_output_lifetime") == "terminal-success"
        and row["status"] == "passed"
    )
    try:
        return _finalize_declared_success(db, run_id)
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
        if not declared:
            raise
        # Disposition findings never change successful proof status or receipts.
        with closing(state._connect(db)) as conn:
            state._insert_note(
                conn,
                run_id=run_id,
                kind="finding",
                body=json.dumps(
                    {
                        "schema": "molt.proof-cargo-generation-sealed-retirement.v1",
                        "run_id": run_id,
                        "cargo_output_lifetime": "terminal-success",
                        "state": "finalization-unresolved",
                        "error": f"{type(exc).__name__}: {exc}",
                    },
                    sort_keys=True,
                ),
            )
        raise


def resume_declared_successes(db: Path, *, limit: int = 8) -> None:
    """Resume a bounded pending set, never failed or partial disposition.

    Notes select candidates; exact owner/receipt custody alone authorizes
    deletion. Historical disposition errors never gate unrelated new work.
    """
    _validate_batch_limit(limit)
    with closing(sqlite3.connect(f"{db.resolve().as_uri()}?mode=ro", uri=True)) as conn:
        rows = list(
            conn.execute(
                f"""
            SELECT p.run_id FROM proof_runs p WHERE p.status = 'passed'
            AND CASE WHEN json_valid(p.command_envelope_json)
                THEN json_extract(p.command_envelope_json, '$.cargo_output_lifetime') = 'terminal-success' ELSE 0 END
            AND NOT EXISTS (SELECT 1 FROM proof_notes n WHERE n.run_id = p.run_id AND {_DECLARED_FINDING})
            ORDER BY p.finished_at, p.run_id LIMIT ?
        """,
                (limit,),
            )
        )
    for (run_id,) in rows:
        try:
            finalize_declared_success(db, run_id)
        except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
            print(
                f"Cargo output finalization unresolved for {run_id}; proof result unchanged: {exc}",
                file=sys.stderr,
            )


def unresolved_dispositions(
    conn: sqlite3.Connection, *, limit: int = 20
) -> list[dict[str, object]]:
    """Read latest disposition signals without walking artifact history."""
    _validate_batch_limit(limit)
    latest_finding = _DECLARED_FINDING.replace("n.", "m.")
    rows = conn.execute(
        f"""
        SELECT n.body FROM proof_notes n WHERE {_DECLARED_FINDING}
        AND n.note_id = (SELECT MAX(m.note_id) FROM proof_notes m
            WHERE m.run_id = n.run_id AND {latest_finding})
        AND CASE WHEN json_valid(n.body) THEN json_extract(n.body, '$.state') != 'retired-sealed' ELSE 0 END
        ORDER BY n.note_id DESC LIMIT ?
    """,
        (limit,),
    )
    return [loads_exact(row[0]) for row in rows]


def _validate_batch_limit(limit: int) -> None:
    if type(limit) is not int or not 1 <= limit <= 100:
        raise ValueError(
            "Cargo output lifecycle batch limit must be an integer from 1 to 100"
        )
