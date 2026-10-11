"""Live source/toolchain and child-process custody for proof execution.

Endpoint hashes prove only the endpoints.  This module supplies the missing
execution-time authority: kernel filesystem notifications retain observed
write, rename, deletion, and metadata events until the parent consumes them. On
Linux, writable-close events remain necessary for closed writable mappings;
inotify alone does not report mmap writes or identify their process. The
Python/Node launch hooks reject child executables before launch unless the
admitted envelope declares their captured toolchain identity or the native
supervisor has admitted their run-owned output root.
"""

from __future__ import annotations

import ctypes
import hashlib
import json
import os
import secrets
import select
import socket
import struct
import sys
import threading
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.exact_json import canonical_json_sha256, loads_exact
from molt.toolchain_identity import (
    StableRegularFileVersion,
    stable_regular_file_version,
    verify_stable_regular_file_identity,
)
from tools.proof_queue_pkg import process_image_capture, windows_createprocess

from tools.proof_queue_pkg.python_child_custody import (
    CHILD_POLICY_ENV as CHILD_POLICY_ENV,
    CHILD_ENDPOINT_ENV as CHILD_ENDPOINT_ENV,
    CHILD_TOKEN_ENV as CHILD_TOKEN_ENV,
    _environment_value,
    install_python_child_custody as install_python_child_custody,
)


def _norm(path: Path | str) -> str:
    return process_image_capture._image_path_key(Path(path))


@dataclass(frozen=True)
class WatchSpec:
    root: Path
    paths: frozenset[str] | None = None

    def owns(self, candidate: Path) -> bool:
        if self.paths is None:
            return True
        try:
            normalized = _norm(candidate)
        except (OSError, ValueError):
            # A vanished/replaced entry cannot establish exclusion from this
            # watch. Retain its event as an input mutation, never as admission.
            return True
        if normalized in self.paths:
            return True
        prefix = normalized.rstrip(os.sep) + os.sep
        return any(path.startswith(prefix) for path in self.paths)


CHILD_POLICY_SCHEMA = "molt.proof-child-custody.v1"
# v3: every apparatus class is validated; selected empty uv-lock closes carry
# captured Python/uv owner identities. Older permissive receivers cannot admit it.
LIVE_CUSTODY_RECEIPT_SCHEMA = "molt.proof-live-custody.v3"

# Filesystem events that the proof apparatus itself causes inside a watched
# root. They carry no information about the proof's inputs and are recorded
# under their own class instead of as input mutations.
APPARATUS_GIT_INDEX_REFRESH = "git-index-refresh"
APPARATUS_UV_ENVIRONMENT_LOCK = "uv-environment-lock-close"
_EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"


@dataclass(frozen=True)
class CapturedUvEnvironmentLocks:
    uv_identity_sha256: str | None
    environments: tuple[tuple[Path, str], ...]


def captured_uv_environment_locks(
    toolchains: Mapping[str, object],
) -> CapturedUvEnvironmentLocks:
    """Project one already validated full capture; never infer from watch roots.

    A compound operation can capture its uv owner and selected Python
    environment in different contexts. No new selection or probe occurs here.
    Retain only finite owner facts, not the full environment/SDK inventories.
    """
    uv = toolchains.get("uv")
    uv_identity = str(uv["identity_sha256"]) if isinstance(uv, Mapping) else None
    environments: list[tuple[Path, str]] = []
    python = toolchains.get("python")
    if isinstance(python, Mapping):
        location = python["location"]
        environment = python["environment"]
        assert isinstance(location, Mapping) and isinstance(environment, Mapping)
        prefix = Path(str(location["prefix"]))
        if not prefix.is_absolute():
            raise ValueError("operational environment lock has no absolute owner")
        tree = environment["tree"]
        assert isinstance(tree, Mapping)
        entries, nodes = tree["entries"], tree["file_nodes"]
        assert isinstance(entries, list) and isinstance(nodes, list)
        lock = next((row for row in entries if row["path"] == ".lock"), None)
        if (
            lock is not None
            and lock.get("kind") == "file"
            and lock.get("access")
            == {"readable": True, "writable": True, "executable": False}
        ):
            node = next((row for row in nodes if row["id"] == lock["node"]), None)
            if (
                node is not None
                and node.get("size") == 0
                and node.get("sha256") == _EMPTY_SHA256
            ):
                path = prefix / ".lock"
                custody = python["file_custody"]
                assert isinstance(custody, list)
                if {
                    "path": str(path),
                    "size": 0,
                    "sha256": _EMPTY_SHA256,
                } not in custody:
                    raise ValueError(
                        "operational environment lock lacks captured file custody"
                    )
                environments.append((path, str(python["identity_sha256"])))
    return CapturedUvEnvironmentLocks(uv_identity, tuple(environments))


def _uv_environment_lock_records(
    captures: Sequence[CapturedUvEnvironmentLocks],
) -> dict[str, dict[str, str]]:
    uv_identities = {
        capture.uv_identity_sha256
        for capture in captures
        if capture.uv_identity_sha256 is not None
    }
    if not uv_identities:
        return {}
    if len(uv_identities) != 1:
        raise ValueError("operational environment lock has ambiguous uv ownership")
    uv_identity = next(iter(uv_identities))
    records: dict[str, dict[str, str]] = {}
    for capture in captures:
        for path, python_identity in capture.environments:
            record = {
                "action": "inotify:0x8",
                "path": str(path),
                "apparatus": APPARATUS_UV_ENVIRONMENT_LOCK,
                "python_identity_sha256": python_identity,
                "uv_identity_sha256": uv_identity,
            }
            key = _norm(path)
            if key in records and records[key] != record:
                raise ValueError(
                    "operational environment lock has conflicting captures"
                )
            records[key] = record
    return records


def validated_uv_environment_lock_capture(
    toolchains: Mapping[str, object],
) -> CapturedUvEnvironmentLocks:
    """Validate a retained capture before granting new operational authority.

    Canonical guarded execution has already performed these validations. The
    receipt receiver and compound private capture composition reuse the same
    validators here; a CAS binding alone is not semantic owner validation.
    """
    if not any(name in toolchains for name in ("python", "uv")):
        return CapturedUvEnvironmentLocks(None, ())

    from tools import proof_plan
    from tools.proof_queue_pkg import command_identity

    plan = proof_plan.ProofPlan.load()
    for name in ("python", "uv"):
        if name not in toolchains:
            continue
        identity = toolchains[name]
        if not isinstance(identity, Mapping):
            raise ValueError("operational environment lock owner is malformed")
        command_identity._validate_toolchain_identity(
            plan, name, identity, full_capture=True
        )
        if name == "uv":
            material = dict(identity)
            digest = material.pop("identity_sha256", None)
            if digest != canonical_json_sha256(material):
                raise ValueError("uv operational owner identity digest is invalid")
    return captured_uv_environment_locks(toolchains)


