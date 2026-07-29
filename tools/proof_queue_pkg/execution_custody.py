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
import shlex
import shutil
import struct
import sys
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping, Sequence

CHILD_POLICY_ENV = "MOLT_PROOF_CHILD_CUSTODY_JSON"
CHILD_JOURNAL_ENV = "MOLT_PROOF_CHILD_CUSTODY_JOURNAL"


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
    return [
        WatchSpec(root, None if paths is None else frozenset(paths))
        for root, paths in merged.values()
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

    def __enter__(self) -> LiveCustodyMonitor:
        if sys.platform == "win32":
            target = self._run_windows
        elif sys.platform.startswith("linux"):
            target = self._run_linux
        else:
            raise RuntimeError(
                f"proof live custody has no lossless kernel watcher on {sys.platform}"
            )
        self._thread = threading.Thread(
            target=target, name="proof-live-custody", daemon=True
        )
        self._thread.start()
        if not self._ready.wait(timeout=10.0):
            self._stop.set()
            raise RuntimeError("proof live custody watcher did not become ready")
        if self._errors:
            raise RuntimeError(self._errors[0])
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        del exc_type, exc, traceback
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

    def receipt(self) -> dict[str, object]:
        with self._lock:
            events = list(self._events)
            errors = list(self._errors)
        material = {"events": events, "errors": errors}
        return {
            "schema": "molt.proof-live-custody.v1",
            "watch_roots": len(self.specs),
            "events": events,
            "errors": errors,
            "stable": not events and not errors,
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
        try:
            for spec in self.specs:
                handle = create_file(
                    str(spec.root),
                    0x0001,
                    0x00000001 | 0x00000002 | 0x00000004,
                    None,
                    3,
                    0x02000000,
                    None,
                )
                if int(handle) == invalid:
                    raise OSError(
                        ctypes.get_last_error(),
                        f"cannot watch proof custody root {spec.root}",
                    )
                self._handles.append(handle)
                thread = threading.Thread(
                    target=self._read_windows_root,
                    args=(spec, handle, read_changes),
                    daemon=True,
                )
                thread.start()
                threads.append(thread)
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
        self, spec: WatchSpec, handle: object, read_changes: object
    ) -> None:
        from ctypes import wintypes

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
        while not self._stop.is_set():
            buffer = ctypes.create_string_buffer(64 * 1024)
            returned = wintypes.DWORD()
            ok = read_changes(
                handle,
                buffer,
                len(buffer),
                True,
                notify_filter,
                ctypes.byref(returned),
                None,
                None,
            )
            if not ok:
                error = ctypes.get_last_error()
                if self._stop.is_set() and error in {0, 995}:
                    return
                self._record_error(
                    f"ReadDirectoryChangesW failed for {spec.root}: winerror={error}"
                )
                return
            if returned.value == 0:
                self._record_error(f"ReadDirectoryChangesW overflowed for {spec.root}")
                continue
            offset = 0
            while offset < returned.value:
                next_offset, action, name_bytes = struct.unpack_from(
                    "<III", buffer.raw, offset
                )
                start = offset + 12
                name = buffer.raw[start : start + name_bytes].decode("utf-16-le")
                self._record_event(
                    spec, actions.get(action, str(action)), spec.root / name
                )
                if next_offset == 0:
                    break
                offset += next_offset

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
        try:
            for spec in self.specs:
                for directory in [
                    spec.root,
                    *[p for p in spec.root.rglob("*") if p.is_dir()],
                ]:
                    wd = add(fd, os.fsencode(directory), mask)
                    if wd < 0:
                        raise OSError(
                            ctypes.get_errno(),
                            f"cannot watch proof custody root {directory}",
                        )
                    watches[wd] = (spec, directory)
            self._ready.set()
            while not self._stop.is_set():
                readable, _, _ = select.select([fd], [], [], 0.25)
                if not readable:
                    continue
                payload = os.read(fd, 1024 * 1024)
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
        except BaseException as exc:
            self._record_error(f"{type(exc).__name__}: {exc}")
            self._ready.set()


def _identity_paths(payload: object) -> list[Path]:
    paths: list[Path] = []
    path_keys = {
        "path",
        "resolved_path",
        "executable",
        "content_path",
        "entry",
        "manifest",
        "lexical_path",
    }

    def visit(value: object, key: str | None = None) -> None:
        if isinstance(value, Mapping):
            for nested_key, nested in value.items():
                visit(nested, str(nested_key))
        elif isinstance(value, list):
            for nested in value:
                visit(nested, key)
        elif key in path_keys and isinstance(value, str):
            candidate = Path(value)
            try:
                if candidate.is_file():
                    paths.append(candidate.resolve(strict=True))
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
    source_files = frozenset(_norm(path) for path in tracked_paths)
    if source_files:
        specs.append(WatchSpec(source_root.resolve(strict=True), source_files))
    for root in broad_roots:
        if root.is_dir():
            specs.append(WatchSpec(root.resolve(strict=True), None))
    by_parent: dict[str, tuple[Path, set[str]]] = {}
    for path in _identity_paths(list(identities)):
        try:
            path.relative_to(source_root)
            if _norm(path) in source_files:
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


def _journal(payload: Mapping[str, object]) -> None:
    raw = os.environ.get(CHILD_JOURNAL_ENV)
    if not raw:
        return
    line = json.dumps(dict(payload), sort_keys=True, separators=(",", ":")) + "\n"
    descriptor = os.open(raw, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, line.encode())
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _resolve_child_executable(token: object, child_env: object = None) -> Path | None:
    if isinstance(token, bytes):
        token = os.fsdecode(token)
    if not isinstance(token, str) or not token:
        return None
    candidate = Path(token)
    if candidate.is_absolute() or candidate.parent != Path("."):
        return Path(os.path.abspath(candidate))
    path_value = None
    if isinstance(child_env, Mapping):
        path_value = child_env.get("PATH") or child_env.get(b"PATH")
        if isinstance(path_value, bytes):
            path_value = os.fsdecode(path_value)
    found = shutil.which(
        token, path=path_value if isinstance(path_value, str) else None
    )
    return Path(found) if found else None


def _admit_child(
    policy: Mapping[str, object], token: object, child_env: object = None
) -> None:
    path = _resolve_child_executable(token, child_env)
    event: dict[str, object] = {
        "event": "child-process",
        "requested": os.fsdecode(token) if isinstance(token, bytes) else str(token),
        "resolved": str(path) if path is not None else None,
        "admitted": False,
    }
    if policy.get("descendants") == "declared-toolchains" and path is not None:
        normalized = _norm(path)
        digest = None
        try:
            with path.open("rb") as handle:
                digest = hashlib.file_digest(handle, "sha256").hexdigest()
        except OSError:
            pass
        for authority in policy.get("allowed", []):
            if (
                isinstance(authority, Mapping)
                and authority.get("path") == normalized
                and authority.get("sha256") == digest
            ):
                event.update(
                    {"admitted": True, "toolchain": authority.get("toolchain")}
                )
                _journal(event)
                return
    _journal(event)
    raise PermissionError(
        f"proof child executable is outside admitted toolchain closure: {token!r}"
    )


def install_python_child_custody() -> None:
    raw = os.environ.get(CHILD_POLICY_ENV)
    if not raw:
        return
    policy = json.loads(raw)
    if (
        not isinstance(policy, dict)
        or policy.get("schema") != "molt.proof-child-custody.v1"
    ):
        raise RuntimeError("malformed proof child custody policy")

    def audit(event: str, args: tuple[object, ...]) -> None:
        if event == "subprocess.Popen":
            executable = args[0] if args else None
            if executable is None and len(args) > 1:
                command_args = args[1]
                if isinstance(command_args, (list, tuple)) and command_args:
                    executable = command_args[0]
                elif isinstance(command_args, (str, bytes)):
                    command_line = (
                        os.fsdecode(command_args)
                        if isinstance(command_args, bytes)
                        else command_args
                    )
                    split = shlex.split(command_line, posix=os.name != "nt")
                    executable = split[0].strip('"') if split else None
            child_env = args[3] if len(args) > 3 else None
            _admit_child(policy, executable, child_env)
        elif event in {
            "os.system",
            "os.posix_spawn",
            "os.posix_spawnp",
            "os.spawn",
            "os.fork",
            "os.forkpty",
        }:
            _journal({"event": event, "admitted": False})
            raise PermissionError(
                f"opaque process creation is forbidden in proof custody: {event}"
            )
        elif policy.get("descendants") == "forbidden" and event in {
            "ctypes.dlopen",
            "ctypes.dlsym",
            "ctypes.dlsym/handle",
            "ctypes.call_function",
        }:
            _journal({"event": event, "admitted": False})
            raise PermissionError(
                f"native process bypass surface is forbidden: {event}"
            )

    sys.addaudithook(audit)


def child_journal_receipt(path: Path) -> dict[str, object]:
    events: list[dict[str, object]] = []
    errors: list[str] = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        lines = []
    except OSError as exc:
        lines = []
        errors.append(f"{type(exc).__name__}: {exc}")
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError as exc:
            errors.append(f"JSONDecodeError: {exc}")
            continue
        if not isinstance(event, dict):
            errors.append("child journal contains a non-object event")
            continue
        events.append(event)
    violations = [event for event in events if event.get("admitted") is not True]
    material = {"events": events, "errors": errors}
    return {
        "schema": "molt.proof-child-custody-receipt.v1",
        "events": events,
        "errors": errors,
        "violations": violations,
        "complete": not errors and not violations,
        "identity_sha256": hashlib.sha256(
            json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
    }
