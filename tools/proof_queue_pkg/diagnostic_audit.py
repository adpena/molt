"""Proof queue audit, frontier, and row-selection authority."""

from __future__ import annotations

import argparse
import sqlite3
from typing import Sequence

from tools.proof_queue_pkg import state
from tools.proof_queue_pkg.diagnostic_model import _diagnostics_have_signal

AUDIT_ERROR_DIAGNOSTICS = frozenset(
    {
        "memory-guard-summary-incomplete",
        "memory-guard-timeout",
        "native-call-lane-memory-guard-timeout",
        "proof-log-missing",
        "queue-preexecution-failure",
    }
)

AUDIT_WARNING_DIAGNOSTICS = frozenset(
    {
        "queue-infra-warning",
        "memory-guard-orphan-cleanup",
        "nested-memory-guard-orphan-cleanup",
        "queue-policy-rejection",
        "runtime-wasm-rust-target-missing",
        "wasm-toolchain-contract-import-missing",
        "running-pytest-failures-observed",
        "running-pytest-current-test-missing",
    }
)

FRONTIER_SUPERSEDING_EDGE_KINDS = frozenset({"reruns", "supersedes"})

FRONTIER_SUPERSEDING_CHILD_STATUSES = frozenset(
    {"queued", "dispatched", "running", "passed", "failed"}
)


def _audit_issue(
    *,
    signal_id: str,
    severity: str,
    summary: str,
    next_action: str,
    run_id: str | None = None,
    evidence: str = "",
    artifacts: Sequence[str] = (),
) -> dict[str, object]:
    return {
        "signal_id": signal_id,
        "severity": severity,
        "run_id": run_id,
        "summary": summary,
        "evidence": state._shorten(evidence, 320),
        "next_action": next_action,
        "artifacts": list(artifacts),
    }


def _audit_severity_for_diagnostic(row: sqlite3.Row, signal_id: str) -> str | None:
    if signal_id == "memory-guard-summary-incomplete" and row["status"] == "stale":
        return "warning"
    if signal_id in AUDIT_ERROR_DIAGNOSTICS:
        return "error"
    if signal_id in AUDIT_WARNING_DIAGNOSTICS:
        return "warning"
    return None


def _frontier_failure(
    row: sqlite3.Row, diagnostics: list[dict[str, object]]
) -> dict[str, object] | None:
    if _diagnostics_have_signal(diagnostics, "memory-guard-summary-incomplete"):
        return None
    for item in diagnostics:
        if str(item["severity"]) != "error":
            continue
        signal_id = str(item["signal_id"])
        if (
            signal_id in AUDIT_ERROR_DIAGNOSTICS
            or signal_id in AUDIT_WARNING_DIAGNOSTICS
        ):
            continue
        return {
            "run_id": row["run_id"],
            "logical_id": row["logical_id"],
            "diagnostic": signal_id,
            "summary": item["summary"],
            "evidence": item["evidence"],
            "next_action": item["next_action"],
            "log_path": row["log_path"],
            "finished_at": row["finished_at"],
        }
    return None


def _frontier_superseded(dag: dict[str, list[dict[str, object]]]) -> bool:
    for edge in dag.get("children", []):
        if str(edge["kind"]) not in FRONTIER_SUPERSEDING_EDGE_KINDS:
            continue
        if str(edge["child_status"]) in FRONTIER_SUPERSEDING_CHILD_STATUSES:
            return True
    return False


def _audit_rows(
    conn: sqlite3.Connection, args: argparse.Namespace
) -> list[sqlite3.Row]:
    conn.row_factory = sqlite3.Row
    active = list(
        conn.execute(
            f"SELECT * FROM proof_runs WHERE status IN ({state.ACTIVE_SQL_STATUSES}) "
            "ORDER BY started_at"
        )
    )
    if args.all:
        historical = list(
            conn.execute(
                f"SELECT * FROM proof_runs WHERE status NOT IN ({state.ACTIVE_SQL_STATUSES}) "
                "ORDER BY rowid DESC"
            )
        )
    else:
        historical = list(
            conn.execute(
                """
                SELECT * FROM proof_runs
                WHERE status NOT IN ('queued', 'dispatched', 'running')
                ORDER BY rowid DESC
                LIMIT ?
                """,
                (args.limit,),
            )
        )
    seen: set[str] = set()
    rows: list[sqlite3.Row] = []
    for row in [*active, *historical]:
        run_id = str(row["run_id"])
        if run_id in seen:
            continue
        seen.add(run_id)
        rows.append(row)
    return rows


def _diagnose_row(conn: sqlite3.Connection, args: argparse.Namespace) -> sqlite3.Row:
    conn.row_factory = sqlite3.Row
    if args.run_id:
        row = conn.execute(
            "SELECT * FROM proof_runs WHERE run_id = ?",
            (args.run_id,),
        ).fetchone()
    elif args.logical_id:
        row = conn.execute(
            """
            SELECT * FROM proof_runs
            WHERE logical_id = ?
            ORDER BY rowid DESC
            LIMIT 1
            """,
            (args.logical_id,),
        ).fetchone()
    else:
        row = conn.execute(
            "SELECT * FROM proof_runs ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
    if row is None:
        selector = args.run_id or args.logical_id or "latest proof run"
        raise SystemExit(f"unknown proof run selector {selector!r}")
    return row