def validate_apparatus_events(
    events: Sequence[object], captures: Sequence[CapturedUvEnvironmentLocks]
) -> None:
    """Validate every apparatus class, including each captured uv-lock owner."""
    expected = _uv_environment_lock_records(captures)
    for event in events:
        if not isinstance(event, Mapping):
            raise ValueError("live custody apparatus event is malformed")
        path, action = event.get("path"), event.get("action")
        if (
            not isinstance(path, str)
            or not Path(path).is_absolute()
            or not isinstance(action, str)
        ):
            raise ValueError("live custody apparatus event has invalid path/action")
        if event.get("apparatus") == APPARATUS_GIT_INDEX_REFRESH:
            if (
                set(event) != {"path", "action", "apparatus"}
                or _git_refresh_candidate(Path(Path(path).anchor), Path(path), action)
                is None
            ):
                raise ValueError(
                    "git apparatus event differs from index-refresh authority"
                )
        elif event.get(
            "apparatus"
        ) != APPARATUS_UV_ENVIRONMENT_LOCK or event != expected.get(_norm(path)):
            raise ValueError("uv environment lock event differs from captured owners")


def _git_refresh_candidate(
    root: Path, path: Path, action: str
) -> tuple[Path, bool] | None:
    """Pure path/action authority, also usable for retained event receipts."""
    try:
        root_key = _norm(root)
        # This event classifier may observe an already removed index.lock.
        # Its exact event basename is bookkeeping under a still-existing,
        # independently canonicalized directory. This does not admit a file
        # identity or recover a missing executable coordinate.
        path_key = (
            str(Path(_norm(path.parent)) / path.name)
            if path.name == "index.lock" and not path.exists()
            else _norm(path)
        )
        if path_key != root_key and not path_key.startswith(
            root_key.rstrip(os.sep) + os.sep
        ):
            return None
        relative = Path(path_key[len(root_key) :].lstrip(os.sep))
    except (OSError, ValueError):
        return None
    parts = relative.parts
    if ".git" not in parts:
        return None
    tail = parts[parts.index(".git") + 1 :]
    if tail not in ((), ("index.lock",)) and not (
        len(tail) >= 2 and tail[0] == "worktrees" and tail[2:] in ((), ("index.lock",))
    ):
        return None
    lock_event = bool(tail and tail[-1] == "index.lock")
    directory = path.parent if lock_event else path
    if lock_event:
        return directory, True
    safe_directory_change = action == "modified"
    if action.startswith("inotify:"):
        try:
            mask = int(action.partition(":")[2], 16)
        except ValueError:
            return None
        safe_directory_change = bool(mask & 0x6) and not (mask & ~0x40000006)
    elif action.startswith("fsevents:"):
        try:
            flags = int(action.partition(":")[2], 16)
        except ValueError:
            return None
        safe_directory_change = bool(flags & 0x1400) and not (flags & ~0x21400)
    return (directory, False) if safe_directory_change else None


def classify_apparatus_event(root: Path, path: Path, action: str) -> str | None:
    """Classify Git refresh events, checking live directory ownership here.

    Linked-worktree .git files, missing/replaced directories, symlink aliases,
    rewritten indexes and other Git changes remain ordinary input events.
    """
    candidate = _git_refresh_candidate(root, path, action)
    if candidate is None:
        return None
    directory, lock_event = candidate
    if not directory.is_dir() or directory.resolve() != Path(_norm(directory)):
        return None
    if lock_event and (path.is_dir() or path.is_symlink()):
        return None
    return APPARATUS_GIT_INDEX_REFRESH


def _compact_specs(specs: Iterable[WatchSpec]) -> list[WatchSpec]:
    merged: dict[str, tuple[Path, set[str] | None]] = {}
    for spec in specs:
        root = spec.root.resolve(strict=True)
        key = _norm(root)
        existing = merged.get(key)
        if existing is None:
            merged[key] = (root, None if spec.paths is None else set(spec.paths))
            continue
        if existing[1] is None or spec.paths is None:
            merged[key] = (root, None)
        else:
            existing[1].update(spec.paths)
    compact = [
        WatchSpec(root, None if paths is None else frozenset(paths))
        for root, paths in merged.values()
    ]
    broad_roots = tuple(_norm(spec.root) for spec in compact if spec.paths is None)
    return [
        spec
        for spec in compact
        if not any(
            _norm(spec.root).startswith(broad.rstrip(os.sep) + os.sep)
            for broad in broad_roots
        )
    ]


