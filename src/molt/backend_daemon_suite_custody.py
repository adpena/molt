"""Suite-owned backend daemon custody; an OS lease and EOF guardian own transfer."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import contextlib
import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
import uuid

from molt import backend_daemon_custody as daemon
from molt.exact_json import canonical_json_bytes, encode_exact, read_exact, write_exact
from molt.file_locks import (
    _FileLockHandle,
    _acquire_file_lock,
    _enter_file_lock_fork_protocol,
    _file_lock_atomic_mutation,
    _leave_file_lock_fork_protocol,
    _try_acquire_file_lock,
    _release_file_lock,
)

LEASE_ENV = "MOLT_BACKEND_DAEMON_SUITE_LEASE"
SCHEMA = "molt.backend_daemon.suite-lease.v1"
_TOKEN = re.compile(r"[0-9a-f]{32}")
_SHA = re.compile(r"[0-9a-f]{64}")


# Suite custody owns pipe descriptors; canonical file_locks owns lock copies.
# Fork children must not keep the owner's EOF pipe alive.
_LEASE_PIPE_FDS: set[int] = set()
_DRAIN_BUDGET_S = 5.0
_LEASE_DESCRIPTOR_MUTEX = threading.Lock()
_LEASE_FORK_LOCAL = threading.local()
_LEASE_CHILD_CUSTODY_ERROR: str | None = None


@contextlib.contextmanager
def _lease_descriptor_mutation():
    if _LEASE_CHILD_CUSTODY_ERROR is not None:
        raise RuntimeError(
            "inherited suite pipe cleanup failed; child custody is unavailable: "
            + _LEASE_CHILD_CUSTODY_ERROR
        )
    with (
        _file_lock_atomic_mutation("suite descriptor mutation"),
        _LEASE_DESCRIPTOR_MUTEX,
    ):
        yield


def _before_lease_fork() -> None:
    _LEASE_FORK_LOCAL.mutex_held = False
    _enter_file_lock_fork_protocol()
    _LEASE_FORK_LOCAL.protocol_entered = True
    _LEASE_DESCRIPTOR_MUTEX.acquire()
    _LEASE_FORK_LOCAL.mutex_held = True


def _after_parent_lease_fork() -> None:
    try:
        if getattr(_LEASE_FORK_LOCAL, "mutex_held", False):
            _LEASE_FORK_LOCAL.mutex_held = False
            _LEASE_DESCRIPTOR_MUTEX.release()
    finally:
        if getattr(_LEASE_FORK_LOCAL, "protocol_entered", False):
            _LEASE_FORK_LOCAL.protocol_entered = False
            _leave_file_lock_fork_protocol()


def _close_inherited_lease_descriptors() -> None:
    global _LEASE_DESCRIPTOR_MUTEX, _LEASE_CHILD_CUSTODY_ERROR, _LEASE_FORK_LOCAL
    # Generic file_locks has already reset child TLS. Mark this callback too:
    # close/profile callbacks must not recursively fork during pipe cleanup.
    _enter_file_lock_fork_protocol()
    errors = [] if _LEASE_CHILD_CUSTODY_ERROR is None else [_LEASE_CHILD_CUSTODY_ERROR]
    try:
        for fd in tuple(_LEASE_PIPE_FDS):
            try:
                os.close(fd)
            except BaseException as exc:
                errors.append(f"pipe {fd}: {type(exc).__name__}: {exc}")
    finally:
        # Never retry an uncertain numeric descriptor: a callback may have
        # closed/reused it. Attempt every copy, reset inherited mutex state,
        # and preserve failed-close authority across nested fork generations.
        _LEASE_PIPE_FDS.clear()
        _LEASE_DESCRIPTOR_MUTEX = threading.Lock()
        _LEASE_FORK_LOCAL = threading.local()
        _LEASE_CHILD_CUSTODY_ERROR = "; ".join(errors) if errors else None
        _leave_file_lock_fork_protocol()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(
        before=_before_lease_fork,
        after_in_parent=_after_parent_lease_fork,
        after_in_child=_close_inherited_lease_descriptors,
    )


def _source_digest() -> str:
    value = hashlib.sha256()
    # This binds custody implementation files, not loaded compiler images.
    from molt import exact_json, file_locks, file_publication
    from tools.memory_guard_core import process_model

    for source in (
        Path(__file__),
        Path(daemon.__file__),
        Path(file_locks.__file__),
        Path(exact_json.__file__),
        Path(file_publication.__file__),
        Path(process_model.__file__),
    ):
        value.update(source.read_bytes())
    return value.hexdigest()


def read_lease(path: Path) -> dict | None:
    if _LEASE_CHILD_CUSTODY_ERROR is not None:
        return None
    try:
        record = read_exact(path, max_bytes=16384, label="daemon suite lease")
    except (OSError, ValueError):
        return None
    expected_keys = {
        "schema",
        "token",
        "state",
        "owner_pid",
        "owner_started_at_ns",
        "guardian_pid",
        "guardian_started_at_ns",
        "project_root",
        "daemon_root",
        "source_digest",
        "interpreter",
        "interpreter_sha256",
    }
    if (
        not isinstance(record, dict)
        or set(record) != expected_keys
        or record.get("schema") != SCHEMA
    ):
        return None
    if not isinstance(record.get("token"), str) or not _TOKEN.fullmatch(
        record["token"]
    ):
        return None
    if path.name != "lease.json" or path.parent.name != record["token"]:
        return None
    for name in (
        "owner_pid",
        "owner_started_at_ns",
        "guardian_pid",
        "guardian_started_at_ns",
    ):
        if type(record.get(name)) is not int or record[name] <= 0:
            return None
    for name in ("project_root", "daemon_root", "source_digest", "interpreter_sha256"):
        if not isinstance(record.get(name), str) or not record[name]:
            return None
    if any(
        not _SHA.fullmatch(record[name])
        for name in ("source_digest", "interpreter_sha256")
    ):
        return None
    if not isinstance(record.get("interpreter"), str) or not record["interpreter"]:
        return None
    if record.get("state") not in {"active", "closing", "closed"}:
        return None
    return record


def live_lease(path: Path, *, project_root: Path, samples: Mapping) -> dict | None:
    """Admit only a held suite lease with matching owner and guardian births."""
    record = read_lease(path)
    if record is None or record["state"] != "active":
        return None
    try:
        if Path(record["project_root"]).resolve() != project_root.resolve():
            return None
        expected = (
            Path(record["daemon_root"])
            / "suite-leases"
            / record["token"]
            / "lease.json"
        )
        if (
            path.resolve() != expected.resolve()
            or record["source_digest"] != _source_digest()
        ):
            return None
        for prefix in ("owner", "guardian"):
            sample = samples.get(record[f"{prefix}_pid"])
            if (
                sample is None
                or sample.started_at_ns != record[f"{prefix}_started_at_ns"]
            ):
                return None
        # Metadata or a live PID alone is insufficient: the OS lease must be held.
        lock_path = path.parent / "owner.lock"
        if not lock_path.is_file():
            return None
        contender = _try_acquire_file_lock(lock_path)
        if contender is not None:
            _release_file_lock(contender)
            return None
    except (OSError, ValueError, RuntimeError):
        return None  # Probe failures never establish held custody.
    return record


def transferable_groups(
    environ: Mapping[str, str],
    *,
    project_root: Path,
    samples: Mapping,
    acknowledge: bool = True,
):
    raw = environ.get(LEASE_ENV)
    if not raw:
        return ()
    path = Path(raw)
    lease = live_lease(path, project_root=project_root, samples=samples)
    if lease is None:
        return ()
    return registered_groups(
        path,
        lease=lease,
        samples=samples,
        daemon_root=daemon.backend_daemon_root_from_env(
            environ, project_root=project_root
        ),
        require_live_leader=True,
        donor_environ=environ,
        acknowledge=acknowledge,
    )


def acknowledge_started_daemon(
    environ: Mapping[str, str], *, project_root: Path, daemon_pid: int
) -> bool:
    """Publish receiver custody before the producer announces readiness.

    An unleased command retains ordinary guard ownership. Windows Job members
    are never exported to this POSIX suite protocol.
    """
    if not environ.get(LEASE_ENV):
        return True
    if os.name != "posix":
        return False
    from tools.memory_guard_core.process_model import sample_processes

    try:
        samples = sample_processes()
        groups = transferable_groups(
            environ, project_root=project_root, samples=samples
        )
        return any(identity.pid == daemon_pid for _lease, identity, _members in groups)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
        return False


ADOPTION_SCHEMA = "molt.backend_daemon.suite-adoption.v1"
ADOPTION_INDEX_SCHEMA = "molt.backend_daemon.suite-adoption-index.v1"
_ADOPTION_BYTES = 262144
_ADOPTION_LOCK_S = 0.5
_ADOPTION_NAME = re.compile(r"[1-9][0-9]*\.[1-9][0-9]*\.identity\.json")


def _new_adoption_index(lease: dict) -> dict:
    return {
        "schema": ADOPTION_INDEX_SCHEMA,
        "lease_token": lease["token"],
        "source_digest": lease["source_digest"],
        "state": "active",
        "receipts": {},
        "pending": {},
    }


def _read_adoption_index(path: Path, *, lease: dict) -> dict:
    index = read_exact(
        path.parent / "adoption-index.json",
        max_bytes=_ADOPTION_BYTES,
        label="suite adoption index",
    )
    if (
        not isinstance(index, dict)
        or set(index)
        != {"schema", "lease_token", "source_digest", "state", "receipts", "pending"}
        or index["schema"] != ADOPTION_INDEX_SCHEMA
        or index["lease_token"] != lease["token"]
        or index["source_digest"] != lease["source_digest"]
        or index.get("state") not in {"active", "closing", "closed"}
        or not isinstance(index["receipts"], dict)
        or not isinstance(index.get("pending"), dict)
    ):
        raise ValueError("suite adoption index does not match receiving lease")
    for name, digest in (*index["receipts"].items(), *index["pending"].items()):
        if (
            not isinstance(name, str)
            or not _ADOPTION_NAME.fullmatch(name)
            or not isinstance(digest, str)
            or not _SHA.fullmatch(digest)
        ):
            raise ValueError("suite adoption index receipt is malformed")
    return index


def _adoption_path(path: Path, identity: daemon.BackendDaemonIdentity) -> Path:
    return (
        path.parent
        / "adoptions"
        / f"{identity.pid}.{identity.started_at_ns}.identity.json"
    )


def _read_adoption(
    path: Path, *, lease_path: Path, lease: dict, digest: str | None = None
):
    payload = read_exact(path, max_bytes=_ADOPTION_BYTES, label="suite adoption")
    if (
        digest is not None
        and hashlib.sha256(canonical_json_bytes(payload)).hexdigest() != digest
    ):
        raise ValueError("suite adoption content differs from acknowledgement")
    identity = daemon.read_backend_daemon_identity(path)
    if (
        not isinstance(payload, dict)
        or identity is None
        or any(
            payload.get(key) != value
            for key, value in daemon.backend_daemon_identity_payload(identity).items()
        )
        or not identity.config_digest
        or not _SHA.fullmatch(identity.config_digest)
        or not identity.backend_sha256
        or not _SHA.fullmatch(identity.backend_sha256)
        or payload.get("adoption_schema") != ADOPTION_SCHEMA
        or payload.get("lease_token") != lease["token"]
        or payload.get("source_digest") != lease["source_digest"]
        or identity.suite_lease != str(lease_path)
        or identity.project_root.resolve() != Path(lease["project_root"]).resolve()
        or path.name != _adoption_path(lease_path, identity).name
    ):
        raise ValueError("suite adoption identity does not match receiving lease")
    members = payload.get("custody_members")
    if not isinstance(members, dict) or not members:
        raise ValueError("suite adoption lacks observed process births")
    parsed = {}
    for raw_pid, born in members.items():
        if (
            not isinstance(raw_pid, str)
            or not raw_pid.isdecimal()
            or str(int(raw_pid)) != raw_pid
            or int(raw_pid) <= 0
            or type(born) is not int
            or born <= 0
        ):
            raise ValueError("suite adoption member birth is malformed")
        parsed[int(raw_pid)] = born
    if parsed.get(identity.pid) != identity.started_at_ns:
        raise ValueError("suite adoption lacks its verified leader birth")
    return daemon.BackendDaemonIdentityRecord(identity=identity, path=path), parsed


def _adoption_identity_valid(identity, *, path: Path, lease: dict) -> bool:
    return (
        type(identity.pid) is int
        and identity.pid > 0
        and type(identity.started_at_ns) is int
        and identity.started_at_ns > 0
        and isinstance(identity.config_digest, str)
        and _SHA.fullmatch(identity.config_digest) is not None
        and isinstance(identity.backend_sha256, str)
        and _SHA.fullmatch(identity.backend_sha256) is not None
        and identity.suite_lease == str(path)
        and identity.project_root.resolve() == Path(lease["project_root"]).resolve()
    )


def _adoption_transaction(path: Path, *, timeout_s: float = _ADOPTION_LOCK_S):
    return _acquire_file_lock(
        path.parent / "adoption-transaction.lock",
        timeout_s=timeout_s,
        timeout_message="suite adoption transaction lock timed out",
    )


def _recover_adoption_index(path: Path, *, lease: dict, index: dict) -> list[str]:
    """Recover only content already committed by the held-lock journal."""
    errors = []
    changed = False
    for name, pending_digest in tuple(index["pending"].items()):
        receipt = path.parent / "adoptions" / name
        try:
            receipt.lstat()  # Preserve explicit absence before codec wraps OS errors.
            payload = read_exact(
                receipt, max_bytes=_ADOPTION_BYTES, label="pending suite adoption"
            )
            digest = hashlib.sha256(canonical_json_bytes(payload)).hexdigest()
            if digest not in {pending_digest, index["receipts"].get(name)}:
                raise ValueError(
                    "pending suite adoption matches neither committed nor staged content"
                )
            _read_adoption(receipt, lease_path=path, lease=lease, digest=digest)
            index["receipts"][name] = digest
            del index["pending"][name]
            changed = True
        except FileNotFoundError as exc:
            if name not in index["receipts"]:
                # This staged new generation was never committed/exported.
                # Missing already-committed evidence remains unresolved.
                del index["pending"][name]
                changed = True
            else:
                errors.append(f"pending {name}: {type(exc).__name__}: {exc}")
        except (OSError, ValueError, RecursionError) as exc:
            errors.append(f"pending {name}: {type(exc).__name__}: {exc}")
    if changed:
        write_exact(path.parent / "adoption-index.json", index)
    return errors


def _record_adoption(path: Path, *, lease: dict, identity, members) -> bool:
    """Durable receiver acknowledgement precedes command tracker export."""
    if not _adoption_identity_valid(identity, path=path, lease=lease):
        return False
    if (
        identity.pid not in members
        or any(
            type(pid) is not int
            or pid <= 0
            or item.pid != pid
            or type(item.started_at_ns) is not int
            or item.started_at_ns <= 0
            for pid, item in members.items()
        )
        or members[identity.pid].started_at_ns != identity.started_at_ns
    ):
        return False
    receipt = _adoption_path(path, identity)
    lock = None
    try:
        receipt.parent.mkdir(parents=True, exist_ok=True)
        lock = _adoption_transaction(path)
        current = read_lease(path)
        if (
            current is None
            or current["state"] != "active"
            or current["token"] != lease["token"]
            or current["source_digest"] != lease["source_digest"]
        ):
            return False
        # Fresh OS births and held owner capability are rechecked after waiting
        # for the publication lock, not just at the caller's earlier snapshot.
        if (
            daemon.process_started_at_ns(current["owner_pid"])
            != current["owner_started_at_ns"]
            or daemon.process_started_at_ns(current["guardian_pid"])
            != current["guardian_started_at_ns"]
        ):
            return False
        contender = _try_acquire_file_lock(path.parent / "owner.lock")
        if contender is not None:
            _release_file_lock(contender)
            return False
        index = _read_adoption_index(path, lease=lease)
        if _recover_adoption_index(path, lease=lease, index=index):
            return False
        if index["state"] != "active":
            return False
        actual = {entry.name for entry in receipt.parent.glob("*.identity.json")}
        if actual != set(index["receipts"]):
            return False
        observed = {}
        old_payload = None
        if receipt.name in index["receipts"]:
            old_record, observed = _read_adoption(
                receipt,
                lease_path=path,
                lease=lease,
                digest=index["receipts"][receipt.name],
            )
            if old_record.identity != identity:
                return False
            old_payload = read_exact(
                receipt, max_bytes=_ADOPTION_BYTES, label="suite adoption"
            )
        observed.update({pid: item.started_at_ns for pid, item in members.items()})
        payload = {
            **daemon.backend_daemon_identity_payload(identity),
            "adoption_schema": ADOPTION_SCHEMA,
            "lease_token": lease["token"],
            "source_digest": lease["source_digest"],
            "custody_members": {
                str(pid): born for pid, born in sorted(observed.items())
            },
        }
        digest = hashlib.sha256(canonical_json_bytes(payload)).hexdigest()
        staged_index = {**index, "pending": {**index["pending"], receipt.name: digest}}
        committed_index = {
            **index,
            "receipts": {**index["receipts"], receipt.name: digest},
            "pending": {
                name: value
                for name, value in index["pending"].items()
                if name != receipt.name
            },
        }
        # read_exact bounds the formatted bytes write_exact actually publishes.
        if (
            len(encode_exact(payload)) > _ADOPTION_BYTES
            or len(encode_exact(staged_index)) > _ADOPTION_BYTES
            or len(encode_exact({**committed_index, "state": "closing"}))
            > _ADOPTION_BYTES
        ):
            return False
        if payload != old_payload:
            # Commit the prospective digest before changing the receipt. A
            # crashed publisher leaves either the old or staged generation;
            # recovery accepts only those exact, schema-validated contents.
            write_exact(path.parent / "adoption-index.json", staged_index)
            write_exact(receipt, payload)
            write_exact(path.parent / "adoption-index.json", committed_index)
        return True
    except (OSError, ValueError, RecursionError, RuntimeError):
        return False
    finally:
        if lock is not None:
            _release_file_lock(lock)


def _custody_records(
    path: Path,
    *,
    lease: dict,
    daemon_root: Path | None = None,
    timeout_s: float = _ADOPTION_LOCK_S,
):
    records = {}
    errors = []
    lock = None
    try:
        lock = _adoption_transaction(path, timeout_s=timeout_s)
        adoption_root = path.parent / "adoptions"
        try:
            index = _read_adoption_index(path, lease=lease)
            errors.extend(_recover_adoption_index(path, lease=lease, index=index))
            actual = {entry.name for entry in adoption_root.glob("*.identity.json")}
            if actual != (set(index["receipts"]) | set(index["pending"])):
                errors.append("suite adoption receipts differ from required index")
        except (OSError, ValueError, RecursionError) as exc:
            index = None
            errors.append(f"adoption index: {type(exc).__name__}: {exc}")
        if index is not None:
            for name, digest in sorted(index["receipts"].items()):
                receipt = adoption_root / name
                try:
                    record, members = _read_adoption(
                        receipt, lease_path=path, lease=lease, digest=digest
                    )
                except (OSError, ValueError, RecursionError) as exc:
                    errors.append(f"{name}: {type(exc).__name__}: {exc}")
                    continue
                records[(record.identity.pid, record.identity.started_at_ns)] = (
                    record,
                    members,
                )
        roots = {Path(lease["daemon_root"])}
        if daemon_root is not None:
            roots.add(daemon_root)
        for root in roots:
            for identity_path in sorted(root.glob("*.identity.json")):
                try:
                    identity = daemon.read_backend_daemon_identity(identity_path)
                except (OSError, ValueError, RecursionError) as exc:
                    errors.append(
                        f"operational {identity_path.name}: {type(exc).__name__}: {exc}"
                    )
                    continue
                if identity is None or identity.suite_lease != str(path):
                    continue
                if not _adoption_identity_valid(identity, path=path, lease=lease):
                    errors.append(
                        f"operational {identity_path.name}: identity differs from lease or lacks required digests"
                    )
                    continue
                record = daemon.BackendDaemonIdentityRecord(
                    identity=identity, path=identity_path
                )
                records.setdefault((identity.pid, identity.started_at_ns), (record, {}))
    except (OSError, ValueError, RecursionError, RuntimeError) as exc:
        errors.append(f"adoption transaction: {type(exc).__name__}: {exc}")
    finally:
        if lock is not None:
            _release_file_lock(lock)
    return tuple(records.values()), errors


def _seal_adoption_index(
    path: Path, *, lease: dict, state: str, timeout_s: float = _ADOPTION_LOCK_S
) -> list[str]:
    lock = None
    try:
        lock = _adoption_transaction(path, timeout_s=timeout_s)
        index = _read_adoption_index(path, lease=lease)
        recovery_errors = _recover_adoption_index(path, lease=lease, index=index)
        if recovery_errors:
            return recovery_errors
        # Never reopen an index, including on a repeated drain attempt.
        if index["state"] != "closed":
            candidate = {**index, "state": state}
            if len(encode_exact(candidate)) > _ADOPTION_BYTES:
                raise ValueError("sealed adoption index exceeds byte limit")
            write_exact(path.parent / "adoption-index.json", candidate)
        return []
    except (OSError, ValueError, RecursionError, RuntimeError) as exc:
        return [f"adoption seal: {type(exc).__name__}: {exc}"]
    finally:
        if lock is not None:
            _release_file_lock(lock)


def registered_groups(
    path: Path,
    *,
    lease: dict,
    samples: Mapping,
    daemon_root: Path | None = None,
    require_live_leader: bool = False,
    donor_environ: Mapping[str, str] | None = None,
    acknowledge: bool = True,
):
    """Receiving scope observes receipts independently of mutable daemon lookup."""
    from tools.memory_guard_core.process_model import birth_fenced_descendants

    groups = []
    records, errors = _custody_records(path, lease=lease, daemon_root=daemon_root)
    if errors and acknowledge:
        return ()  # Corrupt custody evidence never grants a new export.
    for record, observed in records:
        identity = record.identity
        if donor_environ is not None and not persistent_daemon_paths_allowed(
            donor_environ,
            (
                identity.project_root,
                identity.backend_bin,
                identity.socket_path,
                record.path,
                path,
            ),
        ):
            continue
        if not acknowledge:
            members, _unresolved = birth_fenced_descendants(samples, observed)
            if members:
                groups.append((lease, identity, members))
            continue
        root = samples.get(identity.pid)
        if root is not None:
            if (
                root.started_at_ns != identity.started_at_ns
                or root.pgid != identity.pid
                or not identity.config_digest
                or not _SHA.fullmatch(identity.config_digest)
                or not identity.backend_sha256
                or not _SHA.fullmatch(identity.backend_sha256)
                or not daemon.backend_daemon_command_matches_identity(
                    root.argv if root.argv is not None else root.command,
                    backend_bin=identity.backend_bin,
                    socket_path=identity.socket_path,
                )
            ):
                continue
            members = {
                pid: item for pid, item in samples.items() if item.pgid == identity.pid
            }
            if any(item.started_at_ns is None for item in members.values()):
                continue
            members, unresolved = birth_fenced_descendants(
                samples,
                {
                    **observed,
                    **{pid: item.started_at_ns for pid, item in members.items()},
                },
            )
            if unresolved:
                continue  # Unknown descendants cannot acquire export authority.
            if not _record_adoption(
                path, lease=lease, identity=identity, members=members
            ):
                continue
        elif not require_live_leader:
            members, _unresolved = birth_fenced_descendants(samples, observed)
        else:
            continue
        if members:
            groups.append((lease, identity, members))
    return tuple(groups)


def persistent_daemon_paths_allowed(environ: Mapping[str, str], paths) -> bool:
    """Persistent custody cannot export dependencies in a donor's scratch."""
    scratch = environ.get("MOLT_GUARD_SCRATCH_ROOT")
    if not scratch:
        return True
    try:
        root = Path(scratch).resolve()
        return all(not Path(path).resolve().is_relative_to(root) for path in paths)
    except (OSError, ValueError, RuntimeError):
        return False


