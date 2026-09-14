"""Guard-owned scratch: terminal receipts, never age or PID, permit deletion.

The parent allocates and binds the target before exposing it to the child.
The parent guard alone publishes terminal evidence after process-tree closure.
Owner/terminal records and the OS lock live outside the deletable target.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
import os
import re
import stat
import tempfile
import time

from molt.exact_json import canonical_json_sha256, read_exact, write_exact
from molt.file_deletion import delete_path
from molt.file_locks import _FileLockHandle, _try_acquire_file_lock, _release_file_lock
from molt.file_publication import (
    durable_namespace_publish_directory_exclusive,
    is_link_like,
    resolve_owned_path,
)
from molt.memory_guard_paths import memory_guard_state_root


SCHEMA = "molt.guard-scratch.v1"
_TOKEN = re.compile(r"[0-9a-f]{32}")
_TARGET_NAME = re.compile(r"pt-[a-z0-9_]{8}")
_MAX_RECEIPT_BYTES = 65536
SCRATCH_ENV = "MOLT_GUARD_SCRATCH_ROOT"


class ScratchBusy(RuntimeError):
    """An active owner or another reclaimer holds the generation lock."""


@dataclass(frozen=True, slots=True)
class ScratchRetention:
    count: int = 3
    bytes: int = 2 * 1024**3

    def __post_init__(self) -> None:
        if any(
            type(value) is not int or value < 0 for value in (self.count, self.bytes)
        ):
            raise ValueError("scratch retention limits must be nonnegative integers")


@dataclass(slots=True)
class GuardScratchLease:
    generation: Path
    target: Path
    owner: dict[str, object]
    lock: _FileLockHandle | None

    def release(self) -> None:
        if self.lock is not None:
            _release_file_lock(self.lock)
            self.lock = None


def scratch_root(repo_root: Path, environ: Mapping[str, str]) -> Path:
    # A short root preserves Windows compiler/linker path budget. It follows the
    # existing state-root projection, including proof-queue external roots.
    return resolve_owned_path(memory_guard_state_root(repo_root, environ).parent / "gs")


def _generation(root: Path, token: str) -> Path:
    if _TOKEN.fullmatch(token) is None:
        raise ValueError("scratch requires an exact guard token")
    return resolve_owned_path(root / token)


def _identity(path: Path) -> dict[str, int]:
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or is_link_like(path) or not metadata.st_ino:
        raise ValueError("scratch target must be a direct directory")
    return {"device": metadata.st_dev, "inode": metadata.st_ino}


def _target(generation: Path, owner: Mapping[str, object]) -> Path:
    raw = owner.get("target")
    if not isinstance(raw, str):
        raise ValueError("scratch target path is missing")
    target = resolve_owned_path(Path(raw))
    if owner.get("state") in {"leased", "indeterminate"}:
        if (
            target.parent != generation.parent.parent
            or _TARGET_NAME.fullmatch(target.name) is None
        ):
            raise ValueError("scratch target is outside its exact allocation namespace")
    elif target != generation / "payload":
        raise ValueError("terminal scratch must be inside its own generation")
    return target


@contextmanager
def _locked(generation: Path) -> Iterator[None]:
    resolve_owned_path(generation)
    _identity(generation)
    handle = _try_acquire_file_lock(resolve_owned_path(generation / "lock"))
    if handle is None:
        raise ScratchBusy(f"scratch generation is busy: {generation}")
    try:
        resolve_owned_path(generation)
        yield
    finally:
        _release_file_lock(handle)


def _owner(generation: Path) -> dict[str, object]:
    value = read_exact(
        resolve_owned_path(generation / "owner.json"),
        max_bytes=_MAX_RECEIPT_BYTES,
        label="scratch owner",
    )
    if (
        not isinstance(value, dict)
        or value.get("schema") != SCHEMA
        or value.get("token") != generation.name
        or value.get("generation") != str(generation)
        or value.get("state")
        not in {
            "leased",
            "indeterminate",
            "retiring",
            "retained",
            "reclaiming",
            "reclaimed",
            "blocked",
        }
        or not isinstance(value.get("target_identity"), dict)
    ):
        raise ValueError(f"scratch owner mismatch: {generation}")
    return value


def acquire_guard_scratch(
    repo_root: Path, environ: Mapping[str, str]
) -> GuardScratchLease:
    """Parent-only allocation; never recover ownership from child-writable data."""
    token = environ["MOLT_MEMORY_GUARD_TOKEN"]
    marker = resolve_owned_path(Path(environ["MOLT_MEMORY_GUARD_MARKER"]))
    generation = _generation(scratch_root(repo_root, environ), token)
    generation.mkdir(parents=True)
    handle = _try_acquire_file_lock(resolve_owned_path(generation / "lock"))
    if handle is None:
        raise RuntimeError("new scratch generation unexpectedly locked")
    target: Path | None = None
    try:
        target = Path(tempfile.mkdtemp(prefix="pt-", dir=generation.parent.parent))
        owner = {
            "schema": SCHEMA,
            "token": token,
            "generation": str(generation),
            "guard_marker": str(marker),
            "target": str(target),
            "target_identity": _identity(target),
            "state": "leased",
        }
        write_exact(generation / "owner.json", owner, exclusive=True)
        return GuardScratchLease(generation, target, owner, handle)
    except BaseException as error:
        _release_file_lock(handle)
        error.add_note(
            f"scratch allocation preserved: generation={generation} target={target}"
        )
        raise


def guard_scratch(repo_root: Path, environ: Mapping[str, str]) -> Path:
    """Consume the parent's allocation; child metadata never grants deletion."""
    token = environ.get("MOLT_MEMORY_GUARD_TOKEN", "")
    generation = _generation(scratch_root(repo_root, environ), token)
    owner = _owner(generation)
    target = _target(generation, owner)
    if (
        owner["state"] != "leased"
        or owner.get("guard_marker") != environ.get("MOLT_MEMORY_GUARD_MARKER")
        or str(target) != environ.get(SCRATCH_ENV)
        or _identity(target) != owner["target_identity"]
    ):
        raise ValueError("scratch is not the active parent's allocation")
    return target


