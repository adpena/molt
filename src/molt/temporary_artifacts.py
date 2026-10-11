"""Guard-owned scratch: terminal receipts, never age or PID, permit deletion.

The parent allocates and binds the target before exposing it to the child.
The parent guard publishes terminal evidence after process-tree closure. When
a guard dies or cannot prove closure, marker reconciliation proves it later
and hands that evidence to ``resolve_guard_scratch``.
Owner/terminal records and the OS lock live outside the deletable target.
A reclaimed generation holds no custody, so its receipts are removed: the
generation namespace holds only live, retained, blocked or unresolved work.
"""

from __future__ import annotations

from collections.abc import Generator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
import errno
import os
import re
import reprlib
import secrets
import stat
import tempfile
import time
import warnings
import weakref

from molt.exact_json import (
    canonical_json_sha256,
    encode_exact,
    read_exact,
    write_exact,
)
from molt.file_deletion import delete_path
from molt.file_locks import (
    _FileLockHandle,
    _try_acquire_file_lock,
    _release_file_lock,
    _file_lock_owned_operation,
)
from molt.file_publication import (
    durable_namespace_publish_directory_exclusive,
    durable_publish_exclusive,
    is_link_like,
    namespace_move_exclusive,
    resolve_owned_path,
)
from molt.disk_capacity import require_scratch_capacity
from molt.dx import scratch_dir
from molt.memory_guard_paths import STATE_ROOT_ENV, memory_guard_state_root


SCHEMA = "molt.guard-scratch.v1"
_TOKEN = re.compile(r"[0-9a-f]{32}")
_TARGET_NAME = re.compile(r"pt-[a-z0-9_]{8}")
_MAX_RECEIPT_BYTES = 65536
SCRATCH_ENV = "MOLT_GUARD_SCRATCH_ROOT"
# The generation namespace. A lease target is its sibling:
# <scratch>/gs/<token> owns <scratch>/pt-*.
_ROOT_DIRNAME = "gs"
# Reclaimed generations move here before their receipts are deleted.
_REMOVING_DIRNAME = "removing"
# Allocation receipt inside a lease target. A file identity alone cannot
# tell a replacement directory from the original: Linux reuses a freed inode
# number. The receipt's nonce, recorded in the owner, can.
_TARGET_RECEIPT = ".molt-scratch-target.json"


class ScratchBusy(RuntimeError):
    """An active owner or another reclaimer holds the generation lock."""


@dataclass(frozen=True, slots=True)
class ScratchRetention:
    """A newest-first retention bound: a maximum entry count and byte total.

    Guard scratch and proof-queue run evidence share this rule. A consumer
    visits entries newest first and keeps each one that `admits` allows.
    """

    count: int = 3
    bytes: int = 2 * 1024**3

    def __post_init__(self) -> None:
        if any(
            type(value) is not int or value < 0 for value in (self.count, self.bytes)
        ):
            raise ValueError("scratch retention limits must be nonnegative integers")

    def admits(self, *, kept_count: int, kept_bytes: int, size: int) -> bool:
        """Keep the next entry only while both bounds still hold."""
        return kept_count < self.count and kept_bytes + size <= self.bytes


@dataclass(frozen=True, slots=True)
class _ScratchTerminal:
    digest: str
    finished_ns: int
    retained_bytes: int
    success: bool


@dataclass(slots=True)
class GuardScratchLease:
    generation: Path
    target: Path
    owner: dict[str, object]
    lock: _FileLockHandle | None
    # Set by the owner's finish when its generation reached reclaimed.
    reclaimed: bool = False

    def release(self) -> None:
        if self.lock is not None:
            _release_file_lock(self.lock)
            self.lock = None


