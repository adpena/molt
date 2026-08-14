"""Atomic admission, dependencies, contention mutexes, and lease claims."""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path
from typing import Sequence

from tools import lane_maturity
from tools.proof_queue_pkg import command_admission, evidence, state


def _lane_maturity_admission(
    *,
    conn: sqlite3.Connection,
    repo_root: Path,
    logical_id: str,
    resource_family: str,
    depends_on: Sequence[str] = (),
) -> lane_maturity.Decision:
    parent_lanes = {
        str(row[0])
        for parent in depends_on
        for row in conn.execute(
            "SELECT logical_id FROM proof_runs WHERE run_id = ?", (parent,)
        ).fetchall()
    }
    cross_lane = bool(parent_lanes - {logical_id})
    return lane_maturity.admission_check(
        repo_root=repo_root,
        lane_id=logical_id,
        resource_family=resource_family,
        cross_lane=cross_lane,
    )


def _active_for_key(conn: sqlite3.Connection, key: str) -> list[sqlite3.Row]:
    conn.row_factory = sqlite3.Row
    return list(
        conn.execute(
            f"""
            SELECT * FROM proof_runs
            WHERE contention_key = ? AND status IN ({state.LAUNCHED_SQL_STATUSES})
            ORDER BY started_at DESC
            """,
            (key,),
        )
    )


def _row_command(row: sqlite3.Row) -> list[str]:
    try:
        payload = json.loads(row["command_json"])
    except (TypeError, json.JSONDecodeError):
        return []
    if not isinstance(payload, list):
        return []
    return [str(part) for part in payload]


def _active_contention_conflicts(
    conn: sqlite3.Connection,
    *,
    resource_family: str,
    contention_key: str,
    command: Sequence[str],
    existing_run_id: str | None = None,
) -> list[tuple[str, sqlite3.Row]]:
    conn.row_factory = sqlite3.Row
    mutex_key = state._resource_mutex_key(
        resource_family=resource_family,
        contention_key=contention_key,
        command=command,
    )
    conflicts: list[tuple[str, sqlite3.Row]] = []
    seen: set[str] = set()

    def consider(kind: str, row: sqlite3.Row) -> None:
        if existing_run_id is not None and row["run_id"] == existing_run_id:
            return
        run_id = str(row["run_id"])
        if run_id in seen:
            return
        seen.add(run_id)
        conflicts.append((kind, row))

    for row in _active_for_key(conn, contention_key):
        consider(f"contention key {contention_key!r}", row)
    if mutex_key is None:
        return conflicts

    rows = list(
        conn.execute(
            f"""
            SELECT * FROM proof_runs
            WHERE status IN ({state.LAUNCHED_SQL_STATUSES})
            ORDER BY started_at DESC
            """
        )
    )
    for row in rows:
        row_mutex = state._resource_mutex_key(
            resource_family=str(row["resource_family"]),
            contention_key=str(row["contention_key"]),
            command=_row_command(row),
        )
        if row_mutex == mutex_key:
            consider(f"resource mutex {mutex_key!r}", row)
    return conflicts


def _format_active_contention_conflicts(
    conflicts: Sequence[tuple[str, sqlite3.Row]],
) -> str:
    lines = [f"{conflicts[0][0]} already has active run(s):"]
    for kind, row in conflicts:
        lines.append(f"- {row['status']} {row['run_id']} {row['reason']} ({kind})")
    return "\n".join(lines)


def _print_active_contention_conflicts(
    conflicts: Sequence[tuple[str, sqlite3.Row]],
) -> None:
    if conflicts:
        print(_format_active_contention_conflicts(conflicts), file=sys.stderr)


