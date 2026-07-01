#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shlex
import sqlite3
import subprocess
import sys
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.dx import development_artifact_env  # noqa: E402

RUNNING = {"queued", "running"}


def _utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat()


def _elapsed_since(started_at: str | None, elapsed_s: float | None = None) -> str:
    if elapsed_s is not None:
        return f"{elapsed_s:.1f}s"
    if not started_at:
        return "?"
    try:
        started = dt.datetime.fromisoformat(started_at)
    except ValueError:
        return "?"
    if started.tzinfo is None:
        started = started.replace(tzinfo=dt.UTC)
    elapsed = max(0.0, (dt.datetime.now(dt.UTC) - started).total_seconds())
    return f"{elapsed:.1f}s"


def _shorten(text: str, limit: int = 180) -> str:
    collapsed = " ".join(text.strip().split())
    if len(collapsed) <= limit:
        return collapsed
    return collapsed[: max(0, limit - 3)] + "..."


def _format_duration(seconds: float) -> str:
    if seconds < 60.0:
        return f"{seconds:.1f}s"
    if seconds < 3600.0:
        return f"{seconds / 60.0:.1f}m"
    return f"{seconds / 3600.0:.1f}h"


def _last_nonempty_log_line(path: Path) -> str | None:
    try:
        size = path.stat().st_size
        with path.open("rb") as handle:
            handle.seek(max(0, size - 65536))
            text = handle.read().decode("utf-8", errors="replace")
    except OSError:
        return None
    for line in reversed(text.splitlines()):
        stripped = line.strip()
        if stripped:
            return _shorten(stripped)
    return None


def _active_log_status(row: sqlite3.Row) -> list[str]:
    path = Path(row["log_path"])
    try:
        stat = path.stat()
    except OSError:
        return [f"  log={path} (missing)"]
    age = _format_duration(max(0.0, time.time() - stat.st_mtime))
    lines = [f"  log={path}", f"  last_log_age={age}"]
    last = _last_nonempty_log_line(path)
    if last:
        lines[-1] = f"{lines[-1]} last={last}"
    return lines


def _compact_utc() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%S")


def _slug(text: str) -> str:
    out = "".join(c.lower() if c.isalnum() else "-" for c in text.strip())
    out = "-".join(part for part in out.split("-") if part)
    return out[:72] or "proof"


def _proof_session_id(resource_family: str, contention_key: str) -> str:
    digest = hashlib.sha256(contention_key.encode("utf-8")).hexdigest()[:12]
    family = _slug(resource_family)[:10]
    label = _slug(contention_key)[:8]
    return f"proof-{family}-{digest}-{label}"


