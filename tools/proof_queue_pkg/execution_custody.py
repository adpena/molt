"""Live source/toolchain and child-process custody for proof execution.

Endpoint hashes prove only the endpoints.  This module supplies the missing
execution-time authority: kernel filesystem notifications retain any write,
rename, deletion, or metadata mutation until the parent consumes it, and the
Python/Node launch hooks reject child executables before launch unless the
admitted envelope declares their captured toolchain identity.
"""

from __future__ import annotations

import ctypes
import hashlib
import json
import os
import select
import secrets
import shlex
import socket
import struct
import sys
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping, Sequence

CHILD_POLICY_ENV = "MOLT_PROOF_CHILD_CUSTODY_JSON"
CHILD_ENDPOINT_ENV = "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT"
CHILD_TOKEN_ENV = "MOLT_PROOF_CHILD_CUSTODY_TOKEN"


def _norm(path: Path | str) -> str:
    return os.path.normcase(os.path.abspath(os.fspath(path)))


@dataclass(frozen=True)
class WatchSpec:
    root: Path
    paths: frozenset[str] | None = None

    def owns(self, candidate: Path) -> bool:
        if self.paths is None:
            return True
        normalized = _norm(candidate)
        if normalized in self.paths:
            return True
        prefix = normalized.rstrip(os.sep) + os.sep
        return any(path.startswith(prefix) for path in self.paths)


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
    broad_roots = tuple(spec.root for spec in compact if spec.paths is None)
    return [
        spec
        for spec in compact
        if not any(
            spec.root != broad and spec.root.is_relative_to(broad)
            for broad in broad_roots
        )
    ]


