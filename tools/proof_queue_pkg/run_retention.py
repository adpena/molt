"""Queue-owned retention for proof run evidence files.

The queue database is the authority for run evidence. Retention never deletes
a row, a note or a DAG edge. It reclaims the files that one terminal run wrote
beside its log, and that run's own scratch directory, only after it records a
claim in the run's row. Every evidence lookup reads that record first, so a
lookup never follows a row to a file that has gone without knowing why.

The claim is the point of no return. A pass that stops after the claim leaves
the row in the ``reclaiming`` state with its exact path list, and the next pass
finishes the deletion. Each path is checked against the row's custody again
before it is deleted.

These runs keep all their files: unresolved rows, failed rows, runs pinned by a
``retain`` note, parents of unresolved rows, runs whose Cargo generation target
is still live, and rows whose evidence custody cannot be proved. Of the other
terminal runs, the newest runs that fit the count and byte bound stay. The
newest such run always stays, so the run that just finished is inspectable.
"""

from __future__ import annotations

from contextlib import closing
from dataclasses import dataclass, replace
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sqlite3
from typing import Iterator, Mapping, Sequence

from molt.exact_json import loads_exact, read_exact
from molt.file_deletion import delete_path
from molt.file_publication import metadata_is_link_like, resolve_owned_path
from molt.temporary_artifacts import ScratchRetention, tree_bytes
from tools.proof_queue_pkg import (
    cargo_cache_custody,
    cargo_output_layout,
    cargo_output_lifecycle,
    command_identity,
    evidence,
    state,
)

RETAIN_RUNS_ENV = "MOLT_PROOF_QUEUE_RETAIN_RUNS"
RETAIN_GB_ENV = "MOLT_PROOF_QUEUE_RETAIN_GB"

# About one week of work: one host recorded 2,720 runs in about four months.
DEFAULT_RETAIN_RUNS = 200

# A third of the 25 GiB build-admission floor, so kept evidence never eats the
# headroom a build needs. At the measured ~1.8 MB per ordinary run, 200 runs
# use ~360 MB; only scratch-heavy runs reach this bound.
DEFAULT_RETAIN_GB = 8.0

# An automatic pass follows each proof. It reclaims a bounded batch, so a large
# backlog never delays one proof for long; the explicit command has no cap.
AUTOMATIC_RECLAIM_LIMIT = 32

REPORT_SCHEMA = "molt.proof-run-retention-report.v1"

UNRESOLVED_STATUSES = ("queued", "dispatched", "running", "stale")
RECLAIMABLE_STATUSES = ("passed", "non-evidence", "blocked")

KEEP_UNRESOLVED = "unresolved"
KEEP_FAILED = "failed"
KEEP_UNKNOWN_STATUS = "unknown-status"
KEEP_PINNED = "pinned"
KEEP_ACTIVE_PARENT = "active-dependency"
KEEP_CARGO = "cargo-generation-live"
KEEP_UNVERIFIED = "custody-unverified"
KEEP_WINDOW = "window"
RECLAIM = "reclaim"
RECLAIMING = "reclaiming"
RECLAIMED = "reclaimed"

_SIMPLE_KEY = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")
_NONCE = re.compile(r"[0-9a-f]{64}")
_MAX_REQUEST_BYTES = 16 * 1024 * 1024


def _sql_list(values: Sequence[str]) -> str:
    return ", ".join(f"'{value}'" for value in values)


# Receipt contexts can be large. Retention reads only the two facts it needs.
_CONTEXT_FACT = (
    "CASE WHEN json_valid(receipt_context_json) "
    "THEN json_extract(receipt_context_json, '{path}') END"
)