def scratch_root(repo_root: Path, environ: Mapping[str, str]) -> Path:
    # A short root preserves Windows compiler/linker path budget. A
    # queue-issued guard state root carries its scratch beside it; otherwise
    # guard scratch is run scratch and follows the selected scratch storage.
    if environ.get(STATE_ROOT_ENV, "").strip():
        return resolve_owned_path(
            memory_guard_state_root(repo_root, environ).parent / _ROOT_DIRNAME
        )
    return resolve_owned_path(scratch_dir(repo_root, _ROOT_DIRNAME, environ))


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
def _locked(generation: Path) -> Generator[None]:
    """Hold one existing generation's lock; never create or revive custody.

    A remover moves a reclaimed generation away without its lock. Opening
    the lock must not create the generation again, and a generation that
    moved between the check and the lock is gone.
    """
    resolve_owned_path(generation)
    identity = _identity(generation)
    handle = _try_acquire_file_lock(
        resolve_owned_path(generation / "lock"), create=False
    )
    if handle is None:
        raise ScratchBusy(f"scratch generation is busy: {generation}")
    try:
        resolve_owned_path(generation)
        if _identity(generation) != identity:
            raise FileNotFoundError(
                errno.ENOENT, "scratch generation was removed", str(generation)
            )
        yield
    finally:
        _release_file_lock(handle)


def _owner_mismatch(
    generation: Path, field: str, expected: object, observed: object
) -> ValueError:
    formatter = reprlib.Repr()
    formatter.maxstring = 512

    def display(value: str) -> str:
        # Diagnostics must not echo custody tokens or unbounded child metadata.
        return formatter.repr(re.sub(r"[0-9a-f]{32}", "<token>", value, flags=re.I))

    details = []
    for label, value in (("expected", expected), ("observed", observed)):
        detail = f"{label}_type={type(value).__name__}"
        if isinstance(value, str):
            if field == "token":
                detail += f" {label}=<redacted,length={len(value)}>"
            else:
                detail += f" {label}={display(value)}"
        details.append(detail)
    return ValueError(
        f"scratch owner mismatch: {display(str(generation))}; field={field}; "
        + "; ".join(details)
    )


def _same_owned_path(expected: object, observed: object) -> bool:
    return (
        isinstance(expected, str)
        and isinstance(observed, str)
        and resolve_owned_path(Path(expected)) == resolve_owned_path(Path(observed))
    )


def _owner(generation: Path) -> dict[str, object]:
    value = read_exact(
        resolve_owned_path(generation / "owner.json"),
        max_bytes=_MAX_RECEIPT_BYTES,
        label="scratch owner",
    )
    if not isinstance(value, dict):
        raise _owner_mismatch(generation, "owner", {}, value)
    for field, expected in (
        ("schema", SCHEMA),
        ("token", generation.name),
        ("generation", str(generation)),
    ):
        observed = value.get(field)
        matches = (
            _same_owned_path(expected, observed)
            if field == "generation"
            else observed == expected
        )
        if not matches:
            raise _owner_mismatch(generation, field, expected, observed)
    states = {
        "leased",
        "indeterminate",
        "retiring",
        "retained",
        "reclaiming",
        "reclaimed",
        "blocked",
    }
    state = value.get("state")
    if not isinstance(state, str) or state not in states:
        raise _owner_mismatch(
            generation, "state", "one of " + ", ".join(sorted(states)), state
        )
    if not isinstance(value.get("target_identity"), dict):
        raise _owner_mismatch(
            generation, "target_identity", {}, value.get("target_identity")
        )
    return value


def windows_temporary_directory_mode(mode: int) -> int:
    """Retain inherited Windows ACLs instead of installing a user-only DACL."""
    return 0o755 if mode == 0o700 else mode


def new_temporary_directory(root: Path, *, prefix: str = "tmp") -> Path:
    """Atomically allocate under admitted custody, preserving Windows inherited ACLs."""
    if not isinstance(prefix, str) or any(
        value in prefix for value in ("/", "\\", "\0")
    ):
        raise ValueError("temporary directory prefix must be a confined basename")
    root = root.resolve(strict=True)
    mode = windows_temporary_directory_mode(0o700) if os.name == "nt" else 0o700
    for _attempt in range(100):
        name = "".join(
            secrets.choice("abcdefghijklmnopqrstuvwxyz0123456789_") for _ in range(8)
        )
        path = root / (prefix + name)
        try:
            path.mkdir(mode=mode)
        except FileExistsError:
            continue
        return path
    raise FileExistsError("cannot allocate a unique guard-owned scratch directory")