def _connect(db: Path) -> sqlite3.Connection:
    db.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS proof_runs (
            run_id TEXT PRIMARY KEY,
            logical_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            status TEXT NOT NULL,
            returncode INTEGER,
            command_json TEXT NOT NULL,
            cwd TEXT NOT NULL,
            resource_family TEXT NOT NULL,
            contention_key TEXT NOT NULL,
            scopes_json TEXT NOT NULL,
            env_json TEXT NOT NULL DEFAULT '{}',
            git_json TEXT NOT NULL DEFAULT '{}',
            log_path TEXT NOT NULL,
            summary_json TEXT NOT NULL,
            guard_pid INTEGER,
            started_at TEXT,
            finished_at TEXT,
            elapsed_s REAL
        )
        """
    )
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS proof_notes (
            note_id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            author TEXT NOT NULL,
            kind TEXT NOT NULL,
            body TEXT NOT NULL,
            FOREIGN KEY(run_id) REFERENCES proof_runs(run_id)
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS proof_notes_run_id_note_id ON proof_notes(run_id, note_id)"
    )
    conn.execute(
        """
        CREATE TRIGGER IF NOT EXISTS proof_notes_append_only_no_update
        BEFORE UPDATE ON proof_notes
        BEGIN
            SELECT RAISE(ABORT, 'proof_notes is append-only');
        END
        """
    )
    conn.execute(
        """
        CREATE TRIGGER IF NOT EXISTS proof_notes_append_only_no_delete
        BEFORE DELETE ON proof_notes
        BEGIN
            SELECT RAISE(ABORT, 'proof_notes is append-only');
        END
        """
    )
    columns = {row[1] for row in conn.execute("PRAGMA table_info(proof_runs)")}
    if "env_json" not in columns:
        conn.execute(
            "ALTER TABLE proof_runs ADD COLUMN env_json TEXT NOT NULL DEFAULT '{}'"
        )
    if "git_json" not in columns:
        conn.execute(
            "ALTER TABLE proof_runs ADD COLUMN git_json TEXT NOT NULL DEFAULT '{}'"
        )
    conn.commit()
    return conn


def _default_note_author() -> str:
    for name in ("MOLT_PROOF_QUEUE_AUTHOR", "USERNAME", "USER"):
        value = os.environ.get(name)
        if value and value.strip():
            return value.strip()
    return "agent"


def _insert_note(
    conn: sqlite3.Connection,
    *,
    run_id: str,
    body: str,
    kind: str = "note",
    author: str | None = None,
) -> int:
    body = body.strip()
    kind = kind.strip() or "note"
    author = (author or _default_note_author()).strip() or "agent"
    if not body:
        raise SystemExit("proof note body must not be empty")
    exists = conn.execute(
        "SELECT 1 FROM proof_runs WHERE run_id = ?",
        (run_id,),
    ).fetchone()
    if exists is None:
        raise SystemExit(f"unknown proof run {run_id!r}")
    cursor = conn.execute(
        """
        INSERT INTO proof_notes (run_id, created_at, author, kind, body)
        VALUES (?, ?, ?, ?, ?)
        """,
        (run_id, _utc_now(), author, kind, body),
    )
    conn.commit()
    return int(cursor.lastrowid)


def _notes_from_raw(raw: object) -> list[str]:
    if raw is None:
        return []
    if isinstance(raw, str):
        return [raw]
    if isinstance(raw, list) and all(isinstance(item, str) for item in raw):
        return list(raw)
    raise SystemExit("proof notes must be a string or list of strings")


def _notes_for_run_ids(
    conn: sqlite3.Connection, run_ids: list[str]
) -> dict[str, list[dict[str, object]]]:
    if not run_ids:
        return {}
    placeholders = ",".join("?" for _ in run_ids)
    conn.row_factory = sqlite3.Row
    rows = list(
        conn.execute(
            f"""
            SELECT note_id, run_id, created_at, author, kind, body
            FROM proof_notes
            WHERE run_id IN ({placeholders})
            ORDER BY run_id, note_id
            """,
            tuple(run_ids),
        )
    )
    out: dict[str, list[dict[str, object]]] = {run_id: [] for run_id in run_ids}
    for row in rows:
        out.setdefault(row["run_id"], []).append(
            {
                "note_id": row["note_id"],
                "run_id": row["run_id"],
                "created_at": row["created_at"],
                "author": row["author"],
                "kind": row["kind"],
                "body": row["body"],
            }
        )
    return out


def _format_note_summary(notes: list[dict[str, object]]) -> str | None:
    if not notes:
        return None
    last = notes[-1]
    return (
        f"  notes={len(notes)} last_note="
        f"{last['kind']} by {last['author']}: {_shorten(str(last['body']))}"
    )


def _git_snapshot(cwd: Path) -> dict[str, object]:
    def run_git(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args],
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )

    head = run_git("rev-parse", "HEAD")
    if head.returncode != 0:
        return {"available": False}
    status = run_git("status", "--short")
    status_lines = status.stdout.splitlines() if status.returncode == 0 else []
    return {
        "available": True,
        "head": head.stdout.strip(),
        "dirty": bool(status_lines),
        "status": status_lines[:200],
    }


def _notebooks_root(args: argparse.Namespace) -> Path:
    return (
        Path(args.notebooks_root)
        if getattr(args, "notebooks_root", None)
        else _logs_root(args).parent / "notebooks"
    )


def _run_payload_with_notes(
    conn: sqlite3.Connection, rows: list[sqlite3.Row]
) -> list[dict[str, object]]:
    payload = [_row_to_payload(row) for row in rows]
    notes = _notes_for_run_ids(conn, [str(item["run_id"]) for item in payload])
    for item in payload:
        item["notes"] = notes.get(str(item["run_id"]), [])
    return payload


def _marimo_notebook_text(run: dict[str, object]) -> str:
    run_json = json.dumps(run, indent=2, sort_keys=True)
    return f'''# /// script
# dependencies = [
#   "marimo",
# ]
# ///
import marimo

__generated_with = "molt proof_queue"
app = marimo.App(width="medium")


@app.cell
def _():
    import json
    from pathlib import Path
    import marimo as mo

    run = json.loads({run_json!r})
    notes = run.get("notes", [])
    return Path, mo, notes, run


@app.cell
def _(mo, run):
    git = run.get("git", {{}})
    head = git.get("head", "unknown")
    dirty = "dirty" if git.get("dirty") else "clean"
    mo.md(
        f"""