def _active_running_rows(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    conn.row_factory = sqlite3.Row
    return list(
        conn.execute(
            f"""
            SELECT * FROM proof_runs
            WHERE status IN ({state.LAUNCHED_SQL_STATUSES})
            ORDER BY started_at DESC
            """
        )
    )


def _parent_statuses(conn: sqlite3.Connection, run_id: str) -> list[sqlite3.Row]:
    conn.row_factory = sqlite3.Row
    return list(
        conn.execute(
            """
            SELECT edge.parent_run_id, edge.kind, parent.status
            FROM proof_run_edges edge
            JOIN proof_runs parent ON parent.run_id = edge.parent_run_id
            WHERE edge.child_run_id = ?
            ORDER BY edge.edge_id
            """,
            (run_id,),
        )
    )


def _dependency_state(
    conn: sqlite3.Connection, run_id: str
) -> tuple[str, list[sqlite3.Row]]:
    # depends_on is the only scheduling edge; reruns/supersedes/compares/
    # derives_from preserve lineage for evidence review and never gate
    # execution (a rerun's parent is failed or stale by definition).
    parents = [
        row
        for row in _parent_statuses(conn, run_id)
        if row["kind"] in state._SCHEDULING_EDGE_KINDS
    ]
    waiting = [row for row in parents if row["status"] in state.RUNNING]
    if waiting:
        return "waiting", waiting
    blockers = [row for row in parents if row["status"] != "passed"]
    if blockers:
        return "blocked", blockers
    return "ready", []


def _blocker_summary(blockers: Sequence[sqlite3.Row]) -> str:
    return ", ".join(
        f"{blocker['parent_run_id']}:{blocker['status']}" for blocker in blockers
    )


def _mark_queued_dependency_blocked(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    row: sqlite3.Row,
    blockers: Sequence[sqlite3.Row],
    *,
    announce: bool = False,
) -> str:
    blocker_summary = _blocker_summary(blockers)
    payload = evidence._row_to_payload(row)
    state._update_run(
        conn,
        row["run_id"],
        status="blocked",
        finished_at=state._utc_now(),
        receipt_context_json=json.dumps(
            state._unattested_receipt_context(
                status="not-executed",
                phase="dependency blocking",
                reason=f"blocked by {blocker_summary}",
            ),
            sort_keys=True,
        ),
    )
    log_path = Path(str(payload["log_path"]))
    evidence._write_failed_run_log(
        log_path,
        run_id=str(payload["run_id"]),
        logical_id=str(payload["logical_id"]),
        reason=str(payload["reason"]),
        repo_root=state._repo_root(args),
        command=list(payload["command"]),
        lines=[
            "proof queue blocked by dependency before command execution:",
            f"parents={blocker_summary}",
            "",
            "No proof command was launched for this row.",
        ],
    )
    evidence._try_write_marimo_notebook(
        args,
        conn,
        str(payload["run_id"]),
        log_path=log_path,
        phase="blocked projection",
    )
    if announce:
        print(f"blocked {row['run_id']} parents={blocker_summary}")
    return blocker_summary


def _refresh_blocked_queued_runs(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    run_ids: Sequence[str] | None = None,
    announce: bool = False,
) -> int:
    conn.row_factory = sqlite3.Row
    if run_ids:
        placeholders = ",".join("?" for _ in run_ids)
        rows = list(
            conn.execute(
                "SELECT * FROM proof_runs "
                "WHERE status = 'queued' "
                f"AND run_id IN ({placeholders}) "
                "ORDER BY rowid",
                tuple(run_ids),
            )
        )
    else:
        rows = list(
            conn.execute(
                "SELECT * FROM proof_runs WHERE status = 'queued' ORDER BY rowid"
            )
        )
    blocked_count = 0
    for row in rows:
        state, blockers = _dependency_state(conn, row["run_id"])
        if state != "blocked":
            continue
        _mark_queued_dependency_blocked(
            args,
            conn,
            row,
            blockers,
            announce=announce,
        )
        blocked_count += 1
    return blocked_count


def _insert_run(
    conn: sqlite3.Connection,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    command: list[str],
    cwd: Path,
    resource_family: str,
    contention_key: str,
    scopes: list[str],
    env_overrides: dict[str, str] | None = None,
    git_snapshot: dict[str, object] | None = None,
    log_path: Path,
    summary_json: Path,
) -> None:
    resource_mutex_key = state._resource_mutex_key(
        resource_family=resource_family,
        contention_key=contention_key,
        command=command,
    )
    conn.execute(
        """
        INSERT INTO proof_runs (
            run_id, logical_id, reason, status, command_json,
            command_envelope_json, receipt_context_json, cwd,
            resource_family, contention_key, resource_mutex_key, scopes_json,
            env_json, git_json, log_path, summary_json
        ) VALUES (?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            run_id,
            logical_id,
            reason,
            json.dumps(command),
            json.dumps(command_admission.admission_envelope(command), sort_keys=True),
            json.dumps(
                state._unattested_receipt_context(
                    status="not-executed",
                    phase="queued",
                    reason="execution worker has not captured receipt context",
                ),
                sort_keys=True,
            ),
            str(cwd),
            resource_family,
            contention_key,
            resource_mutex_key,
            json.dumps(scopes),
            json.dumps(env_overrides or {}, sort_keys=True),
            json.dumps(
                git_snapshot if git_snapshot is not None else state._git_snapshot(cwd),
                sort_keys=True,
            ),
            str(log_path),
            str(summary_json),
        ),
    )
    conn.commit()


def _admit_run(
    conn: sqlite3.Connection,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    command: list[str],
    cwd: Path,
    resource_family: str,
    contention_key: str,
    scopes: list[str],
    env_overrides: dict[str, str] | None = None,
    git_snapshot: dict[str, object] | None = None,
    log_path: Path,
    summary_json: Path,
) -> list[sqlite3.Row] | None:
    """Atomically insert a queued run.

    Queued rows are wait-list state, not resource custody. Multiple queued rows
    may share a contention key so dependency chains and follow-up proofs can be
    parked while earlier work is still running. The launch path owns the
    dispatched/running contention and capacity checks.
    """
    conn.row_factory = sqlite3.Row
    if conn.in_transaction:
        conn.commit()
    conn.execute("BEGIN IMMEDIATE")
    try:
        resource_mutex_key = state._resource_mutex_key(
            resource_family=resource_family,
            contention_key=contention_key,
            command=command,
        )
        conn.execute(
            """
            INSERT INTO proof_runs (
                run_id, logical_id, reason, status, command_json,
                command_envelope_json, receipt_context_json, cwd,
                resource_family, contention_key, resource_mutex_key,
                scopes_json, env_json, git_json, log_path, summary_json
            ) VALUES (?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                run_id,
                logical_id,
                reason,
                json.dumps(command),
                json.dumps(
                    command_admission.admission_envelope(command), sort_keys=True
                ),
                json.dumps(
                    state._unattested_receipt_context(
                        status="not-executed",
                        phase="queued",
                        reason="execution worker has not captured receipt context",
                    ),
                    sort_keys=True,
                ),
                str(cwd),
                resource_family,
                contention_key,
                resource_mutex_key,
                json.dumps(scopes),
                json.dumps(env_overrides or {}, sort_keys=True),
                json.dumps(
                    git_snapshot
                    if git_snapshot is not None
                    else state._git_snapshot(cwd),
                    sort_keys=True,
                ),
                str(log_path),
                str(summary_json),
            ),
        )
    except BaseException:
        conn.rollback()
        raise
    state._commit_with_locked_retry(conn)
    return None