_CLAIM_SQL = f"""
    UPDATE proof_runs SET evidence_retention_json = ?
    WHERE run_id = ?
      AND evidence_retention_json IS NULL
      AND status IN ({_sql_list(RECLAIMABLE_STATUSES)})
      AND NOT EXISTS (
          SELECT 1 FROM proof_notes n
          WHERE n.run_id = proof_runs.run_id AND n.kind = ?
      )
      AND NOT EXISTS (
          SELECT 1 FROM proof_run_edges e
          JOIN proof_runs c ON c.run_id = e.child_run_id
          WHERE e.parent_run_id = proof_runs.run_id
            AND c.status IN ({_sql_list(UNRESOLVED_STATUSES)})
      )
"""

_FINISH_SQL = """
    UPDATE proof_runs SET evidence_retention_json = ?
    WHERE run_id = ?
      AND json_extract(evidence_retention_json, '$.state') = 'reclaiming'
"""


class CustodyUnverified(ValueError):
    """Retention cannot prove that it owns a run's evidence files."""


@dataclass(frozen=True, slots=True)
class RunEvidence:
    run_id: str
    status: str
    finished_at: str | None
    disposition: str
    paths: tuple[Path, ...] = ()
    bytes: int | None = None
    detail: str = ""


@dataclass(frozen=True, slots=True)
class _Entry:
    path: Path
    size: int
    regular: bool


def configured_policy(
    env: Mapping[str, str],
    *,
    runs: int | None = None,
    gb: float | None = None,
) -> ScratchRetention:
    """Resolve the count and byte bound: option, then environment, then default."""
    raw_runs: object = runs if runs is not None else env.get(RETAIN_RUNS_ENV)
    raw_gb: object = gb if gb is not None else env.get(RETAIN_GB_ENV)
    count = DEFAULT_RETAIN_RUNS
    if raw_runs is not None and str(raw_runs).strip():
        try:
            count = int(str(raw_runs).strip())
        except ValueError as exc:
            raise ValueError(
                f"{RETAIN_RUNS_ENV} must be a positive integer, got {raw_runs!r}"
            ) from exc
    if count < 1:
        raise ValueError(f"{RETAIN_RUNS_ENV} must be a positive integer, got {count}")
    size_gb = DEFAULT_RETAIN_GB
    if raw_gb is not None and str(raw_gb).strip():
        try:
            size_gb = float(str(raw_gb).strip())
        except ValueError as exc:
            raise ValueError(
                f"{RETAIN_GB_ENV} must be a positive number, got {raw_gb!r}"
            ) from exc
    if not math.isfinite(size_gb) or size_gb <= 0:
        raise ValueError(f"{RETAIN_GB_ENV} must be a positive number, got {size_gb}")
    return ScratchRetention(count=count, bytes=int(size_gb * 1024**3))


def _key(name: str) -> str:
    """Run evidence names are ``<stem>.<suffix>``; the stem owns the file."""
    return name.split(".", 1)[0]


def _same_path(left: Path, right: Path) -> bool:
    return os.path.normcase(str(left)) == os.path.normcase(str(right))


def _scan_result_root(result_root: Path) -> dict[str, list[_Entry]]:
    """Group the direct entries of the result root by evidence stem."""
    grouped: dict[str, list[_Entry]] = {}
    try:
        iterator = os.scandir(result_root)
    except FileNotFoundError:
        return grouped
    with iterator as entries:
        for entry in entries:
            if "." not in entry.name:
                # Shared stores (custody CAS, Cargo metadata, run scratch)
                # have no suffix. They are not one run's evidence files.
                continue
            metadata = entry.stat(follow_symlinks=False)
            regular = entry.is_file(
                follow_symlinks=False
            ) and not metadata_is_link_like(metadata)
            grouped.setdefault(_key(entry.name), []).append(
                _Entry(Path(entry.path), metadata.st_size, regular)
            )
    return grouped


def _row_keys(row: sqlite3.Row) -> set[str]:
    return {
        str(row["run_id"]),
        _key(Path(str(row["log_path"])).name),
        _key(Path(str(row["summary_json"])).name),
    }


def _has_table(conn: sqlite3.Connection, name: str) -> bool:
    return (
        conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", (name,)
        ).fetchone()
        is not None
    )


