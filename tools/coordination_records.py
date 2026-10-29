"""Coordination record schema, discovery and terminal evidence harvest.

Library authority shared by the coordination CLI and daily disk guard. Keep
this module's dependencies within the standard library and molt: execution
custody must not import the coordination CLI, reporting or release ledgers.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
import contextlib
from dataclasses import dataclass
from datetime import UTC, datetime
from functools import partial
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import time
from typing import Any

from molt.checkout_state import (
    WalkIncomplete,
    WalkRotation,
    agent_evidence_idle_s,
    checkout_state_retirement,
    compact_evidence,
)
from molt.file_publication import resolve_owned_path
from molt.portable_paths import portable_path_component, portable_relative_path

LOG_ROOT = Path("logs/agents")

COORDINATION_RECORD_GLOB = "*/coordination.json"

COORDINATION_RECORD_SOURCE = f"{LOG_ROOT.as_posix()}/{COORDINATION_RECORD_GLOB}"

SCHEMA_VERSION = 1

ACTIVE_STATUSES = frozenset({"running", "paused", "blocked"})

OWNERSHIP_REQUEST_STATUSES = frozenset(
    {"pending", "acknowledged", "declined", "withdrawn"}
)

BROAD_ROLE = "broad-sweep coordinator"


@dataclass(frozen=True)
class CoordinationRecord:
    task: str
    path: Path
    payload: dict[str, Any]
    raw: bytes | None = None
    signature: tuple[int, int, int, int] | None = None

    @property
    def status(self) -> str:
        return str(self.payload.get("status") or "")

    @property
    def proof_role(self) -> str:
        return str(self.payload.get("proof_role") or "")

    @property
    def planned_proof_lane(self) -> str:
        return str(self.payload.get("planned_proof_lane") or "")

    @property
    def shared_target_root(self) -> str:
        return str(self.payload.get("shared_target_root") or "")

    @property
    def active(self) -> bool:
        return self.status in ACTIVE_STATUSES

    @property
    def broad_coordinator(self) -> bool:
        return self.proof_role == BROAD_ROLE


def validate_task_name(task: str) -> str:
    try:
        portable_path_component(task)
        if task != task.strip():
            raise ValueError("task name has surrounding whitespace")
    except ValueError as exc:
        raise ValueError(
            f"task name must be one directory directly under logs/agents: {task!r}"
        ) from exc
    return task


def task_dir(repo_root: Path, task: str) -> Path:
    return repo_root / LOG_ROOT / validate_task_name(task)


def task_artifacts_dir(
    repo_root: Path, task: str, environment: Mapping[str, str]
) -> Path:
    from molt.dx import storage_path, checkout_component

    return (
        storage_path(repo_root, "lane-evidence", environment)
        / checkout_component(repo_root)
        / validate_task_name(task)
        / "artifacts"
    )


def _validate_ownership_request(value: object) -> None:
    if value is None:
        return
    if not isinstance(value, dict):
        raise ValueError("ownership_request must be an object")
    owner = value.get("owner")
    if not isinstance(owner, str) or validate_task_name(owner) != owner:
        raise ValueError("ownership_request owner must be a coordination task name")
    status = value.get("status")
    if not isinstance(status, str) or status not in OWNERSHIP_REQUEST_STATUSES:
        raise ValueError("ownership_request status is invalid")
    paths = value.get("requested_paths")
    if not isinstance(paths, list) or not paths:
        raise ValueError("ownership_request requested_paths must be a nonempty list")
    for path in paths:
        try:
            # One trailing slash denotes a requested directory, not an alias.
            portable_relative_path(path.removesuffix("/"))
        except (AttributeError, ValueError) as exc:
            raise ValueError(
                "ownership_request paths must be portable repository-relative paths"
            ) from exc
    if len(set(paths)) != len(paths):
        raise ValueError("ownership_request paths must be unique")
    stamp = value.get("requested_at_utc")
    try:
        instant = datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    except (AttributeError, TypeError, ValueError) as exc:
        raise ValueError("ownership_request requested_at_utc is invalid") from exc
    if instant.utcoffset() != UTC.utcoffset(None):
        raise ValueError("ownership_request requested_at_utc must include UTC")


def _decode_record_bytes(data: bytes) -> str:
    """Decode a coordination record tolerant of BOM / encoding variance.

    Records are canonically UTF-8 (see ``write_json``), but an agent that writes
    a record through a Windows shell redirect (PowerShell defaults to UTF-16-LE
    with a BOM) emits UTF-16 or UTF-8-BOM bytes. The read boundary must tolerate
    all of these: a single stray-encoded record previously raised
    ``UnicodeDecodeError`` out of ``load_records`` (the UTF-16 case) or was
    silently dropped as invalid (the UTF-8-BOM case), which made ``scan`` /
    ``check`` unusable and silently defeated cross-agent coordination on Windows.
    """
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return data.decode("utf-16")
    if data[:3] == b"\xef\xbb\xbf":
        return data.decode("utf-8-sig")
    return data.decode("utf-8")


def _record_signature(path: Path) -> tuple[int, int, int, int]:
    found = path.stat()
    return found.st_dev, found.st_ino, found.st_mtime_ns, found.st_size


def load_records(repo_root: Path) -> list[CoordinationRecord]:
    root = repo_root / LOG_ROOT
    if not root.is_dir():
        return []
    records: list[CoordinationRecord] = []
    # init and task_dir own exactly one directory per task. Nested copies are
    # evidence, never registrations, regardless of their names or claimed state.
    for path in sorted(root.glob(COORDINATION_RECORD_GLOB)):
        task = path.parent.name
        raw = None
        signature = None
        try:
            if path.is_symlink() or path.parent.is_symlink():
                raise ValueError("coordination record must not traverse a symlink")
            signature = _record_signature(path)
            with path.open("rb") as source:
                raw = source.read(_EVIDENCE_TEXT_LIMIT + 1)
            if len(raw) > _EVIDENCE_TEXT_LIMIT:
                raise ValueError("coordination record exceeds 1 MiB")
            if _record_signature(path) != signature:
                raise ValueError("coordination record changed while reading")
            payload = json.loads(_decode_record_bytes(raw))
            if not isinstance(payload, dict):
                raise ValueError("coordination record must be an object")
            if validate_task_name(task) != task or payload.get("task") != task:
                raise ValueError(
                    f"coordination task {payload.get('task')!r} does not match "
                    f"canonical task directory {task!r}"
                )
            if not isinstance(payload.get("status"), str) or not payload["status"]:
                raise ValueError("coordination status must be a nonempty string")
            owned_paths = payload.get("owned_paths", [])
            if not isinstance(owned_paths, list) or not all(
                isinstance(path, str) for path in owned_paths
            ):
                raise ValueError("owned_paths must be a list of paths")
            _validate_ownership_request(payload.get("ownership_request"))
        except (OSError, ValueError, RecursionError) as exc:
            payload = {
                "schema_version": SCHEMA_VERSION,
                "task": task,
                "status": "invalid",
                "error": str(exc),
            }
        records.append(
            CoordinationRecord(
                task=task,
                path=path,
                payload=payload,
                raw=raw,
                signature=signature,
            )
        )
    return records


_EVIDENCE_TEXT_LIMIT = 1024 * 1024

_EVIDENCE_NAMES = re.compile(
    r"(?:^|[_.-])(?:reports?|handoffs?|verdicts?|progress)(?:[_.-]|$)", re.I
)


def _record_strings(value: object):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from _record_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from _record_strings(child)


def active_task_references(records: Sequence[CoordinationRecord]) -> set[str]:
    """References are task names or logs/agents coordinates, including nested fields."""
    tasks = {record.task for record in records}
    referenced: set[str] = set()
    for record in records:
        if not record.active:
            continue
        for value in _record_strings(record.payload):
            if value in tasks:
                referenced.add(value)
            referenced.update(
                match.group(1)
                for match in re.finditer(r"logs[/\\]agents[/\\]([^/\\\s]+)", value)
                if match.group(1) in tasks
            )
    return referenced


def _evidence_inventory(root: Path, deadline: float) -> tuple[int, float, list[Path]]:
    """One metadata walk, without following links or reading payloads."""
    newest, size, text = root.stat().st_mtime, 0, []
    stack = [root]
    while stack:
        directory = stack.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                if time.monotonic() >= deadline:
                    raise TimeoutError("agent evidence harvest budget exhausted")
                found = entry.stat(follow_symlinks=False)
                newest = max(newest, found.st_mtime)
                path = Path(entry.path)
                if stat.S_ISDIR(found.st_mode):
                    stack.append(path)
                    continue
                size += found.st_size
                relative = path.relative_to(root)
                if (
                    stat.S_ISREG(found.st_mode)
                    and found.st_size <= _EVIDENCE_TEXT_LIMIT
                    and (
                        relative == Path("coordination.json")
                        or _EVIDENCE_NAMES.search(path.name)
                        or any(
                            part in {"reports", "handoffs", "verdicts", "progress"}
                            for part in relative.parts[:-1]
                        )
                    )
                ):
                    text.append(path)
    return size, newest, sorted(text)


def _evidence_blob_id(data: bytes, algorithm: str) -> str:
    digest = hashlib.new(algorithm)
    digest.update(f"blob {len(data)}\0".encode("ascii"))
    digest.update(data)
    return digest.hexdigest()


def harvest_agent_evidence(
    repo_root: Path,
    *,
    environment: Mapping[str, str],
    records: Sequence[CoordinationRecord],
    references: set[str],
    record_stamps: dict[Path, tuple[int, int, int, int]],
    registry_stamps: dict[Path, int | None],
    now: float,
    apply: bool,
    tasks: Sequence[Path],
    walks: WalkRotation,
    protected: Sequence[Path] = (),
) -> list[dict[str, Any]]:
    """Keep terminal tasks' small records and retire each payload with one rename.

    ``tasks`` are task directories of this checkout in the order that the
    ``agent-evidence`` walk rotation visits them. A task's walks and text
    checks spend that family's budget; an incomplete one is deferred or
    reported over budget, and it never harvests (HF-528).

    Read candidate text once, hash its Git blob framing, and query one lazy
    cat-file stream for the whole checkout. Small records are relinked after
    retirement, so only one text buffer is live. Raw payloads are never read. A
    failed inventory, malformed registration, changed active reference set or
    unavailable Git object store cannot authorize a harvest.
    """
    idle = agent_evidence_idle_s(environment)
    log_root = repo_root / LOG_ROOT
    if not log_root.is_dir() or log_root.is_symlink():
        return []
    by_task = {record.task: record for record in records}
    results: list[dict[str, Any]] = []
    stream = None
    algorithm = ""
    git_failed = False
    git_environment = {
        key: value
        for key, value in environment.items()
        if key
        not in {
            "GIT_DIR",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        }
    }

    def references_unchanged() -> bool:
        return all(
            (path.stat().st_mtime_ns if path.exists() else None) == stamp
            for path, stamp in registry_stamps.items()
        ) and all(
            _record_signature(path) == stamp for path, stamp in record_stamps.items()
        )

    def is_blob(data: bytes) -> bool:
        nonlocal stream, algorithm, git_failed
        git_failed = True
        if stream is None:
            identity = subprocess.run(
                [
                    "git",
                    "--no-replace-objects",
                    "-C",
                    str(repo_root),
                    "rev-parse",
                    "--show-object-format",
                ],
                capture_output=True,
                check=True,
                timeout=10,
                env=git_environment,
            )
            algorithm = identity.stdout.decode("ascii").strip()
            if algorithm not in {"sha1", "sha256"}:
                raise ValueError(f"unsupported repository object format: {algorithm}")
            stream = subprocess.Popen(
                [
                    "git",
                    "--no-replace-objects",
                    "-C",
                    str(repo_root),
                    "cat-file",
                    "--batch-check",
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                env=git_environment,
            )
        oid = _evidence_blob_id(data, algorithm)
        assert stream.stdin is not None and stream.stdout is not None
        stream.stdin.write((oid + "\n").encode("ascii"))
        stream.stdin.flush()
        answer = stream.stdout.readline().decode("ascii").strip()
        if answer == f"{oid} missing":
            git_failed = False
            return False
        if answer != f"{oid} blob {len(data)}":
            raise ValueError(f"invalid repository blob response: {answer!r}")
        git_failed = False
        return True

    def unique_text(
        task: Path, record: CoordinationRecord, text: list[Path], deadline: float
    ) -> dict[Path, int]:
        """The small text records to keep: decodable and not a repository blob."""
        kept: dict[Path, int] = {}
        for path in text:
            if time.monotonic() >= deadline:
                raise TimeoutError("agent evidence harvest budget exhausted")
            resolve_owned_path(path)
            if path == record.path:
                assert record.raw is not None
                data = record.raw
            else:
                with path.open("rb") as source:
                    data = source.read(_EVIDENCE_TEXT_LIMIT + 1)
            if len(data) > _EVIDENCE_TEXT_LIMIT:
                raise ValueError(f"evidence changed during harvest: {path}")
            # Only decodable text is evidence; opaque bytes retire.
            try:
                if "\0" in _decode_record_bytes(data):
                    continue
            except UnicodeError:
                continue
            if path == record.path or not is_blob(data):
                kept[path.relative_to(task)] = len(data)
        return kept

    try:
        for task in tasks:
            item: dict[str, Any] = {"path": str(task), "kind": "agent-evidence"}
            if task.parent != log_root or task.is_symlink() or not task.is_dir():
                continue
            record = by_task.get(task.name)
            if record is None:
                results.append({**item, "reason": "missing-coordination-record"})
                continue
            if (
                record.status == "invalid"
                or record.raw is None
                or len(record.raw) > _EVIDENCE_TEXT_LIMIT
            ):
                results.append({**item, "reason": "invalid-coordination-record"})
                continue
            if record.active or task.name in references:
                results.append({**item, "reason": "active-or-referenced"})
                continue
            if any(
                path == task or task in path.parents or path in task.parents
                for path in protected
            ):
                results.append({**item, "reason": "protected"})
                continue
            try:
                with (
                    checkout_state_retirement(task)
                    if apply
                    else contextlib.nullcontext(True) as exclusive
                ):
                    if not exclusive:
                        results.append({**item, "reason": "state-in-use"})
                        continue
                    if _record_signature(record.path) != record.signature:
                        results.append({**item, "reason": "coordination-changed"})
                        continue
                    record_stat = record.path.stat()
                    newest = max(task.stat().st_mtime, record_stat.st_mtime)
                    for key in ("updated_at_utc", "created_at_utc"):
                        if stamp := record.payload.get(key):
                            newest = max(
                                newest,
                                datetime.fromisoformat(
                                    stamp.replace("Z", "+00:00")
                                ).timestamp(),
                            )
                    if now - newest < idle:
                        results.append({**item, "reason": "recent-activity"})
                        continue
                    key = str(task)
                    size, newest, text = walks.spend(
                        "agent-evidence", key, partial(_evidence_inventory, task)
                    )
                    if now - newest < idle:
                        results.append({**item, "reason": "recent-activity"})
                        continue
                    kept = walks.spend(
                        "agent-evidence", key, partial(unique_text, task, record, text)
                    )
                    payload_bytes = size - sum(kept.values())
                    if payload_bytes <= 0:
                        continue
                    if (
                        not references_unchanged()
                        or _record_signature(record.path) != record.signature
                    ):
                        results.append({**item, "reason": "coordination-changed"})
                        break
                    item.update(est_bytes=payload_bytes, kept_files=len(kept))
                    if apply:
                        # Raw writers also count: inspect every payload's activity
                        # again under custody immediately before the atomic move.
                        _, current_use, _ = walks.spend(
                            "agent-evidence", key, partial(_evidence_inventory, task)
                        )
                        if now - current_use < idle:
                            results.append({**item, "reason": "recent-activity"})
                            continue
                        # Serialize final reference observation with every canonical
                        # task registration/update, only across publication.
                        with contextlib.ExitStack() as registry_custody:
                            exclusive_registries = [
                                registry_custody.enter_context(
                                    checkout_state_retirement(registry)
                                )
                                for registry in sorted(registry_stamps)
                            ]
                            if (
                                not all(exclusive_registries)
                                or not references_unchanged()
                            ):
                                results.append(
                                    {**item, "reason": "coordination-changed"}
                                )
                                continue
                            archive_root = (
                                task_artifacts_dir(
                                    repo_root, record.task, environment
                                ).parent
                                / "archive"
                            )
                            archive = compact_evidence(
                                task, archive_root, kind="lane-evidence", keep=kept
                            )
                            item["retired_path"] = str(archive)
                            registry_stamps[log_root] = log_root.stat().st_mtime_ns
                            record_stamps[record.path] = _record_signature(record.path)
                    results.append(item)
            except WalkIncomplete as incomplete:
                results.append({**item, "reason": incomplete.reason})
            except (OSError, ValueError, subprocess.SubprocessError) as exc:
                results.append({**item, "reason": str(exc), "error": True})
                if git_failed or isinstance(exc, TimeoutError):
                    break
    finally:
        if stream is not None:
            assert stream.stdin is not None and stream.stdout is not None
            stream.stdin.close()
            stream.stdout.close()
            if (returncode := stream.wait(timeout=10)) != 0:
                results.append(
                    {
                        "path": str(log_root),
                        "kind": "agent-evidence",
                        "reason": f"repository blob check failed (exit {returncode})",
                        "error": True,
                    }
                )
    return results
