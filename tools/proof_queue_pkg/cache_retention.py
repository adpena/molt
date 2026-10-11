"""Coalesced development-cache maintenance under the existing proof queue owner."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import sys
import time
import uuid
from collections.abc import Mapping

from molt.cache_eviction import AUTOMATIC_PRUNE_INTERVAL_SECONDS, resolve_budget
from molt.default_paths import _default_molt_cache, cache_control_root
from molt.file_locks import (
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)
from tools.proof_queue_pkg import policy, runner, scheduling, state

LOGICAL_ID = "maintenance.cache-retention"
TIMEOUT_SECONDS = 120.0


def schedule_after_proof(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    completed_run_id: str,
    environ: Mapping[str, str],
) -> str | None:
    """Admit at most one maintenance row, with no recursive I/O or DB waiting.

    The cache's persistent prune lock serializes admission across checkouts and
    queue databases. Its timestamp throttles attempts, including failed workers;
    successful workers refresh it on completion. Existing queued rows are reused.
    Installed ordinary CLI builds do not import or depend on the proof queue.
    """
    if environ.get("MOLT_CACHE_PRUNE", "1") == "0":
        return None
    lock = None
    prior_timeout = None
    try:
        prior_timeout = int(conn.execute("PRAGMA busy_timeout").fetchone()[0])
        conn.execute("PRAGMA busy_timeout=0")
        parent = state._row_by_run_id(conn, completed_run_id)
        if parent is None or parent["logical_id"] == LOGICAL_ID:
            return None
        repo_root = state._repo_root(args)
        # This is a source-development boundary, not an installed-CLI hook.
        if not (repo_root / "tools/proof_queue.py").is_file():
            return None
        root = _default_molt_cache(environ=environ).resolve(strict=False)
        if not root.is_dir():
            return None
        max_gb, max_age = resolve_budget(None, None, environ)
        lock_path = cache_control_root(root) / "prune.lock"
        lock = _try_acquire_file_lock(lock_path)
        if lock is None:
            return None

        def admit() -> tuple[str, bool] | None:
            key = "cache-retention:" + hashlib.sha256(os.fsencode(root)).hexdigest()
            pending = conn.execute(
                "SELECT run_id, status FROM proof_runs WHERE contention_key = ? "
                "AND status IN ('queued', 'dispatched', 'running', 'stale') ORDER BY rowid LIMIT 1",
                (key,),
            ).fetchone()
            if pending is not None:
                return str(pending["run_id"]), pending["status"] == "queued"
            now = time.time()
            lock.file.seek(0)
            stamp = lock.file.read(64)
            if stamp:
                try:
                    elapsed = now - float(stamp)
                except ValueError:
                    elapsed = AUTOMATIC_PRUNE_INTERVAL_SECONDS
                if 0 <= elapsed < AUTOMATIC_PRUNE_INTERVAL_SECONDS:
                    return None
            command = [
                sys.executable,
                "-m",
                "molt.cache_eviction",
                "--cache-dir",
                str(root),
                "--max-gb",
                str(max_gb),
                "--max-age-days",
                str(max_age),
            ]
            env = {
                name: environ[name]
                for name in ("MOLT_HOME", "MOLT_BIN")
                if name in environ
            }
            env.update(MOLT_CACHE=str(root), MOLT_CACHE_PRUNE="0")
            error = policy._proof_command_policy_error(
                command
            ) or policy._proof_env_policy_error(env)
            if error:
                raise ValueError(error)
            run_id = f"{state._compact_utc()}-cache-retention-{uuid.uuid4().hex[:16]}"
            logs = state._logs_root(args)
            scheduling._insert_run(
                conn,
                run_id=run_id,
                logical_id=LOGICAL_ID,
                reason="Apply canonical development cache budget after proof completion",
                command=command,
                cwd=repo_root,
                resource_family="maintenance",
                contention_key=key,
                scopes=["development-cache-retention"],
                env_overrides=env,
                git_snapshot=json.loads(parent["git_json"]),
                log_path=logs / f"{run_id}.log",
                summary_json=logs / f"{run_id}.memory_guard.json",
            )
            state._insert_note(
                conn,
                run_id=run_id,
                kind="submission",
                body=f"Automatic retention admitted after {completed_run_id}; source/toolchain custody is captured by the maintenance execution, not by this admission.",
            )
            # Record only admitted attempts. A DB/policy failure remains eligible;
            # a queued or failed worker is retained by the queue's own lifecycle.
            lock.file.seek(0)
            lock.file.write(str(now).encode("ascii"))
            lock.file.truncate()
            lock.file.flush()
            return run_id, True

        with _file_lock_owned_operation(lock, expected_lock_path=lock_path):
            admitted = admit()
        if admitted is None:
            return None
        run_id, dispatch = admitted
        # Never launch while holding the lock the maintenance payload must take.
        _release_file_lock(lock)
        lock = None
        if dispatch:
            runner._dispatch_detached_runner(
                args, conn, run_id=run_id, timeout=TIMEOUT_SECONDS
            )
        return run_id
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
        if conn.in_transaction:
            conn.rollback()
        print(
            f"cache maintenance admission failed (proof result unchanged): {exc}",
            file=sys.stderr,
        )
        return None
    finally:
        if lock is not None:
            _release_file_lock(lock)
        if prior_timeout is not None:
            conn.execute(f"PRAGMA busy_timeout={prior_timeout}")
