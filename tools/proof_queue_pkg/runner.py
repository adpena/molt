"""Queue submission and memory-guarded proof execution lifecycle."""

from __future__ import annotations

import argparse
import hashlib
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
    command_envelope,
    custody,
    evidence,
    policy,
    scheduling,
    state,
)
from tools.proof_queue_pkg import diagnostics as diagnostic_engine


def _file_receipt_identity(path: Path) -> dict[str, object]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            size += len(chunk)
            digest.update(chunk)
    return {"path": str(path), "size_bytes": size, "sha256": digest.hexdigest()}


def _validated_guard_receipt(
    summary_json: Path,
    *,
    guarded_command: list[str],
    returncode: int,
    run_id: str,
    execution_nonce: str,
    guard_pid: int,
) -> dict[str, object]:
    payload = json.loads(summary_json.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError("memory-guard receipt is not an object")
    if payload.get("command") != guarded_command:
        raise ValueError("memory-guard receipt command substitution detected")
    if payload.get("returncode") != returncode:
        raise ValueError("memory-guard receipt return-code substitution detected")
    dirty_terminal_fields = {
        "violation": payload.get("violation"),
        "timed_out": payload.get("timed_out"),
        "exit_signal": payload.get("exit_signal"),
        "guard_signal": payload.get("guard_signal"),
        "incident": payload.get("incident"),
        "orphaned_process_groups": payload.get("orphaned_process_groups"),
    }
    if (
        dirty_terminal_fields["violation"] is not None
        or dirty_terminal_fields["timed_out"] is not False
        or dirty_terminal_fields["exit_signal"] is not None
        or dirty_terminal_fields["guard_signal"] is not None
        or dirty_terminal_fields["incident"] is not None
        or dirty_terminal_fields["orphaned_process_groups"] not in ([], ())
    ):
        raise ValueError(
            "memory-guard receipt does not prove a clean terminal outcome: "
            + json.dumps(dirty_terminal_fields, sort_keys=True)
        )
    windows_cleanup = payload.get("windows_job_cleanup")
    if os.name == "nt" and not isinstance(windows_cleanup, dict):
        raise ValueError("memory-guard Windows job cleanup is missing")
    if isinstance(windows_cleanup, dict):
        after_cleanup = windows_cleanup.get("after")
        if (
            windows_cleanup.get("completed") is not True
            or not isinstance(after_cleanup, dict)
            or after_cleanup.get("active_processes") != 0
            or windows_cleanup.get("terminated_remaining_processes") is not False
        ):
            raise ValueError("memory-guard Windows job cleanup is incomplete")
    sampling = payload.get("sampling_telemetry")
    if (
        not isinstance(sampling, dict)
        or sampling.get("enforcement_complete") is not True
        or not isinstance(sampling.get("attempts"), int)
        or sampling.get("attempts") != sampling.get("successes")
        or sampling.get("transient_failures") != 0
    ):
        raise ValueError("memory-guard sampling enforcement is incomplete")
    receipt = _file_receipt_identity(summary_json)
    receipt["identity_sha256"] = hashlib.sha256(
        json.dumps(
            {
                **receipt,
                "run_id": run_id,
                "execution_nonce": execution_nonce,
                "guard_pid": guard_pid,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    return receipt


def _validated_execution_context(
    context: dict[str, object],
    *,
    execution_path: Path,
    envelope: dict[str, object],
    run_id: str,
    execution_nonce: str,
    returncode: int,
) -> None:
    if context.get("run_id") != run_id:
        raise ValueError("guarded receipt context run identity mismatch")
    expected_nonce_hash = hashlib.sha256(execution_nonce.encode()).hexdigest()
    if context.get("execution_nonce_sha256") != expected_nonce_hash:
        raise ValueError("guarded receipt context nonce substitution detected")
    if context.get("command_envelope") != envelope:
        raise ValueError("guarded receipt context command substitution detected")
    requested_toolchains = envelope.get("toolchains")
    captured_toolchains = context.get("toolchains")
    custody = context.get("toolchain_custody")
    if (
        not isinstance(requested_toolchains, list)
        or not requested_toolchains
        or not all(isinstance(name, str) and name for name in requested_toolchains)
    ):
        raise ValueError("guarded command envelope has no toolchain authority")
    if (
        not isinstance(captured_toolchains, dict)
        or set(captured_toolchains) != set(requested_toolchains)
        or any(
            not isinstance(identity, dict)
            or not isinstance(identity.get("identity_sha256"), str)
            for identity in captured_toolchains.values()
        )
    ):
        raise ValueError("guarded receipt toolchain closure is incomplete")
    if (
        not isinstance(custody, dict)
        or custody.get("identical") is not True
        or not isinstance(custody.get("prelaunch"), dict)
        or not isinstance(custody.get("postcompletion"), dict)
        or command_envelope._stable_toolchain_custody(custody["prelaunch"])
        != command_envelope._stable_toolchain_custody(captured_toolchains)
        or command_envelope._stable_toolchain_custody(custody["postcompletion"])
        != command_envelope._stable_toolchain_custody(captured_toolchains)
    ):
        raise ValueError("guarded receipt toolchain closure is not stable")
    expected_custody = command_envelope.execution_custody_sha256(
        context,
        run_id=run_id,
        returncode=returncode,
    )
    if context.get("execution_custody_sha256") != expected_custody:
        raise ValueError("guarded execution custody digest mismatch")
    transcript = context.get("command_transcript")
    if not isinstance(transcript, dict):
        raise ValueError("guarded receipt context has no command transcript")
    for stream_name in ("stdout", "stderr"):
        expected_path = execution_path.with_suffix(f".{stream_name}.bin")
        expected = transcript.get(stream_name)
        if not isinstance(expected, dict) or expected.get("path") != str(expected_path):
            raise ValueError(
                f"guarded {stream_name} transcript path substitution detected"
            )
        actual = command_envelope._transcript_identity(expected_path)
        if expected != actual:
            raise ValueError(
                f"guarded {stream_name} transcript content substitution detected"
            )
    transcript_material = {name: transcript[name] for name in ("stdout", "stderr")}
    transcript_digest = hashlib.sha256(
        json.dumps(
            transcript_material,
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    if transcript.get("identity_sha256") != transcript_digest:
        raise ValueError("guarded command transcript digest mismatch")


def _write_execution_request(
    *,
    row: sqlite3.Row,
    command: list[str],
    repo_root: Path,
    resource_family: str,
    run_id: str,
    env_override_names: list[str],
    log_path: Path,
    summary_path: Path,
) -> tuple[Path, Path, dict[str, object], str]:
    envelope = json.loads(str(row["command_envelope_json"]))
    if not isinstance(envelope, dict):
        raise ValueError("proof row command envelope is malformed")
    command_envelope.validate_envelope(envelope, command)
    request_path = log_path.with_suffix(".execution-request.json")
    result_path = log_path.with_suffix(".execution.json")
    execution_nonce = uuid.uuid4().hex + uuid.uuid4().hex
    for stale in (
        request_path,
        result_path,
        result_path.with_suffix(".stdout.bin"),
        result_path.with_suffix(".stderr.bin"),
        summary_path,
    ):
        try:
            stale.unlink()
        except FileNotFoundError:
            pass
    request = {
        "schema": command_envelope.EXECUTION_SCHEMA,
        "command": command,
        "envelope": envelope,
        "cwd": str(repo_root),
        "resource_family": resource_family,
        "result_path": str(result_path),
        "run_id": run_id,
        "execution_nonce": execution_nonce,
        "env_override_names": sorted(env_override_names, key=str.casefold),
    }
    request_path.write_text(
        json.dumps(request, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return request_path, result_path, envelope, execution_nonce


def _read_execution_record(path: Path) -> dict[str, object]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(
            f"guarded proof execution record is unavailable: {exc}"
        ) from exc
    if (
        not isinstance(payload, dict)
        or payload.get("schema") != command_envelope.EXECUTION_SCHEMA
    ):
        raise ValueError("guarded proof execution record schema mismatch")
    return payload


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
                    custody._terminate_queue_owned_guard_process(
                        proc, log, run_id=run_id
                    )
                return str(row["status"]), row["returncode"], elapsed
            diagnostics = diagnostic_engine._run_diagnostics(row)
            if not diagnostic_engine._diagnostics_have_terminal_stale_signal(
                diagnostics
            ):
                continue
            diagnostic_summary = diagnostic_engine._format_diagnostic_summary(
                diagnostics
            )
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
            guard_rc = custody._terminate_queue_owned_guard_process(
                proc, log, run_id=run_id
            )
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
    secret_error = command_envelope.command_secret_policy_error(command)
    env_error = policy._proof_env_policy_error(env_overrides)
    if secret_error is not None or env_error is not None:
        print(secret_error or env_error, file=sys.stderr)
        return 2, None
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
            state._insert_note(
                conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND
            )
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
                + scheduling._format_active_contention_conflicts(active).replace(
                    "\n", "; "
                ),
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
        pid, runner_log = custody._launch_detached_runner(
            args, run_id=run_id, timeout=timeout
        )
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
        receipt_context_json=json.dumps(
            state._unattested_receipt_context(
                status="not-executed",
                phase="command policy rejection",
                reason=policy_error,
            ),
            sort_keys=True,
        ),
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
    secret_error = command_envelope.command_secret_policy_error(command)
    env_error = policy._proof_env_policy_error(env_overrides)
    if secret_error is not None or env_error is not None:
        print(secret_error or env_error, file=sys.stderr)
        return 2
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
    run_id = (
        existing_run_id or f"{state._compact_utc()}-{state._slug(logical_id)}-{suffix}"
    )
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
                state._insert_note(
                    conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND
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
        row = state._row_by_run_id(conn, run_id)
        if row is None:
            raise ValueError(f"proof run {run_id!r} disappeared before execution")
        request_path, execution_path, envelope, execution_nonce = (
            _write_execution_request(
                row=row,
                command=command,
                repo_root=repo_root,
                resource_family=resource_family,
                run_id=run_id,
                env_override_names=list(env_overrides),
                log_path=log_path,
                summary_path=summary_json,
            )
        )
        poll_interval = custody._proof_queue_memory_guard_poll_sec(env_overrides)
        env[custody.MEMORY_GUARD_POLL_SEC_ENV] = poll_interval
        guarded_command = [
            sys.executable,
            str(state.ROOT / "tools" / "proof_queue_pkg" / "command_envelope.py"),
            "--request",
            str(request_path),
        ]
        wrapped = custody._memory_guard_command(
            command=guarded_command,
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
                "env_override_names="
                + json.dumps(sorted(env_overrides, key=str.casefold)),
                file=log,
            )
        print(f"proof_session_id={session_id}", file=log)
        print(f"cargo_target_dir={env.get('CARGO_TARGET_DIR', '')}", file=log)
        print(
            "command_envelope=" + json.dumps(envelope, sort_keys=True),
            file=log,
        )
        print(f"memory_guard_poll_sec={poll_interval}", file=log)
        print(f"memory_guard_summary_json={summary_json}", file=log)
        print(f"memory_guard_command={shlex.join(wrapped)}", file=log)
        print("", file=log, flush=True)
        proc = custody._launch_queued_command(
            wrapped,
            cwd=repo_root,
            env=env,
            stdout=log,
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
    receipt_context: dict[str, object] | None = None
    execution_error: str | None = None
    try:
        execution_record = _read_execution_record(execution_path)
        if execution_record.get("run_id") != run_id:
            raise ValueError("guarded execution record run identity mismatch")
        if execution_record.get("execution_nonce") != execution_nonce:
            raise ValueError("guarded execution record nonce mismatch")
        if execution_record.get("envelope") != envelope:
            raise ValueError("guarded execution record changed the admitted envelope")
        raw_context = execution_record.get("receipt_context")
        if isinstance(raw_context, dict):
            receipt_context = raw_context
        if execution_record.get("phase") == "complete":
            command_rc = execution_record.get("command_returncode")
            if not isinstance(command_rc, int) or command_rc != rc:
                raise ValueError(
                    "guard/command return-code custody mismatch: "
                    f"guard={rc!r} command={command_rc!r}"
                )
            if receipt_context is None:
                raise ValueError("complete guarded execution has no receipt context")
            _validated_execution_context(
                receipt_context,
                execution_path=execution_path,
                envelope=envelope,
                run_id=run_id,
                execution_nonce=execution_nonce,
                returncode=command_rc,
            )
            guard_receipt = _validated_guard_receipt(
                summary_json,
                guarded_command=guarded_command,
                returncode=rc,
                run_id=run_id,
                execution_nonce=execution_nonce,
                guard_pid=proc.pid,
            )
            receipt_context["guard_receipt"] = guard_receipt
            receipt_context["terminal_evidence_sha256"] = (
                command_envelope.terminal_evidence_sha256(
                    receipt_context,
                    run_id=run_id,
                    returncode=command_rc,
                )
            )
            execution_record["receipt_context"] = receipt_context
            execution_temporary = execution_path.with_suffix(
                execution_path.suffix + ".terminal.tmp"
            )
            execution_temporary.write_text(
                json.dumps(execution_record, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            os.replace(execution_temporary, execution_path)
            source_custody = (
                receipt_context.get("source_custody")
                if receipt_context is not None
                else None
            )
            eligible = (
                isinstance(source_custody, dict)
                and source_custody.get("evidence_eligible") is True
            )
            if status == "passed" and not eligible:
                status = "non-evidence"
                rc = 2
                execution_error = (
                    "source custody changed or was unavailable between prelaunch "
                    "and postcompletion"
                )
        elif status == "passed":
            raise ValueError(
                "memory guard passed without a complete command execution record"
            )
        if isinstance(execution_record.get("error"), str):
            execution_error = str(execution_record["error"])
    except Exception as exc:
        execution_error = f"{type(exc).__name__}: {exc}"
        if status == "passed":
            status = "failed"
            rc = 2
    if receipt_context is None:
        receipt_context = state._unattested_receipt_context(
            status="non-evidence",
            phase="guarded command envelope",
            reason=execution_error or "guarded execution produced no receipt context",
        )
    if execution_error:
        with log_path.open("a", encoding="utf-8") as terminal_log:
            print(
                f"proof_queue execution custody: {execution_error}", file=terminal_log
            )
    state._update_run(
        conn,
        run_id,
        status=status,
        returncode=rc,
        finished_at=state._utc_now(),
        elapsed_s=elapsed,
        receipt_context_json=json.dumps(receipt_context, sort_keys=True),
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