def _guardian_import_path(inherited: str | None) -> str:
    """Put the owner's own `molt` and `tools` roots first on the guardian path.

    The guardian runs from the guest project root, so neither its working
    directory nor a relative inherited entry can find the custody modules the
    owner loaded; the lease's source digest binds exactly those files.
    """
    from tools.memory_guard_core import process_model

    roots = [
        str(Path(__file__).resolve().parents[1]),
        str(Path(process_model.__file__).resolve().parents[2]),
    ]
    entries = [entry for entry in (inherited or "").split(os.pathsep) if entry]
    return os.pathsep.join(dict.fromkeys([*roots, *entries]))


def persistent_daemon_env(environ: Mapping[str, str]) -> dict[str, str]:
    """A suite-owned daemon never inherits a command's scratch or guard token."""
    result = dict(environ)
    scratch = result.pop("MOLT_GUARD_SCRATCH_ROOT", None)
    result.pop("MOLT_MEMORY_GUARD_TOKEN", None)
    result.pop("MOLT_MEMORY_GUARD_MARKER", None)
    if scratch:
        root = Path(scratch).resolve()
        for key in ("PYTEST_DEBUG_TEMPROOT", "TMPDIR", "TMP", "TEMP"):
            raw = result.get(key)
            if raw:
                try:
                    Path(raw).resolve().relative_to(root)
                except ValueError:
                    continue
                result.pop(key, None)
    return result


