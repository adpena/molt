"""Where compiler code reports the inputs it reads
(docs/design/build_early_cutoff.md).

Readers call these functions whether or not a build is recorded; with no
recording they cost one global read. While `molt.build_record` records a
build, facts go to its ledger in this process and to a per-process log in each
worker. An audit hook observes the reads no reader reports.

Lowering code imports this module, so it is part of the frontend's semantic
fingerprint: it stays small, and it imports no other Molt module at import
time.
"""

from __future__ import annotations

import errno
import json
import os
import stat
import threading
from enum import IntFlag, StrEnum
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Protocol, assert_never

# (device, inode, mode, file size or POSIX directory size, mtime_ns, content-change ns)
StatToken = tuple[int, int, int, int, int, int]
# A lookup's resolved target and file type; an empty target carries the errno
# of a lookup that failed for a reason other than absence; None is absent.
ProbeOutcome = tuple[str, int] | None
ReadReceiptId = tuple[int, int]


class FileEvidence(IntFlag):
    UNBOUND = 1
    CUSTODY = 2
    CONSUMED = 4


class WorkerFactKind(StrEnum):
    OBSERVED = "observed"
    FILE = "file"
    DIRECTORY = "directory"
    PROBE = "probe"
    REFUSE = "refuse"
    BUILD_OUTPUT = "build_output"
    BUILD_PRODUCTS = "build_products"
    BUILD_CLAIM = "build_claim"
    BUILD_RESULT = "build_result"
    READ_STARTED = "read_started"
    READ_FINISHED = "read_finished"
    CONSUMED_FILE = "consumed_file"


def stat_token(
    path: Path | str, metadata: os.stat_result | None = None
) -> StatToken | None:
    """The token of ``path`` (following links), or None when it cannot be
    stated or the platform cannot report its content-change time."""
    try:
        metadata = metadata if metadata is not None else os.stat(path)
    except (OSError, ValueError):
        return None
    if os.name == "nt":
        from molt.file_hashing import content_change_time_ns

        change = content_change_time_ns(Path(path), metadata)
        if change is None:
            return None
    else:
        change = metadata.st_ctime_ns
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        # Windows by-name and handle metadata can report 4096 and 0 for
        # the same unchanged directory. Its allocation length is not a
        # namespace generation; retain object identity and both change clocks.
        0 if os.name == "nt" and stat.S_ISDIR(metadata.st_mode) else metadata.st_size,
        metadata.st_mtime_ns,
        change,
    )


def resolved_probe(path: Path | str) -> ProbeOutcome:
    """Resolve one lookup the way the build saw it: absent, failed with an
    errno, or a target of one file type."""
    try:
        resolved = os.path.realpath(path, strict=True)
        return resolved, stat.S_IFMT(os.stat(resolved).st_mode)
    except (FileNotFoundError, NotADirectoryError):
        return None
    except OSError as error:
        return "", error.errno or errno.EIO
    except (RuntimeError, ValueError):
        return "", errno.ELOOP


class FactSink(Protocol):
    """The recorder that receives facts in the recording process."""

    def file(
        self, path: Path | str, token: StatToken | None, sha256: str | None
    ) -> None: ...

    def consumed_file(
        self, path: str, token: StatToken | None, sha256: str
    ) -> None: ...

    def read_started(self, receipt: ReadReceiptId, path: str) -> None: ...

    def read_finished(
        self,
        receipt: ReadReceiptId,
        path: str,
        token: StatToken | None,
        sha256: str | None,
    ) -> None: ...

    def directory(self, path: Path | str, token: StatToken | None) -> None: ...

    def probe(self, path: Path | str, outcome: ProbeOutcome) -> None: ...

    def refuse(self, reason: str) -> None: ...

    def observe(self, entry: tuple[str, str]) -> None: ...

    def build_output(self, selection: object) -> None: ...

    def build_products(self, products: object, /) -> None: ...

    def build_claim(self, claim: object, /) -> None: ...

    def build_result(self, result: object, /) -> None: ...