# Proof run `{{run["run_id"]}}`

- logical id: `{{run["logical_id"]}}`
- status: `{{run["status"]}}`, return code: `{{run["returncode"]}}`
- git: `{{head}}` (`{{dirty}}`)
- contention key: `{{run["contention_key"]}}`
- reason: {{run["reason"]}}
"""
    )
    return


@app.cell
def _(run):
    run
    return


@app.cell
def _(notes):
    notes
    return


@app.cell
def _(Path, run):
    log_path = Path(run["log_path"])
    if log_path.exists():
        log_tail = "\\n".join(
            log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-120:]
        )
    else:
        log_tail = ""
    log_tail
    return


if __name__ == "__main__":
    app.run()
'''


def _write_marimo_notebook(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    run_id: str,
    output: str | None = None,
) -> Path:
    conn.row_factory = sqlite3.Row
    row = conn.execute("SELECT * FROM proof_runs WHERE run_id = ?", (run_id,)).fetchone()
    if row is None:
        raise SystemExit(f"unknown proof run {run_id!r}")
    run = _run_payload_with_notes(conn, [row])[0]
    path = Path(output) if output else _notebooks_root(args) / f"{run_id}.py"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(_marimo_notebook_text(run), encoding="utf-8")
    return path


def _db_path(args: argparse.Namespace) -> Path:
    return (
        Path(args.db)
        if args.db
        else ROOT / "logs" / "proof_queue" / "proof_queue.sqlite3"
    )


def _logs_root(args: argparse.Namespace) -> Path:
    return (
        Path(args.logs_root)
        if args.logs_root
        else ROOT / "logs" / "proof_queue" / "runs"
    )


def _repo_root(args: argparse.Namespace) -> Path:
    return Path(args.repo_root).resolve() if args.repo_root else ROOT


def _row_to_payload(row: sqlite3.Row) -> dict[str, object]:
    return {
        "run_id": row["run_id"],
        "logical_id": row["logical_id"],
        "reason": row["reason"],
        "status": row["status"],
        "returncode": row["returncode"],
        "command": json.loads(row["command_json"]),
        "cwd": row["cwd"],
        "resource_family": row["resource_family"],
        "contention_key": row["contention_key"],
        "scopes": json.loads(row["scopes_json"]),
        "env": json.loads(row["env_json"]),
        "git": json.loads(row["git_json"]),
        "log_path": row["log_path"],
        "summary_json": row["summary_json"],
        "guard_pid": row["guard_pid"],
        "started_at": row["started_at"],
        "finished_at": row["finished_at"],
        "elapsed_s": row["elapsed_s"],
    }


def _active_for_key(conn: sqlite3.Connection, key: str) -> list[sqlite3.Row]:
    conn.row_factory = sqlite3.Row
    return list(
        conn.execute(
            """
            SELECT * FROM proof_runs
            WHERE contention_key = ? AND status IN ('queued', 'running')
            ORDER BY started_at DESC
            """,
            (key,),
        )
    )


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
    conn.execute(
        """
        INSERT INTO proof_runs (
            run_id, logical_id, reason, status, command_json, cwd,
            resource_family, contention_key, scopes_json, env_json, git_json,
            log_path, summary_json
        ) VALUES (?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            run_id,
            logical_id,
            reason,
            json.dumps(command),
            str(cwd),
            resource_family,
            contention_key,
            json.dumps(scopes),
            json.dumps(env_overrides or {}, sort_keys=True),
            json.dumps(git_snapshot if git_snapshot is not None else _git_snapshot(cwd), sort_keys=True),
            str(log_path),
            str(summary_json),
        ),
    )
    conn.commit()


def _update_run(conn: sqlite3.Connection, run_id: str, **values: object) -> None:
    if not values:
        return
    assignments = ", ".join(f"{key} = ?" for key in values)
    conn.execute(
        f"UPDATE proof_runs SET {assignments} WHERE run_id = ?",
        (*values.values(), run_id),
    )
    conn.commit()