def _cleanup_temporary_directory(
    path: Path, identity: dict[str, int], *, warn: bool = False
) -> None:
    try:
        current = _identity(path)
    except FileNotFoundError:
        return
    if current != identity:
        raise ValueError("temporary directory allocation changed before cleanup")
    removed, error = delete_path(path)
    if not removed:
        raise OSError(f"cannot clean owned temporary directory {path}: {error}")
    if warn:
        warnings.warn(
            f"Implicitly cleaning up owned temporary directory {path}", ResourceWarning
        )


class OwnedTemporaryDirectory:
    """One host-correct directory allocation and its identity-fenced lifetime."""

    def __init__(self, *, prefix: str = "tmp", dir: str | Path | None = None) -> None:
        self._path = new_temporary_directory(
            Path(tempfile.gettempdir()) if dir is None else Path(dir), prefix=prefix
        )
        self.name = str(self._path)
        self._identity = _identity(self._path)
        self._finalizer = weakref.finalize(
            self, _cleanup_temporary_directory, self._path, self._identity, warn=True
        )

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.cleanup()

    def cleanup(self) -> None:
        self._finalizer.detach()
        _cleanup_temporary_directory(self._path, self._identity)


def acquire_guard_scratch(
    repo_root: Path, environ: Mapping[str, str]
) -> GuardScratchLease:
    """Parent-only allocation; never recover ownership from child-writable data.

    Admission runs first: the run does not start unless the scratch volume
    has the scratch budget free.
    """
    token = environ["MOLT_MEMORY_GUARD_TOKEN"]
    marker = resolve_owned_path(Path(environ["MOLT_MEMORY_GUARD_MARKER"]))
    root = scratch_root(repo_root, environ)
    require_scratch_capacity([root], env=environ)
    generation = _generation(root, token)
    generation.mkdir(parents=True)
    handle = _try_acquire_file_lock(resolve_owned_path(generation / "lock"))
    if handle is None:
        raise RuntimeError("new scratch generation unexpectedly locked")
    target: Path | None = None
    try:
        target = new_temporary_directory(generation.parent.parent, prefix="pt-")
        nonce = secrets.token_hex(16)
        _write_target_receipt(target, nonce)
        owner = {
            "schema": SCHEMA,
            "token": token,
            "generation": str(generation),
            "guard_marker": str(marker),
            "target": str(target),
            "target_identity": _identity(target),
            "target_receipt": nonce,
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


def _write_target_receipt(target: Path, nonce: str) -> None:
    """Write the allocation receipt with no durability barrier.

    A crash that loses or truncates it only blocks adoption of this target,
    which fails closed; every guarded launch is spared an fsync.
    """
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    with os.fdopen(os.open(target / _TARGET_RECEIPT, flags, 0o600), "wb") as stream:
        stream.write(encode_exact({"schema": SCHEMA, "nonce": nonce}))


def _generation_of_target(target: Path, token: str) -> Path:
    """Return the generation that owns a lease target (``acquire_guard_scratch``)."""
    return _generation(resolve_owned_path(target.parent / _ROOT_DIRNAME), token)


def guard_scratch(environ: Mapping[str, str]) -> Path:
    """Consume the parent's allocation; child metadata never grants deletion.

    The generation comes from the allocation this process inherited, its
    lease target and guard token, never from a state root. A process may
    point the guards it starts at another state root (the test session
    does); that cannot move the lease it already holds.
    """
    raw_target = environ.get(SCRATCH_ENV, "").strip()
    if not raw_target:
        raise ValueError("scratch is not the active parent's allocation")
    generation = _generation_of_target(
        Path(raw_target), environ.get("MOLT_MEMORY_GUARD_TOKEN", "")
    )
    owner = _owner(generation)
    target = _target(generation, owner)
    if (
        owner["state"] != "leased"
        or not _same_owned_path(
            owner.get("guard_marker"), environ.get("MOLT_MEMORY_GUARD_MARKER")
        )
        or not _same_owned_path(str(target), environ.get(SCRATCH_ENV))
        or _identity(target) != owner["target_identity"]
    ):
        raise ValueError("scratch is not the active parent's allocation")
    return target


def tree_bytes(target: Path) -> int:
    """Sum regular-file bytes below one directory; never follow a link.

    A scratch allocation receipt is custody, not payload, so it never counts.
    """
    total = 0
    receipt = target / _TARGET_RECEIPT
    stack = [target]
    while stack:
        with os.scandir(stack.pop()) as entries:
            for entry in entries:
                path = Path(entry.path)
                if is_link_like(path) or path == receipt:
                    continue
                if entry.is_dir(follow_symlinks=False):
                    stack.append(path)
                else:
                    total += entry.stat(follow_symlinks=False).st_size
    return total


def _terminal(generation: Path, owner: Mapping[str, object]) -> _ScratchTerminal:
    _target(generation, owner)
    terminal = read_exact(
        resolve_owned_path(generation / "terminal.json"),
        max_bytes=_MAX_RECEIPT_BYTES,
        label="scratch terminal",
    )
    if not isinstance(terminal, dict):
        raise ValueError("scratch terminal receipt does not authorize reclamation")
    digest = canonical_json_sha256(terminal)
    success = terminal.get("success")
    finished_ns = terminal.get("finished_ns")
    retained_bytes = terminal.get("retained_bytes")
    if (
        terminal.get("schema") != SCHEMA
        or terminal.get("token") != owner["token"]
        or terminal.get("target_identity") != owner["target_identity"]
        or not _same_owned_path(terminal.get("target"), owner.get("target"))
        or not _same_owned_path(str(generation), terminal.get("generation"))
        or terminal.get("closed") is not True
        or type(success) is not bool
        or type(finished_ns) is not int
        or finished_ns < 0
        or type(retained_bytes) is not int
        or retained_bytes < 0
        or digest != owner.get("terminal_digest")
    ):
        raise ValueError("scratch terminal receipt does not authorize reclamation")
    return _ScratchTerminal(
        digest=digest,
        finished_ns=finished_ns,
        retained_bytes=retained_bytes,
        success=success,
    )


def _index_path(generation: Path) -> Path:
    return resolve_owned_path(
        generation.parent / "pending" / (generation.name + ".json")
    )


def _drop_index(generation: Path) -> None:
    _index_path(generation).unlink(missing_ok=True)


def _publish_index(generation: Path, terminal_digest: str) -> None:
    # The shared discovery namespace contains committed entries only. Atomic
    # JSON's private write stages belong behind this generation's held lock,
    # never beside entries that another guard may be enumerating.
    staged = resolve_owned_path(generation / "pending.json")
    write_exact(
        staged,
        {"schema": SCHEMA, "terminal_digest": terminal_digest},
        exclusive=True,
    )
    destination = _index_path(generation)
    destination.parent.mkdir(parents=True, exist_ok=True)
    durable_publish_exclusive(staged, _index_path(generation))


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


def _block_uncommitted_retirement_locked(
    generation: Path, owner: dict[str, object], indexed_digest: object
) -> dict[str, object]:
    """Resolve a pending index whose owner never adopted its terminal receipt.

    Retirement publishes the index before the owner records the terminal
    digest, so an interruption between the two leaves a leased (or
    indeterminate) owner beside an index naming a terminal it never adopted.
    Verify that terminal against the retiring owner the finisher would have
    committed, then record the transition as blocked with the payload
    preserved and retire the index: the sweep reports the interruption once
    instead of failing every later guarded command on the host.
    """
    committed = {
        key: value for key, value in owner.items() if key not in {"error", "closure"}
    }
    committed.update(
        target=str(resolve_owned_path(generation / "payload")),
        state="retiring",
        terminal_digest=indexed_digest,
    )
    _terminal(generation, committed)
    blocked = {
        **committed,
        "state": "blocked",
        "error": "interrupted before the terminal owner commit; payload preserved without retry",
    }
    write_exact(generation / "owner.json", blocked)
    _drop_index(generation)
    return blocked


def _reclaim_locked(generation: Path, owner: dict[str, object]) -> dict[str, object]:
    """Delete one retained payload; the caller removes a reclaimed generation.

    The pending index stays until ``_remove_reclaimed_generation`` commits, so
    a removal that cannot run now is found again by the next sweep.
    """
    if owner["state"] == "reclaimed":
        _terminal(generation, owner)
        if _target(generation, owner).exists():
            raise ValueError("reclaimed scratch target unexpectedly exists")
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
    if not ok:
        _drop_index(generation)
    return owner


def _remove_reclaimed_generation(generation: Path) -> str | None:
    """Remove the receipts of one reclaimed generation; return a deferral.

    Call without the generation lock, after the locked authority verified
    ``reclaimed``. That state is final, so no transition can race this one.
    The atomic move into ``removing/`` ends the generation for every reader:
    a contender that locked it just before finds it absent, which means
    removed. Windows cannot move a directory while a contender holds its lock
    file open. The pending index then stays, and the next sweep removes it.
    """
    removing = resolve_owned_path(generation.parent / _REMOVING_DIRNAME)
    removing.mkdir(exist_ok=True)
    tombstone = removing / generation.name
    try:
        # No durability barrier: a crash that rolls the move back leaves a
        # reclaimed generation with its index, which the next sweep removes.
        namespace_move_exclusive(resolve_owned_path(generation), tombstone)
    except (OSError, ValueError) as error:
        if os.path.lexists(generation):
            return f"{generation}: reclaimed receipt removal deferred: {error}"
        # Another remover committed this final transition and owns the
        # tombstone; it also drops the index.
        return None
    _drop_index(generation)
    ok, error = delete_path(tombstone)
    if not ok:
        raise OSError(f"reclaimed scratch receipts remain at {tombstone}: {error}")
    return None


def _retire_locked(
    generation: Path,
    owner: dict[str, object],
    target: Path,
    *,
    success: bool,
    closure: Mapping[str, object],
    finished_ns: int,
) -> dict[str, object]:
    """Move a closed run's payload into its generation and index the work."""
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
        "closure": dict(closure),
        "finished_ns": finished_ns,
        "retained_bytes": 0 if success else tree_bytes(target),
    }
    write_exact(generation / "terminal.json", terminal, exclusive=True)
    owner = {
        **owner,
        "target": str(destination),
        "state": "retiring",
        "terminal_digest": canonical_json_sha256(terminal),
    }
    # The pending-work index is a projection, not deletion authority.
    # Index first: interruption must not leave a retired payload undiscoverable.
    _publish_index(generation, owner["terminal_digest"])
    write_exact(generation / "owner.json", owner)
    try:
        # Both resolved absolute paths are bound to this allocation and
        # its exact generation before any recursive move/deletion occurs.
        durable_namespace_publish_directory_exclusive(target, destination)
    except OSError as error:
        owner = {**owner, "state": "blocked", "error": str(error)}
        write_exact(generation / "owner.json", owner)
        _drop_index(generation)
        return owner
    owner = {**owner, "state": "retained"}
    write_exact(generation / "owner.json", owner)
    return owner