# Process-global, not context-local: frontend worker threads report to it.
_SINK: FactSink | None = None
_CAPTURE_RESULT = False
# Data files a reader reports on every use; repeats cost a set lookup.
_REPORTED: set[str] = set()
# Owners of immutable captures already reported to this recording, by id. The
# value keeps each owner alive, so its id cannot name a later object.
_REPORTED_OWNERS: dict[int, object] = {}
_REPORTED_LOCK = threading.Lock()

# The audit hook sees every readable file this process opens, every directory
# it lists and every program it starts. Worker processes log their facts to
# a directory the recording process merges.
_HOOK_EVENTS = frozenset({"open", "os.listdir", "os.scandir", "subprocess.Popen"})
_HOOK_INSTALLED = False
_HOOK_GUARD = threading.local()
# Set in the recording process (for its workers) and in each worker (its own
# log). A worker opens its log on its first fact.
_WORKER_LOG_DIR: str | None = None
_WORKER_LOG: Any = None
_WORKER_FAILURE_SUFFIX = ".failed"
_WORKER_LOG_LOCK = threading.RLock()
_READ_CAPTURE = threading.local()
_RECORDING_SESSION: object | None = None
_READ_SEQUENCE = 0


def install_sink(sink: FactSink, log_dir: str, *, capture_result: bool = False) -> None:
    """Send this process's facts to ``sink`` and its workers' to ``log_dir``."""
    global _SINK, _WORKER_LOG_DIR, _RECORDING_SESSION, _CAPTURE_RESULT
    _install_hook()
    with _REPORTED_LOCK:
        _REPORTED.clear()
        _REPORTED_OWNERS.clear()
        _WORKER_LOG_DIR = log_dir
        _SINK = sink
        _CAPTURE_RESULT = capture_result
        _RECORDING_SESSION = object()


def remove_sink() -> None:
    global _SINK, _WORKER_LOG_DIR, _RECORDING_SESSION, _CAPTURE_RESULT
    _CAPTURE_RESULT = False
    with _REPORTED_LOCK:
        _SINK = None
        _WORKER_LOG_DIR = None
        _RECORDING_SESSION = None


_O_TMPFILE = getattr(os, "O_TMPFILE", 0)


def _reads_existing_content(flags: int) -> bool:
    # An open that creates a new file (O_CREAT|O_EXCL, or Linux's unnamed
    # O_TMPFILE) or truncates one with write access reads nothing that
    # existed before it: `tempfile` opens both ways (HF-250).
    access = flags & (os.O_WRONLY | os.O_RDWR)
    if (
        flags & (os.O_CREAT | os.O_EXCL) == os.O_CREAT | os.O_EXCL
        or (_O_TMPFILE and flags & _O_TMPFILE == _O_TMPFILE)
        or (access and flags & os.O_TRUNC)
    ):
        return False
    return access != os.O_WRONLY or bool(flags & os.O_APPEND)


def _worker_log(entry: list[Any]) -> None:
    """Append one fact to this worker's log; a lost fact marks the log failed."""
    global _WORKER_LOG
    log_dir = _WORKER_LOG_DIR
    if log_dir is None or _SINK is not None:
        return
    name = os.path.join(log_dir, f"{os.getpid()}.jsonl")
    with _WORKER_LOG_LOCK:
        busy = getattr(_HOOK_GUARD, "busy", False)
        _HOOK_GUARD.busy = True
        try:
            if _WORKER_LOG is None:
                _WORKER_LOG = open(name, "a", encoding="utf-8")  # noqa: SIM115
            _WORKER_LOG.write(json.dumps(entry) + "\n")
            _WORKER_LOG.flush()
        except (OSError, TypeError, ValueError):
            try:
                open(name + _WORKER_FAILURE_SUFFIX, "ab").close()
            except OSError:
                pass
        finally:
            _HOOK_GUARD.busy = busy