def _memory_guard_command(
    *,
    command: list[str],
    summary_json: Path,
    timeout: float,
) -> list[str]:
    return [
        sys.executable,
        str(ROOT / "tools" / "memory_guard.py"),
        "--max-rss-gb",
        "12.0",
        "--max-total-rss-gb",
        "18.0",
        "--poll-interval",
        "0.1",
        "--summary-json",
        str(summary_json),
        "--child-rlimit-gb",
        "12.0",
        "--timeout",
        str(timeout),
        "--",
        *command,
    ]


def _command_basename(command: str) -> str:
    return Path(command).name.lower()


def _has_option(command: list[str], option: str, value: str | None = None) -> bool:
    for index, arg in enumerate(command):
        if arg == option:
            return value is None or (
                index + 1 < len(command) and command[index + 1] == value
            )
        if value is not None and arg == f"{option}={value}":
            return True
    return False


def _proof_command_policy_error(command: list[str]) -> str | None:
    if len(command) < 2:
        return None
    if (
        _command_basename(command[0]) != "uv.exe"
        and _command_basename(command[0]) != "uv"
    ):
        return None
    if command[1] != "run":
        return None
    missing = []
    if not _has_option(command, "--active"):
        missing.append("--active")
    if not _has_option(command, "--project", "."):
        missing.append("--project .")
    if not _has_option(command, "--python", "3.12"):
        missing.append("--python 3.12")
    if not missing:
        return None
    return (
        "proof queue refuses `uv run` commands without the active project "
        "interpreter contract; missing "
        + ", ".join(missing)
        + ". Use `uv run --active --project . --python 3.12 ...`."
    )


def _parse_env_pair(pair: str) -> tuple[str, str]:
    if "=" not in pair:
        raise SystemExit(f"env override {pair!r} must be NAME=VALUE")
    name, value = pair.split("=", 1)
    if not name:
        raise SystemExit("env override name must not be empty")
    return name, value


def _env_overrides_from_pairs(pairs: list[str]) -> dict[str, str]:
    env: dict[str, str] = {}
    for pair in pairs:
        name, value = _parse_env_pair(pair)
        env[name] = value
    return env


def _env_overrides_from_spec(raw: object) -> dict[str, str]:
    if raw is None:
        return {}
    if isinstance(raw, dict):
        if not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in raw.items()
        ):
            raise SystemExit(
                "proof env table must contain string keys and string values"
            )
        return dict(raw)
    if isinstance(raw, list) and all(isinstance(item, str) for item in raw):
        return _env_overrides_from_pairs(list(raw))
    raise SystemExit(
        "proof env must be a table of strings or a list of NAME=VALUE strings"
    )


def _uv_active_python_command(
    *args: str, with_packages: list[str] | None = None
) -> list[str]:
    command = ["uv", "run", "--active", "--project", ".", "--python", "3.12"]
    for package in with_packages or []:
        command.extend(["--with", package])
    command.append("python")
    command.extend(args)
    return command


def _pact_witness_acceptance_spec(timeout: float | None = None) -> dict[str, object]:
    return {
        "logical_id": "pact-witness-acceptance",
        "reason": (
            "Run the Pact Kernel A browser/WASM witness acceptance aperture "
            "through queue custody."
        ),
        "command": _uv_active_python_command(
            "-m",
            "molt",
            "build",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "--target",
            "wasm",
            "--profile",
            "browser",
            "--wasm-profile",
            "auto",
            "--split-runtime",
            "--out-dir",
            "tmp/pact_witness_acceptance_queue",
        ),
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "wasm/browser_embed.js",
            "wasm/browser_host.js",
            "wasm/run_wasm.js",
        ],
        "env_overrides": {},
        "timeout": timeout if timeout is not None else 1800.0,
    }


def _pact_witness_oracle_spec(timeout: float | None = None) -> dict[str, object]:
    return {
        "logical_id": "pact-witness-oracle-parity",
        "reason": (
            "Regenerate the Pact Kernel A fixture/reference pair and prove the "
            "check_parity.py oracle under queue custody."
        ),
        "command": _uv_active_python_command(
            "tools/pact_witness_oracle.py",
            with_packages=["numpy==1.26.4", "scipy==1.17.1"],
        ),
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "tools/pact_witness_oracle.py",
        ],
        "env_overrides": {},
        "timeout": timeout if timeout is not None else 900.0,
    }


