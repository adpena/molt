#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import sqlite3
import subprocess
import sys
import time
import tomllib
from typing import Sequence
import uuid

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.dx import development_artifact_env  # noqa: E402
from molt.cli import wasm_toolchain  # noqa: E402

RUNNING = {"queued", "running"}
NOTE_KIND_DESCRIPTIONS = {
    "submission": "note captured when the run is submitted",
    "change": "source, config, artifact, or environment change being proved",
    "hypothesis": "expected cause or behavior before the run finishes",
    "test": "what the command is meant to prove or falsify",
    "observation": "live status, log interpretation, or post-submit context",
    "finding": "conclusion from evidence after inspection",
    "decision": "chosen next structural move or rejected alternative",
    "followup": "bounded next action that remains after the run",
    "handoff": "context needed by another agent or future session",
}
NOTE_KINDS = frozenset(NOTE_KIND_DESCRIPTIONS)
DEFAULT_NOTE_KIND = "observation"
SUBMISSION_NOTE_KIND = "submission"
EDGE_KIND_DESCRIPTIONS = {
    "depends_on": "child proof must wait for the parent proof to pass",
    "derives_from": "child proof explores or narrows evidence from the parent proof",
    "reruns": "child proof repeats the parent proof after a change",
    "compares": "child proof is intended for side-by-side comparison with the parent",
    "supersedes": "child proof replaces the parent proof as current evidence",
}
EDGE_KINDS = frozenset(EDGE_KIND_DESCRIPTIONS)
DEFAULT_EDGE_KIND = "depends_on"

WASM_RESOURCE_FAMILIES = frozenset({"wasm", "wasm-browser"})
DIAGNOSTIC_LOG_TAIL_BYTES = 256 * 1024
STATIC_PYMOD_EXEC_RE = re.compile(
    r"ImportError:\s+"
    r"(?P<module>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)"
    r": static-link PyModuleDef Py_mod_exec slot returned non-zero"
    r"(?P<detail>[^\r\n]*)"
)
UNDEFINED_SYMBOL_RE = re.compile(
    r"(?:wasm-ld: error: .*?undefined symbol:|undefined symbol:)\s+"
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_@.$]*)"
)
UNSUPPORTED_DIRECT_CALL_RE = re.compile(
    r"(?is)(?:unsupported|not supported|not linkable).*?"
    r"(?:direct call|direct-call).*?"
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_.]*)"
)


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