def _observe(entry: tuple[str, str]) -> None:
    # The observing interpreter owns cache layout, including pycache_prefix.
    # Keep the actual read as well as its source dependency: CPython validates
    # timestamp/size or source hash, not equality of cached code with source.
    kind, name = entry
    names = (name,)
    if kind == "file" and name.endswith((".pyc", ".pyo")):
        import importlib.util

        # Actual bytecode remains an input even for an unmapped cache name.
        report_data_file(name)
        try:
            source = importlib.util.source_from_cache(name)
        except (ValueError, NotImplementedError):
            pass  # Unmapped/sourceless bytecode remains an actual file input.
        else:
            # CPython strips the source drive under pycache_prefix on Windows:
            # C:\same\a.py and D:\same\a.py can name the very same cache.
            # No inverse can prove which source the loader stat-ed. Retain the
            # actual cache and refuse reuse rather than invent its drive.
            if os.name == "nt" and not os.path.splitdrive(source)[0]:
                refuse(f"bytecode source drive is ambiguous: {name}")
            else:
                names += (source,)
                # Explicit potential-input facts survive product-root filtering.
                # A readable-open audit is pre-syscall: absence is a probe, never
                # proof that the process created or successfully read this file.
                report_data_file(source)
    sink = _SINK
    for observed in names:
        if sink is not None:
            sink.observe((kind, observed))
        else:
            _worker_log([WorkerFactKind.OBSERVED, kind, observed])


def _audit_hook(event: str, args: tuple[Any, ...]) -> None:
    if event not in _HOOK_EVENTS or (_SINK is None and _WORKER_LOG_DIR is None):
        return
    if getattr(_HOOK_GUARD, "busy", False):
        return
    _HOOK_GUARD.busy = True
    try:
        if event == "open":
            # CPython raises every "open" event with int flags: FileIO and
            # os.open pass their open(2) flags, and the fopen-style producers
            # (_Py_fopen_obj, _Py_wfopen, readline) pass 0, a read. Flags of
            # any other type raise here and refuse the record.
            path, _mode, flags = args
            if isinstance(path, (str, bytes, os.PathLike)) and _reads_existing_content(
                flags
            ):
                name = os.path.abspath(os.fsdecode(path))
                pending = getattr(_READ_CAPTURE, "pending", None)
                if (
                    pending is not None
                    and pending.current()
                    and pending.path == name
                    and not pending.covered_open
                ):
                    pending.covered_open = True
                else:
                    _observe(("file", name))
        elif event in ("os.listdir", "os.scandir"):
            path = args[0]
            if path is None:
                path = "."
            if isinstance(path, (str, bytes, os.PathLike)):
                _observe(("directory", os.path.abspath(os.fsdecode(path))))
        else:
            executable, argv, cwd, env = args
            program = executable if executable is not None else (argv or [None])[0]
            if isinstance(program, (str, bytes, os.PathLike)):
                _report_program_selection(
                    os.fsdecode(program),
                    env if env is not None else os.environ,
                    Path(os.fsdecode(cwd)) if cwd is not None else Path.cwd(),
                )
    except Exception:  # noqa: BLE001 - an audit hook must never break the build
        refuse(f"unobservable {event} event")
    finally:
        _HOOK_GUARD.busy = False


def _report_program_selection(
    program: str, environment: Mapping[Any, Any], cwd: Path
) -> None:
    """Report how a started program is selected, through the search authority."""
    from molt.toolchain_identity import executable_candidates

    search: dict[str, str] = {}
    for name in ("PATH", "PATHEXT"):
        # Popen accepts str- or bytes-keyed environments (`os.environb`).
        for key in (name, name.encode()):
            try:
                value = environment.get(key)
            except TypeError:
                continue
            if value is not None:
                search[name] = os.fsdecode(value)
                break
    next(executable_candidates(program, environment=search, cwd=cwd), None)


def _install_hook() -> None:
    global _HOOK_INSTALLED
    if not _HOOK_INSTALLED:
        import sys

        if os.name == "nt":
            from molt.file_hashing import _windows_file_api

            # Metadata queries must be ready before observing their own
            # imports. Otherwise a bytecode-open audit can reenter the lazy
            # binder with a partially initialized ctypes.wintypes module.
            _windows_file_api()
        sys.addaudithook(_audit_hook)
        _HOOK_INSTALLED = True


