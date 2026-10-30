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
import tempfile
import threading
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import BinaryIO, Literal, cast, get_args

from molt import python_environment_contract as _contract  # noqa: E402
from molt import process_launch
from molt.exact_json import canonical_json_sha256, loads_exact
from molt.process_environment import NameRule, ProcessEnvironment
from molt.python_environment_location import validate_python_environment_location
from molt.python_runtime_identity import (
    PYTHON_RUNTIME_SELECTION_SCHEMA,
    python_runtime_selection_locations,
)
from molt.toolchain_identity import (
    StableRegularFileVersion,
    open_stable_regular_file,
    stable_executable_probe,
    stable_regular_file_version,
    verify_stable_regular_file_identity,
)
from tools.proof_queue_pkg import process_image_capture, windows_createprocess

from tools.proof_queue_pkg.python_child_custody import (
    CHILD_POLICY_ENV as CHILD_POLICY_ENV,
    CHILD_ENDPOINT_ENV as CHILD_ENDPOINT_ENV,
    CHILD_TOKEN_ENV as CHILD_TOKEN_ENV,
    install_python_child_custody as install_python_child_custody,
)


def _norm(path: Path | str) -> str:
    return process_image_capture._image_path_key(Path(path))


def _event_key(candidate: Path) -> str:
    """The custody key of an event path, including one that has vanished.

    Windows custody names an actual directory entry and raises for an absent
    one, so a renamed-away or deleted entry takes its key from its nearest
    existing ancestor plus the names below it. After coordinate validation,
    POSIX custody is lexical. Raises for invalid coordinates or when no ancestor
    has a key.
    """
    if not candidate.is_absolute():
        raise ValueError(f"watch custody requires an absolute path: {candidate}")
    process_image_capture.require_custody_coordinate(candidate)
    try:
        return _norm(candidate)
    except (OSError, ValueError):
        pass
    names: list[str] = []
    current = candidate
    while current.parent != current:
        if "~" in current.name:
            # The Windows watcher may report an 8.3 short name, which no key
            # can match once the entry is gone; its custody is unknown.
            raise ValueError(f"vanished watch event may be a short name: {candidate}")
        names.append(current.name)
        current = current.parent
        try:
            return os.path.join(_norm(current), *reversed(names))
        except (OSError, ValueError):
            continue
    raise ValueError(f"watch event has no custody key: {candidate}")


@dataclass(frozen=True)
class WatchSpec:
    root: Path
    paths: frozenset[str] | None = None
    _root_key: str = field(init=False, repr=False, compare=False)

    def __post_init__(self) -> None:
        if not self.root.is_absolute():
            raise ValueError(f"watch custody requires an absolute root: {self.root}")
        object.__setattr__(self, "_root_key", _norm(self.root))

    def owns(self, candidate: Path) -> bool:
        normalized = _event_key(candidate)
        # Canonical entry strings preserve Windows case-sensitive directories
        # and lexical aliases. Path containment would fold distinct entries.
        if normalized != self._root_key and not normalized.startswith(
            self._root_key.rstrip(os.sep) + os.sep
        ):
            return False
        if self.paths is None:
            return True
        if normalized in self.paths:
            return True
        prefix = normalized.rstrip(os.sep) + os.sep
        return any(path.startswith(prefix) for path in self.paths)


CHILD_POLICY_SCHEMA = "molt.proof-child-custody.v2"
CHILD_CUSTODY_RECEIPT_SCHEMA = "molt.proof-child-custody-receipt.v4"


def node_bootstrap_options() -> str:
    """The queue-owned Node observer options, without caller engine settings."""
    hook = Path(__file__).with_name("node_child_custody.cjs").resolve(strict=True)
    return f"--no-global-search-paths --require={hook}"


# v3: every apparatus class is validated; selected empty uv-lock closes carry
# captured Python/uv owner identities. Older permissive receivers cannot admit it.
LIVE_CUSTODY_RECEIPT_SCHEMA = "molt.proof-live-custody.v3"

# How long the arm and drain fences wait for the watcher to deliver their
# cookie. Watchman's `sync_timeout` default is 60 s for the same protocol
# (https://facebook.github.io/watchman/docs/cookies.html). A cookie that never
# arrives means the watcher cannot vouch for the window, so the receipt fails
# closed.
LIVE_CUSTODY_COOKIE_TIMEOUT_S = 60.0