class LiveCustodyMonitor:
    """Fail-closed kernel event monitor for immutable execution inputs."""

    def __init__(self, specs: Sequence[WatchSpec]) -> None:
        self.specs = _compact_specs(specs)
        self._events: list[dict[str, str]] = []
        self._errors: list[str] = []
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._ready = threading.Event()
        self._thread: threading.Thread | None = None
        self._handles: list[object] = []
        self._state = "CREATED"
        self._lifecycle = ["CREATED"]

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
            errors = list(self._errors)
            state = self._state
            lifecycle = list(self._lifecycle)
        if state != "DRAINED":
            errors.append(f"proof live custody receipt requested in state {state}")
        material = {
            "events": events,
            "errors": errors,
            "state": state,
            "lifecycle": lifecycle,
        }
        return {
            "schema": "molt.proof-live-custody.v1",
            "watch_roots": len(self.specs),
            "events": events,
            "errors": errors,
            "state": state,
            "lifecycle": lifecycle,
            "stable": state == "DRAINED" and not events and not errors,
            "identity_sha256": hashlib.sha256(
                json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
        }

    def _record_error(self, message: str) -> None:
        with self._lock:
            if message not in self._errors:
                self._errors.append(message)

    def _record_event(self, spec: WatchSpec, action: str, path: Path) -> None:
        if not spec.owns(path):
            return
        event = {"action": action, "path": str(path)}
        with self._lock:
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
                if not get_result(handle, ctypes.byref(overlapped), ctypes.byref(returned), False):
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
                if event_mask & 0x40000000 and event_mask & (
                    0x00000100 | 0x00000080
                ):
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
                    0x00000001
                    | 0x00000002
                    | 0x00000004
                    | 0x00000008
                    | 0x00000020
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


def _identity_paths(
    payload: object, *, broad_roots: Sequence[Path] = ()
) -> list[Path]:
    paths: list[Path] = []
    broad = tuple(root.resolve(strict=True) for root in broad_roots)
    path_keys = {
        "path",
        "resolved_path",
        "executable",
        "content_path",
        "entry",
        "manifest",
    }

    def visit(value: object, key: str | None = None) -> None:
        if isinstance(value, Mapping):
            owner_root = value.get("owner_root")
            if isinstance(owner_root, str):
                owner = Path(owner_root)
                if owner.is_absolute() and any(
                    owner == root or owner.is_relative_to(root) for root in broad
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
            candidate = Path(value)
            try:
                if candidate.is_file():
                    paths.append(
                        candidate
                        if key == "resolved_path" and candidate.is_absolute()
                        else candidate.resolve(strict=True)
                    )
            except OSError:
                pass

    visit(payload)
    return list(dict.fromkeys(paths))


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
    for path in _identity_paths(list(identities), broad_roots=broad_roots):
        try:
            path.relative_to(source_root)
            continue
        except ValueError:
            pass
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
    envelope: Mapping[str, object], toolchains: Mapping[str, object]
) -> dict[str, object]:
    closure = envelope.get("process_closure")
    if not isinstance(closure, Mapping):
        raise ValueError("proof envelope has no child-process closure authority")
    descendants = closure.get("descendants")
    if descendants not in {"forbidden", "declared-toolchains"}:
        raise ValueError("proof envelope has an unknown child-process policy")
    allowed: list[dict[str, str]] = []
    if descendants == "declared-toolchains":
        for name, identity in toolchains.items():
            if not isinstance(identity, Mapping):
                continue
            candidates: list[tuple[object, object]] = []
            if name == "python":
                candidates.append(
                    (identity.get("executable"), identity.get("executable_sha256"))
                )
            else:
                candidates.extend(
                    [
                        (identity.get("path"), identity.get("launcher_sha256")),
                        (
                            identity.get("content_path"),
                            identity.get("executable_sha256"),
                        ),
                    ]
                )
            for raw_path, digest in candidates:
                if not isinstance(raw_path, str) or not isinstance(digest, str):
                    continue
                path = Path(raw_path)
                if not path.is_file():
                    continue
                allowed.append(
                    {"toolchain": str(name), "path": _norm(path), "sha256": digest}
                )
    return {
        "schema": "molt.proof-child-custody.v1",
        "descendants": descendants,
        "allowed": allowed,
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
                            raise ValueError("root child custody runtime handshake mismatch")
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
                            raise ValueError("duplicate child custody terminal handshake")
                        saw_end = True
                        payload["connection_id"] = connection_id
                        with self._lock:
                            self._events.append(payload)
                        break
                    elif event == "spawn-intent":
                        sequence = payload.get("sequence")
                        if not isinstance(sequence, int) or sequence != last_sequence + 1:
                            raise ValueError("child custody sequence is not monotonic")
                        last_sequence = sequence
                        decision = self._decide_child(payload)
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
                        raise ValueError(
                            f"unknown child custody event {event!r}"
                        )
                    payload["connection_id"] = connection_id
                    with self._lock:
                        self._events.append(payload)
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
        if not saw_start:
            self._record_error("child custody connection has no authenticated start")
        elif not saw_end:
            self._record_error("child custody connection has no terminal handshake")

    def _decide_child(self, intent: Mapping[str, object]) -> dict[str, object]:
        token = intent.get("requested")
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
        path = _resolve_child_executable(
            token,
            child_env,
            child_cwd if isinstance(child_cwd, str) else None,
        )
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
        normalized = _norm(path)
        try:
            with path.open("rb") as handle:
                digest = hashlib.file_digest(handle, "sha256").hexdigest()
        except OSError as exc:
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
            self._state == "DRAINED"
            and not errors
            and runtime_handshake_complete
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


class ExecutionCustodySession:
    """One ordered authority for watcher, child broker, and drain fences."""

    def __init__(
        self,
        *,
        monitor: LiveCustodyMonitor,
        child_server: ChildCustodyEventServer,
    ) -> None:
        self.monitor = monitor
        self.child_server = child_server
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
        self.child_server.__enter__()
        try:
            self.monitor.__enter__()
        except BaseException:
            self.child_server.__exit__(None, None, None)
            raise
        self._transition("CREATED", "ARMED")
        return self

    def mark_captured(self) -> None:
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
            self.child_server.__exit__(None, None, None)
        self._transition("DRAINING", "DRAINED")

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        del exc_type, exc, traceback
        if self.state == "RUNNING":
            self.mark_quiescent()
        if self.state == "CAPTURED":
            self.mark_running()
            self.mark_quiescent()
        if self.state == "ARMED":
            self.mark_captured()
            self.mark_running()
            self.mark_quiescent()
        if self.state == "QUIESCENT":
            self.mark_verifying()
        if self.state == "VERIFYING":
            self.drain()

    def receipt(self) -> dict[str, object]:
        if self.state != "DRAINED":
            raise RuntimeError(
                f"execution custody receipt requested in state {self.state}"
            )
        return {
            "schema": "molt.proof-execution-custody-session.v1",
            "state": self.state,
            "lifecycle": list(self.lifecycle),
            "live_input_custody": self.monitor.receipt(),
            "child_process_custody": self.child_server.receipt(),
        }


_child_channel: socket.socket | None = None
_child_channel_reader: object | None = None
_child_channel_lock = threading.Lock()
_child_sequence = 0


def _journal(payload: Mapping[str, object]) -> None:
    if _child_channel is None:
        raise RuntimeError("proof child custody event channel is unavailable")
    line = json.dumps(dict(payload), sort_keys=True, separators=(",", ":")) + "\n"
    with _child_channel_lock:
        _child_channel.sendall(line.encode())


def _environment_value(environment: object, name: str) -> object | None:
    if not isinstance(environment, Mapping):
        return None
    expected = name.upper()
    return next(
        (
            value
            for key, value in environment.items()
            if (os.fsdecode(key) if isinstance(key, bytes) else str(key)).upper()
            == expected
        ),
        None,
    )


def _request_child_decision(
    token: object, child_env: object = None, child_cwd: object = None
) -> dict[str, object]:
    global _child_sequence
    if _child_channel is None or _child_channel_reader is None:
        raise RuntimeError("proof child custody decision channel is unavailable")
    path_value = None
    if isinstance(child_env, Mapping):
        path_value = _environment_value(child_env, "PATH")
        if isinstance(path_value, bytes):
            path_value = os.fsdecode(path_value)
    if not isinstance(path_value, str):
        path_value = os.environ.get("PATH", "")
    path_ext = None
    if isinstance(child_env, Mapping):
        path_ext = _environment_value(child_env, "PATHEXT")
        if isinstance(path_ext, bytes):
            path_ext = os.fsdecode(path_ext)
    if not isinstance(path_ext, str):
        path_ext = os.environ.get("PATHEXT", "")
    if isinstance(child_cwd, bytes):
        child_cwd = os.fsdecode(child_cwd)
    effective_cwd = (
        os.path.abspath(child_cwd)
        if isinstance(child_cwd, str) and child_cwd
        else os.getcwd()
    )
    with _child_channel_lock:
        _child_sequence += 1
        sequence = _child_sequence
        intent = {
            "event": "spawn-intent",
            "sequence": sequence,
            "requested": os.fsdecode(token) if isinstance(token, bytes) else str(token),
            "path": path_value,
            "path_ext": path_ext,
            "cwd": effective_cwd,
        }
        _child_channel.sendall(
            (json.dumps(intent, sort_keys=True, separators=(",", ":")) + "\n").encode()
        )
        raw = _child_channel_reader.readline()
    if not raw:
        raise RuntimeError("proof child custody broker closed before decision")
    decision = json.loads(raw)
    if (
        not isinstance(decision, dict)
        or decision.get("event") != "spawn-decision"
        or decision.get("sequence") != sequence
    ):
        raise RuntimeError("proof child custody broker returned an invalid decision")
    return decision


def _resolve_child_executable(
    token: object, child_env: object = None, child_cwd: object = None
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
    if candidate.is_absolute() or candidate.parent != Path("."):
        return Path(os.path.abspath(cwd / candidate))
    path_value = None
    if isinstance(child_env, Mapping):
        path_value = _environment_value(child_env, "PATH")
        if isinstance(path_value, bytes):
            path_value = os.fsdecode(path_value)
    path_entries = (
        path_value.split(os.pathsep)
        if isinstance(path_value, str)
        else os.get_exec_path()
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
            for extension in str(
                raw_extensions or ".COM;.EXE;.BAT;.CMD"
            ).split(os.pathsep)
            if extension
        ]
    for entry in path_entries:
        directory = Path(entry) if entry else cwd
        if not directory.is_absolute():
            directory = cwd / directory
        for extension in extensions:
            resolved = directory / f"{token}{extension}"
            if resolved.is_file() and os.access(resolved, os.X_OK):
                return resolved.resolve(strict=True)
    return None


def _admit_child(
    policy: Mapping[str, object],
    token: object,
    child_env: object = None,
    child_cwd: object = None,
) -> None:
    del policy
    decision = _request_child_decision(token, child_env, child_cwd)
    if decision.get("admitted") is True:
        return
    raise PermissionError(
        f"proof child executable is outside admitted toolchain closure: {token!r}"
    )


def install_python_child_custody() -> None:
    global _child_channel, _child_channel_reader
    raw = os.environ.get(CHILD_POLICY_ENV)
    if not raw:
        return
    policy = json.loads(raw)
    if (
        not isinstance(policy, dict)
        or policy.get("schema") != "molt.proof-child-custody.v1"
    ):
        raise RuntimeError("malformed proof child custody policy")
    # Capture enforcement callables before payload execution.  The audit hook
    # must never resolve a mutable module-global name that proof code can replace
    # after bootstrap.
    admit_executable = _admit_child
    record_event = _journal
    decode_path = os.fsdecode
    split_command = shlex.split
    windows = os.name == "nt"
    endpoint = os.environ.get(CHILD_ENDPOINT_ENV, "")
    token = os.environ.get(CHILD_TOKEN_ENV, "")
    try:
        host, port_raw = endpoint.rsplit(":", 1)
        channel = socket.create_connection((host, int(port_raw)), timeout=10.0)
    except (OSError, ValueError) as exc:
        raise RuntimeError(f"proof child custody channel connection failed: {exc}") from exc
    channel.settimeout(None)
    _child_channel = channel
    _child_channel_reader = channel.makefile("rb")
    record_event(
        {
            "event": "hook-start",
            "runtime": "python",
            "pid": os.getpid(),
            "token": token,
            "admitted": True,
        }
    )
    ready_raw = _child_channel_reader.readline()
    if not ready_raw:
        raise RuntimeError("proof child custody broker closed before hook readiness")
    ready = json.loads(ready_raw)
    if not isinstance(ready, dict) or ready != {
        "event": "hook-ready",
        "runtime": "python",
    }:
        raise RuntimeError("proof child custody broker returned invalid hook readiness")

    import atexit

    def close_channel() -> None:
        global _child_channel, _child_channel_reader
        active = _child_channel
        if active is None:
            return
        try:
            record_event(
                {
                    "event": "hook-end",
                    "runtime": "python",
                    "pid": os.getpid(),
                    "admitted": True,
                }
            )
            active.shutdown(socket.SHUT_WR)
        finally:
            active.close()
            _child_channel = None
            _child_channel_reader = None

    atexit.register(close_channel)

    def audit(event: str, args: tuple[object, ...]) -> None:
        if event == "subprocess.Popen":
            executable = args[0] if args else None
            if executable is None and len(args) > 1:
                command_args = args[1]
                if isinstance(command_args, (list, tuple)) and command_args:
                    executable = command_args[0]
                elif isinstance(command_args, (str, bytes)):
                    command_line = (
                        decode_path(command_args)
                        if isinstance(command_args, bytes)
                        else command_args
                    )
                    split = split_command(command_line, posix=not windows)
                    executable = split[0].strip('"') if split else None
            child_env = args[3] if len(args) > 3 else None
            child_cwd = args[2] if len(args) > 2 else None
            admit_executable(policy, executable, child_env, child_cwd)
        elif event in {
            "os.system",
            "os.exec",
            "os.posix_spawn",
            "os.posix_spawnp",
            "os.spawn",
            "os.fork",
            "os.forkpty",
        }:
            record_event({"event": "policy-violation", "surface": event})
            raise PermissionError(
                f"opaque process creation is forbidden in proof custody: {event}"
            )
    sys.addaudithook(audit)
    # The bootstrap loaded this authority by a private file-module name.  Remove
    # every alias to that module before returning to payload code so the proof
    # cannot mutate enforcement globals through sys.modules.
    authority_module = sys.modules.get(__name__)
    if authority_module is not None:
        for module_name, loaded in tuple(sys.modules.items()):
            if loaded is authority_module:
                sys.modules.pop(module_name, None)