def _after_fork_in_child() -> None:
    """A forked child copies the parent's sink and log handle. It drops both
    and logs its own facts to the same directory."""
    global _SINK, _WORKER_LOG, _REPORTED, _REPORTED_OWNERS, _REPORTED_LOCK
    global _READ_CAPTURE, _READ_SEQUENCE, _RECORDING_SESSION, _WORKER_LOG_LOCK
    _SINK = None
    _WORKER_LOG = None
    _REPORTED = set()
    _REPORTED_OWNERS = {}
    # Another thread may have held this lock at fork; that thread does not
    # exist in the child and can never release its inherited lock.
    _REPORTED_LOCK = threading.Lock()
    _WORKER_LOG_LOCK = threading.RLock()
    _READ_CAPTURE = threading.local()
    _READ_SEQUENCE = 0
    _RECORDING_SESSION = object() if _WORKER_LOG_DIR is not None else None


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork_in_child)


def worker_log_dir() -> str | None:
    """The directory a process pool's workers log their facts to, or None."""
    return _WORKER_LOG_DIR


def worker_initializer(log_dir: str | None) -> None:
    """ProcessPoolExecutor initializer: log this worker's facts."""
    global _WORKER_LOG_DIR, _WORKER_LOG, _RECORDING_SESSION
    if log_dir is not None:
        _install_hook()
        with _REPORTED_LOCK, _WORKER_LOG_LOCK:
            if _WORKER_LOG is not None and _WORKER_LOG_DIR != log_dir:
                _WORKER_LOG.close()
                _WORKER_LOG = None
            _REPORTED.clear()
            _REPORTED_OWNERS.clear()
            _WORKER_LOG_DIR = log_dir
            _RECORDING_SESSION = object()


def merge_worker_logs(sink: FactSink, log_dir: str) -> None:
    """Replay every worker's logged facts into ``sink``; a lost or malformed
    log refuses the record."""
    try:
        names = os.listdir(log_dir)
    except OSError:
        sink.refuse("worker read logs unavailable")
        return
    for name in names:
        if name.endswith(_WORKER_FAILURE_SUFFIX):
            sink.refuse(f"worker read log {name} lost facts")
            continue
        try:
            with open(os.path.join(log_dir, name), encoding="utf-8") as handle:
                for line in handle:
                    _merge_worker_fact(sink, json.loads(line))
        except Exception:  # noqa: BLE001 - any malformed log refuses the record
            sink.refuse(f"unreadable worker read log {name}")


def _merge_worker_fact(sink: FactSink, entry: list[Any]) -> None:
    kind = WorkerFactKind(entry[0])
    if kind is WorkerFactKind.OBSERVED:
        _, observed_kind, name = entry
        sink.observe((observed_kind, name))
    elif kind is WorkerFactKind.FILE:
        _, name, token, sha256 = entry
        sink.file(name, None if token is None else tuple(token), sha256)
    elif kind is WorkerFactKind.DIRECTORY:
        _, name, token = entry
        sink.directory(name, None if token is None else tuple(token))
    elif kind is WorkerFactKind.PROBE:
        _, name, outcome = entry
        sink.probe(name, None if outcome is None else (outcome[0], outcome[1]))
    elif kind is WorkerFactKind.REFUSE:
        sink.refuse(entry[1])
    elif kind is WorkerFactKind.BUILD_OUTPUT:
        _, selection = entry
        sink.build_output(selection)
    elif kind is WorkerFactKind.BUILD_PRODUCTS:
        sink.build_products(entry[1])
    elif kind is WorkerFactKind.BUILD_CLAIM:
        sink.build_claim(entry[1])
    elif kind is WorkerFactKind.BUILD_RESULT:
        sink.build_result(entry[1])
    elif kind is WorkerFactKind.CONSUMED_FILE:
        _, name, token, sha256 = entry
        sink.consumed_file(name, None if token is None else tuple(token), sha256)
    elif kind is WorkerFactKind.READ_STARTED or kind is WorkerFactKind.READ_FINISHED:
        identity = entry[1]
        if (
            not isinstance(identity, (list, tuple))
            or len(identity) != 2
            or any(type(value) is not int or value <= 0 for value in identity)
        ):
            raise ValueError("invalid captured-read identity")
        receipt = (identity[0], identity[1])
        if kind is WorkerFactKind.READ_STARTED:
            _, _, name = entry
            sink.read_started(receipt, name)
        else:
            _, _, name, token, sha256 = entry
            sink.read_finished(
                receipt, name, None if token is None else tuple(token), sha256
            )
    else:
        assert_never(kind)