# Why live input custody refuses before any watcher, worker, provisioning or
# payload. The code is the identity; the detail is its prose projection.
# ``platform-unsupported``: the platform has no admitted lossless watcher.
# macOS is such a platform: XNU posts FSEvents content modification only in
# vn_close, so a write held open across a fence reaches no cookie-ordered
# stream, and Watchman's cookie sync documents FSEvents as unsupported
# (https://github.com/facebook/watchman/blob/182b779c88ca2b64de1f5ddeb55a597df7d90253/website/docs/cookies.md#limitation-macos-fsevents).
# A future kqueue backend belongs with the macOS process-closure cell (HF-07).
LiveCustodyRefusalReason = Literal["platform-unsupported"]
LIVE_CUSTODY_REFUSAL_REASONS: frozenset[str] = frozenset(
    get_args(LiveCustodyRefusalReason)
)
_LIVE_CUSTODY_REFUSAL_DETAIL: Mapping[LiveCustodyRefusalReason, str] = {
    "platform-unsupported": "no lossless input-mutation watcher is admitted",
}

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
    broad_roots = tuple(spec._root_key for spec in compact if spec.paths is None)
    return [
        spec
        for spec in compact
        if not any(
            spec._root_key.startswith(broad.rstrip(os.sep) + os.sep)
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


class LiveCustodyUnavailable(RuntimeError):
    """Live input custody refuses this platform before any watcher or payload.

    The refusal is an unsupported cell (HF-552): its consumers skip with the
    typed reason and never pass, as for the native process-closure refusal.
    """

    def __init__(
        self,
        platform: str,
        reason: LiveCustodyRefusalReason = "platform-unsupported",
    ) -> None:
        if reason not in LIVE_CUSTODY_REFUSAL_REASONS:
            raise ValueError(f"unknown live custody refusal reason {reason!r}")
        self.platform = platform
        self.reason: LiveCustodyRefusalReason = reason
        self.capability: dict[str, object] = {
            "platform": platform,
            "admission": {"state": "ineligible", "reason": reason},
        }
        super().__init__(
            f"live input custody prelaunch refused on {platform}: {reason} "
            f"({_LIVE_CUSTODY_REFUSAL_DETAIL[reason]})"
        )

    @classmethod
    def decode(
        cls, capability: object, *, expected_platform: str
    ) -> LiveCustodyUnavailable:
        """Decode one recorded refusal exactly; any other shape is not one."""
        if not isinstance(capability, dict) or set(capability) != {
            "platform",
            "admission",
        }:
            raise ValueError("live custody refusal capability has the wrong shape")
        admission = capability["admission"]
        if not isinstance(admission, dict) or set(admission) != {"state", "reason"}:
            raise ValueError("live custody refusal admission has the wrong shape")
        reason = admission["reason"]
        if admission["state"] != "ineligible" or reason not in (
            LIVE_CUSTODY_REFUSAL_REASONS
        ):
            raise ValueError(f"unknown live custody refusal {admission!r}")
        platform = capability["platform"]
        if platform != expected_platform or live_custody_platform_supported(
            str(platform)
        ):
            raise ValueError(
                f"live custody refusal names platform {platform!r} on "
                f"{expected_platform!r}"
            )
        return cls(expected_platform, cast(LiveCustodyRefusalReason, reason))


def live_custody_platform_supported(platform: str) -> bool:
    """Whether ``platform`` has an admitted lossless input-mutation watcher."""
    return platform == "win32" or platform.startswith("linux")


class LiveCustodyMonitor:
    """Fail-closed kernel event monitor for immutable execution inputs.

    Linux inotify and Windows ``ReadDirectoryChangesW`` are the admitted
    watchers; every other platform refuses before a watcher thread starts
    (``LiveCustodyUnavailable``, ``platform-unsupported``). macOS refuses:
    XNU posts FSEvents content modification only when the writer closes the
    file, so no fence can order a write that is still open.

    On Linux the receipt covers the window between two cookie fences, as
    Watchman's cookie sync does
    (https://facebook.github.io/watchman/docs/cookies.html). inotify posts
    ``IN_MODIFY`` at write time and one descriptor queues every watch in order,
    so one cookie fences: each fence writes a unique cookie into a private
    directory watched by the same descriptor as every root and waits until the
    watcher delivers it; every earlier event has then arrived. Events delivered
    before the arm fence precede the window (the prelaunch snapshot follows
    ARMED), events after the drain fence follow it, and cookie events are never
    inputs. A cookie that does not arrive within
    ``LIVE_CUSTODY_COOKIE_TIMEOUT_S`` is an error, so a stalled watcher cannot
    certify a quiet window. The fence directory is the monitor's own, under
    ``TMPDIR``; cookies never go into a root's existing directories: roots
    include unwritable system directories, and roots that concurrent proofs
    share would see a foreign cookie as an input mutation.

    Windows watches each root through its own handle, so no single cookie can
    order them. Its file systems report a change while the changing operation
    runs and buffer it until the next read, so draining every handle until a
    fresh read no longer completes at once is its terminal watermark, as
    reading inotify to ``EAGAIN`` is on Linux.
    """

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
        # The private fence directory and its cookies, keyed by the path the
        # watcher reports. A watcher driven without fences records everything
        # it delivers; ``__enter__`` closes the window until the arm cookie.
        self._cookie_root: Path | None = None
        self._cookies: dict[str, tuple[str, threading.Event]] = {}
        self._window = "open"

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
            if not any(spec.owns(path) for spec in self.specs):
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

    @staticmethod
    def require_supported_platform() -> None:
        """Refuse before any watcher, worker, provisioning or payload."""
        if not live_custody_platform_supported(sys.platform):
            raise LiveCustodyUnavailable(sys.platform)

    def __enter__(self) -> LiveCustodyMonitor:
        self.require_supported_platform()
        fenced = sys.platform != "win32"
        target = self._run_linux if fenced else self._run_windows
        if fenced:
            # The watcher arms this directory beside the roots; it must exist
            # first. It is new and private to this monitor.
            self._cookie_root = Path(
                tempfile.mkdtemp(prefix="molt-live-custody-")
            ).resolve(strict=True)
            self._window = "awaiting-arm-cookie"
        self._transition("CREATED", "ARMING")
        self._thread = threading.Thread(
            target=target, name="proof-live-custody", daemon=True
        )
        self._thread.start()
        if not self._ready.wait(timeout=10.0):
            self._record_error("proof live custody watcher did not become ready")
        elif not self._errors and fenced:
            self._sync_cookie("arm", "open")
        if self._errors:
            self._transition("ARMING", "ARMED")
            self.drain()
            raise RuntimeError(self._errors[0])
        self._transition("ARMING", "ARMED")
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        del exc_type, exc, traceback
        self.drain()

    def _sync_cookie(self, name: str, window: str) -> None:
        """Write one cookie; move the receipt window once the watcher delivers it."""
        assert self._cookie_root is not None
        cookie = self._cookie_root / f"{name}-{secrets.token_hex(16)}"
        delivered = threading.Event()
        with self._lock:
            self._cookies[str(cookie)] = (window, delivered)
        try:
            os.close(os.open(cookie, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600))
        except OSError as exc:
            self._record_error(
                f"proof live custody cannot write its {name} cookie: {exc}"
            )
            return
        if not delivered.wait(timeout=LIVE_CUSTODY_COOKIE_TIMEOUT_S):
            self._record_error(
                f"proof live custody watcher did not deliver its {name} cookie "
                f"within {LIVE_CUSTODY_COOKIE_TIMEOUT_S:g} s"
            )

    def _observe_cookie(self, path: Path) -> bool:
        """Consume a fence event: one of this monitor's cookies or their directory."""
        if path == self._cookie_root:
            return True
        with self._lock:
            cookie = self._cookies.get(str(path))
            if cookie is None:
                return False
            window, delivered = cookie
            if not delivered.is_set():
                # Set on the watcher thread, in delivery order: every event
                # delivered before this one stays on the earlier side.
                self._window = window
                delivered.set()
        return True

    def _remove_cookie_root(self) -> None:
        if self._cookie_root is None:
            return
        try:
            for raw_cookie in self._cookies:
                Path(raw_cookie).unlink(missing_ok=True)
            self._cookie_root.rmdir()
        except OSError as exc:
            # Only this monitor writes there; anything else breaks the fence.
            self._record_error(f"proof live custody cookie directory changed: {exc}")

    def drain(self) -> None:
        """Fence and consume the platform event stream exactly once."""
        with self._lock:
            if self._state == "DRAINED":
                return
            # Only an armed fence can close; a failed arm already recorded why.
            fenced = self._cookie_root is not None and self._window == "open"
        self._transition("ARMED", "DRAINING")
        if fenced:
            self._sync_cookie("drain", "closed")
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
        self._remove_cookie_root()
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

    def _record_event(self, spec: WatchSpec, action: str, path: Path) -> bool:
        try:
            if not spec.owns(path):
                return False
            apparatus = classify_apparatus_event(spec.root, path, action)
            uv_key = _norm(path) if action == "inotify:0x8" else None
        except (OSError, ValueError) as exc:
            # An uncertain event must invalidate the receipt, never establish
            # positive admission or escape a native callback without evidence.
            self._record_error(
                f"watch custody failed for {path}: {type(exc).__name__}: {exc}"
            )
            return False
        event = {"action": action, "path": str(path)}
        with self._lock:
            if self._window != "open":
                # Owned, but outside the fenced window: before the prelaunch
                # snapshot, or after the payload's quiescent fence.
                return True
            if uv_key is not None:
                uv_lock = self._uv_lock_records.get(uv_key)
                if uv_lock is not None:
                    if uv_lock not in self._apparatus_events:
                        self._apparatus_events.append(dict(uv_lock))
                    return True
            if apparatus is not None:
                classified = {**event, "apparatus": apparatus}
                if classified not in self._apparatus_events:
                    self._apparatus_events.append(classified)
                return True
            if event not in self._events:
                self._events.append(event)
        return True

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
        cancel_io = kernel32.CancelIoEx
        cancel_io.argtypes = [wintypes.HANDLE, ctypes.POINTER(Overlapped)]
        cancel_io.restype = wintypes.BOOL
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
            # After the stop fence the handle is read until a fresh read finds
            # nothing buffered. The file system buffers each change between
            # reads and completes the next read from that buffer before it
            # returns, so a read still pending holds no change from before the
            # fence: the terminal watermark. Stopping at the first batch after
            # the fence lost every change buffered behind it.
            draining = False
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
                if draining and wait_for_single(event, 0) != 0:
                    cancel_io(handle, ctypes.byref(overlapped))
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
                        if draining:
                            return
                        draining = True
                        continue
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
                draining = draining or self._stop.is_set()
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
                if self._observe_cookie(candidate) or directory == self._cookie_root:
                    # The private fence directory holds no input. Its watch
                    # replaces a root's watch of the same inode (inotify keeps
                    # one per inode); anything else written there fails the
                    # directory's removal at drain.
                    continue
                owned = self._record_event(spec, f"inotify:{event_mask:#x}", candidate)
                if (
                    event_mask & 0x40000000
                    and event_mask & (0x00000100 | 0x00000080)
                    and owned
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
                if spec.paths is None:
                    for candidate in spec.root.rglob("*"):
                        if candidate.is_dir():
                            add_directory(spec, candidate)
                else:
                    # Finite selectors need only their existing ancestors.
                    # Never enumerate unrelated siblings of an exact input.
                    ancestors = {
                        parent
                        for path in spec.paths
                        for parent in Path(path).parents
                        if parent != spec.root and parent.is_relative_to(spec.root)
                    }
                    for directory in sorted(
                        ancestors, key=lambda path: len(path.parts)
                    ):
                        if directory.is_dir():
                            add_directory(spec, directory)
            if self._cookie_root is not None:
                # Last, so this watch owns the inode even inside a root. One
                # descriptor queues every watch in order: the fence cookie.
                add_directory(
                    WatchSpec(self._cookie_root, frozenset()), self._cookie_root
                )
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


class WatchCoordinates:
    """Project lexical paths onto the canonical entries that live watches own.

    Watch roots are canonical directories, and every watcher reports events
    beneath them: inotify and ReadDirectoryChangesW under the registered root.
    A lexical spelling through a symlink or
    junction, such as uv's minor-version installation link (the
    ``cpython-<major>.<minor>-<platform>`` entry that points at its patch
    release directory), lies beneath no root. It selects its input through entries named inside their
    canonical parents: every alias on the path (a retarget changes the
    selection), the path's own entry, and the entry's resolved target (whose
    bytes are the input). Watch selection and the armed-coverage check use
    this one projection, so a watch selects exactly the entries it can own.

    One instance serves one consumer pass. It memoizes each directory, so a
    manifest of many rows in few directories costs one ``lstat`` per distinct
    directory component plus one per row.
    """

    def __init__(self) -> None:
        self._directories: dict[str, tuple[Path, tuple[Path, ...]]] = {}

    def _directory(self, directory: Path) -> tuple[Path, tuple[Path, ...]]:
        """The canonical directory and the alias entries that select it."""
        key = str(directory)
        known = self._directories.get(key)
        if known is not None:
            return known
        if directory.parent == directory:
            projected: tuple[Path, tuple[Path, ...]] = (directory, ())
        else:
            parent, aliases = self._directory(directory.parent)
            entry = parent / directory.name
            if entry.is_symlink() or entry.is_junction():
                projected = (entry.resolve(strict=False), (*aliases, entry))
            else:
                projected = (entry, aliases)
        self._directories[key] = projected
        return projected

    def of(self, path: Path, *, strict: bool = False) -> tuple[Path, ...]:
        """Every canonical entry that must be armed for ``path`` to be armed.

        ``strict`` requires the selected input to exist. Absent startup
        selectors are not strict: their entry still needs creation events.
        """
        if not path.is_absolute():
            raise ValueError(f"watch custody requires an absolute path: {path}")
        process_image_capture.require_custody_coordinate(path)
        parent, aliases = self._directory(path.parent)
        entry = parent / path.name
        if strict or entry.is_symlink() or entry.is_junction():
            resolved = entry.resolve(strict=strict)
        else:
            resolved = entry
        return (*aliases, entry) if resolved == entry else (*aliases, entry, resolved)


def unarmed_paths(specs: Sequence[WatchSpec], paths: Iterable[Path]) -> list[str]:
    """The paths that some entry selecting them leaves outside every live watch."""
    coordinates = WatchCoordinates()
    return [
        str(path)
        for path in paths
        if not all(
            any(spec.owns(coordinate) for spec in specs)
            for coordinate in coordinates.of(path)
        )
    ]


def _identity_paths(payload: object, *, broad_roots: Sequence[Path] = ()) -> list[Path]:
    paths: list[Path] = []
    coordinates = WatchCoordinates()
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
            if value.get("schema") == _contract.PYTHON_ENVIRONMENT_LOCATION_SCHEMA:
                location = validate_python_environment_location(dict(value))
                for role in (
                    "selected_executable",
                    "base_executable",
                    "base_executable_selector",
                ):
                    add_path(Path(str(location[role])), selected=True)
                for raw_path in cast(list[str], location["file_paths"]):
                    add_path(Path(raw_path), selected=True)
                return
            if value.get("schema") == PYTHON_RUNTIME_SELECTION_SCHEMA:
                for location, _directory_role in python_runtime_selection_locations(
                    value
                ):
                    for spelling in ("lexical", "resolved"):
                        add_path(Path(str(location[spelling])), selected=True)
                return
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
        elif key == "resource_roots" and isinstance(value, str):
            # Native compiler resources may be files, trees, or aliases. The
            # broad projection admits only real directory content; retain the
            # exact selected coordinate and every retargetable alias here.
            add_path(Path(value), selected=True)
        elif key in path_keys and isinstance(value, str):
            path = Path(value)
            # Runtime inventory members are relative to their declared root.
            # Absolute file custody and startup selections own their inputs;
            # a root-reference must not select a same-named file beneath cwd.
            if not path.is_absolute():
                return
            candidate = process_image_capture.custody_path(path)
            try:
                if candidate.is_file():
                    add_path(candidate, selected=False)
            except OSError:
                pass

    def add_path(candidate: Path, *, selected: bool) -> None:
        # Keep every retargetable alias entry, the selected entry and its
        # content coordinate. Absent startup selectors also need creation
        # events.
        paths.extend(coordinates.of(candidate, strict=not selected))

    visit(payload)
    return list({_event_key(path): path for path in paths}.values())


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
        key = _event_key(path)
        if key == source_key or key.startswith(source_key.rstrip(os.sep) + os.sep):
            continue
        parent = path.parent
        while not parent.is_dir():
            if parent.parent == parent:
                raise ValueError(
                    f"selected watch input has no directory ancestor: {path}"
                )
            parent = parent.parent
        parent = parent.resolve(strict=True)
        key = _norm(parent)
        if key not in by_parent:
            by_parent[key] = (parent, set())
        by_parent[key][1].add(_event_key(path))
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
    allowed: list[dict[str, object]] = []
    if descendants == "declared-toolchains":
        for name, identity in toolchains.items():
            if not isinstance(identity, Mapping):
                continue
            python_bytecode = None
            if name == "python":
                from molt.python_environment_location import environment_bytecode_prefix

                location = identity.get("location")
                prefix = (
                    location.get("prefix") if isinstance(location, Mapping) else None
                )
                if not isinstance(prefix, str) or not Path(prefix).is_absolute():
                    raise ValueError(
                        "Python child custody requires captured environment location"
                    )
                python_bytecode = {
                    "environment_root": prefix,
                    "pycache_prefix": str(environment_bytecode_prefix(Path(prefix))),
                }
            images = process_image_capture.toolchain_images(str(name), identity)
            python_interpreters = {
                str(image["path"])
                for image in images
                if name == "python"
                and _python_image_launch_kind(str(image["role"])) == "interpreter"
            }
            windows_transports: dict[str, str] = {}
            if name == "python" and os.name == "nt":
                for image in images:
                    path = str(image["path"])
                    if path in python_interpreters and path not in windows_transports:
                        windows_transports[path] = (
                            _windows_python_interpreter_transport(
                                Path(path),
                                python_interpreters,
                                sha256=str(image["sha256"]),
                            )
                        )
            for image in images:
                path = Path(str(image["path"]))
                python_authority: dict[str, object] = {}
                if name == "python":
                    launch_kind = _python_image_launch_kind(str(image["role"]))
                    python_authority = {
                        "python_launch": launch_kind,
                        "python_bytecode": (
                            None if launch_kind == "native" else python_bytecode
                        ),
                    }
                    if launch_kind == "interpreter" and os.name == "nt":
                        python_authority["python_windows_transport"] = (
                            windows_transports[str(path)]
                        )
                    if launch_kind == "console-script":
                        if os.name == "nt" and path.suffix.casefold() == ".exe":
                            startup, launcher_transport = (
                                _windows_python_console_startup(
                                    path,
                                    python_interpreters,
                                    sha256=str(image["sha256"]),
                                )
                            )
                            python_authority["python_startup_argv"] = startup
                            python_authority["python_windows_transport"] = (
                                "unicode"
                                if launcher_transport
                                == windows_transports[_norm(Path(startup[0]))]
                                == "unicode"
                                else "ascii"
                            )
                        else:
                            python_authority["python_startup_argv"] = None
                allowed.append(
                    {
                        "toolchain": str(name),
                        "path": _norm(path),
                        "sha256": str(image["sha256"]),
                        **python_authority,
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
    distinct: dict[tuple[object, ...], dict[str, object]] = {}
    for row in allowed:
        # Preserve every applicable startup contract when producer roles share
        # an image. A native or generic grant cannot erase either obligation.
        key = (row["toolchain"], row["path"], row["sha256"], row.get("python_launch"))
        previous = distinct.setdefault(key, row)
        if previous != row:
            raise ValueError("Python child image has conflicting startup authority")
    allowed = list(distinct.values())
    allowed.sort(key=lambda row: (row["toolchain"], row["path"], row["sha256"]))
    return {
        "schema": CHILD_POLICY_SCHEMA,
        "descendants": descendants,
        "allowed": allowed,
        "derived_roots": projected_roots,
    }


def _python_image_launch_kind(role: str) -> str:
    """Project launch semantics from the existing producer's closed roles."""
    if role in {"python", "selected-interpreter", "base-interpreter"} or (
        role.startswith("interpreter-alias:")
        and role.removeprefix("interpreter-alias:")
    ):
        return "interpreter"
    if role.startswith("console-script:") and role.removeprefix("console-script:"):
        return "console-script"
    if role.startswith("distribution-executable:") and role.removeprefix(
        "distribution-executable:"
    ):
        return "native"
    raise ValueError(f"Python child image has unknown launch role: {role!r}")


def _windows_python_console_startup(
    path: Path, interpreters: set[str], *, sha256: str
) -> tuple[list[str], str]:
    """Decode a captured console image's actual Python startup prefix.

    distlib/scripts.py writes launcher + shebang + a one-entry ZIP archive;
    PC/launcher.c finds the executable's .exe boundary and places the embedded
    options before the quoted launcher path. Caller arguments follow the
    script, so even --help/-I/-X remain payload arguments. Unknown native
    launcher grammars cannot establish Python startup custody.

    https://github.com/pypa/distlib/blob/master/PC/launcher.c
    https://github.com/pypa/distlib/blob/master/distlib/scripts.py
    https://github.com/astral-sh/uv/blob/main/crates/uv-trampoline/src/bounce.rs
    """
    from molt.python_interpreter import parse_python_invocation
    from tools.proof_command_argv import (
        split_windows_python_command_line,
    )

    with (
        stable_executable_probe(path, label="Python console launcher") as (
            entrypoint,
            identity,
        ),
        open_stable_regular_file(
            identity.path, label="Python console launcher", observed=identity
        ) as opened,
    ):
        metadata = _windows_uv_launcher_metadata(entrypoint)
        transport = "unicode" if metadata is None or metadata["utf8"] else "ascii"
        if metadata is None:
            interpreter, options = _distlib_python_console_prefix(opened.stream)
        else:
            if metadata["kind"] != "script":
                raise ValueError("Python console image is not a script trampoline")
            interpreter = str(metadata["interpreter"])
            if (
                not (str(interpreter).isascii() and str(path).isascii())
                and not metadata["utf8"]
            ):
                raise ValueError(
                    "uv Python console requires its captured UTF-8 manifest"
                )
            # uv's Script resource places the launcher script immediately
            # after the interpreter. It accepts no embedded interpreter flags.
            options = ""
        if (
            not Path(interpreter).is_absolute()
            or _norm(Path(interpreter)) not in interpreters
        ):
            raise ValueError("Python console launcher interpreter is outside capture")
        argv = split_windows_python_command_line(f'"{interpreter}" {options} "{path}"')
        invocation = parse_python_invocation(argv)
        if (
            invocation.mode != "script"
            or invocation.target != str(path)
            or invocation.arguments
        ):
            raise ValueError("Python console launcher has no exact script boundary")
        opened.stream.seek(0)
        if hashlib.file_digest(opened.stream, "sha256").hexdigest() != sha256:
            raise ValueError("Python console startup differs from its captured image")
        return list(argv), transport


def _windows_python_interpreter_transport(
    path: Path, interpreters: set[str], *, sha256: str
) -> str:
    """Capture the actual Windows interpreter/proxy text transport once."""
    with (
        stable_executable_probe(path, label="Python interpreter launcher") as (
            entrypoint,
            identity,
        ),
        open_stable_regular_file(
            identity.path, label="Python interpreter launcher", observed=identity
        ) as opened,
    ):
        metadata = _windows_uv_launcher_metadata(entrypoint)
        if metadata is not None and (
            metadata["kind"] != "python"
            or _norm(Path(str(metadata["interpreter"]))) not in interpreters
        ):
            raise ValueError("Python interpreter proxy is outside its captured closure")
        if (
            metadata is not None
            and not metadata["utf8"]
            and not (str(path).isascii() and str(metadata["interpreter"]).isascii())
        ):
            raise ValueError(
                "uv Python interpreter requires its captured UTF-8 manifest"
            )
        if hashlib.file_digest(opened.stream, "sha256").hexdigest() != sha256:
            raise ValueError(
                "Python interpreter transport differs from its captured image"
            )
        # Native CPython and its venv redirector use wide Windows APIs. uv uses
        # UTF-8 bytes and CreateProcessA, requiring its own activation manifest.
        return "unicode" if metadata is None or metadata["utf8"] else "ascii"


def _windows_uv_launcher_metadata(path: Path) -> dict[str, object] | None:
    """Read uv's explicit PE launch metadata without executing the image."""
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    load_library = kernel.LoadLibraryExW
    load_library.argtypes = [wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD]
    load_library.restype = wintypes.HMODULE
    find_resource = kernel.FindResourceW
    find_resource.argtypes = [wintypes.HMODULE, ctypes.c_void_p, ctypes.c_void_p]
    find_resource.restype = wintypes.HANDLE
    size_of_resource = kernel.SizeofResource
    size_of_resource.argtypes = [wintypes.HMODULE, wintypes.HANDLE]
    size_of_resource.restype = wintypes.DWORD
    load_resource = kernel.LoadResource
    load_resource.argtypes = [wintypes.HMODULE, wintypes.HANDLE]
    load_resource.restype = wintypes.HANDLE
    lock_resource = kernel.LockResource
    lock_resource.argtypes = [wintypes.HANDLE]
    lock_resource.restype = ctypes.c_void_p
    free_library = kernel.FreeLibrary
    free_library.argtypes = [wintypes.HMODULE]
    free_library.restype = wintypes.BOOL
    # LOAD_LIBRARY_AS_DATAFILE: resource inspection does not call DllMain or
    # execute image initialization, exactly as uv's Launcher metadata reader.
    module = load_library(str(path), None, 0x00000002)
    if not module:
        raise ValueError("Python console launcher cannot be read as Windows data")
    try:

        def resource(name: str | int, kind: int = 10) -> bytes | None:
            identifier = (
                ctypes.c_wchar_p(name)
                if isinstance(name, str)
                else ctypes.c_void_p(name)
            )
            handle = find_resource(
                module, ctypes.cast(identifier, ctypes.c_void_p), ctypes.c_void_p(kind)
            )
            if not handle:
                return None
            size = size_of_resource(module, handle)
            if not 0 < size <= 128 * 1024:
                raise ValueError("uv Python console resource has invalid size")
            loaded = load_resource(module, handle)
            pointer = lock_resource(loaded) if loaded else None
            if not pointer:
                raise ValueError("uv Python console resource cannot be read")
            return ctypes.string_at(pointer, size)

        kind = resource("UV_TRAMPOLINE_KIND")
        if kind is None:
            return None
        raw_path = resource("UV_PYTHON_PATH")
        if kind not in {b"\x01", b"\x02"} or raw_path is None or b"\x00" in raw_path:
            raise ValueError("uv Python launcher has unknown trampoline metadata")
        try:
            interpreter = Path(raw_path.decode("utf-8"))
        except UnicodeError as exc:
            raise ValueError("uv Python console interpreter is not UTF-8") from exc
        if not interpreter.is_absolute():
            interpreter = path.parent / interpreter
            interpreter = interpreter.resolve(strict=True)
        elif kind == b"\x01":
            # Script trampolines canonicalize even an absolute target; Python
            # trampolines preserve absolute selected paths to retain venvs.
            interpreter = interpreter.resolve(strict=True)
        # uv's build.rs embeds its own UTF-8 activation manifest. The broker's
        # active codepage cannot prove the separate launcher's interpretation.
        return {
            "kind": "script" if kind == b"\x01" else "python",
            "interpreter": str(interpreter),
            "utf8": sys.getwindowsversion().build >= 18362
            and _windows_console_manifest_uses_utf8(resource(1, 24)),  # RT_MANIFEST
        }
    finally:
        if not free_library(module):
            raise ValueError("Python console resource ownership could not be released")


def _windows_console_manifest_uses_utf8(raw: bytes | None) -> bool:
    """Read the launcher's own Windows 1903+ activeCodePage declaration.

    https://github.com/astral-sh/uv/blob/main/crates/uv-trampoline/build.rs
    https://learn.microsoft.com/windows/win32/sbscs/application-manifests
    """
    from xml.etree import ElementTree

    if raw is None:
        return False
    try:
        text = raw.decode("utf-8-sig")
        if "<!DOCTYPE" in text.upper() or "<!ENTITY" in text.upper():
            return False
        manifest = ElementTree.fromstring(text)
    except (UnicodeError, ElementTree.ParseError):
        return False
    if manifest.tag != "{urn:schemas-microsoft-com:asm.v1}assembly":
        return False
    declarations = manifest.findall(
        "./{urn:schemas-microsoft-com:asm.v3}application/"
        "{urn:schemas-microsoft-com:asm.v3}windowsSettings/"
        "{http://schemas.microsoft.com/SMI/2019/WindowsSettings}activeCodePage"
    )
    return (
        len(declarations) == 1
        and declarations[0].text == "UTF-8"
        and not declarations[0].attrib
        and not len(declarations[0])
    )


def _distlib_python_console_prefix(stream: BinaryIO) -> tuple[str, str]:
    """Decode distlib's .exe boundary from its exact appended archive form."""
    import re
    import zipfile

    try:
        stream.seek(0)
        if stream.read(2) != b"MZ":
            raise ValueError("Python console launcher is not a Windows image")
        stream.seek(0, os.SEEK_END)
        file_size = stream.tell()
        if file_size < 22:
            raise ValueError("Python console launcher has no ZIP end record")
        stream.seek(-22, os.SEEK_END)
        end = struct.unpack("<4s4H2LH", stream.read(22))
        # Bound ZipFile's metadata allocation before it reads the central
        # directory. A single ZIP entry has one fixed header and three 16-bit
        # length fields; distlib writes no archive comment or ZIP64 records.
        if (
            end[0] != b"PK\x05\x06"
            or end[1:5] != (0, 0, 1, 1)
            or end[5] > 46 + 3 * 65535
            or end[6] == 0xFFFFFFFF
            or end[7] != 0
        ):
            raise ValueError("Python console launcher has unknown archive shape")
        if file_size >= 42:
            stream.seek(-42, os.SEEK_END)
            if stream.read(4) == b"PK\x06\x07":
                raise ValueError(
                    "Python console launcher has unsupported ZIP64 metadata"
                )
        with zipfile.ZipFile(stream) as archive:
            entries = archive.infolist()
            if (
                archive.comment
                or len(entries) != 1
                or entries[0].filename != "__main__.py"
            ):
                raise ValueError("Python console launcher has unknown archive shape")
            start = entries[0].header_offset
            # distlib's C launcher anchors to the classic archive base, not a
            # separately indexed local entry. Its one entry begins at offset0.
            archive_base = file_size - 22 - end[5] - end[6]
            if not 0 <= archive_base == start < file_size:
                raise ValueError("Python console launcher has invalid archive offset")
            stream.seek(max(0, start - 260))
            before_zip = stream.read(min(start, 260))
        marker = before_zip.rfind(b"#!")
        if marker < 0:
            raise ValueError("Python console launcher has no bounded distlib shebang")
        shebang = before_zip[marker + 2 :]
        if not shebang.endswith(b"\n") or any(
            byte in shebang[:-1] for byte in (0, 10, 13)
        ):
            raise ValueError("Python console launcher shebang has unknown terminator")
        line = shebang[:-1].decode("utf-8").lstrip(" \t")
        boundary = re.search(r'\.exe(?=$|[" \t])', line, re.IGNORECASE)
        if boundary is None:
            raise ValueError("Python console launcher has no explicit interpreter")
        end = boundary.end()
        if line.startswith('"'):
            if line[end : end + 1] != '"':
                raise ValueError("Python console launcher interpreter quote is open")
            interpreter, tail = line[1:end], line[end + 1 :]
        else:
            interpreter, tail = line[:end], line[end:]
        if (tail and tail[0] not in " \t") or '"' in interpreter:
            raise ValueError("Python console launcher interpreter boundary is unknown")
        return interpreter, tail
    except (OSError, UnicodeError, zipfile.BadZipFile) as exc:
        raise ValueError("Python console launcher startup cannot be captured") from exc


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
        identity: dict[str, object] = {}
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
                        pid = payload.get("pid")
                        thread_id = payload.get("thread_id")
                        if type(pid) is not int or pid <= 0:
                            raise ValueError(
                                "child custody process identity is invalid"
                            )
                        identity = {"runtime": runtime, "pid": pid}
                        if runtime == "node":
                            if type(thread_id) is not int or thread_id < 0:
                                raise ValueError(
                                    "Node custody thread identity is invalid"
                                )
                            identity["thread_id"] = thread_id
                            if thread_id != 0:
                                parent_id = payload.get("parent_connection_id")
                                if type(parent_id) is not int or parent_id < 0:
                                    raise ValueError(
                                        "Node Worker creator identity is invalid"
                                    )
                                identity["parent_connection_id"] = parent_id
                        saw_start = True
                        connection.sendall(
                            (
                                json.dumps(
                                    {
                                        "event": "hook-ready",
                                        "runtime": runtime,
                                        **(
                                            {"connection_id": connection_id}
                                            if runtime == "node"
                                            else {}
                                        ),
                                    },
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
                    elif event in {"node-worker-start", "node-worker-exit"}:
                        if runtime != "node":
                            raise ValueError("Worker observation is not from Node")
                        # The authenticated connection owns parent identity. A
                        # payload cannot nominate a different observing parent.
                        payload = {
                            "event": event,
                            **identity,
                            "child_thread_id": payload.get("child_thread_id"),
                            "connection_id": connection_id,
                            "admitted": True,
                            **(
                                {"exit_code": payload.get("exit_code")}
                                if event == "node-worker-exit"
                                else {}
                            ),
                        }
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
        # A terminated Worker can close its socket without executing JS exit
        # hooks. Only its creator's actual exit observation can close that
        # lifetime; validate the joined event stream after every handler drains.

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
        matching = [
            authority
            for authority in self.policy.get("allowed", [])
            if (
                isinstance(authority, Mapping)
                and authority.get("path") == normalized
                and authority.get("sha256") == digest
            )
        ]
        # An overlapping generic tool grant must not bypass the captured
        # interpreter's startup contract. Validate before choosing any grant.
        for authority in matching:
            if authority.get("toolchain") == "python":
                try:
                    _validate_python_child_launch(
                        authority, intent, windows=predicts_createprocess
                    )
                except ValueError as exc:
                    decision["reason"] = f"python-bytecode-custody:{exc}"
                    return decision
        for authority in matching:
            decision.update({"admitted": True, "toolchain": authority.get("toolchain")})
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
        if self.expected_runtime is not None and not starts:
            errors.append("mandatory child custody hook did not connect")
        if self.expected_runtime is None and starts:
            errors.append("child custody hook connected without a runtime owner")
        if self._state != "DRAINED":
            errors.append(f"child custody receipt requested in state {self._state}")
        violations = [
            event
            for event in events
            if event.get("event") not in {"hook-start", "hook-end"}
            and event.get("admitted") is not True
        ]
        lifetime_error = _child_hook_lifetime_error(events)
        if lifetime_error is not None:
            errors.append(lifetime_error)
        broker_complete = self._state == "DRAINED" and not errors
        material = {"events": events, "errors": errors, "state": self._state}
        return {
            "schema": CHILD_CUSTODY_RECEIPT_SCHEMA,
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


def _child_hook_lifetime_error(events: list[object]) -> str | None:
    """Validate hooks and creator-observed Worker exits, never socket EOF.

    Creation binds a child thread to one authenticated creator connection.
    Every creation needs that creator's exit observation, even when the child
    also ran its own terminal hook. Native process closure remains a separate
    mandatory supervisor fact.
    """
    starts: dict[int, tuple[str, int, int | None, int | None]] = {}
    ended: set[int] = set()
    worker_connections: dict[tuple[int, int], int] = {}
    created: set[tuple[int, int]] = set()
    exited: set[tuple[int, int]] = set()
    for event in events:
        if not isinstance(event, Mapping):
            return "child custody event is not an object"
        kind = event.get("event")
        connection_id = event.get("connection_id")
        if type(connection_id) is not int or connection_id < 0:
            return "child custody event has no connection identity"
        if kind == "hook-start":
            runtime, pid = event.get("runtime"), event.get("pid")
            thread_id = event.get("thread_id") if runtime == "node" else None
            parent_id = event.get("parent_connection_id")
            if (
                runtime not in ("python", "node")
                or type(pid) is not int
                or pid <= 0
                or connection_id in starts
                or (runtime == "node" and (type(thread_id) is not int or thread_id < 0))
                or (
                    runtime == "node"
                    and thread_id != 0
                    and (type(parent_id) is not int or parent_id < 0)
                )
                or ((runtime != "node" or thread_id == 0) and parent_id is not None)
            ):
                return "child custody start identity is invalid or duplicated"
            starts[connection_id] = (runtime, pid, thread_id, parent_id)
            if parent_id is not None:
                key = (parent_id, thread_id)
                if key in worker_connections:
                    return "Node Worker has multiple connections"
                worker_connections[key] = connection_id
            continue
        if connection_id not in starts or connection_id in ended:
            return "child custody event is outside its authenticated lifetime"
        runtime, pid, thread_id, parent_id = starts[connection_id]
        if kind == "child-process":
            continue  # Launch decisions and violations retain their own admission.
        if (
            event.get("runtime") != runtime
            or event.get("pid") != pid
            or (runtime == "node" and event.get("thread_id") != thread_id)
            or event.get("parent_connection_id") != parent_id
        ):
            return "child custody event identity differs from its connection"
        if kind == "hook-end":
            ended.add(connection_id)
        elif kind in ("node-worker-start", "node-worker-exit"):
            child_thread_id = event.get("child_thread_id")
            if (
                runtime != "node"
                or type(child_thread_id) is not int
                or child_thread_id <= 0
                or child_thread_id == thread_id
                or event.get("admitted") is not True
            ):
                return "Node Worker identity is invalid"
            key = (connection_id, child_thread_id)
            if kind == "node-worker-start":
                if key in created:
                    return "Node Worker creation is duplicated"
                created.add(key)
            else:
                if key not in created or key in exited:
                    return "Node Worker exit has no unique matching creator"
                if type(event.get("exit_code")) is not int:
                    return "Node Worker exit code is invalid"
                exited.add(key)
        else:
            return "unknown child custody event"
    if created != exited:
        return "Node Worker creation has no creator-observed exit"
    for connection_id, (runtime, pid, thread_id, parent_id) in starts.items():
        if parent_id is not None:
            if (parent_id, thread_id) not in created:
                return "Node Worker hook has no matching creation"
            parent = starts[parent_id]
            if parent[0] != "node" or parent[1] != pid:
                return "Node Worker hook differs from its creator process"
        elif connection_id not in ended:
            return "child custody connection has no terminal handshake"
    # Cross-connection arrival order is irrelevant: a child can authenticate
    # before its creator's start event reaches the broker. Reject orphan/cyclic
    # ancestry, joining each connection to a root at most once.
    rooted: set[int] = set()
    for connection_id in starts:
        chain: set[int] = set()
        current = connection_id
        while current not in rooted:
            if current in chain:
                return "Node Worker ancestry is cyclic"
            chain.add(current)
            parent_id = starts[current][3]
            if parent_id is None:
                break
            current = parent_id
        rooted.update(chain)
    return None


def child_receipt_is_admitted(receipt: Mapping[str, object]) -> bool:
    """A complete transport is not proof that every requested child was allowed."""
    events = receipt.get("events")
    return (
        receipt.get("schema") == CHILD_CUSTODY_RECEIPT_SCHEMA
        and receipt.get("broker_complete") is True
        and receipt.get("violations") == []
        and receipt.get("errors") == []
        and isinstance(events, list)
        and _child_hook_lifetime_error(events) is None
        and all(
            event.get("event") != "child-process" or event.get("admitted") is True
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


def _validate_python_child_launch(
    authority: Mapping[str, object], intent: Mapping[str, object], *, windows: bool
) -> None:
    """Read only captured image policy and the actual prelaunch observation."""
    from molt.python_environment_location import environment_bytecode_prefix
    from molt.python_interpreter import parse_python_invocation
    from tools.proof_command_argv import (
        split_windows_python_command_line,
        validate_python_environment_command,
        validate_windows_python_launch_prefix,
    )

    if "python_bytecode" not in authority:
        raise ValueError("Python image has no captured bytecode authority")
    launch_kind = authority.get("python_launch")
    if launch_kind not in {"interpreter", "console-script", "native"}:
        raise ValueError("Python image has no captured launch semantics")
    selection = authority["python_bytecode"]
    if launch_kind == "native":
        if selection is not None:
            raise ValueError("Native executable has conflicting Python authority")
        return
    if not isinstance(selection, Mapping) or set(selection) != {
        "environment_root",
        "pycache_prefix",
    }:
        raise ValueError("Python bytecode authority is malformed")
    root = selection.get("environment_root")
    if not isinstance(root, str) or not Path(root).is_absolute():
        raise ValueError("Python environment root is not captured")
    if selection.get("pycache_prefix") != str(environment_bytecode_prefix(Path(root))):
        raise ValueError("Python bytecode namespace disagrees with captured location")
    if launch_kind == "console-script":
        argv = authority.get("python_startup_argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            raise ValueError("Python console startup has no captured argv")
        invocation = parse_python_invocation(argv)
        if (
            invocation.mode != "script"
            or invocation.target != authority.get("path")
            or invocation.arguments
        ):
            raise ValueError("Python console startup has no exact script boundary")
        if os.name == "nt":
            transport = authority.get("python_windows_transport")
            if transport not in {"unicode", "ascii"}:
                raise ValueError("Windows Python text transport is not captured")
            observed = intent.get("command_line") if windows else intent.get("argv")
            values = (
                [*argv, observed]
                if isinstance(observed, str)
                else [*argv, *(observed if isinstance(observed, list) else [])]
            )
            if transport == "ascii" and any(
                not isinstance(arg, str) or not arg.isascii() for arg in values
            ):
                raise ValueError(
                    "Windows Python launcher has unproven Unicode transport"
                )
    elif windows:
        command_line = intent.get("command_line")
        if not isinstance(command_line, str) or not command_line:
            raise ValueError("Windows Python command line is unavailable")
        transport = authority.get("python_windows_transport")
        if transport not in {"unicode", "ascii"}:
            raise ValueError("Windows Python text transport is not captured")
        if transport == "ascii" and not command_line.isascii():
            raise ValueError("Windows Python launcher has unproven Unicode transport")
        validate_windows_python_launch_prefix(command_line)
        argv = split_windows_python_command_line(command_line)
    else:
        argv = intent.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            raise ValueError("Python child argv is unavailable")
        if os.name == "nt" and intent.get("windows_verbatim_arguments") is not False:
            raise ValueError(
                "Python verbatim Windows argv cannot establish startup options"
            )
        if os.name == "nt":
            transport = authority.get("python_windows_transport")
            if transport not in {"unicode", "ascii"}:
                raise ValueError("Windows Python text transport is not captured")
            if transport == "ascii" and any(not arg.isascii() for arg in argv):
                raise ValueError(
                    "Windows Python launcher has unproven Unicode transport"
                )
            # libuv serializes ordinary argv, but arbitrary argv0 quotes and
            # control delimiters do not share the interpreter/forwarder split.
            if not argv or any(character in argv[0] for character in '"\x00\r\n\v\f'):
                raise ValueError(
                    "Windows Python launcher argv0 has ambiguous forwarding"
                )
    environment = intent.get("python_environment")
    names = {"PYTHONPYCACHEPREFIX", "PYTHONDONTWRITEBYTECODE"}
    if not isinstance(environment, Mapping) or set(environment) != names:
        raise ValueError("Python child environment observation is unavailable")
    if any(
        value is not None and not isinstance(value, str)
        for value in environment.values()
    ):
        raise ValueError("Python child environment observation is malformed")
    validate_python_environment_command(
        argv,
        environment_root=Path(root),
        environment={
            key: value for key, value in environment.items() if isinstance(value, str)
        },
    )


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
    path_entries = process_launch.posix_search(child_env, os.fspath(cwd)).path
    extensions = [""]
    if os.name == "nt" and not candidate.suffix:
        raw_extensions = None
        if isinstance(child_env, Mapping):
            raw_extensions = ProcessEnvironment(child_env, rule=NameRule.WINDOWS).get(
                "PATHEXT"
            )
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