@dataclass
class SuiteDaemonLease:
    path: Path
    record: dict
    lock: _FileLockHandle | None
    write_fd: int
    guardian: subprocess.Popen

    @classmethod
    def start(cls, *, project_root: Path, environ: Mapping[str, str]):
        if os.name != "posix":
            return None  # The backend daemon itself is POSIX-only.
        owner_birth = daemon.process_started_at_ns(os.getpid())
        if owner_birth is None:
            raise RuntimeError(
                "suite daemon lease requires owner process birth custody"
            )
        interpreter_sha256 = daemon.backend_content_sha256(
            Path(sys.executable).resolve()
        )
        if interpreter_sha256 is None:
            raise RuntimeError("suite daemon guardian interpreter digest unavailable")
        token = uuid.uuid4().hex
        daemon_root = daemon.backend_daemon_root_from_env(
            environ, project_root=project_root
        )
        directory = daemon_root / "suite-leases" / token
        directory.mkdir(parents=True)
        # Fork serialization covers lock acquisition as well as pipe admission:
        # otherwise a concurrent fork could inherit the lock before registration.
        with _lease_descriptor_mutation():
            lock = _try_acquire_file_lock(directory / "owner.lock")
            if lock is None:
                raise RuntimeError("new suite daemon lease was already held")
            read_fd = write_fd = -1
            try:
                read_fd, write_fd = os.pipe()
                _LEASE_PIPE_FDS.update((read_fd, write_fd))
                os.set_inheritable(write_fd, False)
            except BaseException:
                for fd in (read_fd, write_fd):
                    if fd >= 0:
                        _LEASE_PIPE_FDS.discard(fd)
                        with contextlib.suppress(OSError):
                            os.close(fd)
                _release_file_lock(lock)
                raise
        guardian = None
        try:
            path = directory / "lease.json"
            env = persistent_daemon_env(environ)
            env["PYTHONPATH"] = _guardian_import_path(env.get("PYTHONPATH"))
            with (directory / "guardian.log").open("ab") as log:
                guardian = subprocess.Popen(
                    [
                        sys.executable,
                        "-m",
                        "molt.backend_daemon_suite_custody",
                        "--watch",
                        str(path),
                        "--read-fd",
                        str(read_fd),
                    ],
                    cwd=project_root,
                    env=env,
                    pass_fds=(read_fd,),
                    close_fds=True,
                    start_new_session=True,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )
            with _lease_descriptor_mutation():
                _LEASE_PIPE_FDS.discard(read_fd)
                os.close(read_fd)
                read_fd = -1
            deadline = time.perf_counter() + 2.0
            guardian_birth = None
            while guardian_birth is None and time.perf_counter() < deadline:
                if guardian.poll() is not None:
                    raise RuntimeError(
                        "suite daemon guardian exited before lease admission"
                    )
                guardian_birth = daemon.process_started_at_ns(guardian.pid)
                if guardian_birth is None:
                    time.sleep(0.01)
            if guardian_birth is None:
                raise RuntimeError(
                    "suite daemon guardian birth could not be established"
                )
            record = {
                "schema": SCHEMA,
                "token": token,
                "state": "active",
                "owner_pid": os.getpid(),
                "owner_started_at_ns": owner_birth,
                "guardian_pid": guardian.pid,
                "guardian_started_at_ns": guardian_birth,
                "project_root": str(project_root.resolve()),
                "daemon_root": str(daemon_root.resolve()),
                "source_digest": _source_digest(),
                "interpreter": str(Path(sys.executable).resolve()),
                "interpreter_sha256": interpreter_sha256,
            }
            write_exact(
                path.parent / "adoption-index.json", _new_adoption_index(record)
            )
            write_exact(path, record)
            return cls(path, record, lock, write_fd, guardian)
        except BaseException:
            with _lease_descriptor_mutation():
                _LEASE_PIPE_FDS.discard(write_fd)
                os.close(write_fd)
                if read_fd != -1:
                    _LEASE_PIPE_FDS.discard(read_fd)
                    os.close(read_fd)
            if guardian is not None:
                with contextlib.suppress(subprocess.TimeoutExpired):
                    guardian.wait(timeout=2.0)
                if guardian.poll() is None:
                    guardian.kill()
                    guardian.wait(timeout=2.0)
            _release_file_lock(lock)
            raise

    def close(self, *, timeout: float = 7.0):
        # An inherited Python lease object never acquires the parent's custody.
        if self.record["owner_pid"] != os.getpid() or self.lock is None:
            return
        try:
            self.record["state"] = "closing"
            write_exact(self.path, self.record)
            with _lease_descriptor_mutation():
                if self.write_fd != -1:
                    os.close(self.write_fd)
                    _LEASE_PIPE_FDS.discard(self.write_fd)
                    self.write_fd = -1
            self.guardian.wait(timeout=timeout)
            if self.guardian.returncode != 0:
                raise RuntimeError(
                    f"suite daemon guardian failed: {self.path.parent / 'drain.json'}"
                )
            self.record["state"] = "closed"
            write_exact(self.path, self.record)
        finally:
            with _lease_descriptor_mutation():
                # Canonical file_locks owns inherited lock copies through drain.
                if self.write_fd != -1:
                    _LEASE_PIPE_FDS.discard(self.write_fd)
                    os.close(self.write_fd)
                    self.write_fd = -1
                _release_file_lock(self.lock)
                self.lock = None