def active() -> bool:
    """True while this process records a build or logs facts for one."""
    return _SINK is not None or _WORKER_LOG_DIR is not None


def result_active() -> bool:
    """Only native JSON recording pays for result identities and claims."""
    return _CAPTURE_RESULT and active()


def _report_file_fact(
    path: Path | str, token: StatToken | None, sha256: str | None
) -> None:
    sink = _SINK
    if sink is not None:
        sink.file(path, token, sha256)
    else:
        _worker_log([WorkerFactKind.FILE, os.fspath(path), token, sha256])


def _report_probe_fact(path: Path | str, outcome: ProbeOutcome) -> None:
    sink = _SINK
    if sink is not None:
        sink.probe(path, outcome)
    else:
        _worker_log([WorkerFactKind.PROBE, os.fspath(path), outcome])


class _ReadCapture:
    """A reporting receipt for one semantic reader's owned primary open."""

    def __init__(self, path: Path):
        self.path = os.path.abspath(os.fspath(path))
        self.covered_open = False
        self.session: object | None = None
        self.sink: FactSink | None = None
        self.receipt: ReadReceiptId | None = None
        self.thread = 0
        self.previous: _ReadCapture | None = None
        self.token: StatToken | None = None
        self.sha256: str | None = None
        self.completed = False
        self.descriptor_bound = False

    def current(self) -> bool:
        return (
            self.receipt is not None
            and self.session is not None
            and self.session is _RECORDING_SESSION
            and self.receipt[0] == os.getpid()
            and self.thread == threading.get_ident()
        )

    def _invalid(self) -> None:
        reason = f"captured read crossed its recording, process or thread: {self.path}"
        if self.sink is not None:
            self.sink.refuse(reason)
        refuse(reason)

    def __enter__(self) -> _ReadCapture:
        global _READ_SEQUENCE
        with _REPORTED_LOCK:
            if not active():
                return self
            self.session = _RECORDING_SESSION
            self.sink = _SINK
            self.thread = threading.get_ident()
            _READ_SEQUENCE += 1
            self.receipt = (os.getpid(), _READ_SEQUENCE)
            self.previous = getattr(_READ_CAPTURE, "pending", None)
            if self.previous is not None and self.previous.current():
                self.previous._invalid()
                refuse("nested captured read before the primary open")
            if self.sink is not None:
                self.sink.read_started(self.receipt, self.path)
            else:
                _worker_log([WorkerFactKind.READ_STARTED, self.receipt, self.path])
            _READ_CAPTURE.pending = self
        return self

    def opened(self) -> None:
        # Native Windows CreateFileW need not raise a Python pathname event.
        # The semantic reader proves its handle; no consumer open is covered.
        if self.session is not None and not self.current():
            self._invalid()
        else:
            self.descriptor_bound = True
        if getattr(_READ_CAPTURE, "pending", None) is self:
            _READ_CAPTURE.pending = self.previous

    def complete(self, token: StatToken | None, sha256: str) -> None:
        if self.session is not None and not self.current():
            self._invalid()
            return
        if not self.descriptor_bound or self.completed:
            refuse(f"captured read has no unique bound completion: {self.path}")
            return
        self.token = token
        self.sha256 = sha256
        self.completed = True

    def __exit__(self, *error: object) -> None:
        if getattr(_READ_CAPTURE, "pending", None) is self:
            _READ_CAPTURE.pending = self.previous
        if self.receipt is None:
            return
        with _REPORTED_LOCK:
            if not self.current():
                self._invalid()
                return
            token = self.token if self.completed and error[0] is None else None
            sha256 = self.sha256 if token is not None else None
            if self.sink is not None:
                self.sink.read_finished(self.receipt, self.path, token, sha256)
            else:
                _worker_log(
                    [
                        WorkerFactKind.READ_FINISHED,
                        self.receipt,
                        self.path,
                        token,
                        sha256,
                    ]
                )


def capture_read(path: Path) -> _ReadCapture:
    return _ReadCapture(path)