def _run_named_spec(args: argparse.Namespace, spec: dict[str, object]) -> int:
    env_overrides = dict(spec["env_overrides"])
    env_overrides.update(_env_overrides_from_pairs(args.env))
    initial_notes = _notes_from_raw(spec.get("note"))
    initial_notes.extend(_notes_from_raw(spec.get("notes")))
    initial_notes.extend(getattr(args, "note", []) or [])
    runnable = {
        **spec,
        "env_overrides": env_overrides,
    }
    if args.print_spec:
        print(json.dumps(runnable, indent=2, sort_keys=True))
        return 0
    return _run_one(
        args,
        logical_id=str(runnable["logical_id"]),
        reason=str(runnable["reason"]),
        command=list(runnable["command"]),
        resource_family=str(runnable["resource_family"]),
        contention_key=str(runnable["contention_key"]),
        scopes=list(runnable["scopes"]),
        env_overrides=dict(runnable["env_overrides"]),
        timeout=float(runnable["timeout"]),
        initial_notes=initial_notes,
    )


def _cmd_pact_witness_acceptance(args: argparse.Namespace) -> int:
    return _run_named_spec(args, _pact_witness_acceptance_spec(args.timeout))


def _cmd_pact_witness_oracle(args: argparse.Namespace) -> int:
    return _run_named_spec(args, _pact_witness_oracle_spec(args.timeout))


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
    existing_run_id: str | None = None,
    existing_log_path: Path | None = None,
    existing_summary_json: Path | None = None,
) -> int:
    if not command:
        raise SystemExit("proof command is empty")
    db = _db_path(args)
    logs_root = _logs_root(args)
    repo_root = _repo_root(args)
    conn = _connect(db)
    active = [
        row
        for row in _active_for_key(conn, contention_key)
        if existing_run_id is None or row["run_id"] != existing_run_id
    ]
    if active:
        print(
            f"contention key {contention_key!r} already has active run(s):",
            file=sys.stderr,
        )
        for row in active:
            print(f"- {row['status']} {row['run_id']} {row['reason']}", file=sys.stderr)
        return 2
    suffix = uuid.uuid4().hex[:16]
    run_id = existing_run_id or f"{_compact_utc()}-{_slug(logical_id)}-{suffix}"
    logs_root.mkdir(parents=True, exist_ok=True)
    log_path = existing_log_path or logs_root / f"{run_id}.log"
    summary_json = existing_summary_json or logs_root / f"{run_id}.memory_guard.json"
    inserted_run = existing_run_id is None
    if existing_run_id is None:
        _insert_run(
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
        for note in initial_notes or []:
            _insert_note(conn, run_id=run_id, body=note, kind="submission")
        if initial_notes:
            _write_marimo_notebook(args, conn, run_id)
    policy_error = _proof_command_policy_error(command)
    if policy_error is not None:
        now = _utc_now()
        _update_run(
            conn,
            run_id,
            status="failed",
            returncode=2,
            started_at=now,
            finished_at=now,
            elapsed_s=0.0,
        )
        with log_path.open("w", encoding="utf-8") as log:
            print(f"proof_queue run_id={run_id}", file=log)
            print(f"logical_id={logical_id}", file=log)
            print(f"reason={reason}", file=log)
            print(f"cwd={repo_root}", file=log)
            print(f"command={shlex.join(command)}", file=log)
            print("", file=log)
            print(policy_error, file=log)
        print(f"rejected {run_id} rc=2")
        print(policy_error, file=sys.stderr)
        print(f"log: {log_path}")
        if _notes_for_run_ids(conn, [run_id]).get(run_id):
            _write_marimo_notebook(args, conn, run_id)
        return 2
    session_id = _proof_session_id(resource_family, contention_key)
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
    wrapped = _memory_guard_command(
        command=command,
        summary_json=summary_json,
        timeout=timeout,
    )
    start = time.monotonic()
    started_at = _utc_now()
    _update_run(conn, run_id, status="running", started_at=started_at)
    with log_path.open("w", encoding="utf-8") as log:
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
        )
        _update_run(conn, run_id, guard_pid=proc.pid)
        rc = proc.wait()
        elapsed = time.monotonic() - start
        status = "passed" if rc == 0 else "failed"
        print(
            f"\nproof_queue finished status={status} exit_code={rc} elapsed={elapsed:.3f}s",
            file=log,
        )
    _update_run(
        conn,
        run_id,
        status=status,
        returncode=rc,
        finished_at=_utc_now(),
        elapsed_s=elapsed,
    )
    if _notes_for_run_ids(conn, [run_id]).get(run_id):
        _write_marimo_notebook(args, conn, run_id)
    print(f"{status} {run_id} rc={rc} elapsed={elapsed:.1f}s")
    print(f"log: {log_path}")
    return rc