def drain_lease(path: Path) -> bool:
    from tools import memory_guard

    lease = read_lease(path)
    if lease is None or lease["guardian_pid"] != os.getpid():
        return False
    deadline = time.perf_counter() + _DRAIN_BUDGET_S
    seal_errors = _seal_adoption_index(
        path,
        lease=lease,
        state="closing",
        timeout_s=min(_ADOPTION_LOCK_S, max(0.0, deadline - time.perf_counter())),
    )
    errors: list[str] = list(seal_errors)
    unresolved: list[int] = []
    failed_pass = bool(seal_errors)
    clean_passes = 0
    observed_history: dict[tuple[int, int], dict[int, int]] = {}
    unverified_history: dict[tuple[int, int], set[int]] = {}
    from tools.memory_guard_core.process_model import birth_fenced_descendants

    sample_errors = (
        OSError,
        subprocess.SubprocessError,
        memory_guard.ProcessSnapshotError,
        TimeoutError,
    )

    def bounded_sample():
        if time.perf_counter() >= deadline:
            raise TimeoutError("suite daemon drain budget elapsed")
        samples = memory_guard.sample_processes()
        if time.perf_counter() >= deadline:
            raise TimeoutError("suite daemon drain budget elapsed during snapshot")
        return samples

    while True:
        retryable_seal = any(
            error.startswith("adoption seal: OSError:")
            or "suite adoption transaction lock timed out" in error
            for error in seal_errors
        )
        if retryable_seal and time.perf_counter() < deadline:
            seal_errors = _seal_adoption_index(
                path,
                lease=lease,
                state="closing",
                timeout_s=min(
                    _ADOPTION_LOCK_S, max(0.0, deadline - time.perf_counter())
                ),
            )
            errors.extend(seal_errors)
        failed_pass = bool(seal_errors)
        try:
            samples = bounded_sample()
            guardian = samples.get(os.getpid())
            if (
                guardian is None
                or guardian.started_at_ns != lease["guardian_started_at_ns"]
            ):
                failed_pass = True
                errors.append("guardian birth could not be verified")
            else:
                unresolved = []
                records, adoption_errors = _custody_records(
                    path,
                    lease=lease,
                    timeout_s=min(
                        _ADOPTION_LOCK_S, max(0.0, deadline - time.perf_counter())
                    ),
                )
                errors.extend(adoption_errors)
                failed_pass = failed_pass or bool(adoption_errors)
                for record, observed_members in records:
                    identity = record.identity
                    if time.perf_counter() >= deadline:
                        unresolved.append(identity.pid)
                        failed_pass = True
                        continue
                    root = samples.get(identity.pid)
                    leader_reused = (
                        root is not None
                        and root.started_at_ns != identity.started_at_ns
                    )
                    # A reused leader PID cannot identify the old group. Old
                    # receipted workers that moved groups still have authority.
                    group = (
                        {}
                        if leader_reused
                        else {
                            pid: item
                            for pid, item in samples.items()
                            if item.pgid == identity.pid
                        }
                    )
                    history = observed_history.setdefault(
                        (identity.pid, identity.started_at_ns), {}
                    )
                    history.update(observed_members)
                    historical, unknown_descendants = birth_fenced_descendants(
                        samples, history
                    )
                    history.update(
                        {
                            pid: item.started_at_ns
                            for pid, item in historical.items()
                            if item.started_at_ns is not None
                        }
                    )
                    pending_unknown = unverified_history.setdefault(
                        (identity.pid, identity.started_at_ns), set()
                    )
                    pending_unknown.update(unknown_descendants)
                    pending_unknown.difference_update(historical)
                    pending_unknown.intersection_update(samples)
                    if pending_unknown:
                        unresolved.append(identity.pid)
                        failed_pass = True
                        errors.append(
                            "unverified descendant births: "
                            + ",".join(map(str, sorted(pending_unknown)))
                        )
                    if not group and not historical:
                        continue
                    live_leader = (
                        root is not None
                        and root.started_at_ns == identity.started_at_ns
                        and root.pgid == identity.pid
                        and _adoption_identity_valid(identity, path=path, lease=lease)
                        and daemon.backend_daemon_command_matches_identity(
                            root.argv if root.argv is not None else root.command,
                            backend_bin=identity.backend_bin,
                            socket_path=identity.socket_path,
                        )
                    )
                    owned = dict(historical)
                    known_group = {
                        pid: item
                        for pid, item in group.items()
                        if type(item.started_at_ns) is int and item.started_at_ns > 0
                    }
                    if live_leader:
                        owned.update(known_group)
                    if not owned:
                        unresolved.append(identity.pid)
                        continue
                    expected = {
                        pid: memory_guard.process_identity(item)
                        for pid, item in owned.items()
                    }
                    try:
                        if live_leader and len(known_group) == len(group):
                            # Only the live, verified session leader grants
                            # current group authority. Historical moved workers
                            # always receive individual birth-matched signals.
                            memory_guard.terminate_watched_processes(
                                identity.pid,
                                samples=samples,
                                watched=set(group),
                                expected_identities={
                                    pid: expected[pid] for pid in group
                                },
                                sampler=bounded_sample,
                                root_owned=True,
                                grace=min(
                                    0.25, max(0.0, deadline - time.perf_counter())
                                ),
                                reason="suite_daemon_lease_eof",
                            )
                            individual = set(owned) - set(group)
                        else:
                            # Never invoke the process-group terminator for a
                            # dead/unknown leader, even with root_owned=False.
                            individual = set(owned)
                        for pid in sorted(individual):
                            memory_guard.terminate_verified_pid(
                                pid,
                                expected[pid],
                                sampler=bounded_sample,
                                grace=min(
                                    0.25, max(0.0, deadline - time.perf_counter())
                                ),
                            )
                        fresh = bounded_sample()
                        fresh_root = fresh.get(identity.pid)
                        group_reused = (
                            fresh_root is not None
                            and fresh_root.started_at_ns != identity.started_at_ns
                        )
                        if (
                            not group_reused
                            and any(
                                item.pgid == identity.pid for item in fresh.values()
                            )
                        ) or any(
                            pid in fresh
                            and fresh[pid].started_at_ns == marker.started_at_ns
                            for pid, marker in expected.items()
                        ):
                            unresolved.append(identity.pid)
                    except sample_errors as exc:
                        errors.append(f"{type(exc).__name__}: {exc}")
                        unresolved.append(identity.pid)
                        failed_pass = True
        except sample_errors as exc:
            errors.append(f"{type(exc).__name__}: {exc}")
            failed_pass = True
        clean_passes = clean_passes + 1 if not unresolved and not failed_pass else 0
        if clean_passes >= 2 or time.perf_counter() >= deadline:
            break
        time.sleep(min(0.02, max(0.0, deadline - time.perf_counter())))
    closed = clean_passes >= 2 and not unresolved and not failed_pass
    if closed:
        final_seal_errors = _seal_adoption_index(
            path,
            lease=lease,
            state="closed",
            timeout_s=min(_ADOPTION_LOCK_S, max(0.0, deadline - time.perf_counter())),
        )
        errors.extend(final_seal_errors)
        closed = not final_seal_errors
    write_exact(
        path.parent / "drain.json",
        {
            "schema": SCHEMA,
            "token": lease["token"],
            "closed": closed,
            "unresolved_pgids": sorted(set(unresolved)),
            "sampling_errors": errors,
            "clean_passes": clean_passes,
            "observed_members": {
                f"{pid}.{born}": {
                    str(member): marker for member, marker in sorted(history.items())
                }
                for (pid, born), history in observed_history.items()
            },
        },
    )
    return closed


def main():
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--watch", type=Path, required=True)
    parser.add_argument("--read-fd", type=int, required=True)
    args = parser.parse_args()
    # Load the shared drain authority before waiting for owner death.
    from tools import memory_guard as _memory_guard

    del _memory_guard
    try:
        # The suite alone holds the write end. Non-inheritable write custody
        # makes normal closure and hard controller death the same EOF boundary.
        while os.read(args.read_fd, 1):
            pass
    finally:
        os.close(args.read_fd)
    return 0 if drain_lease(args.watch) else 1


if __name__ == "__main__":
    raise SystemExit(main())