def _rows(
    conn: sqlite3.Connection, *, run_id: str | None = None
) -> Iterator[sqlite3.Row]:
    """Stream the row facts retention reads, oldest first."""
    conn.row_factory = sqlite3.Row
    columns = {row[1] for row in conn.execute("PRAGMA table_info(proof_runs)")}
    retention = (
        "evidence_retention_json"
        if "evidence_retention_json" in columns
        else "NULL AS evidence_retention_json"
    )
    prelaunch = _CONTEXT_FACT.format(path="$.derived_root_custody.prelaunch")
    nonce = _CONTEXT_FACT.format(path="$.execution_nonce_sha256")
    where = "" if run_id is None else "WHERE run_id = ?"
    yield from conn.execute(
        f"""
        SELECT rowid AS retention_rowid, run_id, status, finished_at, log_path,
               summary_json, command_envelope_json, {retention},
               {prelaunch} AS prelaunch_json, {nonce} AS nonce_sha256
        FROM proof_runs {where} ORDER BY rowid
        """,
        () if run_id is None else (run_id,),
    )


def _prelaunch(row: sqlite3.Row) -> list[Mapping[str, object]]:
    raw = row["prelaunch_json"]
    if raw is None:
        return []
    try:
        entries = json.loads(str(raw))
    except json.JSONDecodeError as exc:
        raise CustodyUnverified("derived root custody is not JSON") from exc
    if not isinstance(entries, list):
        return []
    return [entry for entry in entries if isinstance(entry, Mapping)]


def _cargo_target_live(row: sqlite3.Row) -> bool:
    """A Cargo generation target that still exists belongs to its owner first."""
    for entry in _prelaunch(row):
        if entry.get("schema") != cargo_cache_custody.SCHEMA:
            continue
        target = entry.get("path")
        if isinstance(target, str) and os.path.lexists(target):
            return True
    return False


def _layout(
    row: sqlite3.Row, result_root: Path
) -> cargo_output_layout.CargoOutputLayout:
    try:
        envelope = loads_exact(str(row["command_envelope_json"]))
    except (TypeError, ValueError) as exc:
        raise CustodyUnverified("admission envelope is not exact JSON") from exc
    if not isinstance(envelope, dict):
        raise CustodyUnverified("admission envelope is malformed")
    try:
        return cargo_output_layout.CargoOutputLayout.create(
            result_root=result_root, declaration=envelope.get("cargo_output_root")
        )
    except (OSError, ValueError) as exc:
        raise CustodyUnverified(f"payload root is unavailable: {exc}") from exc


def _scratch_owned(path: Path, payload_root: Path) -> bool:
    """A run scratch root is ``<payload root>/derived/<execution nonce>``."""
    return _NONCE.fullmatch(path.name) is not None and _same_path(
        path.parent, payload_root / "derived"
    )


def _scratch_roots(
    row: sqlite3.Row, layout: cargo_output_layout.CargoOutputLayout
) -> list[Path]:
    """Find the run's scratch from its receipt and from its execution request."""
    roots: list[Path] = []
    nonce_digest = row["nonce_sha256"]
    for entry in _prelaunch(row):
        if entry.get("role") != "scratch-output" or entry.get("run_owned") is not True:
            continue
        raw = entry.get("path")
        if not isinstance(raw, str):
            raise CustodyUnverified("scratch custody entry has no path")
        scratch = Path(raw)
        root = scratch.parent
        if scratch.name != "scratch" or not _scratch_owned(root, layout.payload_root):
            raise CustodyUnverified(f"scratch is outside the payload root: {scratch}")
        if (
            isinstance(nonce_digest, str)
            and hashlib.sha256(root.name.encode()).hexdigest() != nonce_digest
        ):
            raise CustodyUnverified(f"scratch is not this run's execution: {scratch}")
        roots.append(root)
    request_path, _result = command_identity.execution_record_paths(
        Path(str(row["log_path"]))
    )
    if os.path.lexists(request_path):
        try:
            request = read_exact(
                request_path,
                max_bytes=_MAX_REQUEST_BYTES,
                label="proof execution request",
            )
        except (OSError, ValueError) as exc:
            raise CustodyUnverified(f"execution request is unreadable: {exc}") from exc
        if not isinstance(request, dict) or request.get("run_id") != row["run_id"]:
            raise CustodyUnverified("execution request names another run")
        nonce = request.get("execution_nonce")
        if isinstance(nonce, str) and _NONCE.fullmatch(nonce):
            roots.append(layout.scratch(nonce).parent)
    unique: dict[str, Path] = {}
    for root in roots:
        if not os.path.lexists(root):
            continue
        if metadata_is_link_like(root.lstat()) or not root.is_dir():
            raise CustodyUnverified(f"scratch root is not a direct directory: {root}")
        unique[os.path.normcase(str(root))] = root
    return list(unique.values())