def _target_bytes(target: Path) -> int:
    total = 0
    stack = [target]
    while stack:
        with os.scandir(stack.pop()) as entries:
            for entry in entries:
                path = Path(entry.path)
                if is_link_like(path):
                    continue
                if entry.is_dir(follow_symlinks=False):
                    stack.append(path)
                else:
                    total += entry.stat(follow_symlinks=False).st_size
    return total


def _terminal(generation: Path, owner: Mapping[str, object]) -> dict[str, object]:
    _target(generation, owner)
    terminal = read_exact(
        resolve_owned_path(generation / "terminal.json"),
        max_bytes=_MAX_RECEIPT_BYTES,
        label="scratch terminal",
    )
    if (
        not isinstance(terminal, dict)
        or terminal.get("schema") != SCHEMA
        or terminal.get("token") != owner["token"]
        or terminal.get("target_identity") != owner["target_identity"]
        or terminal.get("target") != owner.get("target")
        or terminal.get("generation") != str(generation)
        or terminal.get("closed") is not True
        or type(terminal.get("success")) is not bool
        or type(terminal.get("finished_ns")) is not int
        or terminal["finished_ns"] < 0
        or type(terminal.get("retained_bytes")) is not int
        or terminal["retained_bytes"] < 0
        or canonical_json_sha256(terminal) != owner.get("terminal_digest")
    ):
        raise ValueError("scratch terminal receipt does not authorize reclamation")
    return terminal


def _index_path(generation: Path) -> Path:
    return resolve_owned_path(
        generation.parent / "pending" / (generation.name + ".json")
    )


def _drop_index(generation: Path) -> None:
    _index_path(generation).unlink(missing_ok=True)


def _recover_transition_locked(
    generation: Path, owner: dict[str, object]
) -> dict[str, object]:
    if owner["state"] in {"retiring", "reclaiming"}:
        _terminal(generation, owner)
        target = _target(generation, owner)
        if owner["state"] == "reclaiming" and not target.exists():
            owner = {**owner, "state": "reclaimed", "error": None}
        elif (
            owner["state"] == "retiring"
            and target.exists()
            and _identity(target) == owner["target_identity"]
        ):
            owner = {**owner, "state": "retained"}
        else:
            owner = {
                **owner,
                "state": "blocked",
                "error": "interrupted scratch transition; payload preserved without retry",
            }
        write_exact(generation / "owner.json", owner)
    return owner