def _command_after_dash(argv: list[str]) -> tuple[list[str], list[str]]:
    if "--" not in argv:
        return argv, []
    index = argv.index("--")
    return argv[:index], argv[index + 1 :]


def _cmd_exec(args: argparse.Namespace) -> int:
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    env_overrides = _env_overrides_from_pairs(args.env)
    initial_notes = getattr(args, "note", []) or []
    return _run_one(
        args,
        logical_id=args.id,
        reason=args.reason,
        command=command,
        resource_family=args.resource_family,
        contention_key=args.contention_key or f"{args.resource_family}:default",
        scopes=args.scope,
        env_overrides=env_overrides,
        timeout=args.timeout,
        initial_notes=initial_notes,
    )


def _load_specs(path: Path) -> list[dict[str, object]]:
    with path.open("rb") as handle:
        payload = tomllib.load(handle)
    raw = payload.get("proof", [])
    if isinstance(raw, dict):
        raw = [raw]
    if not isinstance(raw, list):
        raise SystemExit("proof DSL must contain [[proof]] tables")
    specs: list[dict[str, object]] = []
    for entry in raw:
        if not isinstance(entry, dict):
            raise SystemExit("each proof entry must be a table")
        specs.append(entry)
    return specs


def _cmd_submit(args: argparse.Namespace) -> int:
    specs = _load_specs(Path(args.dsl))
    conn = _connect(_db_path(args))
    for spec in specs:
        logical_id = str(spec.get("id") or spec.get("logical_id") or "proof")
        command = spec.get("command")
        if not isinstance(command, list) or not all(
            isinstance(x, str) for x in command
        ):
            raise SystemExit(f"proof {logical_id!r} needs command = [..]")
        policy_error = _proof_command_policy_error(list(command))
        if policy_error is not None:
            raise SystemExit(f"proof {logical_id!r}: {policy_error}")
        env_overrides = _env_overrides_from_spec(spec.get("env"))
        initial_notes = _notes_from_raw(spec.get("note"))
        initial_notes.extend(_notes_from_raw(spec.get("notes")))
        run_id = f"{_compact_utc()}-{_slug(logical_id)}-{uuid.uuid4().hex[:16]}"
        log_path = _logs_root(args) / f"{run_id}.log"
        summary_json = _logs_root(args) / f"{run_id}.memory_guard.json"
        _insert_run(
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=str(spec.get("reason") or logical_id),
            command=list(command),
            cwd=_repo_root(args),
            resource_family=str(spec.get("resource_family") or "generic"),
            contention_key=str(spec.get("contention_key") or "generic:default"),
            scopes=[str(x) for x in spec.get("scope", [])],
            env_overrides=env_overrides,
            log_path=log_path,
            summary_json=summary_json,
        )
        for note in initial_notes:
            _insert_note(conn, run_id=run_id, body=note, kind="submission")
        if initial_notes:
            _write_marimo_notebook(args, conn, run_id)
        print(f"queued {run_id}")
    return 0