def live_custody_identity_sha256(
    *,
    events: Sequence[Mapping[str, str]],
    apparatus_events: Sequence[Mapping[str, str]],
    errors: Sequence[str],
    state: object,
    lifecycle: Sequence[str],
) -> str:
    """The one identity of a live custody receipt, shared by producer and publisher."""
    material = {
        "events": list(events),
        "apparatus_events": list(apparatus_events),
        "errors": list(errors),
        "state": state,
        "lifecycle": list(lifecycle),
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


class LiveCustodyMonitor:
    """Fail-closed kernel event monitor for immutable execution inputs."""

    def __init__(self, specs: Sequence[WatchSpec]) -> None:
        self.specs = _compact_specs(specs)
        self._events: list[dict[str, str]] = []
        self._apparatus_events: list[dict[str, str]] = []
        self._errors: list[str] = []
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._ready = threading.Event()
        self._thread: threading.Thread | None = None
        self._handles: list[object] = []
        self._state = "CREATED"
        self._lifecycle = ["CREATED"]
        self._uv_lock_records: dict[str, dict[str, str]] = {}
        self._uv_lock_versions: list[StableRegularFileVersion] = []
        self._uv_locks_admitted = False

    def admit_uv_environment_locks(
        self, captures: Sequence[CapturedUvEnvironmentLocks]
    ) -> None:
        """Admit only selected empty uv locks after full capture, before action.

        Callers retain ordinary endpoint custody for every lock. The original
        event stream is never erased or retrospectively reclassified. A lock
        without a complete capture remains an ordinary immutable input.
        """
        with self._lock:
            if self._state != "ARMED" or self._uv_locks_admitted:
                raise ValueError(
                    "uv lock admission requires one armed capture boundary"
                )
        records = _uv_environment_lock_records(captures)
        admitted: dict[str, dict[str, str]] = {}
        versions: list[StableRegularFileVersion] = []
        for key, record in sorted(records.items()):
            path = Path(record["path"])
            if not path.is_absolute() or path.resolve(strict=True) != path:
                raise ValueError("uv environment lock has path indirection")
            if not any(
                path.is_relative_to(spec.root) and spec.owns(path)
                for spec in self.specs
            ):
                raise ValueError("uv environment lock is outside armed custody")
            version = stable_regular_file_version(
                path, label="captured uv environment lock"
            )
            if version.size != 0 or path.lstat().st_nlink != 1:
                raise ValueError("uv environment lock is not an empty single-link file")
            admitted[key] = record
            versions.append(version)
        with self._lock:
            if self._state != "ARMED" or self._uv_locks_admitted:
                raise ValueError("uv lock admission lost its armed capture boundary")
            self._uv_lock_records = admitted
            self._uv_lock_versions = versions
            self._uv_locks_admitted = True

    def _transition(self, expected: str, next_state: str) -> None:
        with self._lock:
            if self._state != expected:
                raise RuntimeError(
                    f"proof live custody state is {self._state}, expected {expected}"
                )
            self._state = next_state
            self._lifecycle.append(next_state)

    def __enter__(self) -> LiveCustodyMonitor:
        if sys.platform == "win32":
            target = self._run_windows
        elif sys.platform.startswith("linux"):
            target = self._run_linux
        elif sys.platform == "darwin":
            target = self._run_darwin
        else:
            raise RuntimeError(
                f"proof live custody has no lossless kernel watcher on {sys.platform}"
            )
        self._transition("CREATED", "ARMING")
        self._thread = threading.Thread(
            target=target, name="proof-live-custody", daemon=True
        )
        self._thread.start()
        if not self._ready.wait(timeout=10.0):
            self._record_error("proof live custody watcher did not become ready")
            self._transition("ARMING", "ARMED")
            self.drain()
            raise RuntimeError(self._errors[0])
        if self._errors:
            self._transition("ARMING", "ARMED")
            self.drain()
            raise RuntimeError(self._errors[0])
        self._transition("ARMING", "ARMED")
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        del exc_type, exc, traceback
        self.drain()

    def drain(self) -> None:
        """Fence and consume the platform event stream exactly once."""
        with self._lock:
            if self._state == "DRAINED":
                return
        self._transition("ARMED", "DRAINING")
        self._stop.set()
        if sys.platform == "win32":
            kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
            for handle in self._handles:
                kernel32.CancelIoEx(handle, None)
        if self._thread is not None:
            self._thread.join(timeout=10.0)
            if self._thread.is_alive():
                self._record_error("proof live custody watcher did not stop")
        for version in self._uv_lock_versions:
            try:
                verify_stable_regular_file_identity(
                    version, label="captured uv environment lock"
                )
                if version.path.lstat().st_nlink != 1:
                    raise ValueError(
                        "captured uv environment lock link ownership changed"
                    )
            except (OSError, ValueError) as exc:
                self._record_error(str(exc))
        for handle in self._handles:
            try:
                if sys.platform == "win32":
                    ctypes.WinDLL("kernel32", use_last_error=True).CloseHandle(handle)
                else:
                    os.close(int(handle))
            except OSError:
                pass
        self._handles.clear()
        self._transition("DRAINING", "DRAINED")

    def receipt(self) -> dict[str, object]:
        with self._lock:
            events = list(self._events)
            apparatus_events = list(self._apparatus_events)
            errors = list(self._errors)
            state = self._state
            lifecycle = list(self._lifecycle)
        if state != "DRAINED":
            errors.append(f"proof live custody receipt requested in state {state}")
        return {
            "schema": LIVE_CUSTODY_RECEIPT_SCHEMA,
            "watch_roots": len(self.specs),
            "events": events,
            "apparatus_events": apparatus_events,
            "errors": errors,
            "state": state,
            "lifecycle": lifecycle,
            "stable": state == "DRAINED" and not events and not errors,
            "identity_sha256": live_custody_identity_sha256(
                events=events,
                apparatus_events=apparatus_events,
                errors=errors,
                state=state,
                lifecycle=lifecycle,
            ),
        }

    def _record_error(self, message: str) -> None:
        with self._lock:
            if message not in self._errors:
                self._errors.append(message)

    def _record_event(self, spec: WatchSpec, action: str, path: Path) -> None:
        if not spec.owns(path):
            return
        event = {"action": action, "path": str(path)}
        apparatus = classify_apparatus_event(spec.root, path, action)
        with self._lock:
            if action == "inotify:0x8":
                uv_lock = self._uv_lock_records.get(_norm(path))
                if uv_lock is not None:
                    if uv_lock not in self._apparatus_events:
                        self._apparatus_events.append(dict(uv_lock))
                    return
            if apparatus is not None:
                classified = {**event, "apparatus": apparatus}
                if classified not in self._apparatus_events:
                    self._apparatus_events.append(classified)
                return
            if event not in self._events:
                self._events.append(event)

    def _run_windows(self) -> None:
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        create_file = kernel32.CreateFileW
        create_file.argtypes = [
            wintypes.LPCWSTR,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.LPVOID,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.HANDLE,
        ]
        create_file.restype = wintypes.HANDLE
        read_changes = kernel32.ReadDirectoryChangesW
        read_changes.argtypes = [
            wintypes.HANDLE,
            wintypes.LPVOID,
            wintypes.DWORD,
            wintypes.BOOL,
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
            wintypes.LPVOID,
            wintypes.LPVOID,
        ]
        read_changes.restype = wintypes.BOOL
        invalid = ctypes.c_void_p(-1).value
        threads: list[threading.Thread] = []
        armed_events: list[threading.Event] = []
        try:
            for spec in self.specs:
                handle = create_file(
                    str(spec.root),
                    0x0001,
                    0x00000001 | 0x00000002 | 0x00000004,
                    None,
                    3,
                    0x02000000 | 0x40000000,
                    None,
                )
                if int(handle) == invalid:
                    raise OSError(
                        ctypes.get_last_error(),
                        f"cannot watch proof custody root {spec.root}",
                    )
                self._handles.append(handle)
                armed = threading.Event()
                thread = threading.Thread(
                    target=self._read_windows_root,
                    args=(spec, handle, read_changes, armed),
                    daemon=True,
                )
                thread.start()
                threads.append(thread)
                armed_events.append(armed)
            for spec, armed in zip(self.specs, armed_events, strict=True):
                if not armed.wait(timeout=10.0):
                    raise RuntimeError(
                        f"proof custody watch was not armed for {spec.root}"
                    )
            if self._errors:
                raise RuntimeError(self._errors[0])
            self._ready.set()
            self._stop.wait()
            for handle in self._handles:
                kernel32.CancelIoEx(handle, None)
            for thread in threads:
                thread.join(timeout=5.0)
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
            self._ready.set()

    def _read_windows_root(
        self,
        spec: WatchSpec,
        handle: object,
        read_changes: object,
        armed: threading.Event,
    ) -> None:
        from ctypes import wintypes

        class Overlapped(ctypes.Structure):
            _fields_ = [
                ("Internal", ctypes.c_size_t),
                ("InternalHigh", ctypes.c_size_t),
                ("Offset", wintypes.DWORD),
                ("OffsetHigh", wintypes.DWORD),
                ("hEvent", wintypes.HANDLE),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        create_event = kernel32.CreateEventW
        create_event.argtypes = [
            wintypes.LPVOID,
            wintypes.BOOL,
            wintypes.BOOL,
            wintypes.LPCWSTR,
        ]
        create_event.restype = wintypes.HANDLE
        reset_event = kernel32.ResetEvent
        reset_event.argtypes = [wintypes.HANDLE]
        reset_event.restype = wintypes.BOOL
        wait_for_single = kernel32.WaitForSingleObject
        wait_for_single.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        wait_for_single.restype = wintypes.DWORD
        get_result = kernel32.GetOverlappedResult
        get_result.argtypes = [
            wintypes.HANDLE,
            ctypes.POINTER(Overlapped),
            ctypes.POINTER(wintypes.DWORD),
            wintypes.BOOL,
        ]
        get_result.restype = wintypes.BOOL
        event = create_event(None, True, False, None)
        if not event:
            self._record_error(
                f"CreateEventW failed for {spec.root}: winerror={ctypes.get_last_error()}"
            )
            armed.set()
            return

        actions = {
            1: "created",
            2: "deleted",
            3: "modified",
            4: "renamed-from",
            5: "renamed-to",
        }
        notify_filter = (
            0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000100
        )
        try:
            first_read = True
            while True:
                buffer = ctypes.create_string_buffer(64 * 1024)
                returned = wintypes.DWORD()
                overlapped = Overlapped(hEvent=event)
                reset_event(event)
                ok = read_changes(
                    handle,
                    buffer,
                    len(buffer),
                    True,
                    notify_filter,
                    None,
                    ctypes.byref(overlapped),
                    None,
                )
                if not ok and ctypes.get_last_error() != 997:
                    raise OSError(
                        ctypes.get_last_error(),
                        f"ReadDirectoryChangesW arm failed for {spec.root}",
                    )
                if first_read:
                    first_read = False
                    armed.set()
                wait_result = wait_for_single(event, 0xFFFFFFFF)
                if wait_result != 0:
                    raise OSError(
                        ctypes.get_last_error(),
                        f"ReadDirectoryChangesW wait failed for {spec.root}",
                    )
                if not get_result(
                    handle, ctypes.byref(overlapped), ctypes.byref(returned), False
                ):
                    error = ctypes.get_last_error()
                    if self._stop.is_set() and error == 995:
                        return
                    raise OSError(
                        error,
                        f"ReadDirectoryChangesW completion failed for {spec.root}",
                    )
                if returned.value == 0:
                    self._record_error(
                        f"ReadDirectoryChangesW overflowed for {spec.root}"
                    )
                else:
                    offset = 0
                    while offset < returned.value:
                        next_offset, action, name_bytes = struct.unpack_from(
                            "<III", buffer.raw, offset
                        )
                        start = offset + 12
                        name = buffer.raw[start : start + name_bytes].decode(
                            "utf-16-le"
                        )
                        self._record_event(
                            spec, actions.get(action, str(action)), spec.root / name
                        )
                        if next_offset == 0:
                            break
                        offset += next_offset
                if self._stop.is_set():
                    return
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
            armed.set()
        finally:
            kernel32.CloseHandle(event)

    def _run_linux(self) -> None:
        libc = ctypes.CDLL(None, use_errno=True)
        init = libc.inotify_init1
        init.argtypes = [ctypes.c_int]
        init.restype = ctypes.c_int
        add = libc.inotify_add_watch
        add.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
        add.restype = ctypes.c_int
        fd = init(os.O_NONBLOCK | os.O_CLOEXEC)
        if fd < 0:
            self._record_error(f"inotify_init1 failed: errno={ctypes.get_errno()}")
            self._ready.set()
            return
        self._handles.append(fd)
        mask = (
            0x00000002
            | 0x00000004
            | 0x00000008
            | 0x00000100
            | 0x00000200
            | 0x00000040
            | 0x00000080
            | 0x00000400
            | 0x00000800
        )
        watches: dict[int, tuple[WatchSpec, Path]] = {}

        def add_directory(spec: WatchSpec, directory: Path) -> None:
            wd = add(fd, os.fsencode(directory), mask)
            if wd < 0:
                raise OSError(
                    ctypes.get_errno(),
                    f"cannot watch proof custody root {directory}",
                )
            watches[wd] = (spec, directory)

        def consume(payload: bytes) -> None:
            offset = 0
            while offset < len(payload):
                wd, event_mask, _cookie, length = struct.unpack_from(
                    "iIII", payload, offset
                )
                raw_name = payload[offset + 16 : offset + 16 + length]
                name = os.fsdecode(raw_name.split(b"\0", 1)[0])
                offset += 16 + length
                if event_mask & 0x00004000:
                    self._record_error("inotify queue overflowed")
                    continue
                owner = watches.get(wd)
                if owner is None:
                    self._record_error("inotify returned an unknown custody watch")
                    continue
                spec, directory = owner
                candidate = directory / name if name else directory
                self._record_event(spec, f"inotify:{event_mask:#x}", candidate)
                if event_mask & 0x40000000 and event_mask & (0x00000100 | 0x00000080):
                    if candidate.is_dir():
                        new_wd = add(fd, os.fsencode(candidate), mask)
                        if new_wd < 0:
                            self._record_error(
                                f"cannot extend inotify custody to {candidate}"
                            )
                        else:
                            watches[new_wd] = (spec, candidate)

        try:
            for spec in self.specs:
                # Root first is the recursive-enumeration fence.  Any directory
                # created after this point is either enumerated below or leaves
                # an event on the already-live parent watch; there is no gap in
                # which an extant subtree can enter the ARMED set unwatched.
                add_directory(spec, spec.root)
                for candidate in spec.root.rglob("*"):
                    if candidate.is_dir():
                        add_directory(spec, candidate)
            self._ready.set()
            while not self._stop.is_set():
                readable, _, _ = select.select([fd], [], [], 0.25)
                if not readable:
                    continue
                consume(os.read(fd, 1024 * 1024))
            # Nonblocking reads to EAGAIN are the Linux terminal watermark.
            while True:
                try:
                    consume(os.read(fd, 1024 * 1024))
                except BlockingIOError:
                    break
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
            self._ready.set()

    def _run_darwin(self) -> None:
        """Watch complete path trees through the native FSEvents journal."""
        core_services = ctypes.CDLL(
            "/System/Library/Frameworks/CoreServices.framework/CoreServices"
        )
        core_foundation = ctypes.CDLL(
            "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
        )
        callback_type = ctypes.CFUNCTYPE(
            None,
            ctypes.c_void_p,
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.POINTER(ctypes.c_uint32),
            ctypes.POINTER(ctypes.c_uint64),
        )
        cf_strings: list[ctypes.c_void_p] = []
        stream = ctypes.c_void_p()
        paths = ctypes.c_void_p()

        def callback(
            _stream: object,
            _context: object,
            count: int,
            event_paths: object,
            event_flags: object,
            _event_ids: object,
        ) -> None:
            del _stream, _context, _event_ids
            for index in range(count):
                flags = int(event_flags[index])
                path = Path(os.fsdecode(event_paths[index]))
                if flags & (
                    0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000020
                ):
                    self._record_error(
                        f"FSEvents history was incomplete for {path}: flags={flags:#x}"
                    )
                for spec in self.specs:
                    try:
                        path.relative_to(spec.root)
                    except ValueError:
                        continue
                    self._record_event(spec, f"fsevents:{flags:#x}", path)
                    break

        callback_ref = callback_type(callback)
        try:
            create_string = core_foundation.CFStringCreateWithCString
            create_string.argtypes = [
                ctypes.c_void_p,
                ctypes.c_char_p,
                ctypes.c_uint32,
            ]
            create_string.restype = ctypes.c_void_p
            for spec in self.specs:
                value = create_string(None, os.fsencode(spec.root), 0x08000100)
                if not value:
                    raise RuntimeError(f"cannot encode FSEvents root {spec.root}")
                cf_strings.append(ctypes.c_void_p(value))
            values = (ctypes.c_void_p * len(cf_strings))(
                *(value.value for value in cf_strings)
            )
            create_array = core_foundation.CFArrayCreate
            create_array.argtypes = [
                ctypes.c_void_p,
                ctypes.POINTER(ctypes.c_void_p),
                ctypes.c_long,
                ctypes.c_void_p,
            ]
            create_array.restype = ctypes.c_void_p
            paths = ctypes.c_void_p(create_array(None, values, len(values), None))
            if not paths:
                raise RuntimeError("cannot create FSEvents root array")

            create_stream = core_services.FSEventStreamCreate
            create_stream.argtypes = [
                ctypes.c_void_p,
                callback_type,
                ctypes.c_void_p,
                ctypes.c_void_p,
                ctypes.c_uint64,
                ctypes.c_double,
                ctypes.c_uint32,
            ]
            create_stream.restype = ctypes.c_void_p
            stream = ctypes.c_void_p(
                create_stream(
                    None,
                    callback_ref,
                    None,
                    paths,
                    0xFFFFFFFFFFFFFFFF,
                    0.05,
                    0x00000002 | 0x00000004 | 0x00000010,
                )
            )
            if not stream:
                raise RuntimeError("FSEventStreamCreate failed")
            core_foundation.CFRunLoopGetCurrent.restype = ctypes.c_void_p
            run_in_mode = core_foundation.CFRunLoopRunInMode
            run_in_mode.argtypes = [ctypes.c_void_p, ctypes.c_double, ctypes.c_bool]
            run_in_mode.restype = ctypes.c_int32
            schedule = core_services.FSEventStreamScheduleWithRunLoop
            schedule.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
            start = core_services.FSEventStreamStart
            start.argtypes = [ctypes.c_void_p]
            start.restype = ctypes.c_bool
            core_services.FSEventStreamFlushSync.argtypes = [ctypes.c_void_p]
            core_services.FSEventStreamStop.argtypes = [ctypes.c_void_p]
            core_services.FSEventStreamInvalidate.argtypes = [ctypes.c_void_p]
            core_services.FSEventStreamRelease.argtypes = [ctypes.c_void_p]
            core_foundation.CFRelease.argtypes = [ctypes.c_void_p]
            run_loop = core_foundation.CFRunLoopGetCurrent()
            default_mode = ctypes.c_void_p.in_dll(
                core_foundation, "kCFRunLoopDefaultMode"
            )
            schedule(stream, run_loop, default_mode)
            if not start(stream):
                raise RuntimeError("FSEventStreamStart failed")
            # Flush after Start is the arm fence: any pre-existing journal
            # records have been delivered before the parent captures its
            # authoritative prelaunch snapshot.
            core_services.FSEventStreamFlushSync(stream)
            self._ready.set()
            while not self._stop.is_set():
                run_in_mode(default_mode, 0.10, True)
            # FlushSync is the terminal watermark.  The parent cannot consume
            # a DRAINED receipt until every event through this call returned.
            core_services.FSEventStreamFlushSync(stream)
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
            self._ready.set()
        finally:
            if stream:
                core_services.FSEventStreamStop(stream)
                core_services.FSEventStreamInvalidate(stream)
                core_services.FSEventStreamRelease(stream)
            if paths:
                core_foundation.CFRelease(paths)
            for value in cf_strings:
                core_foundation.CFRelease(value)


def _identity_paths(payload: object, *, broad_roots: Sequence[Path] = ()) -> list[Path]:
    paths: list[Path] = []
    broad = tuple(_norm(root.resolve(strict=True)) for root in broad_roots)
    path_keys = {
        "path",
        "resolved_path",
        "executable",
        "content_path",
        "entry",
        "manifest",
        "file_paths",
    }

    def visit(value: object, key: str | None = None) -> None:
        if isinstance(value, Mapping):
            owner_root = value.get("owner_root")
            if isinstance(owner_root, str):
                owner = Path(owner_root)
                if owner.is_absolute() and any(
                    _norm(owner) == root
                    or _norm(owner).startswith(root.rstrip(os.sep) + os.sep)
                    for root in broad
                ):
                    return
            for nested_key, nested in value.items():
                # Captured file rows carry both lexical_path and the canonical
                # resolved_path.  Visiting both doubles tens of thousands of
                # filesystem canonicalizations without adding authority.
                if nested_key == "lexical_path" and isinstance(
                    value.get("resolved_path"), str
                ):
                    continue
                visit(nested, str(nested_key))
        elif isinstance(value, list):
            for nested in value:
                visit(nested, key)
        elif key in path_keys and isinstance(value, str):
            candidate = process_image_capture.custody_path(Path(value))
            try:
                if candidate.is_file():
                    paths.append(candidate)
                    resolved = candidate.resolve(strict=True)
                    if _norm(resolved) != _norm(candidate):
                        paths.append(resolved)
                    # Keep every retargetable lexical ancestor, as well as the
                    # content coordinate; resolving alone loses alias events.
                    for entry in (candidate, *candidate.parents):
                        if entry.is_symlink() or entry.is_junction():
                            paths.append(entry)
            except OSError:
                pass

    visit(payload)
    return list({_norm(path): path for path in paths}.values())


def watch_specs(
    *,
    source_root: Path,
    tracked_paths: Sequence[Path],
    identities: Sequence[object],
    broad_roots: Sequence[Path],
) -> list[WatchSpec]:
    specs: list[WatchSpec] = []
    del tracked_paths
    # Source custody is broad by construction.  A transient untracked module,
    # manifest, generated source, or executable can affect a proof even when it
    # is deleted before the endpoint Git snapshot.  Build outputs must therefore
    # live in the queue's explicit external/private artifact roots rather than
    # teaching source custody heuristic exclusions.
    specs.append(WatchSpec(source_root.resolve(strict=True), None))
    for root in broad_roots:
        if root.is_dir():
            specs.append(WatchSpec(root.resolve(strict=True), None))
    by_parent: dict[str, tuple[Path, set[str]]] = {}
    source_key = _norm(source_root)
    for path in _identity_paths(list(identities), broad_roots=broad_roots):
        key = _norm(path)
        if key == source_key or key.startswith(source_key.rstrip(os.sep) + os.sep):
            continue
        parent = path.parent.resolve(strict=True)
        key = _norm(parent)
        if key not in by_parent:
            by_parent[key] = (parent, set())
        by_parent[key][1].add(_norm(path))
    specs.extend(
        WatchSpec(parent, frozenset(paths)) for parent, paths in by_parent.values()
    )
    return _compact_specs(specs)


def child_policy(
    envelope: Mapping[str, object],
    toolchains: Mapping[str, object],
    *,
    environment_executables: Mapping[str, object],
    derived_roots: Sequence[Mapping[str, object]] = (),
) -> dict[str, object]:
    closure = envelope.get("process_closure")
    if not isinstance(closure, Mapping):
        raise ValueError("proof envelope has no child-process closure authority")
    descendants = closure.get("descendants")
    if descendants not in {"forbidden", "declared-toolchains"}:
        raise ValueError("proof envelope has an unknown child-process policy")
    # The native supervisor's prelaunch provenance owns this grant. Do not
    # discover roots from the payload environment or grant arbitrary directories.
    projected_roots: list[dict[str, str]] = []
    for root in derived_roots:
        role, raw_path = root.get("role"), root.get("path")
        if (
            descendants == "forbidden"
            or root.get("run_owned") is not True
            or not isinstance(role, str)
            or not role
            or not isinstance(raw_path, str)
            or not Path(raw_path).is_absolute()
        ):
            raise ValueError(
                "child executable root requires admitted run-owned provenance"
            )
        projected_roots.append({"role": role, "path": raw_path})
    allowed: list[dict[str, str]] = []
    if descendants == "declared-toolchains":
        for name, identity in toolchains.items():
            if not isinstance(identity, Mapping):
                continue
            for image in process_image_capture.toolchain_images(str(name), identity):
                path = Path(str(image["path"]))
                allowed.append(
                    {
                        "toolchain": str(name),
                        "path": _norm(path),
                        "sha256": str(image["sha256"]),
                    }
                )
        for image in process_image_capture.environment_images(environment_executables):
            allowed.append(
                {
                    "toolchain": str(image["role"]),
                    "path": _norm(Path(str(image["path"]))),
                    "sha256": str(image["sha256"]),
                }
            )
    allowed = [
        dict(row)
        for row in {
            (row["toolchain"], row["path"], row["sha256"]): row for row in allowed
        }.values()
    ]
    allowed.sort(key=lambda row: (row["toolchain"], row["path"], row["sha256"]))
    return {
        "schema": CHILD_POLICY_SCHEMA,
        "descendants": descendants,
        "allowed": allowed,
        "derived_roots": projected_roots,
    }


def require_enforceable_process_closure(envelope: Mapping[str, object]) -> None:
    """Reject a leaf whose runtime has no pre-spawn interception authority."""
    closure = envelope.get("process_closure")
    if not isinstance(closure, Mapping):
        raise ValueError("proof envelope has no process closure")
    if closure.get("descendants") != "forbidden":
        return
    if envelope.get("python") is not None:
        return
    argv = envelope.get("argv")
    first = ""
    if isinstance(argv, list) and argv:
        first = Path(str(argv[0])).name.casefold()
    if first in {"node", "node.exe"}:
        return
    raise ValueError(
        "non-exact native launcher has no pre-spawn child custody; use the "
        "guarded typed command family"
    )


class ChildCustodyEventServer:
    """Parent-owned authenticated event channel for runtime launch hooks."""

    def __init__(
        self, expected_runtime: str | None, policy: Mapping[str, object]
    ) -> None:
        self.expected_runtime = expected_runtime
        self.policy = dict(policy)
        declared_runtimes = {
            str(authority.get("toolchain"))
            for authority in self.policy.get("allowed", [])
            if isinstance(authority, Mapping)
            and authority.get("toolchain") in {"python", "node"}
        }
        self.allowed_runtimes = frozenset(
            declared_runtimes
            | ({expected_runtime} if expected_runtime is not None else set())
        )
        self.token = secrets.token_hex(32)
        self._listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._listener.bind(("127.0.0.1", 0))
        self._listener.listen()
        self._listener.settimeout(0.20)
        host, port = self._listener.getsockname()
        self.endpoint = f"{host}:{port}"
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._handlers: list[threading.Thread] = []
        self._events: list[dict[str, object]] = []
        self._errors: list[str] = []
        self._lock = threading.Lock()
        self._next_connection_id = 0
        self._state = "CREATED"

    def environment(self) -> dict[str, str]:
        return {
            CHILD_ENDPOINT_ENV: self.endpoint,
            CHILD_TOKEN_ENV: self.token,
        }

    def __enter__(self) -> ChildCustodyEventServer:
        if self._state != "CREATED":
            raise RuntimeError(f"child custody server cannot start from {self._state}")
        self._state = "ARMED"
        self._thread = threading.Thread(
            target=self._accept, name="proof-child-custody", daemon=True
        )
        self._thread.start()
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        del exc_type, exc, traceback
        if self._state != "ARMED":
            raise RuntimeError(f"child custody server cannot drain from {self._state}")
        self._state = "DRAINING"
        self._stop.set()
        self._listener.close()
        if self._thread is not None:
            self._thread.join(timeout=5.0)
        for handler in self._handlers:
            handler.join(timeout=5.0)
            if handler.is_alive():
                self._record_error("child custody connection did not close")
        self._state = "DRAINED"

    def _record_error(self, message: str) -> None:
        with self._lock:
            self._errors.append(message)

    def _accept(self) -> None:
        while not self._stop.is_set():
            try:
                connection, _address = self._listener.accept()
            except TimeoutError:
                continue
            except OSError as exc:
                if not self._stop.is_set():
                    self._record_error(f"child custody accept failed: {exc}")
                return
            with self._lock:
                connection_id = self._next_connection_id
                self._next_connection_id += 1
            handler = threading.Thread(
                target=self._read_connection,
                args=(connection, connection_id),
                daemon=True,
            )
            handler.start()
            self._handlers.append(handler)

    def _read_connection(self, connection: socket.socket, connection_id: int) -> None:
        saw_start = False
        saw_end = False
        last_sequence = 0
        runtime: object = None
        try:
            with connection, connection.makefile("rb") as stream:
                for raw_line in stream:
                    payload = json.loads(raw_line)
                    if not isinstance(payload, dict):
                        raise ValueError("child custody event is not an object")
                    event = str(payload.get("event") or "")
                    if not saw_start:
                        token = payload.pop("token", None)
                        if event != "hook-start" or not secrets.compare_digest(
                            str(token or ""), self.token
                        ):
                            raise ValueError("child custody hook handshake failed")
                        runtime = payload.get("runtime")
                        if runtime not in self.allowed_runtimes:
                            raise ValueError("child custody runtime handshake mismatch")
                        if connection_id == 0 and runtime != self.expected_runtime:
                            raise ValueError(
                                "root child custody runtime handshake mismatch"
                            )
                        saw_start = True
                        connection.sendall(
                            (
                                json.dumps(
                                    {"event": "hook-ready", "runtime": runtime},
                                    sort_keys=True,
                                    separators=(",", ":"),
                                )
                                + "\n"
                            ).encode()
                        )
                    elif event == "hook-end":
                        if saw_end:
                            raise ValueError(
                                "duplicate child custody terminal handshake"
                            )
                        saw_end = True
                        payload["connection_id"] = connection_id
                        with self._lock:
                            self._events.append(payload)
                        break
                    elif event == "spawn-intent":
                        sequence = payload.get("sequence")
                        if (
                            not isinstance(sequence, int)
                            or sequence != last_sequence + 1
                        ):
                            raise ValueError("child custody sequence is not monotonic")
                        last_sequence = sequence
                        decision = self._decide_child(payload, runtime)
                        decision["connection_id"] = connection_id
                        with self._lock:
                            self._events.append(decision)
                        connection.sendall(
                            (
                                json.dumps(
                                    {
                                        "event": "spawn-decision",
                                        "sequence": payload.get("sequence"),
                                        **{
                                            key: value
                                            for key, value in decision.items()
                                            if key
                                            in {
                                                "admitted",
                                                "resolved",
                                                "toolchain",
                                                "reason",
                                            }
                                        },
                                    },
                                    sort_keys=True,
                                    separators=(",", ":"),
                                )
                                + "\n"
                            ).encode()
                        )
                        continue
                    elif event == "policy-violation":
                        payload = {
                            **payload,
                            "event": "child-process",
                            "admitted": False,
                        }
                    else:
                        raise ValueError(f"unknown child custody event {event!r}")
                    payload["connection_id"] = connection_id
                    with self._lock:
                        self._events.append(payload)
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
        if not saw_start:
            self._record_error("child custody connection has no authenticated start")
        elif not saw_end:
            self._record_error("child custody connection has no terminal handshake")

    def _decide_child(
        self, intent: Mapping[str, object], runtime: object
    ) -> dict[str, object]:
        """Admit the image a hook's launch runs, judged by the declared policy.

        The hook's runtime and this host select one launch model. A Python
        hook cannot choose its image, so on Windows the broker predicts
        CreateProcessW from the caller's facts; on POSIX CPython searches the
        child's PATH from the child's cwd. The Node hook runs the broker's
        selection itself, so its selection is the image on every host.
        """
        predicts_createprocess = runtime == "python" and os.name == "nt"
        token = (
            windows_createprocess.requested_module(intent)
            if predicts_createprocess
            else intent.get("requested")
        )
        try:
            path = (
                _windows_python_launch_image(intent)
                if predicts_createprocess
                else _intent_child_executable(intent)
            )
        except (OSError, ValueError) as exc:
            return {
                "event": "child-process",
                "requested": str(token),
                "resolved": None,
                "admitted": False,
                "reason": f"identity-unavailable:{exc}",
            }
        decision: dict[str, object] = {
            "event": "child-process",
            "requested": str(token),
            "resolved": str(path) if path is not None else None,
            "admitted": False,
        }
        if intent.get("shell") not in {None, False}:
            decision["reason"] = "opaque-shell"
            return decision
        if self.policy.get("descendants") != "declared-toolchains" or path is None:
            decision["reason"] = "descendants-forbidden-or-unresolved"
            return decision
        executable_name = path.name.casefold()
        if executable_name in {
            "cmd",
            "cmd.exe",
            "powershell",
            "powershell.exe",
            "pwsh",
            "pwsh.exe",
            "sh",
            "bash",
            "dash",
            "zsh",
            "fish",
        }:
            decision["reason"] = "opaque-shell"
            return decision
        if path.suffix.casefold() in {".bat", ".cmd", ".ps1"}:
            decision["reason"] = "implicit-interpreter"
            return decision
        try:
            with path.open("rb") as handle:
                if handle.read(2) == b"#!":
                    decision["reason"] = "implicit-interpreter"
                    return decision
        except OSError as exc:
            decision["reason"] = f"identity-unavailable:{type(exc).__name__}"
            return decision
        try:
            normalized = _norm(path)
            with path.open("rb") as handle:
                digest = hashlib.file_digest(handle, "sha256").hexdigest()
        except (OSError, ValueError) as exc:
            decision["reason"] = f"identity-unavailable:{type(exc).__name__}"
            return decision
        for authority in self.policy.get("allowed", []):
            if (
                isinstance(authority, Mapping)
                and authority.get("path") == normalized
                and authority.get("sha256") == digest
            ):
                decision.update(
                    {"admitted": True, "toolchain": authority.get("toolchain")}
                )
                return decision
        try:
            canonical = path.resolve(strict=True)
        except (OSError, ValueError) as exc:
            decision["reason"] = f"identity-unavailable:{type(exc).__name__}"
            return decision
        for root in self.policy.get("derived_roots", []):
            root_path = Path(root["path"])
            # Compare canonical components exactly, as the native supervisor
            # does; a case-folded lexical prefix can grant a sibling directory.
            if canonical.parts[: len(root_path.parts)] == root_path.parts:
                decision.update(
                    {
                        "admitted": True,
                        "resolved": str(canonical),
                        "derived_role": root["role"],
                        "sha256": digest,
                    }
                )
                return decision
        decision["reason"] = "outside-declared-toolchain-closure"
        return decision

    def receipt(self) -> dict[str, object]:
        with self._lock:
            events = list(self._events)
            errors = list(self._errors)
        starts = [event for event in events if event.get("event") == "hook-start"]
        ends = [event for event in events if event.get("event") == "hook-end"]
        if self.expected_runtime is not None and not starts:
            errors.append("mandatory child custody hook did not connect")
        if self._state != "DRAINED":
            errors.append(f"child custody receipt requested in state {self._state}")
        violations = [
            event
            for event in events
            if event.get("event") not in {"hook-start", "hook-end"}
            and event.get("admitted") is not True
        ]
        runtime_handshake_complete = (
            not starts and not ends
            if self.expected_runtime is None
            else bool(starts) and len(starts) == len(ends)
        )
        broker_complete = (
            self._state == "DRAINED" and not errors and runtime_handshake_complete
        )
        material = {"events": events, "errors": errors, "state": self._state}
        return {
            "schema": "molt.proof-child-custody-receipt.v3",
            "transport": "parent-owned-authenticated-loopback",
            "scope": "runtime-hook-broker",
            "state": self._state,
            "events": events,
            "errors": errors,
            "violations": violations,
            "broker_complete": broker_complete,
            # Language hooks diagnose the standard Python/Node launch surfaces;
            # only the OS supervisor can attest the complete native process tree.
            "process_closure_complete": False,
            "identity_sha256": hashlib.sha256(
                json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
        }


def child_receipt_is_admitted(receipt: Mapping[str, object]) -> bool:
    """A complete transport is not proof that every requested child was allowed."""
    events = receipt.get("events")
    return (
        receipt.get("broker_complete") is True
        and receipt.get("violations") == []
        and receipt.get("errors") == []
        and isinstance(events, list)
        and all(
            isinstance(event, Mapping)
            and (
                (
                    event.get("event") in ("hook-start", "hook-end")
                    and event.get("runtime") in ("python", "node")
                    and type(event.get("connection_id")) is int
                    and event["connection_id"] >= 0
                )
                or (
                    event.get("event") == "child-process"
                    and event.get("admitted") is True
                )
            )
            for event in events
        )
    )


def require_derived_child_image_bindings(
    receipt: Mapping[str, object], verified_event_log: Path
) -> None:
    """Bind broker decisions to the native supervisor's actual executed bytes.

    The caller first verifies the native event artifact. Fixed toolchains have
    their prelaunch identity law; generated images need this execution boundary.
    """
    expected: set[tuple[str, str, str]] = set()
    for event in receipt.get("events", []):
        if not isinstance(event, Mapping) or "derived_role" not in event:
            continue
        fields = tuple(
            event.get(name) for name in ("resolved", "sha256", "derived_role")
        )
        if not all(isinstance(value, str) and value for value in fields):
            raise ValueError("derived child decision has no exact image identity")
        expected.add(fields)
    if not expected:
        return
    observed: set[tuple[str, str, str]] = set()
    with verified_event_log.open(encoding="utf-8") as stream:
        for line in stream:
            record = loads_exact(line)
            event = record.get("event") if isinstance(record, Mapping) else None
            image = event.get("image") if isinstance(event, Mapping) else None
            if not isinstance(image, Mapping) or image.get("class") != "derived":
                continue
            for role in image.get("roles", []):
                identity = (image.get("path"), image.get("sha256"), role)
                if identity in expected:
                    observed.add(identity)
    if observed != expected:
        raise ValueError(
            "derived child decision differs from native executed image identity"
        )


class ExecutionCustodySession:
    """One ordered authority for watcher, child broker, and drain fences."""

    def __init__(
        self,
        *,
        monitor: LiveCustodyMonitor,
    ) -> None:
        self.monitor = monitor
        self.child_server: ChildCustodyEventServer | None = None
        self.state = "CREATED"
        self.lifecycle = ["CREATED"]

    def _transition(self, expected: str, next_state: str) -> None:
        if self.state != expected:
            raise RuntimeError(
                f"execution custody state is {self.state}, expected {expected}"
            )
        self.state = next_state
        self.lifecycle.append(next_state)

    def __enter__(self) -> ExecutionCustodySession:
        self.monitor.__enter__()
        self._transition("CREATED", "ARMED")
        return self

    def bind_child_server(self, child_server: ChildCustodyEventServer) -> None:
        if self.state != "ARMED" or self.child_server is not None:
            raise RuntimeError(
                "proof child custody must bind exactly once after live custody arms"
            )
        child_server.__enter__()
        self.child_server = child_server

    def mark_captured(self) -> None:
        if self.child_server is None:
            raise RuntimeError("proof child custody must bind before capture closes")
        self._transition("ARMED", "CAPTURED")

    def mark_running(self) -> None:
        self._transition("CAPTURED", "RUNNING")

    def mark_quiescent(self) -> None:
        self._transition("RUNNING", "QUIESCENT")

    def mark_verifying(self) -> None:
        self._transition("QUIESCENT", "VERIFYING")

    def drain(self) -> None:
        self._transition("VERIFYING", "DRAINING")
        try:
            self.monitor.drain()
        finally:
            if self.child_server is not None:
                self.child_server.__exit__(None, None, None)
        self._transition("DRAINING", "DRAINED")

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        del exc_type, exc, traceback
        if self.state == "RUNNING":
            self.mark_quiescent()
        if self.state in {"ARMED", "CAPTURED"}:
            # Capture failure still drains the watcher, without claiming the
            # payload ran or requiring a broker that was never admitted.
            self._transition(self.state, "VERIFYING")
        if self.state == "QUIESCENT":
            self.mark_verifying()
        if self.state == "VERIFYING":
            self.drain()

    def receipt(self) -> dict[str, object]:
        if self.state != "DRAINED":
            raise RuntimeError(
                f"execution custody receipt requested in state {self.state}"
            )
        if self.child_server is None:
            raise RuntimeError("proof child custody was never bound")
        return {
            "schema": "molt.proof-execution-custody-session.v1",
            "state": self.state,
            "lifecycle": list(self.lifecycle),
            "live_input_custody": self.monitor.receipt(),
            "child_process_custody": self.child_server.receipt(),
        }


def _windows_python_launch_image(intent: Mapping[str, object]) -> Path | None:
    image = windows_createprocess.intent_image(intent)
    return None if image is None else process_image_capture.custody_path(Path(image))


def _intent_child_executable(intent: Mapping[str, object]) -> Path | None:
    path_env = intent.get("path")
    path_ext = intent.get("path_ext")
    child_env = (
        {
            "PATH": path_env,
            **({"PATHEXT": path_ext} if isinstance(path_ext, str) else {}),
        }
        if isinstance(path_env, str)
        else None
    )
    child_cwd = intent.get("cwd")
    return _resolve_child_executable(
        intent.get("requested"),
        child_env,
        child_cwd if isinstance(child_cwd, str) else None,
    )


def _resolve_child_executable(
    token: object, child_env: object = None, child_cwd: object = None
) -> Path | None:
    """Search the child's PATH from the child's cwd, as an exec-path launch does.

    This is the image of a POSIX CPython launch, and the image the Node hook
    runs on every host, because it launches the broker's selection. It is not
    a Windows Python launch: see ``windows_createprocess``.
    """
    path = _search_child_executable(token, child_env, child_cwd)
    if path is not None and os.name == "nt" and not path.suffix:
        # libuv runs a selected path as named only when its name has an
        # extension; otherwise it appends .com or .exe and runs another file.
        raise ValueError(f"Windows child image {str(path)!r} has no extension")
    return path


def _search_child_executable(
    token: object, child_env: object, child_cwd: object
) -> Path | None:
    if isinstance(token, bytes):
        token = os.fsdecode(token)
    if not isinstance(token, str) or not token:
        return None
    candidate = Path(token)
    cwd = (
        Path(os.fsdecode(child_cwd) if isinstance(child_cwd, bytes) else child_cwd)
        if isinstance(child_cwd, (str, bytes)) and child_cwd
        else Path.cwd()
    )
    if candidate.is_absolute() or any(separator in token for separator in "/\\"):
        return process_image_capture.custody_path(cwd / candidate)
    path_entries = os.get_exec_path(
        child_env if isinstance(child_env, Mapping) else None
    )
    extensions = [""]
    if os.name == "nt" and not candidate.suffix:
        raw_extensions = None
        if isinstance(child_env, Mapping):
            raw_extensions = _environment_value(child_env, "PATHEXT")
            if isinstance(raw_extensions, bytes):
                raw_extensions = os.fsdecode(raw_extensions)
        extensions = [
            extension
            for extension in str(raw_extensions or ".COM;.EXE;.BAT;.CMD").split(
                os.pathsep
            )
            if extension
        ]
    for entry in path_entries:
        directory = Path(entry) if entry else cwd
        if not directory.is_absolute():
            directory = cwd / directory
        for extension in extensions:
            resolved = directory / f"{token}{extension}"
            if resolved.is_file() and os.access(resolved, os.X_OK):
                return process_image_capture.custody_path(resolved)
    return None
