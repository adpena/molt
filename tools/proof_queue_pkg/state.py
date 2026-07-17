"""Canonical SQLite schema, row serialization, paths, notes, and DAG state."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import sqlite3
import subprocess
import time
from pathlib import Path
from typing import Sequence

from molt.dx import checkout_custody
from tools.dirty_tree_policy import (
    DEFAULT_DIRTY_TREE_IGNORE_GLOBS,
    filter_status_lines,
)

ROOT = Path(__file__).resolve().parents[2]


RUNNING = {"queued", "dispatched", "running"}

LAUNCHED = {"dispatched", "running"}

ACTIVE_SQL_STATUSES = "'queued', 'dispatched', 'running'"

LAUNCHED_SQL_STATUSES = "'dispatched', 'running'"

ACTIVE_OR_STALE_SQL_STATUSES = "'queued', 'dispatched', 'running', 'stale'"

DETACHED_READY_STATUSES = {"queued", "dispatched"}

PROOF_QUEUE_SIZE_ENV = "MOLT_PROOF_QUEUE_SIZE"

DEFAULT_PROOF_QUEUE_SIZE = 1

WASM_RESOURCE_FAMILIES = frozenset({"wasm", "wasm-browser"})

COMPILER_BUILD_RESOURCE_FAMILIES = WASM_RESOURCE_FAMILIES | frozenset(
    {"native-build", "queue-native-rust", "rust"}
)

COMPILER_BUILD_RESOURCE_MUTEX_KEY = "compiler-build-resource"

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


def _shorten(text: str, limit: int = 180) -> str:
    collapsed = " ".join(text.strip().split())
    if len(collapsed) <= limit:
        return collapsed
    return collapsed[: max(0, limit - 3)] + "..."


def _resource_mutex_key(
    *, resource_family: str, contention_key: str, command: Sequence[str]
) -> str | None:
    del command
    family = resource_family.strip().lower()
    key = contention_key.strip().lower()
    if family in COMPILER_BUILD_RESOURCE_FAMILIES:
        return COMPILER_BUILD_RESOURCE_MUTEX_KEY
    if key.startswith(("cargo:", "rust:", "wasm:", "wasm-browser:")):
        return COMPILER_BUILD_RESOURCE_MUTEX_KEY
    if key in {"wasm-build", "native-build", "queue-native-rust"}:
        return COMPILER_BUILD_RESOURCE_MUTEX_KEY
    return None


def _edge_kind_requires_local_parent(kind: str) -> bool:
    return kind in _SCHEDULING_EDGE_KINDS

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

_SCHEDULING_EDGE_KINDS = frozenset({"depends_on"})

# SQLite busy timeout (milliseconds) for every proof-queue connection. WAL
# serializes writers; under concurrent detached runners a contended write can
# exceed the 5s default, so the terminal status write raised "database is
# locked" and stranded the row 'running'. 30s covers realistic writer waits.
PROOF_QUEUE_SQLITE_BUSY_TIMEOUT_MS = 30_000

# Bounded best-effort retry budget for a terminal/critical status write that
# loses the busy-timeout race and still sees "database is locked".
PROOF_QUEUE_LOCKED_WRITE_RETRIES = 8

PROOF_QUEUE_LOCKED_WRITE_RETRY_SLEEP_SECONDS = 0.25



def _utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat()



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



def _positive_int(value: object, *, source: str) -> int:
    try:
        result = int(str(value).strip())
    except (TypeError, ValueError) as exc:
        raise SystemExit(f"{source} must be a positive integer, got {value!r}") from exc
    if result < 1:
        raise SystemExit(f"{source} must be a positive integer, got {value!r}")
    return result



def _configured_queue_size(value: int | None = None) -> int:
    if value is not None:
        return _positive_int(value, source="--queue-size")
    raw = os.environ.get(PROOF_QUEUE_SIZE_ENV)
    if raw is None or not raw.strip():
        return DEFAULT_PROOF_QUEUE_SIZE
    return _positive_int(raw, source=PROOF_QUEUE_SIZE_ENV)



def _configured_run_limit(args: argparse.Namespace, *, queue_size: int) -> int:
    raw_limit = getattr(args, "limit", None)
    if raw_limit is not None:
        return _positive_int(raw_limit, source="--limit")
    if getattr(args, "detach", False):
        return queue_size
    return 1



def _connect(db: Path) -> sqlite3.Connection:
    db.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA foreign_keys=ON")
    # WAL serializes writers, so a slow write (e.g. the OneDrive-backed
    # checkout under concurrent detached runners) can hold the write lock for
    # seconds. Without a busy timeout SQLite returns SQLITE_BUSY immediately
    # ("database is locked"), which strands the terminal status write and loses
    # the run result. Wait up to 30s (in milliseconds per PRAGMA busy_timeout)
    # for a contended writer before surfacing the error.
    conn.execute(f"PRAGMA busy_timeout={PROOF_QUEUE_SQLITE_BUSY_TIMEOUT_MS}")
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
            resource_mutex_key TEXT,
            scopes_json TEXT NOT NULL,
            env_json TEXT NOT NULL DEFAULT '{}',
            git_json TEXT NOT NULL DEFAULT '{}',
            log_path TEXT NOT NULL,
            summary_json TEXT NOT NULL,
            guard_pid INTEGER,
            guard_identity TEXT,
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
            FOREIGN KEY(child_run_id) REFERENCES proof_runs(run_id),
            UNIQUE(parent_run_id, child_run_id, kind)
        )
        """
    )
    _migrate_proof_run_edges_for_external_lineage(conn)
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
    if "guard_identity" not in columns:
        conn.execute("ALTER TABLE proof_runs ADD COLUMN guard_identity TEXT")
    if "resource_mutex_key" not in columns:
        conn.execute("ALTER TABLE proof_runs ADD COLUMN resource_mutex_key TEXT")
    claimed_active_mutexes: set[str] = set()
    for row in conn.execute(
        """
        SELECT run_id, resource_family, contention_key, command_json, status
        FROM proof_runs
        WHERE resource_mutex_key IS NULL
        ORDER BY rowid
        """
    ):
        try:
            command = json.loads(row[3])
        except (TypeError, json.JSONDecodeError):
            command = []
        if not isinstance(command, list):
            command = []
        resource_mutex_key = _resource_mutex_key(
            resource_family=str(row[1]),
            contention_key=str(row[2]),
            command=[str(part) for part in command],
        )
        if resource_mutex_key is not None:
            if row[4] in LAUNCHED and resource_mutex_key in claimed_active_mutexes:
                continue
            if row[4] in LAUNCHED:
                claimed_active_mutexes.add(resource_mutex_key)
            conn.execute(
                "UPDATE proof_runs SET resource_mutex_key = ? WHERE run_id = ?",
                (resource_mutex_key, row[0]),
            )
    # At most one active launched run per contention key. A partial UNIQUE index makes
    # SQLite itself enforce the hard serialization invariant, closing the
    # check-then-insert / transition TOCTOU where two concurrent admissions each
    # see zero running rows and both reach status='running' (two heavy builds
    # contending for the resource the key exists to serialize). Multiple QUEUED
    # rows per key remain legal: dependency chains (a parent proof plus a
    # dependent child on the same resource) submit several queued rows and the
    # DAG plus the application-level active gate serialize which one runs.
    # Callers handle the resulting IntegrityError as "already active".
    conn.execute(
        """
        CREATE UNIQUE INDEX IF NOT EXISTS proof_runs_one_running_per_contention_key
        ON proof_runs(contention_key)
        WHERE status = 'running'
        """
    )
    conn.execute(
        """
        CREATE UNIQUE INDEX IF NOT EXISTS proof_runs_one_launched_per_contention_key
        ON proof_runs(contention_key)
        WHERE status IN ('dispatched', 'running')
        """
    )
    conn.execute(
        """
        CREATE UNIQUE INDEX IF NOT EXISTS proof_runs_one_launched_per_resource_mutex
        ON proof_runs(resource_mutex_key)
        WHERE resource_mutex_key IS NOT NULL
          AND status IN ('dispatched', 'running')
        """
    )
    conn.commit()
    return conn



