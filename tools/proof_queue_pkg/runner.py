"""Queue submission and memory-guarded proof execution lifecycle."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import sqlite3
import subprocess
import sys
import time
import uuid
from pathlib import Path

from molt.dx import development_artifact_env
from tools.proof_queue_pkg import (
    custody,
    evidence,
    policy,
    scheduling,
    state,
)
from tools.proof_queue_pkg import diagnostics as diagnostic_engine


def _wait_for_guard_completion_or_stale(
    conn: sqlite3.Connection,
    *,
    run_id: str,
    proc: subprocess.Popen[str],
    log: object,
    start: float,
) -> tuple[str, int | None, float]:
    while True:
        try:
            rc = int(proc.wait(timeout=custody.PROOF_QUEUE_ACTIVE_POLL_SECONDS))
        except subprocess.TimeoutExpired:
            row = state._row_by_run_id(conn, run_id)
            if row is None:
                continue
            if row["status"] not in state.RUNNING:
                elapsed = time.monotonic() - start
                if proc.poll() is None:
                    print(
                        "proof_queue noticed externally terminal row "
                        f"status={row['status']} while guard_pid={proc.pid} "
                        "was still live; terminating queue-owned guard",
                        file=log,
                        flush=True,
                    )
                    custody._terminate_queue_owned_guard_process(proc, log, run_id=run_id)
                return str(row["status"]), row["returncode"], elapsed
            diagnostics = diagnostic_engine._run_diagnostics(row)
            if not diagnostic_engine._diagnostics_have_terminal_stale_signal(diagnostics):
                continue
            diagnostic_summary = diagnostic_engine._format_diagnostic_summary(diagnostics)
            print(
                "\nproof_queue stale-running terminalization "
                f"diagnosis={diagnostic_summary}",
                file=log,
                flush=True,
            )
            if diagnostics:
                evidence = diagnostics[0].get("evidence")
                if isinstance(evidence, str) and evidence.strip():
                    print(f"evidence={evidence}", file=log, flush=True)
                artifacts = diagnostic_engine._diagnostic_artifacts(diagnostics)
                if artifacts:
                    print(f"artifacts={', '.join(artifacts)}", file=log, flush=True)
            guard_rc = custody._terminate_queue_owned_guard_process(proc, log, run_id=run_id)
            if guard_rc is not None:
                print(
                    f"proof_queue stale terminalization guard_exit_code={guard_rc}",
                    file=log,
                    flush=True,
                )
            elapsed = time.monotonic() - start
            return "stale", custody.PROOF_QUEUE_STALE_EXIT_CODE, elapsed
        else:
            elapsed = time.monotonic() - start
            status = "passed" if rc == 0 else "failed"
            return status, rc, elapsed



def _queue_one(
    args: argparse.Namespace,
    *,
    logical_id: str,
    reason: str,
    command: list[str],
    resource_family: str,
    contention_key: str,
    scopes: list[str],
    env_overrides: dict[str, str],
    initial_notes: list[str] | None = None,
    depends_on: list[str] | None = None,
    edge_kind: str = state.DEFAULT_EDGE_KIND,
    edge_note: str | None = None,
    policy_error: str | None = None,
) -> tuple[int, str | None]:
    if not command:
        raise SystemExit("proof command is empty")
    if not initial_notes:
        print(
            "queued proof submissions require at least one append-only note; "
            "pass --note or use a named lane with built-in notes",
            file=sys.stderr,
        )
        return 2, None
    db = state._db_path(args)
    logs_root = state._logs_root(args)
    repo_root = state._repo_root(args)
    conn = state._connect(db)
    for parent_run_id in depends_on or []:
        if state._edge_kind_requires_local_parent(edge_kind) and not state._run_exists(
            conn, parent_run_id
        ):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    if edge_kind not in state.EDGE_KINDS:
        allowed = ", ".join(sorted(state.EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    scheduling._refresh_blocked_queued_runs(args, conn)
    maturity = scheduling._lane_maturity_admission(
        conn=conn,
        repo_root=repo_root,
        logical_id=logical_id,
        resource_family=resource_family,
        depends_on=depends_on or (),
    )
    if not maturity.allow:
        print(f"lane maturity refused {logical_id}: {maturity.reason}", file=sys.stderr)
        return 2, None
    run_id = f"{state._compact_utc()}-{state._slug(logical_id)}-{uuid.uuid4().hex[:16]}"
    logs_root.mkdir(parents=True, exist_ok=True)
    log_path = logs_root / f"{run_id}.log"
    active = scheduling._admit_run(
        conn,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        command=command,
        cwd=repo_root,
        resource_family=resource_family,
        contention_key=contention_key,
        scopes=scopes,
        env_overrides=env_overrides,
        log_path=log_path,
        summary_json=logs_root / f"{run_id}.memory_guard.json",
    )
    if active is not None:
        print(
            f"contention key {contention_key!r} already has active run(s):",
            file=sys.stderr,
        )
        for row in active:
            print(f"- {row['status']} {row['run_id']} {row['reason']}", file=sys.stderr)
        return 2, None
    try:
        for parent_run_id in depends_on or []:
            state._insert_edge(
                conn,
                parent_run_id=parent_run_id,
                child_run_id=run_id,
                kind=edge_kind,
                note=edge_note,
            )
        for note in initial_notes or []:
            state._insert_note(conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND)
    except Exception as exc:
        rc = evidence._fail_preexecution_run(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            exc=exc,
            phase="submission metadata",
        )
        return rc, run_id
    policy_error = (
        policy_error
        or policy._proof_command_policy_error(command)
        or policy._proof_env_policy_error(env_overrides)
    )
    if policy_error is not None:
        rc = _record_policy_rejection(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            policy_error=policy_error,
        )
        return rc, run_id
    evidence._write_queued_submission_log(
        log_path,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        repo_root=repo_root,
        command=command,
        resource_family=resource_family,
        contention_key=contention_key,
        scopes=scopes,
        env_overrides=env_overrides,
        depends_on=depends_on or [],
    )
    if initial_notes or depends_on:
        evidence._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="submission projection",
        )
    print(f"queued {run_id}")
    return 0, run_id



def _claim_detached_run(
    conn: sqlite3.Connection,
    run_id: str,
    *,
    queue_size: int,
) -> tuple[sqlite3.Row | None, str | None]:
    """Atomically move one queued row into launched custody."""
    conn.row_factory = sqlite3.Row
    if conn.in_transaction:
        conn.commit()
    conn.execute("BEGIN IMMEDIATE")
    try:
        row = conn.execute(
            "SELECT * FROM proof_runs WHERE run_id = ?",
            (run_id,),
        ).fetchone()
        if row is None:
            conn.rollback()
            raise SystemExit(f"unknown proof run {run_id!r}")
        if row["status"] != "queued":
            conn.rollback()
            return None, f"proof run {run_id!r} is {row['status']}, not queued"

        active_count = int(
            conn.execute(
                f"SELECT COUNT(*) FROM proof_runs "
                f"WHERE status IN ({state.LAUNCHED_SQL_STATUSES})"
            ).fetchone()[0]
        )
        if active_count >= queue_size:
            conn.rollback()
            return (
                None,
                f"queue capacity full active={active_count} queue_size={queue_size}",
            )

        active = scheduling._active_contention_conflicts(
            conn,
            resource_family=str(row["resource_family"]),
            contention_key=str(row["contention_key"]),
            command=scheduling._row_command(row),
            existing_run_id=run_id,
        )
        if active:
            conn.rollback()
            return (
                None,
                f"waiting {run_id} "
                + scheduling._format_active_contention_conflicts(active).replace("\n", "; "),
            )

        now = state._utc_now()
        updated = conn.execute(
            """
            UPDATE proof_runs
            SET status = 'dispatched', started_at = ?
            WHERE run_id = ? AND status = 'queued'
            """,
            (now, run_id),
        )
        if updated.rowcount != 1:
            conn.rollback()
            return None, f"proof run {run_id!r} was claimed by another scheduler"
    except sqlite3.IntegrityError:
        conn.rollback()
        return None, f"proof run {run_id!r} could not be claimed atomically"
    except BaseException:
        conn.rollback()
        raise
    state._commit_with_locked_retry(conn)
    return state._row_by_run_id(conn, run_id), None



def _dispatch_detached_runner(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    run_id: str,
    timeout: float,
) -> tuple[int, Path] | None:
    claimed, skip_reason = _claim_detached_run(
        conn,
        run_id,
        queue_size=state._configured_queue_size(getattr(args, "queue_size", None)),
    )
    if claimed is None:
        if skip_reason:
            print(skip_reason)
        return None
    try:
        pid, runner_log = custody._launch_detached_runner(args, run_id=run_id, timeout=timeout)
    except Exception:
        state._update_run(
            conn,
            run_id,
            status="failed",
            returncode=2,
            finished_at=state._utc_now(),
            elapsed_s=0.0,
        )
        raise
    row = state._row_by_run_id(conn, run_id)
    if row is not None:
        log_path = Path(str(row["log_path"]))
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with log_path.open("a", encoding="utf-8") as log:
            print("\n--- proof_queue detached dispatch ---", file=log)
            print("status=dispatched", file=log)
            print(f"runner_pid={pid}", file=log)
            print(f"runner_log={runner_log}", file=log)
    return pid, runner_log



def _record_policy_rejection(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    repo_root: Path,
    command: list[str],
    log_path: Path,
    policy_error: str,
) -> int:
    now = state._utc_now()
    state._update_run(
        conn,
        run_id,
        status="failed",
        returncode=2,
        started_at=now,
        finished_at=now,
        elapsed_s=0.0,
    )
    evidence._write_failed_run_log(
        log_path,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        repo_root=repo_root,
        command=command,
        lines=[policy_error],
    )
    print(f"rejected {run_id} rc=2")
    print(policy_error, file=sys.stderr)
    print(f"log: {log_path}")
    if state._notes_for_run_ids(conn, [run_id]).get(run_id):
        evidence._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="policy rejection projection",
        )
    return 2



def _ensure_disk_headroom_before_build() -> None:
    """Preemptive, AGENT-SAFE disk reclaim before launching a queued build.

    A heavy Cargo/witness build is exactly the point where C: filled to 0 bytes.
    The disk guard reclaims ONLY stale build-artifact dirs (never a process, never
    an active/lock-held dir) and is a fast no-op above the high-water mark.
    Fail-open: a guard error must never block the build.
    """
    if any(os.environ.get(k) for k in ("PYTEST_CURRENT_TEST", "PYTEST_VERSION")):
        return  # never reclaim real artifacts during a test run
    try:
        from tools import disk_guard

        disk_guard.ensure_free_fail_open()
    except Exception:
        pass

def _run_one(
    args: argparse.Namespace,
    *,
    logical_id: str,
    reason: str,
    command: list[str],
    resource_family: str,
    contention_key: str,
    scopes: list[str],
    env_overrides: dict[str, str],
    timeout: float,
    initial_notes: list[str] | None = None,
    depends_on: list[str] | None = None,
    edge_kind: str = state.DEFAULT_EDGE_KIND,
    edge_note: str | None = None,
    policy_error: str | None = None,
    existing_run_id: str | None = None,
    existing_log_path: Path | None = None,
    existing_summary_json: Path | None = None,
) -> int:
    if not command:
        raise SystemExit("proof command is empty")
    db = state._db_path(args)
    logs_root = state._logs_root(args)
    repo_root = state._repo_root(args)
    conn = state._connect(db)
    for parent_run_id in depends_on or []:
        if not state._run_exists(conn, parent_run_id):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    maturity = scheduling._lane_maturity_admission(
        conn=conn,
        repo_root=repo_root,
        logical_id=logical_id,
        resource_family=resource_family,
        depends_on=depends_on or (),
    )
    if not maturity.allow:
        print(f"lane maturity refused {logical_id}: {maturity.reason}", file=sys.stderr)
        return 2
    if edge_kind not in state.EDGE_KINDS:
        allowed = ", ".join(sorted(state.EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    active = scheduling._active_contention_conflicts(
        conn,
        resource_family=resource_family,
        contention_key=contention_key,
        command=command,
        existing_run_id=existing_run_id,
    )
    if active:
        scheduling._print_active_contention_conflicts(active)
        return 2
    suffix = uuid.uuid4().hex[:16]
    run_id = existing_run_id or f"{state._compact_utc()}-{state._slug(logical_id)}-{suffix}"
    logs_root.mkdir(parents=True, exist_ok=True)
    log_path = existing_log_path or logs_root / f"{run_id}.log"
    summary_json = existing_summary_json or logs_root / f"{run_id}.memory_guard.json"
    inserted_run = existing_run_id is None
    if existing_run_id is None:
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            command=command,
            cwd=repo_root,
            resource_family=resource_family,
            contention_key=contention_key,
            scopes=scopes,
            env_overrides=env_overrides,
            log_path=log_path,
            summary_json=summary_json,
        )
    if inserted_run:
        try:
            for parent_run_id in depends_on or []:
                state._insert_edge(
                    conn,
                    parent_run_id=parent_run_id,
                    child_run_id=run_id,
                    kind=edge_kind,
                    note=edge_note,
                )
            for note in initial_notes or []:
                state._insert_note(conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND)
        except Exception as exc:
            return evidence._fail_preexecution_run(
                args,
                conn,
                run_id=run_id,
                logical_id=logical_id,
                reason=reason,
                repo_root=repo_root,
                command=command,
                log_path=log_path,
                exc=exc,
                phase="submission metadata",
            )
        if initial_notes or depends_on:
            evidence._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=log_path,
                phase="submission projection",
            )
    policy_error = (
        policy_error
        or policy._proof_command_policy_error(command)
        or policy._proof_env_policy_error(env_overrides)
    )
    if policy_error is not None:
        return _record_policy_rejection(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            policy_error=policy_error,
        )
    preflight_errors = policy._ensure_run_toolchain_preflight(
        repo_root=repo_root,
        resource_family=resource_family,
    )
    if preflight_errors is not None:
        now = state._utc_now()
        state._update_run(
            conn,
            run_id,
            status="failed",
            returncode=2,
            started_at=now,
            finished_at=now,
            elapsed_s=0.0,
        )
        lines = ["proof queue toolchain preflight failed:", *preflight_errors]
        evidence._write_failed_run_log(
            log_path,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            lines=lines,
        )
        print(f"rejected {run_id} rc=2")
        for line in lines:
            print(line, file=sys.stderr)
        print(f"log: {log_path}")
        if state._notes_for_run_ids(conn, [run_id]).get(run_id):
            evidence._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=log_path,
                phase="toolchain preflight projection",
            )
        return 2
    try:
        session_id = state._proof_session_id(resource_family, contention_key)
        env = development_artifact_env(
            repo_root,
            os.environ,
            session_prefix=f"proof-{resource_family}",
            session_id=session_id,
        )
        env["MOLT_PROOF_QUEUE"] = "1"
        env["MOLT_PROOF_QUEUE_DB"] = str(db)
        env["MOLT_PROOF_QUEUE_RUN_ID"] = run_id
        env.update(env_overrides)
        poll_interval = custody._proof_queue_memory_guard_poll_sec(env_overrides)
        env[custody.MEMORY_GUARD_POLL_SEC_ENV] = poll_interval
        wrapped = custody._memory_guard_command(
            command=command,
            summary_json=summary_json,
            timeout=timeout,
            poll_interval=poll_interval,
        )
    except Exception as exc:
        return evidence._fail_preexecution_run(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            exc=exc,
            phase="execution environment setup",
        )
    start = time.monotonic()
    started_at = state._utc_now()
    state._update_run(conn, run_id, status="running", started_at=started_at)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        log = log_path.open("a", encoding="utf-8")
        if log.tell() > 0:
            print("\n--- proof_queue command execution ---", file=log)
        print(f"proof_queue run_id={run_id}", file=log)
        print(f"logical_id={logical_id}", file=log)
        print(f"reason={reason}", file=log)
        print(f"cwd={repo_root}", file=log)
        print("memory_guard_prefix=MOLT_PROOF_QUEUE", file=log)
        print(f"command={shlex.join(command)}", file=log)
        if env_overrides:
            print(
                f"env_overrides={json.dumps(env_overrides, sort_keys=True)}", file=log
            )
        print(f"proof_session_id={session_id}", file=log)
        print(f"cargo_target_dir={env.get('CARGO_TARGET_DIR', '')}", file=log)
        print(f"memory_guard_poll_sec={poll_interval}", file=log)
        print(f"memory_guard_summary_json={summary_json}", file=log)
        print(f"memory_guard_command={shlex.join(wrapped)}", file=log)
        print("", file=log, flush=True)
        proc = subprocess.Popen(
            wrapped,
            cwd=repo_root,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            text=True,
            **custody._queued_command_process_kwargs(),
        )
    except Exception as exc:
        try:
            log.close()
        except NameError:
            pass
        return evidence._fail_preexecution_run(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            exc=exc,
            phase="process launch",
        )
    try:
        state._update_run(
            conn,
            run_id,
            guard_pid=proc.pid,
            guard_identity=custody._process_identity(proc.pid),
        )
        status, rc, elapsed = _wait_for_guard_completion_or_stale(
            conn,
            run_id=run_id,
            proc=proc,
            log=log,
            start=start,
        )
        rc_text = "?" if rc is None else str(rc)
        print(
            f"\nproof_queue finished status={status} exit_code={rc_text} "
            f"elapsed={elapsed:.3f}s",
            file=log,
        )
    finally:
        log.close()
    state._update_run(
        conn,
        run_id,
        status=status,
        returncode=rc,
        finished_at=state._utc_now(),
        elapsed_s=elapsed,
    )
    if state._notes_for_run_ids(conn, [run_id]).get(run_id):
        evidence._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="completion projection",
        )
    rc_text = "?" if rc is None else str(rc)
    print(f"{status} {run_id} rc={rc_text} elapsed={elapsed:.1f}s")
    print(f"log: {log_path}")
    return rc if rc is not None else custody.PROOF_QUEUE_STALE_EXIT_CODE