def _evidence_paths(
    row: sqlite3.Row,
    *,
    result_root: Path,
    entries: Mapping[str, list[_Entry]],
    owners: Mapping[str, set[str]],
) -> tuple[list[_Entry], list[Path]]:
    """Return the files and scratch roots one row owns, or raise if unproved."""
    run_id = str(row["run_id"])
    if _SIMPLE_KEY.fullmatch(run_id) is None:
        raise CustodyUnverified("run id is not a simple file stem")
    for column in ("log_path", "summary_json"):
        try:
            parent = resolve_owned_path(Path(str(row[column])).parent)
        except ValueError as exc:
            raise CustodyUnverified(f"{column} parent is not owned: {exc}") from exc
        if not _same_path(parent, result_root):
            raise CustodyUnverified(f"{column} is outside this queue's result root")
    files: list[_Entry] = []
    for key in sorted(_row_keys(row)):
        if _SIMPLE_KEY.fullmatch(key) is None:
            raise CustodyUnverified(f"evidence stem {key!r} is not simple")
        if owners.get(key, set()) != {run_id}:
            raise CustodyUnverified(f"evidence stem {key!r} names another run")
        for entry in entries.get(key, ()):
            if not entry.regular:
                raise CustodyUnverified(f"unexpected evidence entry: {entry.path}")
            files.append(entry)
    return files, _scratch_roots(row, _layout(row, result_root))


def _measure(files: Sequence[_Entry], scratch: Sequence[Path]) -> int:
    return sum(entry.size for entry in files) + sum(
        tree_bytes(root) for root in scratch
    )


def _paths_bytes(paths: Sequence[Path]) -> int:
    total = 0
    for path in paths:
        if not os.path.lexists(path):
            continue
        total += tree_bytes(path) if path.is_dir() else path.lstat().st_size
    return total