def _holds_target_receipt(target: Path, owner: Mapping[str, object]) -> bool:
    """The target still holds the receipt its allocation wrote.

    An owner from before receipts existed names none; its recorded file
    identity is then the only identity it has.
    """
    nonce = owner.get("target_receipt")
    if nonce is None:
        return True
    try:
        receipt = read_exact(
            resolve_owned_path(target / _TARGET_RECEIPT),
            max_bytes=_MAX_RECEIPT_BYTES,
            label="scratch target receipt",
        )
    except (OSError, ValueError):
        return False
    return receipt == {"schema": SCHEMA, "nonce": nonce}


def _adopt_locked(
    generation: Path, owner: dict[str, object], closure: Mapping[str, object]
) -> dict[str, object]:
    """Terminalize a dead guard's lease from the caller's closure proof.

    The payload takes the owner's own failure path. It records no finish
    time (``finished_ns`` 0), so retention keeps it only behind real failures.
    A payload that is already gone resolves with a receipt that says so. A
    target that is now another directory stays blocked and is reported.
    """
    target = _target(generation, owner)
    destination = resolve_owned_path(generation / "payload")
    try:
        identity = _identity(target)
    except FileNotFoundError:
        identity = None
    if identity is None:
        terminal = {
            "schema": SCHEMA,
            "token": owner["token"],
            "generation": str(generation),
            "target_identity": owner["target_identity"],
            "target": str(destination),
            "source_target": str(target),
            "source_target_absent": True,
            "closed": True,
            "success": False,
            "closure": dict(closure),
            "finished_ns": 0,
            "retained_bytes": 0,
        }
        write_exact(generation / "terminal.json", terminal, exclusive=True)
        owner = {
            **owner,
            "target": str(destination),
            "state": "reclaimed",
            "terminal_digest": canonical_json_sha256(terminal),
            "error": None,
        }
        write_exact(generation / "owner.json", owner)
        return owner
    if identity != owner["target_identity"] or not _holds_target_receipt(target, owner):
        owner = {
            **owner,
            "state": "blocked",
            "error": "abandoned scratch target is another directory; preserved",
        }
        write_exact(generation / "owner.json", owner)
        return owner
    return _retire_locked(
        generation, owner, target, success=False, closure=closure, finished_ns=0
    )


