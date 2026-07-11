"""Proof-run execution lifecycle (moved from tools/proof_queue.py).

``_run_one`` drives one queued or dispatched proof: contention gate, insertion, policy
and toolchain preflight, memory-guarded launch, guard-completion wait, and
terminal projection. Shared helpers and monkeypatch-sensitive primitives
stay in the facade module ``pq`` and are reached through ``pq.``.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
import uuid
from pathlib import Path

from molt.dx import development_artifact_env

# Definition-time constants (default argument values) — resolved at
# module load, so bound directly rather than via the call-time proxy.
from tools.proof_queue import DEFAULT_EDGE_KIND  # noqa: E402
from tools.proof_queue_pkg import pq


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
    edge_kind: str = DEFAULT_EDGE_KIND,
    edge_note: str | None = None,
    policy_error: str | None = None,
    existing_run_id: str | None = None,
    existing_log_path: Path | None = None,
    existing_summary_json: Path | None = None,
) -> int:
    if not command:
        raise SystemExit("proof command is empty")
    db = pq._db_path(args)
    logs_root = pq._logs_root(args)
    repo_root = pq._repo_root(args)
    conn = pq._connect(db)
    for parent_run_id in depends_on or []:
        if not pq._run_exists(conn, parent_run_id):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    maturity = pq._lane_maturity_admission(
        conn=conn,
        repo_root=repo_root,
        logical_id=logical_id,
        resource_family=resource_family,
        depends_on=depends_on or (),
    )
    if not maturity.allow:
        print(f"lane maturity refused {logical_id}: {maturity.reason}", file=sys.stderr)
        return 2
    if edge_kind not in pq.EDGE_KINDS:
        allowed = ", ".join(sorted(pq.EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    active = pq._active_contention_conflicts(
        conn,
        resource_family=resource_family,
        contention_key=contention_key,
        command=command,
        existing_run_id=existing_run_id,
    )
    if active:
        pq._print_active_contention_conflicts(active)
        return 2
    suffix = uuid.uuid4().hex[:16]
    run_id = existing_run_id or f"{pq._compact_utc()}-{pq._slug(logical_id)}-{suffix}"
    logs_root.mkdir(parents=True, exist_ok=True)
    log_path = existing_log_path or logs_root / f"{run_id}.log"
    summary_json = existing_summary_json or logs_root / f"{run_id}.memory_guard.json"
    inserted_run = existing_run_id is None
    if existing_run_id is None:
        pq._insert_run(
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
                pq._insert_edge(
                    conn,
                    parent_run_id=parent_run_id,
                    child_run_id=run_id,
                    kind=edge_kind,
                    note=edge_note,
                )
            for note in initial_notes or []:
                pq._insert_note(conn, run_id=run_id, body=note, kind=pq.SUBMISSION_NOTE_KIND)
        except Exception as exc:
            return pq._fail_preexecution_run(
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
            pq._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=log_path,
                phase="submission projection",
            )
    policy_error = (
        policy_error
        or pq._proof_command_policy_error(command)
        or pq._proof_env_policy_error(env_overrides)
    )
    if policy_error is not None:
        return pq._record_policy_rejection(
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
    preflight_errors = pq._ensure_run_toolchain_preflight(
        repo_root=repo_root,
        resource_family=resource_family,
    )
    if preflight_errors is not None:
        now = pq._utc_now()
        pq._update_run(
            conn,
            run_id,
            status="failed",
            returncode=2,
            started_at=now,
            finished_at=now,
            elapsed_s=0.0,
        )
        lines = ["proof queue toolchain preflight failed:", *preflight_errors]
        pq._write_failed_run_log(
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
        if pq._notes_for_run_ids(conn, [run_id]).get(run_id):
            pq._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=log_path,
                phase="toolchain preflight projection",
            )
        return 2
    try:
        session_id = pq._proof_session_id(resource_family, contention_key)
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
        poll_interval = pq._proof_queue_memory_guard_poll_sec(env_overrides)
        env[pq.MEMORY_GUARD_POLL_SEC_ENV] = poll_interval
        wrapped = pq._memory_guard_command(
            command=command,
            summary_json=summary_json,
            timeout=timeout,
            poll_interval=poll_interval,
        )
    except Exception as exc:
        return pq._fail_preexecution_run(
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
    started_at = pq._utc_now()
    pq._update_run(conn, run_id, status="running", started_at=started_at)
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
            **pq._queued_command_process_kwargs(),
        )
    except Exception as exc:
        try:
            log.close()
        except NameError:
            pass
        return pq._fail_preexecution_run(
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
        pq._update_run(conn, run_id, guard_pid=proc.pid)
        status, rc, elapsed = pq._wait_for_guard_completion_or_stale(
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
    pq._update_run(
        conn,
        run_id,
        status=status,
        returncode=rc,
        finished_at=pq._utc_now(),
        elapsed_s=elapsed,
    )
    if pq._notes_for_run_ids(conn, [run_id]).get(run_id):
        pq._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="completion projection",
        )
    rc_text = "?" if rc is None else str(rc)
    print(f"{status} {run_id} rc={rc_text} elapsed={elapsed:.1f}s")
    print(f"log: {log_path}")
    return rc if rc is not None else pq.PROOF_QUEUE_STALE_EXIT_CODE