def plan(
    conn: sqlite3.Connection,
    *,
    result_root: Path,
    policy: ScratchRetention,
    measure_all: bool,
) -> tuple[list[RunEvidence], dict[str, int]]:
    """Classify every row; measure protected rows only when ``measure_all``."""
    if not _has_table(conn, "proof_runs"):
        return [], {"files": 0, "bytes": 0}
    owners: dict[str, set[str]] = {}
    for run_id, log_path, summary_json in conn.execute(
        "SELECT run_id, log_path, summary_json FROM proof_runs"
    ):
        for key in (
            str(run_id),
            _key(Path(str(log_path)).name),
            _key(Path(str(summary_json)).name),
        ):
            owners.setdefault(key, set()).add(str(run_id))
    pinned = {
        str(run_id)
        for (run_id,) in conn.execute(
            "SELECT DISTINCT run_id FROM proof_notes WHERE kind = ?",
            (state.RETAIN_NOTE_KIND,),
        )
    }
    active_parents = {
        str(parent)
        for (parent,) in conn.execute(
            "SELECT DISTINCT e.parent_run_id FROM proof_run_edges e "
            "JOIN proof_runs c ON c.run_id = e.child_run_id "
            f"WHERE c.status IN ({_sql_list(UNRESOLVED_STATUSES)})"
        )
    }
    disposition_open = {
        str(finding.get("run_id"))
        for finding in cargo_output_lifecycle.unresolved_dispositions(conn, limit=100)
    }
    entries = _scan_result_root(result_root)
    unowned = [
        entry
        for key, group in entries.items()
        if key not in owners
        for entry in group
        if entry.regular
    ]
    classified: list[RunEvidence] = []
    candidates: list[tuple[tuple[str, int], RunEvidence, list[_Entry], list[Path]]] = []
    for row in _rows(conn):
        run_id = str(row["run_id"])
        status = str(row["status"])
        base = RunEvidence(
            run_id=run_id,
            status=status,
            finished_at=row["finished_at"],
            disposition="",
        )
        retention = state._evidence_retention(row)
        if retention is not None:
            classified.append(replace(base, disposition=str(retention["state"])))
            continue
        disposition = ""
        if status in UNRESOLVED_STATUSES:
            disposition = KEEP_UNRESOLVED
        elif status == "failed":
            disposition = KEEP_FAILED
        elif status not in RECLAIMABLE_STATUSES:
            disposition = KEEP_UNKNOWN_STATUS
        elif run_id in pinned:
            disposition = KEEP_PINNED
        elif run_id in active_parents:
            disposition = KEEP_ACTIVE_PARENT
        if disposition and not measure_all:
            classified.append(replace(base, disposition=disposition))
            continue
        try:
            if not disposition and (
                run_id in disposition_open or _cargo_target_live(row)
            ):
                disposition = KEEP_CARGO
                if not measure_all:
                    classified.append(replace(base, disposition=disposition))
                    continue
            files, scratch = _evidence_paths(
                row, result_root=result_root, entries=entries, owners=owners
            )
        except (CustodyUnverified, OSError, ValueError) as exc:
            classified.append(
                replace(
                    base, disposition=disposition or KEEP_UNVERIFIED, detail=str(exc)
                )
            )
            continue
        located = replace(base, paths=(*(entry.path for entry in files), *scratch))
        if disposition:
            classified.append(
                replace(
                    located, disposition=disposition, bytes=_measure(files, scratch)
                )
            )
            continue
        order = (str(row["finished_at"] or ""), int(row["retention_rowid"]))
        candidates.append((order, located, files, scratch))
    # Newest first: keep what fits the bound; the newest run always stays.
    kept_count = 0
    kept_bytes = 0
    window_open = True
    candidates.sort(key=lambda item: item[0], reverse=True)
    for index, (_order, located, files, scratch) in enumerate(candidates):
        size = _measure(files, scratch) if window_open or measure_all else None
        if (
            window_open
            and size is not None
            and (
                index == 0
                or policy.admits(
                    kept_count=kept_count, kept_bytes=kept_bytes, size=size
                )
            )
        ):
            kept_count += 1
            kept_bytes += size
            window_open = kept_count < policy.count
            classified.append(replace(located, disposition=KEEP_WINDOW, bytes=size))
            continue
        classified.append(replace(located, disposition=RECLAIM, bytes=size))
    return classified, {
        "files": len(unowned),
        "bytes": sum(entry.size for entry in unowned),
    }


def _owned_for_deletion(
    row: sqlite3.Row, path: Path, *, result_root: Path, payload_root: Path
) -> bool:
    """Recheck one recorded path against the row before it is deleted."""
    if _same_path(path.parent, result_root):
        return _key(path.name) in _row_keys(row)
    return _scratch_owned(path, payload_root)