def scratch_generation(token: str, outcome: Mapping[str, object]) -> Path | None:
    """Return the generation that one guard's scratch outcome names.

    A finished outcome names a receipt inside its generation. A live lease
    names only its target, and ``acquire_guard_scratch`` places that target
    beside the generation namespace. An outcome with neither holds no
    unresolved scratch: a reclaimed generation is already removed.
    """
    receipt = outcome.get("receipt")
    if isinstance(receipt, str):
        return resolve_owned_path(Path(receipt).parent)
    target = outcome.get("target")
    if isinstance(target, str):
        return _generation_of_target(Path(target), token)
    return None


def resolve_guard_scratch(
    generation: Path,
    *,
    guard_marker: Path,
    closure: Mapping[str, object] | None,
) -> dict[str, object]:
    """Resolve one finished guard's scratch generation through this authority.

    ``closure`` is the caller's proof that no process of the guard's run
    remains, or None without that proof. A leased or indeterminate payload
    is adopted only with a proof; a busy lock means a live owner. Retained,
    blocked and interrupted generations belong to the pending sweep, so they
    count as resolved here. ``resolved`` False keeps the guard marker active.
    """
    generation = resolve_owned_path(generation)
    if not os.path.lexists(generation):
        return {"generation": str(generation), "state": "absent", "resolved": True}
    remove = False
    try:
        with _locked(generation):
            owner = _owner(generation)
            if not _same_owned_path(owner.get("guard_marker"), str(guard_marker)):
                raise ValueError("scratch generation belongs to another guard marker")
            if owner["state"] in {"leased", "indeterminate"}:
                if closure is None:
                    return {
                        "generation": str(generation),
                        "state": owner["state"],
                        "resolved": False,
                    }
                owner = _adopt_locked(generation, owner, closure)
            if owner["state"] == "reclaimed":
                _reclaim_locked(generation, owner)
                remove = True
            state = owner["state"]
    except ScratchBusy:
        return {"generation": str(generation), "state": "busy", "resolved": False}
    except (OSError, ValueError, RuntimeError) as error:
        if not os.path.lexists(generation):
            return {"generation": str(generation), "state": "absent", "resolved": True}
        return {
            "generation": str(generation),
            "state": "error",
            "resolved": False,
            "error": str(error),
        }
    outcome: dict[str, object] = {
        "generation": str(generation),
        "state": state,
        "resolved": True,
    }
    if remove:
        try:
            outcome["deferred"] = _remove_reclaimed_generation(generation)
        except OSError as error:
            outcome["error"] = str(error)
    return outcome