def _proof_run_edges_has_parent_fk(conn: sqlite3.Connection) -> bool:
    return any(
        row[3] == "parent_run_id"
        for row in conn.execute("PRAGMA foreign_key_list(proof_run_edges)")
    )



def _migrate_proof_run_edges_for_external_lineage(conn: sqlite3.Connection) -> None:
    if not _proof_run_edges_has_parent_fk(conn):
        return
    conn.commit()
    conn.execute("PRAGMA foreign_keys=OFF")
    try:
        for trigger in (
            "proof_run_edges_append_only_no_update",
            "proof_run_edges_append_only_no_delete",
            "proof_run_edges_known_kind",
        ):
            conn.execute(f"DROP TRIGGER IF EXISTS {trigger}")
        conn.execute("DROP TABLE IF EXISTS proof_run_edges_external_lineage")
        conn.execute(
            """
            CREATE TABLE proof_run_edges_external_lineage (
                edge_id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_run_id TEXT NOT NULL,
                child_run_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                author TEXT NOT NULL,
                kind TEXT NOT NULL,
                note TEXT NOT NULL DEFAULT '',
                FOREIGN KEY(child_run_id) REFERENCES proof_runs(run_id),
                UNIQUE(parent_run_id, child_run_id, kind)
            )
            """
        )
        conn.execute(
            """
            INSERT INTO proof_run_edges_external_lineage (
                edge_id, parent_run_id, child_run_id, created_at, author, kind, note
            )
            SELECT edge_id, parent_run_id, child_run_id, created_at, author, kind, note
            FROM proof_run_edges
            ORDER BY edge_id
            """
        )
        conn.execute("DROP TABLE proof_run_edges")
        conn.execute(
            "ALTER TABLE proof_run_edges_external_lineage RENAME TO proof_run_edges"
        )
        conn.commit()
    finally:
        conn.execute("PRAGMA foreign_keys=ON")



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
    parent_exists = _run_exists(conn, parent_run_id)
    if not parent_exists and _edge_kind_requires_local_parent(kind):
        raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    if not _run_exists(conn, child_run_id):
        raise SystemExit(f"unknown child proof run {child_run_id!r}")
    if parent_run_id == child_run_id:
        raise SystemExit("proof DAG edge cannot point to itself")
    if parent_exists and _edge_would_create_cycle(
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
            LEFT JOIN proof_runs parent ON parent.run_id = edge.parent_run_id
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
    status = run_git("status", "--short", "--untracked-files=all")
    status_lines = status.stdout.splitlines() if status.returncode == 0 else []
    filtered_status, ignored_status = filter_status_lines(
        status_lines,
        DEFAULT_DIRTY_TREE_IGNORE_GLOBS,
    )
    return {
        "available": True,
        "head": head.stdout.strip(),
        "dirty": bool(filtered_status),
        "status": filtered_status[:200],
        "ignored_status_count": len(ignored_status),
    }



def _log_path_for_run(conn: sqlite3.Connection, run_id: str) -> Path:
    conn.row_factory = sqlite3.Row
    row = conn.execute(
        "SELECT log_path FROM proof_runs WHERE run_id = ?", (run_id,)
    ).fetchone()
    if row is None:
        raise SystemExit(f"unknown proof run {run_id!r}")
    return Path(row["log_path"])



def _db_path(args: argparse.Namespace) -> Path:
    if args.db:
        return Path(args.db)
    custody = checkout_custody(ROOT, os.environ)
    state_root = custody.custody_root if custody.ephemeral else custody.source_root
    return state_root / "logs" / "proof_queue" / "proof_queue.sqlite3"



def _logs_root(args: argparse.Namespace) -> Path:
    if args.logs_root:
        return Path(args.logs_root)
    custody = checkout_custody(ROOT, os.environ)
    state_root = custody.custody_root if custody.ephemeral else custody.source_root
    return state_root / "logs" / "proof_queue" / "runs"



def _repo_root(args: argparse.Namespace) -> Path:
    return Path(args.repo_root).resolve() if args.repo_root else ROOT



def _row_by_run_id(conn: sqlite3.Connection, run_id: str) -> sqlite3.Row | None:
    conn.row_factory = sqlite3.Row
    return conn.execute(
        "SELECT * FROM proof_runs WHERE run_id = ?",
        (run_id,),
    ).fetchone()



def _row_value(row: sqlite3.Row, key: str) -> object | None:
    """Fetch ``key`` from a row, tolerating rows without the column."""
    try:
        return row[key]
    except (IndexError, KeyError):
        return None



def _is_sqlite_locked_error(exc: BaseException) -> bool:
    if not isinstance(exc, sqlite3.OperationalError):
        return False
    message = str(exc).lower()
    return "database is locked" in message or "database is busy" in message



def _commit_with_locked_retry(conn: sqlite3.Connection) -> None:
    """Commit, retrying briefly on 'database is locked'.

    ``PRAGMA busy_timeout`` already waits for contended writers, but a terminal
    status write that still loses the race must not strand the row 'running' or
    lose the result. Retry a bounded number of times before re-raising so a
    persistent lock still surfaces rather than silently discarding the write.
    """
    last_exc: sqlite3.OperationalError | None = None
    for attempt in range(PROOF_QUEUE_LOCKED_WRITE_RETRIES):
        try:
            conn.commit()
            return
        except sqlite3.OperationalError as exc:
            if not _is_sqlite_locked_error(exc):
                raise
            last_exc = exc
            if attempt + 1 < PROOF_QUEUE_LOCKED_WRITE_RETRIES:
                time.sleep(PROOF_QUEUE_LOCKED_WRITE_RETRY_SLEEP_SECONDS)
    if last_exc is not None:
        raise last_exc



def _update_run(conn: sqlite3.Connection, run_id: str, **values: object) -> None:
    if not values:
        return
    assignments = ", ".join(f"{key} = ?" for key in values)
    conn.execute(
        f"UPDATE proof_runs SET {assignments} WHERE run_id = ?",
        (*values.values(), run_id),
    )
    _commit_with_locked_retry(conn)