def report_consumed_buffer(
    path: Path, content: bytes, *, previous_session: object | None = None
) -> object | None:
    """Report the immutable source value consumed from a retained snapshot.

    This covers no filesystem open. Other reads keep their own obligations.
    """
    if not active():
        return None
    import hashlib

    # Acknowledgment and fact publication belong to the same recording.
    # Only the snapshot stores this marker: no global owner retains its AST.
    with _REPORTED_LOCK:
        session = _RECORDING_SESSION
        if session is None or not active():
            return None
        if previous_session is session:
            return session
        name = os.path.abspath(os.fspath(path))
        token = stat_token(name)
        digest = hashlib.sha256(content).hexdigest()
        report_probe(name)
        if _SINK is not None:
            _SINK.consumed_file(name, token, digest)
        else:
            _worker_log([WorkerFactKind.CONSUMED_FILE, name, token, digest])
        return session


def report_file(path: Path | str, sha256: str | None = None) -> None:
    """Report a file whose bytes the build used without a captured read token."""
    if active():
        _report_file_fact(path, stat_token(path), sha256)


def report_captured_file(path: Path | str, token: StatToken, sha256: str) -> None:
    """Report a file by the token captured with its digest. The pair stays
    consistent even if the file changes later; a later change only makes the
    next build's check fail."""
    if active():
        _report_file_fact(path, token, sha256)


def first_report(owner: object) -> bool:
    """True the first time this recording sees ``owner``; an immutable capture
    reused across a build reports its facts once."""
    if not active():
        return False
    with _REPORTED_LOCK:
        if id(owner) in _REPORTED_OWNERS:
            return False
        _REPORTED_OWNERS[id(owner)] = owner
        return True


def report_data_file(path: Path | str) -> None:
    """Report a lookup and, when it found a file, that file's token.

    Loaders behind process-global caches call this on every use, so a cached
    answer still names its input; a repeat costs a set lookup.
    """
    if not active():
        return
    name = os.fspath(path)
    with _REPORTED_LOCK:
        if name in _REPORTED:
            return
        _REPORTED.add(name)
    outcome = resolved_probe(name)
    _report_probe_fact(name, outcome)
    if outcome is not None and outcome[0]:
        _report_file_fact(name, stat_token(name), None)


def report_directory(path: Path | str) -> None:
    """Report a directory the build listed or searched. An absent directory is
    an absent lookup, so creating it later is a change."""
    if not active():
        return
    token = stat_token(path)
    if token is None:
        outcome = resolved_probe(path)
        if outcome is None or not outcome[0]:
            _report_probe_fact(path, outcome)
            return
    sink = _SINK
    if sink is not None:
        sink.directory(path, token)
    else:
        _worker_log([WorkerFactKind.DIRECTORY, os.fspath(path), token])


def report_probe(path: Path | str) -> None:
    if active():
        _report_probe_fact(path, resolved_probe(path))


def report_build_output(selection: Mapping[str, str | None]) -> None:
    """Report the native layout's actual selection, not a guessed output name."""
    sink = _SINK
    if sink is not None:
        sink.build_output(dict(selection))
    else:
        _worker_log([WorkerFactKind.BUILD_OUTPUT, dict(selection)])


def report_build_products(consumer: str, products: Mapping[str, str]) -> None:
    """Report the files a successful build published beside its selected output.

    ``products`` maps each result artifact role to its absolute path, as the
    result promises them to the user; ``consumer`` is the one a user runs. A
    recorded build is current only while every product keeps its token.
    """
    payload = {"consumer": consumer, "products": dict(products)}
    if _SINK is not None:
        _SINK.build_products(payload)
    else:
        _worker_log([WorkerFactKind.BUILD_PRODUCTS, payload])


def report_build_claim(claim: Mapping[str, object]) -> None:
    """Report a successful claim at its existing custody authority."""
    if _SINK is not None:
        _SINK.build_claim(dict(claim))
    else:
        _worker_log([WorkerFactKind.BUILD_CLAIM, dict(claim)])


def report_build_result(result: Mapping[str, object]) -> None:
    """Report typed successful result facts, never an emitted JSON envelope."""
    if _SINK is not None:
        _SINK.build_result(dict(result))
    else:
        _worker_log([WorkerFactKind.BUILD_RESULT, dict(result)])


def refuse(reason: str) -> None:
    sink = _SINK
    if sink is not None:
        sink.refuse(reason)
    else:
        _worker_log([WorkerFactKind.REFUSE, reason])