def inspect_guard_scratch(generation: Path) -> str:
    """Read one generation's state without its lock, for dry runs only."""
    try:
        generation = resolve_owned_path(generation)
        if not os.path.lexists(generation):
            return "absent"
        return str(_owner(generation)["state"])
    except (OSError, ValueError) as error:
        return f"error: {error}"


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
    handle = lease.lock
    if handle is None:
        raise ValueError(
            "scratch parent has released its lease or lacks live ownership"
        )
    entered = False
    try:
        with _file_lock_owned_operation(handle, expected_lock_path=generation / "lock"):
            entered = True
            early = _finish_guard_scratch_owned(
                lease, closed=closed, success=success, evidence=evidence
            )
    finally:
        if entered:
            lease.release()
    if early is not None:
        return early
    removal: tuple[str, str] | None = None
    if lease.reclaimed:
        # The owner verified reclaimed under its own lock, and that state is
        # final: remove the receipts now instead of through a sweep's lock.
        try:
            deferral = _remove_reclaimed_generation(generation)
        except OSError as error:
            removal = ("errors", str(error))
        else:
            removal = None if deferral is None else ("deferred", deferral)
    sweep = reclaim_terminal_scratch(root, retention=retention)
    if removal is not None:
        entries = sweep[removal[0]]
        assert isinstance(entries, list)
        entries.append(removal[1])
    try:
        final_owner = _owner(generation)
    except (OSError, ValueError):
        if os.path.lexists(generation):
            raise
        # The generation reached reclaimed and its receipts were removed,
        # by this sweep or a concurrent one under the same final transition.
        return {
            "state": "reclaimed",
            "receipt": None,
            "error": None,
            "retention": sweep,
        }
    return {
        "state": final_owner["state"],
        "receipt": str(generation / "owner.json"),
        "error": final_owner.get("error"),
        "retention": sweep,
    }