def _reclaim_locked(generation: Path, owner: dict[str, object]) -> dict[str, object]:
    if owner["state"] == "reclaimed":
        _terminal(generation, owner)
        if _target(generation, owner).exists():
            raise ValueError("reclaimed scratch target unexpectedly exists")
        _drop_index(generation)
        return owner
    if owner["state"] != "retained":
        raise ValueError("scratch is not terminal and reclaimable")
    _terminal(generation, owner)
    target = _target(generation, owner)
    if _identity(target) != owner["target_identity"]:
        raise ValueError("scratch target generation changed before reclamation")
    owner = {**owner, "state": "reclaiming"}
    write_exact(generation / "owner.json", owner)
    ok, error = delete_path(target)
    owner = {**owner, "state": "reclaimed" if ok else "blocked", "error": error or None}
    write_exact(generation / "owner.json", owner)
    _drop_index(generation)
    return owner


def finish_guard_scratch(
    lease: GuardScratchLease,
    *,
    closed: bool,
    success: bool,
    evidence: Mapping[str, object],
    retention: ScratchRetention = ScratchRetention(),
) -> dict[str, object]:
    """Called only by the owning guard after its existing closure boundary."""
    generation = lease.generation
    root = generation.parent
    if lease.lock is None:
        raise ValueError("scratch parent has released its lease")
    try:
        owner = _owner(generation)
        if owner != lease.owner:
            raise ValueError("scratch owner changed from the parent's allocation")
        target = _target(generation, owner)
        if _identity(target) != owner["target_identity"]:
            raise ValueError("scratch target changed before terminal publication")
        if not closed:
            owner = {**owner, "state": "indeterminate", "closure": dict(evidence)}
            write_exact(generation / "owner.json", owner)
            return {"state": "indeterminate", "receipt": str(generation / "owner.json")}
        destination = resolve_owned_path(generation / "payload")
        terminal = {
            "schema": SCHEMA,
            "token": owner["token"],
            "generation": str(generation),
            "target_identity": owner["target_identity"],
            "target": str(destination),
            "source_target": str(target),
            "closed": True,
            "success": success,
            "closure": dict(evidence),
            "finished_ns": time.time_ns(),
            "retained_bytes": 0 if success else _target_bytes(target),
        }
        write_exact(generation / "terminal.json", terminal, exclusive=True)
        owner = {
            **owner,
            "target": str(destination),
            "state": "retiring",
            "terminal_digest": canonical_json_sha256(terminal),
        }
        # The pending-work index is a projection, not deletion authority.
        # Successful-run receipts never enter the next run's discovery walk.
        # Index first: interruption must not leave a retired payload undiscoverable.
        write_exact(
            _index_path(generation),
            {"schema": SCHEMA, "terminal_digest": owner["terminal_digest"]},
            exclusive=True,
        )
        write_exact(generation / "owner.json", owner)
        try:
            # Both resolved absolute paths are bound to this allocation and
            # its exact generation before any recursive move/deletion occurs.
            durable_namespace_publish_directory_exclusive(target, destination)
        except OSError as error:
            owner = {**owner, "state": "blocked", "error": str(error)}
            write_exact(generation / "owner.json", owner)
            _drop_index(generation)
            return {
                "state": "blocked",
                "receipt": str(generation / "owner.json"),
                "error": str(error),
            }
        owner = {**owner, "state": "retained"}
        write_exact(generation / "owner.json", owner)
        if success:
            owner = _reclaim_locked(generation, owner)
    except BaseException as error:
        # Preserve the original error even if storage failure also prevents the
        # diagnostic write. Never overwrite owner metadata changed by the child.
        try:
            write_exact(
                resolve_owned_path(generation / "finish-error.json"),
                {
                    "schema": SCHEMA,
                    "generation": str(generation),
                    "source_target": str(lease.target),
                    "closed": closed,
                    "closure": dict(evidence),
                    "error_type": type(error).__name__,
                    "error": str(error),
                    "finished_ns": time.time_ns(),
                },
                exclusive=True,
            )
            if _owner(generation) == lease.owner:
                write_exact(
                    generation / "owner.json",
                    {**lease.owner, "state": "indeterminate", "error": str(error)},
                )
        except BaseException as receipt_error:
            error.add_note(
                f"scratch failure receipt could not be completed: {receipt_error}"
            )
        error.add_note(f"scratch preserved; evidence: {generation}")
        raise
    finally:
        lease.release()
    sweep = reclaim_terminal_scratch(root, retention=retention)
    final_owner = _owner(generation)
    return {
        "state": final_owner["state"],
        "receipt": str(generation / "owner.json"),
        "error": final_owner.get("error"),
        "retention": sweep,
    }