def _finish(
    conn: sqlite3.Connection,
    row: sqlite3.Row,
    record: Mapping[str, object],
    *,
    result_root: Path,
) -> list[str]:
    """Delete the recorded paths, then mark the claim reclaimed."""
    raw_paths = record.get("paths")
    errors: list[str] = []
    if not isinstance(raw_paths, list) or not all(
        isinstance(path, str) for path in raw_paths
    ):
        errors.append("retention record has no path list")
        raw_paths = []
    try:
        payload_root = _layout(row, result_root).payload_root
    except CustodyUnverified as exc:
        errors.append(str(exc))
        raw_paths = []
    for path in map(Path, raw_paths):
        if not _owned_for_deletion(
            row, path, result_root=result_root, payload_root=payload_root
        ):
            errors.append(f"{path}: outside the run's evidence custody")
            continue
        deleted, error = delete_path(path)
        if not deleted:
            errors.append(f"{path}: {error}")
        elif os.path.lexists(path):
            errors.append(f"{path}: still present after deletion")
    if errors:
        final = {**record, "error": "; ".join(errors)}
    else:
        final = {
            **record,
            "state": RECLAIMED,
            "reclaimed_at": state._utc_now(),
            "error": None,
        }
    conn.execute(_FINISH_SQL, (json.dumps(final, sort_keys=True), str(row["run_id"])))
    state._commit_with_locked_retry(conn)
    return errors


def _in_result_root(row: sqlite3.Row, result_root: Path) -> bool:
    try:
        parent = resolve_owned_path(Path(str(row["log_path"])).parent)
    except ValueError:
        return False
    return _same_path(parent, result_root)


def _recover(
    conn: sqlite3.Connection, *, result_root: Path
) -> tuple[list[str], list[str]]:
    """Finish every claim an earlier pass left in the reclaiming state."""
    pending = [
        row
        for row in _rows(conn)
        if (record := state._evidence_retention(row)) is not None
        and record["state"] == RECLAIMING
        and _in_result_root(row, result_root)
    ]
    recovered: list[str] = []
    errors: list[str] = []
    for row in pending:
        record = state._evidence_retention(row)
        assert record is not None
        failures = _finish(conn, row, record, result_root=result_root)
        if failures:
            errors.extend(f"{row['run_id']}: {failure}" for failure in failures)
        else:
            recovered.append(str(row["run_id"]))
    return recovered, errors


def _summary(
    runs: Sequence[RunEvidence], unowned: Mapping[str, int]
) -> dict[str, object]:
    classes: dict[str, dict[str, int | None]] = {}
    for run in runs:
        bucket = classes.setdefault(run.disposition, {"runs": 0, "bytes": 0})
        bucket["runs"] = (bucket["runs"] or 0) + 1
        if run.disposition in {RECLAIMED, RECLAIMING}:
            continue
        current = bucket["bytes"]
        bucket["bytes"] = (
            None if current is None or run.bytes is None else current + run.bytes
        )
    return {
        "rows": len(runs),
        "classes": {name: classes[name] for name in sorted(classes)},
        "unowned_files": dict(unowned),
    }


def _run_payload(run: RunEvidence) -> dict[str, object]:
    payload: dict[str, object] = {
        "run_id": run.run_id,
        "status": run.status,
        "finished_at": run.finished_at,
        "bytes": run.bytes,
    }
    if run.detail:
        payload["detail"] = run.detail
    return payload


def _claim_and_reclaim(
    conn: sqlite3.Connection,
    run: RunEvidence,
    *,
    policy: ScratchRetention,
    result_root: Path,
) -> tuple[str, int, list[str]]:
    """Claim one run in its row, then delete its files. Returns the outcome."""
    # Finish the read before the claim writes on the same connection.
    rows = list(_rows(conn, run_id=run.run_id))
    if not rows:
        return "skipped", 0, []
    row = rows[0]
    size = run.bytes if run.bytes is not None else _paths_bytes(run.paths)
    record = {
        "schema": state.EVIDENCE_RETENTION_SCHEMA,
        "state": RECLAIMING,
        "reason": "outside-retention-window",
        "policy": {"runs": policy.count, "bytes": policy.bytes},
        "paths": [str(path) for path in run.paths],
        "bytes": size,
        # Saved before the guard summary goes, so the receipt stays identical.
        "peak_rss_bytes": evidence._queue_peak_rss_bytes(row["summary_json"]),
        "requested_at": state._utc_now(),
        "reclaimed_at": None,
        "error": None,
    }
    # One statement rechecks eligibility: a pin, a new unresolved child or
    # another pass that got here first leaves the row untouched.
    cursor = conn.execute(
        _CLAIM_SQL,
        (json.dumps(record, sort_keys=True), run.run_id, state.RETAIN_NOTE_KIND),
    )
    state._commit_with_locked_retry(conn)
    if cursor.rowcount != 1:
        return "skipped", 0, []
    errors = _finish(conn, row, record, result_root=result_root)
    return ("reclaiming" if errors else "reclaimed"), size, errors