def _finish_guard_scratch_owned(
    lease: GuardScratchLease,
    *,
    closed: bool,
    success: bool,
    evidence: Mapping[str, object],
) -> dict[str, object] | None:
    """Terminal publication/reclamation while the caller pins exact custody."""
    generation = lease.generation
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
        owner = _retire_locked(
            generation,
            owner,
            target,
            success=success,
            closure=evidence,
            finished_ns=time.time_ns(),
        )
        if owner["state"] == "blocked":
            return {
                "state": "blocked",
                "receipt": str(generation / "owner.json"),
                "error": owner.get("error"),
            }
        if success:
            lease.reclaimed = _reclaim_locked(generation, owner)["state"] == "reclaimed"
        return None
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


def reclaim_terminal_scratch(
    root: Path, *, retention: ScratchRetention = ScratchRetention()
) -> dict[str, object]:
    """Bound completed failure scratch; legacy, active and blocked data stay put.

    A generation whose directory is gone was removed by its own reclaimed
    transition; its index entry is resolved, not an error.
    """
    root = resolve_owned_path(root)
    pending = resolve_owned_path(root / "pending")
    if not pending.exists():
        return {
            "reclaimed": [],
            "retained_bytes": 0,
            "retained_count": 0,
            "protected_count": 0,
            "errors": [],
            "deferred": [],
        }
    candidates: list[tuple[int, int, Path, str, bool]] = []
    errors: list[str] = []
    deferred: list[str] = []
    removals: list[Path] = []
    protected_count = 0
    for entry in sorted(pending.iterdir()):
        if entry.suffix != ".json" or _TOKEN.fullmatch(entry.stem) is None:
            errors.append(f"{entry}: invalid scratch pending entry")
            continue
        generation: Path | None = None
        try:
            generation = _generation(root, entry.stem)
            with _locked(generation):
                owner = _owner(generation)
                # A directory snapshot is discovery, not a lease on an index.
                # Its owner may have completed reclamation since enumeration.
                # Accept that transition only from the locked, verified terminal
                # authority; a missing retained owner's index is still an error.
                if owner["state"] == "reclaimed":
                    _reclaim_locked(generation, owner)
                    removals.append(generation)
                    continue
                if owner["state"] == "blocked":
                    errors.append(f"{generation}: {owner.get('error')}")
                    _drop_index(generation)
                    continue
                index = read_exact(
                    resolve_owned_path(entry),
                    max_bytes=_MAX_RECEIPT_BYTES,
                    label="scratch pending index",
                )
                if (
                    isinstance(index, dict)
                    and index.get("schema") == SCHEMA
                    and "terminal_digest" not in owner
                    and owner["state"] in {"leased", "indeterminate"}
                ):
                    owner = _block_uncommitted_retirement_locked(
                        generation, owner, index.get("terminal_digest")
                    )
                    errors.append(f"{generation}: {owner.get('error')}")
                    continue
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
                        _drop_index(generation)
                    if owner["state"] == "reclaimed":
                        removals.append(generation)
                if owner["state"] != "retained":
                    continue
                terminal = _terminal(generation, owner)
                if _identity(_target(generation, owner)) != owner["target_identity"]:
                    raise ValueError("retained scratch payload identity changed")
                candidates.append(
                    (
                        terminal.finished_ns,
                        terminal.retained_bytes,
                        generation,
                        terminal.digest,
                        terminal.success,
                    )
                )
        except ScratchBusy:
            protected_count += 1
        except (OSError, ValueError, RuntimeError) as error:
            if generation is not None and not os.path.lexists(generation):
                _drop_index(generation)
                continue
            errors.append(f"{entry}: {error}")
    kept_bytes = 0
    kept_count = 0
    reclaimed: list[str] = []
    for _, size, generation, digest, success in sorted(candidates, reverse=True):
        try:
            with _locked(generation):
                owner = _owner(generation)
                if owner.get("terminal_digest") != digest:
                    raise ValueError("scratch terminal generation changed")
                if owner["state"] == "reclaimed":
                    _reclaim_locked(generation, owner)
                    removals.append(generation)
                    continue
                if (
                    owner["state"] == "retained"
                    and not success
                    and retention.admits(
                        kept_count=kept_count, kept_bytes=kept_bytes, size=size
                    )
                ):
                    _terminal(generation, owner)
                    if (
                        _identity(_target(generation, owner))
                        != owner["target_identity"]
                    ):
                        raise ValueError("retained scratch payload identity changed")
                    kept_count += 1
                    kept_bytes += size
                    continue
                result = _reclaim_locked(generation, owner)
                if result["state"] == "reclaimed":
                    reclaimed.append(str(generation))
                    removals.append(generation)
                else:
                    errors.append(f"{generation}: {result.get('error')}")
        except ScratchBusy:
            protected_count += 1
        except (OSError, ValueError, RuntimeError) as error:
            if not os.path.lexists(generation):
                continue
            errors.append(f"{generation}: {error}")
    # Remove receipts only after each generation lock is released: Windows
    # cannot move a directory while this process holds a file inside it.
    for generation in removals:
        try:
            deferral = _remove_reclaimed_generation(generation)
        except OSError as error:
            errors.append(str(error))
            continue
        if deferral is not None:
            deferred.append(deferral)
    return {
        "reclaimed": reclaimed,
        "retained_bytes": kept_bytes,
        "retained_count": kept_count,
        "protected_count": protected_count,
        "errors": errors,
        "deferred": deferred,
    }


def new_guarded_directory(environ: Mapping[str, str], *, prefix: str) -> Path:
    """Allocate a helper subtree; the outer guard owns its terminal cleanup."""
    if re.fullmatch(r"[A-Za-z0-9_.-]{1,48}", prefix) is None:
        raise ValueError("scratch prefix must be a short basename")
    root = guard_scratch(environ)
    path = new_temporary_directory(root, prefix=prefix)
    if resolve_owned_path(path).parent != root:
        raise ValueError("scratch helper escaped its owning allocation")
    return path