def reclaim_terminal_scratch(
    root: Path, *, retention: ScratchRetention = ScratchRetention()
) -> dict[str, object]:
    """Bound completed failure scratch; legacy, active and blocked data stay put."""
    root = resolve_owned_path(root)
    pending = resolve_owned_path(root / "pending")
    if not pending.exists():
        return {
            "reclaimed": [],
            "retained_bytes": 0,
            "retained_count": 0,
            "protected_count": 0,
            "errors": [],
        }
    candidates: list[tuple[int, int, Path, str, bool]] = []
    errors: list[str] = []
    protected_count = 0
    for entry in sorted(pending.iterdir()):
        if entry.suffix != ".json" or _TOKEN.fullmatch(entry.stem) is None:
            errors.append(f"{entry}: invalid scratch pending entry")
            continue
        try:
            generation = _generation(root, entry.stem)
            index = read_exact(
                resolve_owned_path(entry),
                max_bytes=_MAX_RECEIPT_BYTES,
                label="scratch pending index",
            )
            with _locked(generation):
                owner = _owner(generation)
                if (
                    not isinstance(index, dict)
                    or index.get("schema") != SCHEMA
                    or index.get("terminal_digest") != owner.get("terminal_digest")
                ):
                    raise ValueError(
                        "scratch pending index differs from terminal owner"
                    )
                if owner["state"] in {"retiring", "reclaiming"}:
                    # Repair only the receipt of an already-completed namespace
                    # transition; never retry an interrupted payload deletion.
                    owner = _recover_transition_locked(generation, owner)
                    if owner["state"] == "blocked":
                        errors.append(f"{generation}: {owner.get('error')}")
                if owner["state"] in {"blocked", "reclaimed"}:
                    _drop_index(generation)
                if owner["state"] != "retained":
                    continue
                terminal = _terminal(generation, owner)
                if _identity(_target(generation, owner)) != owner["target_identity"]:
                    raise ValueError("retained scratch payload identity changed")
                candidates.append(
                    (
                        terminal["finished_ns"],
                        terminal["retained_bytes"],
                        generation,
                        owner["terminal_digest"],
                        terminal["success"],
                    )
                )
        except ScratchBusy:
            protected_count += 1
        except (OSError, ValueError, RuntimeError) as error:
            errors.append(f"{entry}: {error}")
    kept_bytes = 0
    kept_count = 0
    reclaimed: list[str] = []
    for _, size, generation, digest, success in sorted(candidates, reverse=True):
        if (
            not success
            and kept_count < retention.count
            and kept_bytes + size <= retention.bytes
        ):
            kept_count += 1
            kept_bytes += size
            continue
        try:
            with _locked(generation):
                owner = _owner(generation)
                if owner.get("terminal_digest") != digest:
                    raise ValueError("scratch terminal generation changed")
                result = _reclaim_locked(generation, owner)
                if result["state"] == "reclaimed":
                    reclaimed.append(str(generation))
                else:
                    errors.append(f"{generation}: {result.get('error')}")
        except ScratchBusy:
            protected_count += 1
        except (OSError, ValueError, RuntimeError) as error:
            errors.append(f"{generation}: {error}")
    return {
        "reclaimed": reclaimed,
        "retained_bytes": kept_bytes,
        "retained_count": kept_count,
        "protected_count": protected_count,
        "errors": errors,
    }


def new_guarded_directory(
    repo_root: Path, environ: Mapping[str, str], *, prefix: str
) -> Path:
    """Allocate a helper subtree; the outer guard owns its terminal cleanup."""
    if re.fullmatch(r"[A-Za-z0-9_.-]{1,48}", prefix) is None:
        raise ValueError("scratch prefix must be a short basename")
    root = guard_scratch(repo_root, environ)
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=root))
    if resolve_owned_path(path).parent != root:
        raise ValueError("scratch helper escaped its owning allocation")
    return path