def run_pass(
    *,
    db: Path,
    result_root: Path,
    policy: ScratchRetention,
    apply: bool,
    limit: int | None = None,
    measure_all: bool = True,
) -> dict[str, object]:
    """Report retention, and with ``apply`` reclaim the runs outside the window."""
    if limit is not None and (type(limit) is not int or limit < 1):
        raise ValueError("retention limit must be a positive integer")
    result_root = resolve_owned_path(result_root)
    report: dict[str, object] = {
        "schema": REPORT_SCHEMA,
        "apply": apply,
        "db": str(db),
        "result_root": str(result_root),
        "policy": {"runs": policy.count, "bytes": policy.bytes},
    }
    if not apply:
        runs: list[RunEvidence] = []
        unowned = {"files": 0, "bytes": 0}
        if db.exists():
            # Read only: a dry run never migrates or writes queue state.
            uri = f"{db.resolve().as_uri()}?mode=ro"
            with closing(sqlite3.connect(uri, uri=True)) as conn:
                runs, unowned = plan(
                    conn,
                    result_root=result_root,
                    policy=policy,
                    measure_all=measure_all,
                )
        report.update(_summary(runs, unowned))
        report["reclaim"] = [
            _run_payload(run) for run in runs if run.disposition == RECLAIM
        ]
        report["recover"] = [
            run.run_id for run in runs if run.disposition == RECLAIMING
        ]
        report["unverified"] = [
            _run_payload(run) for run in runs if run.disposition == KEEP_UNVERIFIED
        ]
        return report
    reclaimed: list[dict[str, object]] = []
    skipped: list[str] = []
    with closing(state._connect(db)) as conn:
        recovered, errors = _recover(conn, result_root=result_root)
        runs, unowned = plan(
            conn, result_root=result_root, policy=policy, measure_all=measure_all
        )
        # Oldest first, so a bounded or interrupted pass frees the oldest runs.
        candidates = sorted(
            (run for run in runs if run.disposition == RECLAIM),
            key=lambda run: (run.finished_at or "", run.run_id),
        )
        for run in candidates[:limit]:
            outcome, size, failures = _claim_and_reclaim(
                conn, run, policy=policy, result_root=result_root
            )
            if outcome == "skipped":
                skipped.append(run.run_id)
                continue
            errors.extend(f"{run.run_id}: {failure}" for failure in failures)
            if outcome == "reclaimed":
                reclaimed.append({**_run_payload(run), "bytes": size})
    report.update(_summary(runs, unowned))
    report["reclaimed"] = reclaimed
    report["recovered"] = recovered
    report["skipped"] = skipped
    report["errors"] = errors
    report["unverified"] = [
        _run_payload(run) for run in runs if run.disposition == KEEP_UNVERIFIED
    ]
    return report


def automatic_pass(
    *, db: Path, result_root: Path, env: Mapping[str, str]
) -> dict[str, object]:
    """The bounded pass that follows each completed proof."""
    return run_pass(
        db=db,
        result_root=result_root,
        policy=configured_policy(env),
        apply=True,
        limit=AUTOMATIC_RECLAIM_LIMIT,
        measure_all=False,
    )