def _read_log_tail(path: Path, *, limit: int = DIAGNOSTIC_LOG_TAIL_BYTES) -> str:
    try:
        size = path.stat().st_size
        with path.open("rb") as handle:
            handle.seek(max(0, size - limit))
            return handle.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


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
    conn.execute("PRAGMA foreign_keys=ON")
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
        CREATE TABLE IF NOT EXISTS proof_note_kinds (
            kind TEXT PRIMARY KEY,
            description TEXT NOT NULL
        )
        """
    )
    placeholders = ",".join("?" for _ in NOTE_KINDS)
    conn.execute(
        f"DELETE FROM proof_note_kinds WHERE kind NOT IN ({placeholders})",
        tuple(sorted(NOTE_KINDS)),
    )
    conn.executemany(
        """
        INSERT INTO proof_note_kinds (kind, description)
        VALUES (?, ?)
        ON CONFLICT(kind) DO UPDATE SET description = excluded.description
        """,
        sorted(NOTE_KIND_DESCRIPTIONS.items()),
    )
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS proof_edge_kinds (
            kind TEXT PRIMARY KEY,
            description TEXT NOT NULL
        )
        """
    )
    edge_placeholders = ",".join("?" for _ in EDGE_KINDS)
    conn.execute(
        f"DELETE FROM proof_edge_kinds WHERE kind NOT IN ({edge_placeholders})",
        tuple(sorted(EDGE_KINDS)),
    )
    conn.executemany(
        """
        INSERT INTO proof_edge_kinds (kind, description)
        VALUES (?, ?)
        ON CONFLICT(kind) DO UPDATE SET description = excluded.description
        """,
        sorted(EDGE_KIND_DESCRIPTIONS.items()),
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
        CREATE TABLE IF NOT EXISTS proof_run_edges (
            edge_id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_run_id TEXT NOT NULL,
            child_run_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            author TEXT NOT NULL,
            kind TEXT NOT NULL,
            note TEXT NOT NULL DEFAULT '',
            FOREIGN KEY(parent_run_id) REFERENCES proof_runs(run_id),
            FOREIGN KEY(child_run_id) REFERENCES proof_runs(run_id),
            UNIQUE(parent_run_id, child_run_id, kind)
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS proof_run_edges_child_edge_id ON proof_run_edges(child_run_id, edge_id)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS proof_run_edges_parent_edge_id ON proof_run_edges(parent_run_id, edge_id)"
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
    conn.execute(
        """
        CREATE TRIGGER IF NOT EXISTS proof_notes_known_kind
        BEFORE INSERT ON proof_notes
        WHEN NOT EXISTS (
            SELECT 1 FROM proof_note_kinds WHERE kind = NEW.kind
        )
        BEGIN
            SELECT RAISE(ABORT, 'unknown proof note kind');
        END
        """
    )
    conn.execute(
        """
        CREATE TRIGGER IF NOT EXISTS proof_run_edges_append_only_no_update
        BEFORE UPDATE ON proof_run_edges
        BEGIN
            SELECT RAISE(ABORT, 'proof_run_edges is append-only');
        END
        """
    )
    conn.execute(
        """
        CREATE TRIGGER IF NOT EXISTS proof_run_edges_append_only_no_delete
        BEFORE DELETE ON proof_run_edges
        BEGIN
            SELECT RAISE(ABORT, 'proof_run_edges is append-only');
        END
        """
    )
    conn.execute(
        """
        CREATE TRIGGER IF NOT EXISTS proof_run_edges_known_kind
        BEFORE INSERT ON proof_run_edges
        WHEN NOT EXISTS (
            SELECT 1 FROM proof_edge_kinds WHERE kind = NEW.kind
        )
        BEGIN
            SELECT RAISE(ABORT, 'unknown proof edge kind');
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
    kind: str = DEFAULT_NOTE_KIND,
    author: str | None = None,
) -> int:
    body = body.strip()
    kind = kind.strip() or DEFAULT_NOTE_KIND
    author = (author or _default_note_author()).strip() or "agent"
    if not body:
        raise SystemExit("proof note body must not be empty")
    if kind not in NOTE_KINDS:
        allowed = ", ".join(sorted(NOTE_KINDS))
        raise SystemExit(f"unknown proof note kind {kind!r}; allowed: {allowed}")
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


def _strings_from_raw(raw: object, *, field_name: str) -> list[str]:
    if raw is None:
        return []
    if isinstance(raw, str):
        return [raw]
    if isinstance(raw, list) and all(isinstance(item, str) for item in raw):
        return list(raw)
    raise SystemExit(f"{field_name} must be a string or list of strings")


def _notes_from_raw(raw: object) -> list[str]:
    return _strings_from_raw(raw, field_name="proof notes")


def _dependencies_from_raw(raw: object) -> list[str]:
    return _strings_from_raw(raw, field_name="proof dependencies")


def _run_exists(conn: sqlite3.Connection, run_id: str) -> bool:
    return (
        conn.execute("SELECT 1 FROM proof_runs WHERE run_id = ?", (run_id,)).fetchone()
        is not None
    )


def _edge_would_create_cycle(
    conn: sqlite3.Connection, *, parent_run_id: str, child_run_id: str
) -> bool:
    if parent_run_id == child_run_id:
        return True
    row = conn.execute(
        """
        WITH RECURSIVE descendants(run_id) AS (
            SELECT child_run_id
            FROM proof_run_edges
            WHERE parent_run_id = ?
            UNION
            SELECT edge.child_run_id
            FROM proof_run_edges edge
            JOIN descendants ON edge.parent_run_id = descendants.run_id
        )
        SELECT 1
        FROM descendants
        WHERE run_id = ?
        LIMIT 1
        """,
        (child_run_id, parent_run_id),
    ).fetchone()
    return row is not None


def _planned_edge_would_create_cycle(
    children_by_parent: dict[str, list[str]], parent_run_id: str, child_run_id: str
) -> bool:
    if parent_run_id == child_run_id:
        return True
    seen: set[str] = set()
    stack = [child_run_id]
    while stack:
        current = stack.pop()
        if current == parent_run_id:
            return True
        if current in seen:
            continue
        seen.add(current)
        stack.extend(children_by_parent.get(current, []))
    return False


def _insert_edge(
    conn: sqlite3.Connection,
    *,
    parent_run_id: str,
    child_run_id: str,
    kind: str = DEFAULT_EDGE_KIND,
    note: str | None = None,
    author: str | None = None,
) -> int:
    parent_run_id = parent_run_id.strip()
    child_run_id = child_run_id.strip()
    kind = kind.strip() or DEFAULT_EDGE_KIND
    author = (author or _default_note_author()).strip() or "agent"
    note = (note or "").strip()
    if not parent_run_id or not child_run_id:
        raise SystemExit("proof DAG edge endpoints must not be empty")
    if kind not in EDGE_KINDS:
        allowed = ", ".join(sorted(EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {kind!r}; allowed: {allowed}")
    if not _run_exists(conn, parent_run_id):
        raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    if not _run_exists(conn, child_run_id):
        raise SystemExit(f"unknown child proof run {child_run_id!r}")
    if parent_run_id == child_run_id:
        raise SystemExit("proof DAG edge cannot point to itself")
    if _edge_would_create_cycle(
        conn, parent_run_id=parent_run_id, child_run_id=child_run_id
    ):
        raise SystemExit(
            "proof DAG edge would create a cycle: "
            f"{parent_run_id!r} -> {child_run_id!r}"
        )
    try:
        cursor = conn.execute(
            """
            INSERT INTO proof_run_edges (
                parent_run_id, child_run_id, created_at, author, kind, note
            )
            VALUES (?, ?, ?, ?, ?, ?)
            """,
            (parent_run_id, child_run_id, _utc_now(), author, kind, note),
        )
    except sqlite3.IntegrityError as exc:
        if "UNIQUE" in str(exc).upper():
            raise SystemExit(
                "duplicate proof DAG edge: "
                f"{parent_run_id!r} -> {child_run_id!r} ({kind})"
            ) from exc
        raise
    conn.commit()
    return int(cursor.lastrowid)


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


def _edge_payload(row: sqlite3.Row) -> dict[str, object]:
    return {
        "edge_id": row["edge_id"],
        "parent_run_id": row["parent_run_id"],
        "parent_status": row["parent_status"],
        "child_run_id": row["child_run_id"],
        "child_status": row["child_status"],
        "created_at": row["created_at"],
        "author": row["author"],
        "kind": row["kind"],
        "note": row["note"],
    }


def _edges_for_run_ids(
    conn: sqlite3.Connection, run_ids: list[str]
) -> dict[str, dict[str, list[dict[str, object]]]]:
    if not run_ids:
        return {}
    placeholders = ",".join("?" for _ in run_ids)
    conn.row_factory = sqlite3.Row
    rows = list(
        conn.execute(
            f"""
            SELECT
                edge.edge_id,
                edge.parent_run_id,
                parent.status AS parent_status,
                edge.child_run_id,
                child.status AS child_status,
                edge.created_at,
                edge.author,
                edge.kind,
                edge.note
            FROM proof_run_edges edge
            JOIN proof_runs parent ON parent.run_id = edge.parent_run_id
            JOIN proof_runs child ON child.run_id = edge.child_run_id
            WHERE edge.parent_run_id IN ({placeholders})
               OR edge.child_run_id IN ({placeholders})
            ORDER BY edge.edge_id
            """,
            tuple([*run_ids, *run_ids]),
        )
    )
    out: dict[str, dict[str, list[dict[str, object]]]] = {
        run_id: {"parents": [], "children": []} for run_id in run_ids
    }
    for row in rows:
        edge = _edge_payload(row)
        parent_id = str(row["parent_run_id"])
        child_id = str(row["child_run_id"])
        if parent_id in out:
            out[parent_id]["children"].append(edge)
        if child_id in out:
            out[child_id]["parents"].append(edge)
    return out


def _format_note_summary(notes: list[dict[str, object]]) -> str | None:
    if not notes:
        return None
    last = notes[-1]
    return (
        f"  notes={len(notes)} last_note="
        f"{last['kind']} by {last['author']}: {_shorten(str(last['body']))}"
    )


def _note_kind_counts(notes: list[dict[str, object]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for note in notes:
        kind = str(note["kind"])
        counts[kind] = counts.get(kind, 0) + 1
    return {kind: counts[kind] for kind in sorted(counts)}


def _edge_kind_counts(edges: list[dict[str, object]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for edge in edges:
        kind = str(edge["kind"])
        counts[kind] = counts.get(kind, 0) + 1
    return {kind: counts[kind] for kind in sorted(counts)}


def _format_dag_summary(dag: dict[str, object]) -> str | None:
    parents = list(dag.get("parents", []))
    children = list(dag.get("children", []))
    if not parents and not children:
        return None
    parts = [f"parents={len(parents)}", f"children={len(children)}"]
    if parents:
        last = parents[-1]
        parts.append(
            "last_parent="
            f"{last['kind']} {last['parent_run_id']} status={last['parent_status']}"
        )
    return "  dag=" + " ".join(parts)


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
    run_ids = [str(item["run_id"]) for item in payload]
    notes = _notes_for_run_ids(conn, run_ids)
    edges = _edges_for_run_ids(conn, run_ids)
    for row, item in zip(rows, payload, strict=True):
        run_notes = notes.get(str(item["run_id"]), [])
        run_edges = edges.get(str(item["run_id"]), {"parents": [], "children": []})
        item["notes"] = run_notes
        item["note_kind_counts"] = _note_kind_counts(run_notes)
        item["dag"] = {
            "parents": run_edges["parents"],
            "children": run_edges["children"],
            "parent_kind_counts": _edge_kind_counts(run_edges["parents"]),
            "child_kind_counts": _edge_kind_counts(run_edges["children"]),
        }
        item["diagnostics"] = _run_diagnostics(row)
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
    note_counts = run.get("note_kind_counts", {{}})
    dag = run.get("dag", {{}})
    parent_counts = dag.get("parent_kind_counts", {{}})
    child_counts = dag.get("child_kind_counts", {{}})
    note_summary = ", ".join(
        f"{{kind}}={{count}}" for kind, count in note_counts.items()
    ) or "none"
    parent_summary = ", ".join(
        f"{{kind}}={{count}}" for kind, count in parent_counts.items()
    ) or "none"
    child_summary = ", ".join(
        f"{{kind}}={{count}}" for kind, count in child_counts.items()
    ) or "none"
    mo.md(
        f"""
# Proof run `{{run["run_id"]}}`

- logical id: `{{run["logical_id"]}}`
- status: `{{run["status"]}}`, return code: `{{run["returncode"]}}`
- git: `{{head}}` (`{{dirty}}`)
- contention key: `{{run["contention_key"]}}`
- notes: {{note_summary}}
- parents: {{parent_summary}}
- children: {{child_summary}}
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
    row = conn.execute(
        "SELECT * FROM proof_runs WHERE run_id = ?", (run_id,)
    ).fetchone()
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


def _diagnostic(
    *,
    signal_id: str,
    severity: str,
    summary: str,
    evidence: str,
    next_action: str,
    scopes: Sequence[str] = (),
) -> dict[str, object]:
    return {
        "signal_id": signal_id,
        "severity": severity,
        "summary": summary,
        "evidence": _shorten(evidence, 320),
        "next_action": next_action,
        "scopes": list(scopes),
    }


def _run_diagnostics(row: sqlite3.Row) -> list[dict[str, object]]:
    log_tail = _read_log_tail(Path(row["log_path"]))
    diagnostics: list[dict[str, object]] = []
    if not log_tail and row["status"] not in {"passed", "queued", "running"}:
        return [
            _diagnostic(
                signal_id="proof-log-missing",
                severity="infra",
                summary="The proof row is terminal but its queue log is missing.",
                evidence=f"log_path={row['log_path']}",
                next_action=(
                    "Treat this as incomplete evidence; inspect the queue DB and "
                    "rerun through the same queue lane after preserving the row id."
                ),
            )
        ]

    if (
        "proof queue refuses raw `cargo` commands" in log_tail
        or "proof queue refuses `uv run` commands" in log_tail
    ):
        diagnostics.append(
            _diagnostic(
                signal_id="queue-policy-rejection",
                severity="operator",
                summary="The queue rejected a noncanonical command before proof execution.",
                evidence=_last_nonempty_log_line(Path(row["log_path"])) or "",
                next_action=(
                    "Resubmit through the queue-native cargo lane or the active "
                    "uv contract; this row is DX policy evidence, not product proof."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    match = STATIC_PYMOD_EXEC_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        detail = match.group("detail").strip(" ;")
        if detail:
            next_action = (
                "Fix the pending Python/C-API error surfaced by module exec, then "
                "rerun the same queue lane as a rerun edge."
            )
        else:
            next_action = (
                "Do not rerun the heavy lane until the module-exec primitive "
                "changes. Inspect the extension's Py_mod_exec body and route the "
                "missing C-API/ABI primitive through shared runtime authority."
            )
        diagnostics.append(
            _diagnostic(
                signal_id="static-pymodexec-nonzero",
                severity="error",
                summary=(
                    f"Static-linked extension module {module} reached Py_mod_exec "
                    "and returned non-zero."
                ),
                evidence=match.group(0),
                next_action=next_action,
                scopes=(
                    "runtime/molt-cpython-abi/",
                    "runtime/molt-runtime/src/cpython_abi_hooks.rs",
                    "src/molt/cli/external_native.py",
                ),
            )
        )

    match = UNDEFINED_SYMBOL_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
                signal_id="native-undefined-symbol",
                severity="error",
                summary=f"Native/WASM link failed on unresolved symbol {symbol}.",
                evidence=match.group(0),
                next_action=(
                    "Add the symbol to the shared ABI/object-closure authority or "
                    "make package admission fail closed before link; do not patch "
                    "a package-local shim."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/",
                    "src/molt/cli/external_native.py",
                    "tools/proof_queue.py",
                ),
            )
        )

    match = UNSUPPORTED_DIRECT_CALL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="unsupported-direct-call",
                severity="error",
                summary="The compiler reached an unsupported direct-call boundary.",
                evidence=match.group(0),
                next_action=(
                    "Move the callable into package/import/native symbol closure "
                    "authority or fail closed at admission with this exact callable."
                ),
                scopes=("src/molt/cli/", "runtime/molt-backend-wasm/src/"),
            )
        )

    if "candidate_outputs.npz" in log_tail and any(
        token in log_tail.lower() for token in ("not found", "no such file", "missing")
    ):
        diagnostics.append(
            _diagnostic(
                signal_id="pact-candidate-output-missing",
                severity="error",
                summary="Pact acceptance did not produce candidate_outputs.npz.",
                evidence="candidate_outputs.npz was referenced with a missing-file signal",
                next_action=(
                    "Treat this as failed acceptance, not parity evidence. Use the "
                    "named pact-witness-acceptance lane after the structural fix."
                ),
                scopes=("tools/pact_witness_acceptance.py", "collab/pact/"),
            )
        )

    if row["status"] == "failed" and not diagnostics:
        last = _last_nonempty_log_line(Path(row["log_path"])) or ""
        diagnostics.append(
            _diagnostic(
                signal_id="unclassified-failed-proof",
                severity="unknown",
                summary="The proof failed without a recognized queue diagnostic.",
                evidence=last,
                next_action=(
                    "Inspect the log tail once, then add a deterministic diagnosis "
                    "rule before this failure pattern becomes tribal knowledge."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )
    return diagnostics


def _format_diagnostic_summary(diagnostics: list[dict[str, object]]) -> str | None:
    if not diagnostics:
        return None
    first = diagnostics[0]
    return (
        f"{first['signal_id']} [{first['severity']}]: {_shorten(str(first['summary']))}"
    )


def _diagnosis_note_body(row: sqlite3.Row, diagnostics: list[dict[str, object]]) -> str:
    if diagnostics:
        first = diagnostics[0]
        return (
            f"diagnosis: {row['run_id']} {row['status']} rc={row['returncode']} "
            f"{first['signal_id']}: {first['summary']} next: {first['next_action']}"
        )
    return (
        f"diagnosis: {row['run_id']} {row['status']} rc={row['returncode']} "
        "has no queue diagnostic signals."
    )


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
    parents = _parent_statuses(conn, run_id)
    waiting = [row for row in parents if row["status"] in RUNNING]
    if waiting:
        return "waiting", waiting
    blockers = [row for row in parents if row["status"] != "passed"]
    if blockers:
        return "blocked", blockers
    return "ready", []


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
            json.dumps(
                git_snapshot if git_snapshot is not None else _git_snapshot(cwd),
                sort_keys=True,
            ),
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


def _required_rust_targets_for_resource(
    resource_family: str, *, repo_root: Path
) -> tuple[str, ...]:
    if resource_family in WASM_RESOURCE_FAMILIES:
        return wasm_toolchain.rust_toolchain_contract(repo_root).required_wasm_targets
    return ()


def _ensure_run_toolchain_preflight(
    *,
    repo_root: Path,
    resource_family: str,
) -> list[str] | None:
    warnings: list[str] = []
    try:
        required_targets = _required_rust_targets_for_resource(
            resource_family, repo_root=repo_root
        )
    except wasm_toolchain.RustToolchainContractError as exc:
        return [str(exc)]
    for target in required_targets:
        if not wasm_toolchain.ensure_rustup_target(target, warnings, root=repo_root):
            if not warnings:
                warnings.append(f"failed to ensure Rust target {target}")
            return warnings
    return None


def _write_failed_run_log(
    log_path: Path,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    repo_root: Path,
    command: list[str],
    lines: Sequence[str],
) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w", encoding="utf-8") as log:
        print(f"proof_queue run_id={run_id}", file=log)
        print(f"logical_id={logical_id}", file=log)
        print(f"reason={reason}", file=log)
        print(f"cwd={repo_root}", file=log)
        print(f"command={shlex.join(command)}", file=log)
        print("", file=log)
        for line in lines:
            print(line, file=log)


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
    if not command:
        return None
    basename = _command_basename(command[0])
    if basename in {"cargo", "cargo.exe"}:
        return (
            "proof queue refuses raw `cargo` commands; use "
            "`tools/proof_queue.py cargo ... -- <cargo-args>` so the queue owns "
            "the uv, guarded_exec, contention, timeout, and log envelope."
        )
    if len(command) < 2:
        return None
    if basename != "uv.exe" and basename != "uv":
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


def _cargo_package_for_contention(cargo_args: list[str]) -> str:
    for index, arg in enumerate(cargo_args):
        if arg in {"-p", "--package"} and index + 1 < len(cargo_args):
            return _slug(cargo_args[index + 1])
        if arg.startswith("--package="):
            return _slug(arg.split("=", 1)[1])
    return "workspace"


def _canonical_cargo_proof_command(cargo_args: list[str]) -> list[str]:
    args = list(cargo_args)
    if args[:1] == ["--"]:
        args = args[1:]
    if args and _command_basename(args[0]) in {"cargo", "cargo.exe"}:
        args = args[1:]
    if not args:
        raise SystemExit("cargo proof command is empty")
    return _uv_active_python_command(
        "tools/guarded_exec.py",
        "--prefix",
        "MOLT_TEST_SUITE",
        "--",
        "cargo",
        *args,
    )


def _first_existing_manifest_root(
    repo_root: Path, candidates: list[str]
) -> Path | None:
    for candidate in candidates:
        root = repo_root / candidate
        if (root / "extension_manifest.json").is_file() or any(
            root.glob("**/*.extension_manifest.json")
        ):
            return root
    return None


def _pact_witness_native_roots(repo_root: Path = ROOT) -> list[Path]:
    repo_root = Path(repo_root)
    selected: list[Path] = []
    artifact_groups = [
        [
            "tmp/pact_numpy_multiarray_sealed_for_witness",
            "tmp/pact_numpy_multiarray_sealed_axiserror",
            "tmp/worktrees/pact-collab/tmp/pact_numpy_multiarray_molt_ext_wasm_cpython_abi",
        ],
        [
            "tmp/pact_scipy_ndimage_sealed_for_witness_next",
            "tmp/pact_scipy_ndimage_sealed_for_witness",
            "tmp/pact_scipy_ndimage_provider_sealed_support_closure",
            "tmp/pact_scipy_ndimage_provider_sealed_helpers",
            "tmp/pact_scipy_ndimage_provider_sealed",
        ],
    ]
    artifact_roots = [
        _first_existing_manifest_root(repo_root, candidates)
        for candidates in artifact_groups
    ]
    artifact_roots.extend(
        root
        for root in [
            _first_existing_manifest_root(
                repo_root,
                ["tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi"],
            ),
            _first_existing_manifest_root(
                repo_root,
                ["tmp/pact_scipy_rank_filter_1d_molt_ext_wasm_cpython_abi"],
            ),
        ]
        if root is not None
    )
    source_roots = [
        repo_root / "bench/friends/repos/numpy_off_the_shelf",
        repo_root / "bench/friends/repos/scipy_off_the_shelf",
    ]
    for root in [*artifact_roots, *source_roots]:
        if root is None or not root.exists():
            continue
        resolved = root.resolve()
        if resolved not in selected:
            selected.append(resolved)
    return selected


def _pact_witness_env_overrides(repo_root: Path = ROOT) -> dict[str, str]:
    roots = _pact_witness_native_roots(repo_root)
    if not roots:
        return {}
    return {
        "MOLT_MODULE_ROOTS": os.pathsep.join(str(root) for root in roots),
        "MOLT_EXTERNAL_STATIC_PACKAGES": "numpy scipy",
    }


def _pact_witness_acceptance_spec(
    timeout: float | None = None, repo_root: Path = ROOT
) -> dict[str, object]:
    return {
        "logical_id": "pact-witness-acceptance",
        "reason": (
            "Run the Pact Kernel A browser/WASM witness acceptance aperture "
            "through queue custody."
        ),
        "command": _uv_active_python_command(
            "tools/pact_witness_acceptance.py",
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
            "tools/pact_witness_acceptance.py",
            "tmp/pact_numpy_multiarray_sealed_axiserror",
            "tmp/pact_numpy_multiarray_sealed_for_witness",
            "tmp/pact_scipy_ndimage_provider_sealed_support_closure",
            "tmp/pact_scipy_ndimage_sealed_for_witness_next",
            "tmp/pact_scipy_ndimage_sealed_for_witness",
            "tmp/pact_scipy_ndimage_provider_sealed_helpers",
            "tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi",
        ],
        "env_overrides": _pact_witness_env_overrides(repo_root),
        "notes": [
            "Named Pact acceptance auto-admits conventional manifest-led "
            "NumPy/SciPy staging roots when present, builds field_solve.py, "
            "runs the WASM artifact to produce candidate_outputs.npz, and "
            "executes check_parity.py; --env can override for power-user lanes."
        ],
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
    if getattr(args, "detach", False):
        rc, run_id = _queue_one(
            args,
            logical_id=str(runnable["logical_id"]),
            reason=str(runnable["reason"]),
            command=list(runnable["command"]),
            resource_family=str(runnable["resource_family"]),
            contention_key=str(runnable["contention_key"]),
            scopes=list(runnable["scopes"]),
            env_overrides=dict(runnable["env_overrides"]),
            initial_notes=initial_notes,
            depends_on=getattr(args, "depends_on", []) or [],
            edge_kind=getattr(args, "edge_kind", DEFAULT_EDGE_KIND),
            edge_note=getattr(args, "edge_note", None),
        )
        if rc != 0 or run_id is None:
            return rc
        pid, runner_log = _launch_detached_runner(
            args,
            run_id=run_id,
            timeout=float(runnable["timeout"]),
        )
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
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
        depends_on=getattr(args, "depends_on", []) or [],
        edge_kind=getattr(args, "edge_kind", DEFAULT_EDGE_KIND),
        edge_note=getattr(args, "edge_note", None),
    )


def _cmd_pact_witness_acceptance(args: argparse.Namespace) -> int:
    return _run_named_spec(
        args, _pact_witness_acceptance_spec(args.timeout, _repo_root(args))
    )


def _cmd_pact_witness_oracle(args: argparse.Namespace) -> int:
    return _run_named_spec(args, _pact_witness_oracle_spec(args.timeout))


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
    edge_kind: str = DEFAULT_EDGE_KIND,
    edge_note: str | None = None,
) -> tuple[int, str | None]:
    if not command:
        raise SystemExit("proof command is empty")
    db = _db_path(args)
    logs_root = _logs_root(args)
    repo_root = _repo_root(args)
    conn = _connect(db)
    for parent_run_id in depends_on or []:
        if not _run_exists(conn, parent_run_id):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    if edge_kind not in EDGE_KINDS:
        allowed = ", ".join(sorted(EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    policy_error = _proof_command_policy_error(command)
    if policy_error is not None:
        print(policy_error, file=sys.stderr)
        return 2, None
    active = list(_active_for_key(conn, contention_key))
    if active:
        print(
            f"contention key {contention_key!r} already has active run(s):",
            file=sys.stderr,
        )
        for row in active:
            print(f"- {row['status']} {row['run_id']} {row['reason']}", file=sys.stderr)
        return 2, None
    run_id = f"{_compact_utc()}-{_slug(logical_id)}-{uuid.uuid4().hex[:16]}"
    logs_root.mkdir(parents=True, exist_ok=True)
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
        log_path=logs_root / f"{run_id}.log",
        summary_json=logs_root / f"{run_id}.memory_guard.json",
    )
    for parent_run_id in depends_on or []:
        _insert_edge(
            conn,
            parent_run_id=parent_run_id,
            child_run_id=run_id,
            kind=edge_kind,
            note=edge_note,
        )
    for note in initial_notes or []:
        _insert_note(conn, run_id=run_id, body=note, kind=SUBMISSION_NOTE_KIND)
    if initial_notes or depends_on:
        _write_marimo_notebook(args, conn, run_id)
    print(f"queued {run_id}")
    return 0, run_id


def _global_arg_pairs(args: argparse.Namespace) -> list[str]:
    pairs: list[str] = []
    for attr, option in (
        ("db", "--db"),
        ("logs_root", "--logs-root"),
        ("notebooks_root", "--notebooks-root"),
        ("repo_root", "--repo-root"),
    ):
        value = getattr(args, attr, None)
        if value:
            pairs.extend([option, str(value)])
    return pairs


def _launch_detached_runner(
    args: argparse.Namespace, *, run_id: str, timeout: float
) -> tuple[int, Path]:
    logs_root = _logs_root(args)
    logs_root.mkdir(parents=True, exist_ok=True)
    runner_log = logs_root / f"{run_id}.runner.log"
    command = [
        sys.executable,
        str(Path(__file__).resolve()),
        *_global_arg_pairs(args),
        "run",
        "--run-id",
        run_id,
        "--limit",
        "1",
        "--timeout",
        str(timeout),
    ]
    popen_kwargs: dict[str, object] = {
        "cwd": _repo_root(args),
        "stdin": subprocess.DEVNULL,
        "text": True,
    }
    if os.name == "nt":
        flags = 0
        flags |= getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        flags |= getattr(subprocess, "CREATE_NO_WINDOW", 0)
        popen_kwargs["creationflags"] = flags
    else:
        popen_kwargs["start_new_session"] = True
    with runner_log.open("w", encoding="utf-8") as log:
        print(f"proof_queue detached runner for {run_id}", file=log, flush=True)
        print(f"command={shlex.join(command)}", file=log, flush=True)
        proc = subprocess.Popen(
            command,
            stdout=log,
            stderr=subprocess.STDOUT,
            **popen_kwargs,
        )
    return proc.pid, runner_log


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
    for parent_run_id in depends_on or []:
        if not _run_exists(conn, parent_run_id):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    if edge_kind not in EDGE_KINDS:
        allowed = ", ".join(sorted(EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    active = []
    for row in _active_for_key(conn, contention_key):
        if existing_run_id is not None and row["run_id"] == existing_run_id:
            continue
        if existing_run_id is not None and row["status"] == "queued":
            continue
        active.append(row)
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
        for parent_run_id in depends_on or []:
            _insert_edge(
                conn,
                parent_run_id=parent_run_id,
                child_run_id=run_id,
                kind=edge_kind,
                note=edge_note,
            )
        for note in initial_notes or []:
            _insert_note(conn, run_id=run_id, body=note, kind=SUBMISSION_NOTE_KIND)
        if initial_notes or depends_on:
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
        _write_failed_run_log(
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
        if _notes_for_run_ids(conn, [run_id]).get(run_id):
            _write_marimo_notebook(args, conn, run_id)
        return 2
    preflight_errors = _ensure_run_toolchain_preflight(
        repo_root=repo_root,
        resource_family=resource_family,
    )
    if preflight_errors is not None:
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
        lines = ["proof queue toolchain preflight failed:", *preflight_errors]
        _write_failed_run_log(
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
    contention_key = args.contention_key or f"{args.resource_family}:default"
    if args.detach:
        rc, run_id = _queue_one(
            args,
            logical_id=args.id,
            reason=args.reason,
            command=command,
            resource_family=args.resource_family,
            contention_key=contention_key,
            scopes=args.scope,
            env_overrides=env_overrides,
            initial_notes=initial_notes,
            depends_on=args.depends_on,
            edge_kind=args.edge_kind,
            edge_note=args.edge_note,
        )
        if rc != 0 or run_id is None:
            return rc
        pid, runner_log = _launch_detached_runner(
            args,
            run_id=run_id,
            timeout=args.timeout,
        )
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return _run_one(
        args,
        logical_id=args.id,
        reason=args.reason,
        command=command,
        resource_family=args.resource_family,
        contention_key=contention_key,
        scopes=args.scope,
        env_overrides=env_overrides,
        timeout=args.timeout,
        initial_notes=initial_notes,
        depends_on=args.depends_on,
        edge_kind=args.edge_kind,
        edge_note=args.edge_note,
    )


def _cmd_cargo(args: argparse.Namespace) -> int:
    cargo_args = (
        args.cargo_args[1:] if args.cargo_args[:1] == ["--"] else args.cargo_args
    )
    contention_key = args.contention_key or (
        f"cargo:{_cargo_package_for_contention(cargo_args)}"
    )
    command = _canonical_cargo_proof_command(cargo_args)
    env_overrides = _env_overrides_from_pairs(args.env)
    initial_notes = getattr(args, "note", []) or []
    if args.detach:
        rc, run_id = _queue_one(
            args,
            logical_id=args.id,
            reason=args.reason,
            command=command,
            resource_family="rust",
            contention_key=contention_key,
            scopes=args.scope,
            env_overrides=env_overrides,
            initial_notes=initial_notes,
            depends_on=args.depends_on,
            edge_kind=args.edge_kind,
            edge_note=args.edge_note,
        )
        if rc != 0 or run_id is None:
            return rc
        pid, runner_log = _launch_detached_runner(
            args,
            run_id=run_id,
            timeout=args.timeout,
        )
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return _run_one(
        args,
        logical_id=args.id,
        reason=args.reason,
        command=command,
        resource_family="rust",
        contention_key=contention_key,
        scopes=args.scope,
        env_overrides=env_overrides,
        timeout=args.timeout,
        initial_notes=initial_notes,
        depends_on=args.depends_on,
        edge_kind=args.edge_kind,
        edge_note=args.edge_note,
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
    prepared: list[dict[str, object]] = []
    logical_to_run: dict[str, str] = {}
    for spec in specs:
        logical_id = str(spec.get("id") or spec.get("logical_id") or "proof")
        if logical_id in logical_to_run:
            raise SystemExit(f"duplicate proof logical id {logical_id!r}")
        command = spec.get("command")
        if not isinstance(command, list) or not all(
            isinstance(x, str) for x in command
        ):
            raise SystemExit(f"proof {logical_id!r} needs command = [..]")
        policy_error = _proof_command_policy_error(list(command))
        if policy_error is not None:
            raise SystemExit(f"proof {logical_id!r}: {policy_error}")
        edge_kind = str(spec.get("edge_kind") or DEFAULT_EDGE_KIND)
        if edge_kind not in EDGE_KINDS:
            allowed = ", ".join(sorted(EDGE_KINDS))
            raise SystemExit(
                f"proof {logical_id!r}: unknown proof edge kind "
                f"{edge_kind!r}; allowed: {allowed}"
            )
        edge_note_raw = spec.get("edge_note")
        if edge_note_raw is not None and not isinstance(edge_note_raw, str):
            raise SystemExit(f"proof {logical_id!r}: edge_note must be a string")
        env_overrides = _env_overrides_from_spec(spec.get("env"))
        initial_notes = _notes_from_raw(spec.get("note"))
        initial_notes.extend(_notes_from_raw(spec.get("notes")))
        depends_on = _dependencies_from_raw(spec.get("depends_on"))
        depends_on.extend(_dependencies_from_raw(spec.get("after")))
        run_id = f"{_compact_utc()}-{_slug(logical_id)}-{uuid.uuid4().hex[:16]}"
        logical_to_run[logical_id] = run_id
        prepared.append(
            {
                "logical_id": logical_id,
                "command": list(command),
                "reason": str(spec.get("reason") or logical_id),
                "resource_family": str(spec.get("resource_family") or "generic"),
                "contention_key": str(spec.get("contention_key") or "generic:default"),
                "scope": [str(x) for x in spec.get("scope", [])],
                "env_overrides": env_overrides,
                "initial_notes": initial_notes,
                "depends_on": depends_on,
                "edge_kind": edge_kind,
                "edge_note": edge_note_raw or "",
                "run_id": run_id,
            }
        )
    planned_edges: set[tuple[str, str, str]] = set()
    planned_children: dict[str, list[str]] = {}
    for item in prepared:
        child = str(item["run_id"])
        for dependency in item["depends_on"]:
            parent = logical_to_run.get(str(dependency), str(dependency))
            if parent == child:
                raise SystemExit(f"proof {item['logical_id']!r}: depends_on itself")
            if parent not in logical_to_run.values() and not _run_exists(conn, parent):
                raise SystemExit(
                    f"proof {item['logical_id']!r}: unknown dependency {dependency!r}"
                )
            edge = (parent, child, str(item["edge_kind"]))
            if edge in planned_edges:
                raise SystemExit(
                    f"proof {item['logical_id']!r}: duplicate dependency {dependency!r}"
                )
            planned_edges.add(edge)
            planned_children.setdefault(parent, []).append(child)
    for parent, child, _kind in planned_edges:
        if _planned_edge_would_create_cycle(planned_children, parent, child):
            raise SystemExit(
                "proof DSL dependency graph would create a cycle: "
                f"{parent!r} -> {child!r}"
            )
    for item in prepared:
        run_id = str(item["run_id"])
        log_path = _logs_root(args) / f"{run_id}.log"
        summary_json = _logs_root(args) / f"{run_id}.memory_guard.json"
        _insert_run(
            conn,
            run_id=run_id,
            logical_id=str(item["logical_id"]),
            reason=str(item["reason"]),
            command=list(item["command"]),
            cwd=_repo_root(args),
            resource_family=str(item["resource_family"]),
            contention_key=str(item["contention_key"]),
            scopes=list(item["scope"]),
            env_overrides=dict(item["env_overrides"]),
            log_path=log_path,
            summary_json=summary_json,
        )
    for item in prepared:
        run_id = str(item["run_id"])
        for dependency in item["depends_on"]:
            _insert_edge(
                conn,
                parent_run_id=logical_to_run.get(str(dependency), str(dependency)),
                child_run_id=run_id,
                kind=str(item["edge_kind"]),
                note=str(item["edge_note"]),
            )
        for note in item["initial_notes"]:
            _insert_note(conn, run_id=run_id, body=note, kind=SUBMISSION_NOTE_KIND)
        if item["initial_notes"] or item["depends_on"]:
            _write_marimo_notebook(args, conn, run_id)
        print(f"queued {run_id}")
    return 0


def _cmd_run(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    conn.row_factory = sqlite3.Row
    if args.run_id:
        selected = conn.execute(
            "SELECT * FROM proof_runs WHERE run_id = ?",
            (args.run_id,),
        ).fetchone()
        if selected is None:
            raise SystemExit(f"unknown proof run {args.run_id!r}")
        if selected["status"] != "queued":
            raise SystemExit(
                f"proof run {args.run_id!r} is {selected['status']}, not queued"
            )
        queued = [selected]
    else:
        queued = list(
            conn.execute(
                "SELECT * FROM proof_runs WHERE status = 'queued' ORDER BY rowid"
            )
        )
    rows = []
    for row in queued:
        state, blockers = _dependency_state(conn, row["run_id"])
        if state == "ready":
            rows.append(row)
            if args.run_id or len(rows) >= args.limit:
                break
            continue
        blocker_summary = ", ".join(
            f"{blocker['parent_run_id']}:{blocker['status']}" for blocker in blockers
        )
        if state == "waiting":
            print(f"waiting {row['run_id']} parents={blocker_summary}")
            continue
        _update_run(
            conn,
            row["run_id"],
            status="blocked",
            finished_at=_utc_now(),
        )
        print(f"blocked {row['run_id']} parents={blocker_summary}")
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
    if not queued:
        print("no queued proofs")
    elif not rows:
        print("no queued proofs ready")
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
    edges_by_run = _edges_for_run_ids(
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
        dag_summary = _format_dag_summary(
            edges_by_run.get(row["run_id"], {"parents": [], "children": []})
        )
        if dag_summary:
            print(dag_summary)
        diagnostic_summary = _format_diagnostic_summary(_run_diagnostics(row))
        if diagnostic_summary:
            print(f"  diagnosis={diagnostic_summary}")
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
        dag_summary = _format_dag_summary(
            edges_by_run.get(row["run_id"], {"parents": [], "children": []})
        )
        if dag_summary:
            print(dag_summary)
        diagnostic_summary = _format_diagnostic_summary(_run_diagnostics(row))
        if diagnostic_summary:
            print(f"  diagnosis={diagnostic_summary}")
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


def _cmd_diagnose(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    row = _diagnose_row(conn, args)
    diagnostics = _run_diagnostics(row)
    payload = _row_to_payload(row)
    payload["diagnostics"] = diagnostics
    if args.output:
        Path(args.output).write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        rc = "?" if row["returncode"] is None else row["returncode"]
        print(
            f"diagnosis {row['run_id']} status={row['status']} rc={rc} "
            f"log={row['log_path']}"
        )
        if not diagnostics:
            print("- no diagnostic signals")
        for item in diagnostics:
            print(f"- {item['signal_id']} [{item['severity']}] {item['summary']}")
            if item["evidence"]:
                print(f"  evidence: {item['evidence']}")
            print(f"  next: {item['next_action']}")
    if args.append_note:
        note_id = _insert_note(
            conn,
            run_id=row["run_id"],
            body=_diagnosis_note_body(row, diagnostics),
            kind=args.kind,
            author=args.author,
        )
        print(f"noted {row['run_id']} note_id={note_id}")
        if not args.no_notebook:
            path = _write_marimo_notebook(args, conn, row["run_id"])
            print(f"notebook: {path}")
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
    print(
        f"noted {args.run_id} note_ids={','.join(str(note_id) for note_id in note_ids)}"
    )
    if notebook_path is not None:
        print(f"notebook: {notebook_path}")
    return 0


def _cmd_link(args: argparse.Namespace) -> int:
    conn = _connect(_db_path(args))
    edge_id = _insert_edge(
        conn,
        parent_run_id=args.parent,
        child_run_id=args.child_run_id,
        kind=args.kind,
        note=args.note,
        author=args.author,
    )
    notebook_paths = []
    if not args.no_notebook:
        notebook_paths.append(_write_marimo_notebook(args, conn, args.parent))
        notebook_paths.append(_write_marimo_notebook(args, conn, args.child_run_id))
    print(
        f"linked {args.parent} -> {args.child_run_id} "
        f"kind={args.kind} edge_id={edge_id}"
    )
    for path in notebook_paths:
        print(f"notebook: {path}")
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
        "uv run --active --project . --python 3.12 python tools/proof_queue.py cargo "
        '--id focused-cargo-proof --reason "why this proves the Rust contract" '
        '--scope runtime/molt-runtime/src/cpython_abi_hooks.rs --note "change: moved the Rust authority; test: proving the focused invariant" --timeout 900 -- '
        "test -p molt-runtime exact_test_name --lib\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py exec "
        '--id focused-proof --reason "why this proves the changed contract" '
        '--resource-family python --contention-key python:focused --note "change: moved the shared authority; test: proving the focused invariant" --timeout 240 -- '
        "uv run --active --project . --python 3.12 pytest tests/path.py -q"
        "\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py note "
        '<run-id> --kind observation --note "what happened, what it means, and the next bounded action"'
        "\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py diagnose "
        "<run-id> --append-note"
        "\n"
        "uv run --active --project . --python 3.12 python tools/proof_queue.py link "
        '<child-run-id> --parent <parent-run-id> --kind derives_from --note "why this edge exists"'
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
        'depends_on = ["previous-run-id-or-logical-id"]\n'
        'note = "change: moved runtime feature authority into the generator-backed path"\n'
        'notes = ["test: targeted pytest proves the generated selector contract"]\n'
        'edge_kind = "derives_from"\n'
        'edge_note = "Narrows the previous failing proof to the generated selector contract."\n'
        'env = { MOLT_EXTERNAL_STATIC_PACKAGES = "numpy scipy" }\n'
        'command = ["uv", "run", "--active", "--project", ".", "--python", "3.12", "pytest", "tests/path.py", "-q"]\n'
    )
    return 0


def _cmd_cargo_template(args: argparse.Namespace) -> int:
    del args
    print(
        "uv run --active --project . --python 3.12 python tools/proof_queue.py cargo \\\n"
        "  --id runtime-focused-proof \\\n"
        '  --reason "Prove the changed Rust runtime contract." \\\n'
        "  --scope runtime/molt-runtime/src/cpython_abi_hooks.rs \\\n"
        '  --note "change: moved static-link Py_mod_exec diagnostics into the C-API authority" \\\n'
        "  --timeout 900 \\\n"
        "  --detach \\\n"
        "  -- test -p molt-runtime exact_test_name --lib"
    )
    return 0


def _add_dependency_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--depends-on",
        action="append",
        default=[],
        metavar="RUN_ID",
        help="append a proof DAG parent; the run waits until parents pass",
    )
    parser.add_argument(
        "--edge-kind",
        default=DEFAULT_EDGE_KIND,
        choices=sorted(EDGE_KINDS),
        help="canonical relationship kind for --depends-on edges",
    )
    parser.add_argument(
        "--edge-note",
        help="immutable note attached to each --depends-on edge",
    )


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
    exec_p.add_argument(
        "--note",
        action="append",
        default=[],
        help=(
            "append a submission note describing what changed, what is being "
            "tested or explored, and why"
        ),
    )
    _add_dependency_args(exec_p)
    exec_p.add_argument("--timeout", type=float, default=1200.0)
    exec_p.add_argument("--detach", action="store_true")
    exec_p.add_argument("--wait", action="store_true")
    exec_p.add_argument("--wait-timeout", type=float)
    exec_p.add_argument("command", nargs=argparse.REMAINDER)
    exec_p.set_defaults(func=_cmd_exec)

    cargo_p = sub.add_parser(
        "cargo",
        help="submit a queue-owned Cargo proof with canonical uv and guard wrapping",
    )
    cargo_p.add_argument("--id", required=True)
    cargo_p.add_argument("--reason", required=True)
    cargo_p.add_argument("--contention-key")
    cargo_p.add_argument("--scope", action="append", default=[])
    cargo_p.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    cargo_p.add_argument(
        "--note",
        action="append",
        default=[],
        help=(
            "append a submission note describing what changed, what is being "
            "tested or explored, and why"
        ),
    )
    _add_dependency_args(cargo_p)
    cargo_p.add_argument("--timeout", type=float, default=1200.0)
    cargo_p.add_argument("--detach", action="store_true")
    cargo_p.add_argument("cargo_args", nargs=argparse.REMAINDER)
    cargo_p.set_defaults(func=_cmd_cargo)

    submit_p = sub.add_parser("submit", help="submit proof specs from a TOML DSL")
    submit_p.add_argument("dsl")
    submit_p.set_defaults(func=_cmd_submit)

    run_p = sub.add_parser("run", help="run queued proof specs")
    run_p.add_argument("--limit", type=int, default=1)
    run_p.add_argument("--run-id")
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

    diagnose_p = sub.add_parser(
        "diagnose",
        help="classify a proof run failure from recorded queue facts and log tail",
    )
    diagnose_p.add_argument("run_id", nargs="?")
    diagnose_p.add_argument(
        "--logical-id",
        help="diagnose the latest run with this logical id when run_id is omitted",
    )
    diagnose_p.add_argument("--json", action="store_true")
    diagnose_p.add_argument("--output")
    diagnose_p.add_argument(
        "--append-note",
        action="store_true",
        help="append the deterministic diagnosis as an immutable proof note",
    )
    diagnose_p.add_argument(
        "--kind",
        default="finding",
        choices=sorted(NOTE_KINDS),
        help="note kind used with --append-note",
    )
    diagnose_p.add_argument("--author")
    diagnose_p.add_argument("--no-notebook", action="store_true")
    diagnose_p.set_defaults(func=_cmd_diagnose)

    note_p = sub.add_parser("note", help="append an immutable note to a proof run")
    note_p.add_argument("run_id")
    note_p.add_argument("--note", action="append", required=True)
    note_p.add_argument(
        "--kind",
        default=DEFAULT_NOTE_KIND,
        choices=sorted(NOTE_KINDS),
        help="canonical note lane for append-only collaboration",
    )
    note_p.add_argument("--author")
    note_p.add_argument("--output")
    note_p.add_argument("--no-notebook", action="store_true")
    note_p.set_defaults(func=_cmd_note)

    link_p = sub.add_parser(
        "link", help="append an immutable proof DAG edge between existing runs"
    )
    link_p.add_argument("child_run_id")
    link_p.add_argument("--parent", required=True)
    link_p.add_argument(
        "--kind",
        default=DEFAULT_EDGE_KIND,
        choices=sorted(EDGE_KINDS),
        help="canonical proof DAG edge kind",
    )
    link_p.add_argument("--note")
    link_p.add_argument("--author")
    link_p.add_argument("--no-notebook", action="store_true")
    link_p.set_defaults(func=_cmd_link)

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

    cargo_template_p = sub.add_parser(
        "cargo-template", help="print the canonical Cargo proof command shape"
    )
    cargo_template_p.set_defaults(func=_cmd_cargo_template)

    pact_accept_p = sub.add_parser(
        "pact-witness-acceptance",
        help="run the queue-owned Pact Kernel A browser/WASM acceptance aperture",
    )
    pact_accept_p.add_argument(
        "--env", action="append", default=[], metavar="NAME=VALUE"
    )
    pact_accept_p.add_argument(
        "--note",
        action="append",
        default=[],
        help="append submission context to the acceptance run",
    )
    _add_dependency_args(pact_accept_p)
    pact_accept_p.add_argument("--timeout", type=float)
    pact_accept_p.add_argument("--detach", action="store_true")
    pact_accept_p.add_argument("--print-spec", action="store_true")
    pact_accept_p.set_defaults(func=_cmd_pact_witness_acceptance)

    pact_oracle_p = sub.add_parser(
        "pact-witness-oracle",
        help="run the queued Pact Kernel A fixture/reference parity oracle",
    )
    pact_oracle_p.add_argument(
        "--env", action="append", default=[], metavar="NAME=VALUE"
    )
    pact_oracle_p.add_argument(
        "--note",
        action="append",
        default=[],
        help="append submission context to the oracle run",
    )
    _add_dependency_args(pact_oracle_p)
    pact_oracle_p.add_argument("--timeout", type=float)
    pact_oracle_p.add_argument("--detach", action="store_true")
    pact_oracle_p.add_argument("--print-spec", action="store_true")
    pact_oracle_p.set_defaults(func=_cmd_pact_witness_oracle)
    return parser


def main(argv: list[str] | None = None) -> int:
    raw = list(sys.argv[1:] if argv is None else argv)
    if raw and raw[0] in {"exec", "cargo"}:
        before, command = _command_after_dash(raw)
        parser = _build_parser()
        args = parser.parse_args(before)
        if raw[0] == "exec":
            args.command = command
        else:
            args.cargo_args = command
    else:
        parser = _build_parser()
        args = parser.parse_args(raw)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