def _cmd_run(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    conn.row_factory = sqlite3.Row
    rows = list(
        conn.execute(
            "SELECT * FROM proof_runs WHERE status = 'queued' ORDER BY rowid LIMIT ?",
            (args.limit,),
        )
    )
    rc = 0
    for row in rows:
        payload = _row_to_payload(row)
        rc = _run_one(
            args,
            logical_id=str(payload["logical_id"]),
            reason=str(payload["reason"]),
            command=list(payload["command"]),
            resource_family=str(payload["resource_family"]),
            contention_key=str(payload["contention_key"]),
            scopes=list(payload["scopes"]),
            env_overrides=dict(payload["env"]),
            timeout=args.timeout,
            existing_run_id=str(payload["run_id"]),
            existing_log_path=Path(str(payload["log_path"])),
            existing_summary_json=Path(str(payload["summary_json"])),
        )
        if rc != 0:
            break
    if not rows:
        print("no queued proofs")
    return rc


def _cmd_status(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    conn.row_factory = sqlite3.Row
    active = list(
        conn.execute(
            "SELECT * FROM proof_runs WHERE status IN ('queued', 'running') ORDER BY started_at"
        )
    )
    recent = list(
        conn.execute(
            "SELECT * FROM proof_runs WHERE status NOT IN ('queued', 'running') ORDER BY finished_at DESC LIMIT ?",
            (args.recent,),
        )
    )
    notes_by_run = _notes_for_run_ids(
        conn, [row["run_id"] for row in [*active, *recent]]
    )
    print("proof queue")
    print("active:")
    if not active:
        print("- none")
    for row in active:
        elapsed = f" elapsed={_elapsed_since(row['started_at'], row['elapsed_s'])}"
        print(f"- {row['status']}{elapsed} {row['run_id']} {row['reason']}")
        note_summary = _format_note_summary(notes_by_run.get(row["run_id"], []))
        if note_summary:
            print(note_summary)
        for line in _active_log_status(row):
            print(line)
    print("recent:")
    if not recent:
        print("- none")
    for row in recent:
        rc = "?" if row["returncode"] is None else row["returncode"]
        elapsed = "?" if row["elapsed_s"] is None else f"{row['elapsed_s']:.1f}s"
        print(
            f"- {row['status']:9} rc={rc} elapsed={elapsed} {row['run_id']} {row['reason']}"
        )
        note_summary = _format_note_summary(notes_by_run.get(row["run_id"], []))
        if note_summary:
            print(note_summary)
    return 0


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def _cmd_prune_stale(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    conn.row_factory = sqlite3.Row
    rows = list(
        conn.execute(
            "SELECT * FROM proof_runs WHERE status IN ('queued', 'running') ORDER BY started_at"
        )
    )
    pruned = 0
    for row in rows:
        pid = row["guard_pid"]
        if row["status"] == "queued" or (pid is not None and _pid_alive(int(pid))):
            continue
        _update_run(
            conn,
            row["run_id"],
            status="stale",
            finished_at=_utc_now(),
        )
        pruned += 1
        print(f"stale {row['run_id']}")
    print(f"pruned={pruned}")
    return 0


def _cmd_evidence(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    conn.row_factory = sqlite3.Row
    if args.run_id:
        rows = list(
            conn.execute("SELECT * FROM proof_runs WHERE run_id = ?", (args.run_id,))
        )
    else:
        rows = list(
            conn.execute(
                "SELECT * FROM proof_runs ORDER BY rowid DESC LIMIT ?", (args.limit,)
            )
        )
    payload = _run_payload_with_notes(conn, rows)
    text = json.dumps(payload, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).write_text(text + "\n", encoding="utf-8")
    else:
        print(text)
    return 0


def _cmd_note(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    note_ids = []
    for body in args.note:
        note_ids.append(
            _insert_note(
                conn,
                run_id=args.run_id,
                body=body,
                kind=args.kind,
                author=args.author,
            )
        )
    notebook_path = None
    if not args.no_notebook:
        notebook_path = _write_marimo_notebook(args, conn, args.run_id, args.output)
    print(f"noted {args.run_id} note_ids={','.join(str(note_id) for note_id in note_ids)}")
    if notebook_path is not None:
        print(f"notebook: {notebook_path}")
    return 0


def _cmd_notebook(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    path = _write_marimo_notebook(args, conn, args.run_id, args.output)
    print(f"notebook: {path}")
    return 0


def _cmd_quickstart(args: argparse.Namespace) -> int:
    del args
    print(
        "uv run --active --project . --python 3.12 python tools/proof_queue.py status\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py exec "
        '--id focused-proof --reason "why this proves the changed contract" '
        "--resource-family python --contention-key python:focused --timeout 240 -- "
        "uv run --active --project . --python 3.12 pytest tests/path.py -q"
    )
    return 0


def _cmd_template(args: argparse.Namespace) -> int:
    del args
    print(
        "[[proof]]\n"
        'id = "focused-proof"\n'
        'reason = "Prove the changed contract, not a broad ritual."\n'
        'resource_family = "python"\n'
        'contention_key = "python:focused"\n'
        'scope = ["src/molt/cli/runtime_features.py"]\n'
        'env = { MOLT_EXTERNAL_STATIC_PACKAGES = "numpy scipy" }\n'
        'command = ["uv", "run", "--active", "--project", ".", "--python", "3.12", "pytest", "tests/path.py", "-q"]\n'
    )
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Submit, run, and inspect Molt proof lanes with contention limits."
    )
    parser.add_argument("--db")
    parser.add_argument("--logs-root")
    parser.add_argument("--notebooks-root")
    parser.add_argument("--repo-root")
    sub = parser.add_subparsers(dest="cmd", required=True)

    exec_p = sub.add_parser("exec", help="submit and run one inline proof")
    exec_p.add_argument("--id", required=True)
    exec_p.add_argument("--reason", required=True)
    exec_p.add_argument("--resource-family", default="generic")
    exec_p.add_argument("--contention-key")
    exec_p.add_argument("--scope", action="append", default=[])
    exec_p.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    exec_p.add_argument("--note", action="append", default=[])
    exec_p.add_argument("--timeout", type=float, default=1200.0)
    exec_p.add_argument("--wait", action="store_true")
    exec_p.add_argument("--wait-timeout", type=float)
    exec_p.add_argument("command", nargs=argparse.REMAINDER)
    exec_p.set_defaults(func=_cmd_exec)

    submit_p = sub.add_parser("submit", help="submit proof specs from a TOML DSL")
    submit_p.add_argument("dsl")
    submit_p.set_defaults(func=_cmd_submit)

    run_p = sub.add_parser("run", help="run queued proof specs")
    run_p.add_argument("--limit", type=int, default=1)
    run_p.add_argument("--timeout", type=float, default=1200.0)
    run_p.set_defaults(func=_cmd_run)

    status_p = sub.add_parser("status", help="show active and recent proof runs")
    status_p.add_argument("--recent", type=int, default=20)
    status_p.set_defaults(func=_cmd_status)

    evidence_p = sub.add_parser(
        "evidence", help="export machine-readable proof evidence"
    )
    evidence_p.add_argument("--run-id")
    evidence_p.add_argument("--limit", type=int, default=20)
    evidence_p.add_argument("--output")
    evidence_p.set_defaults(func=_cmd_evidence)

    note_p = sub.add_parser("note", help="append an immutable note to a proof run")
    note_p.add_argument("run_id")
    note_p.add_argument("--note", action="append", required=True)
    note_p.add_argument("--kind", default="note")
    note_p.add_argument("--author")
    note_p.add_argument("--output")
    note_p.add_argument("--no-notebook", action="store_true")
    note_p.set_defaults(func=_cmd_note)

    notebook_p = sub.add_parser(
        "notebook", help="write the deterministic marimo notebook for a proof run"
    )
    notebook_p.add_argument("run_id")
    notebook_p.add_argument("--output")
    notebook_p.set_defaults(func=_cmd_notebook)

    prune_p = sub.add_parser("prune-stale", help="mark dead running records stale")
    prune_p.set_defaults(func=_cmd_prune_stale)

    quickstart_p = sub.add_parser(
        "quickstart", help="print canonical queue muscle memory"
    )
    quickstart_p.set_defaults(func=_cmd_quickstart)

    template_p = sub.add_parser("template", help="print a proof DSL template")
    template_p.set_defaults(func=_cmd_template)

    pact_accept_p = sub.add_parser(
        "pact-witness-acceptance",
        help="run the queue-owned Pact Kernel A browser/WASM acceptance aperture",
    )
    pact_accept_p.add_argument(
        "--env", action="append", default=[], metavar="NAME=VALUE"
    )
    pact_accept_p.add_argument("--note", action="append", default=[])
    pact_accept_p.add_argument("--timeout", type=float)
    pact_accept_p.add_argument("--print-spec", action="store_true")
    pact_accept_p.set_defaults(func=_cmd_pact_witness_acceptance)

    pact_oracle_p = sub.add_parser(
        "pact-witness-oracle",
        help="run the queued Pact Kernel A fixture/reference parity oracle",
    )
    pact_oracle_p.add_argument(
        "--env", action="append", default=[], metavar="NAME=VALUE"
    )
    pact_oracle_p.add_argument("--note", action="append", default=[])
    pact_oracle_p.add_argument("--timeout", type=float)
    pact_oracle_p.add_argument("--print-spec", action="store_true")
    pact_oracle_p.set_defaults(func=_cmd_pact_witness_oracle)
    return parser


def main(argv: list[str] | None = None) -> int:
    raw = list(sys.argv[1:] if argv is None else argv)
    if raw and raw[0] == "exec":
        before, command = _command_after_dash(raw)
        parser = _build_parser()
        args = parser.parse_args(before)
        args.command = command
    else:
        parser = _build_parser()
        args = parser.parse_args(raw)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
