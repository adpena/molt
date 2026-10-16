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
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Protocol

# (device, inode, mode, size, mtime_ns, content-change ns)
StatToken = tuple[int, int, int, int, int, int]
# A lookup's resolved target and file type; an empty target carries the errno
# of a lookup that failed for a reason other than absence; None is absent.
ProbeOutcome = tuple[str, int] | None


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
        metadata.st_size,
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

    def directory(self, path: Path | str, token: StatToken | None) -> None: ...

    def probe(self, path: Path | str, outcome: ProbeOutcome) -> None: ...

    def refuse(self, reason: str) -> None: ...

    def observe(self, entry: tuple[str, str]) -> None: ...


# Process-global, not context-local: frontend worker threads report to it.
_SINK: FactSink | None = None
# Data files a reader reports on every use; repeats cost a set lookup.
_REPORTED: set[str] = set()
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


def install_sink(sink: FactSink, log_dir: str) -> None:
    """Send this process's facts to ``sink`` and its workers' to ``log_dir``."""
    global _SINK, _WORKER_LOG_DIR
    _install_hook()
    with _REPORTED_LOCK:
        _REPORTED.clear()
    _WORKER_LOG_DIR = log_dir
    _SINK = sink


def remove_sink() -> None:
    global _SINK, _WORKER_LOG_DIR
    _SINK = None
    _WORKER_LOG_DIR = None


def _reads_existing_content(mode: object, flags: object) -> bool:
    if isinstance(flags, int):
        return flags & (os.O_WRONLY | os.O_RDWR) != os.O_WRONLY or bool(
            flags & os.O_APPEND
        )
    if isinstance(mode, str):
        return "r" in mode or "+" in mode or "a" in mode
    return False


def _worker_log(entry: list[Any]) -> None:
    """Append one fact to this worker's log; a lost fact marks the log failed."""
    global _WORKER_LOG
    log_dir = _WORKER_LOG_DIR
    if log_dir is None or _SINK is not None:
        return
    name = os.path.join(log_dir, f"{os.getpid()}.jsonl")
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


def _observe(entry: tuple[str, str]) -> None:
    sink = _SINK
    if sink is not None:
        sink.observe(entry)
    else:
        _worker_log(["observed", *entry])


def _audit_hook(event: str, args: tuple[Any, ...]) -> None:
    if event not in _HOOK_EVENTS or (_SINK is None and _WORKER_LOG_DIR is None):
        return
    if getattr(_HOOK_GUARD, "busy", False):
        return
    _HOOK_GUARD.busy = True
    try:
        if event == "open":
            path, mode, flags = args
            if isinstance(path, (str, bytes, os.PathLike)) and _reads_existing_content(
                mode, flags
            ):
                _observe(("file", os.path.abspath(os.fsdecode(path))))
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

        sys.addaudithook(_audit_hook)
        _HOOK_INSTALLED = True


def _after_fork_in_child() -> None:
    """A forked child copies the parent's sink and log handle. It drops both
    and logs its own facts to the same directory."""
    global _SINK, _WORKER_LOG, _REPORTED, _REPORTED_LOCK
    _SINK = None
    _WORKER_LOG = None
    _REPORTED = set()
    # Another thread may have held this lock at fork; that thread does not
    # exist in the child and can never release its inherited lock.
    _REPORTED_LOCK = threading.Lock()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork_in_child)


def worker_log_dir() -> str | None:
    """The directory a process pool's workers log their facts to, or None."""
    return _WORKER_LOG_DIR


def worker_initializer(log_dir: str | None) -> None:
    """ProcessPoolExecutor initializer: log this worker's facts."""
    global _WORKER_LOG_DIR
    if log_dir is not None:
        _WORKER_LOG_DIR = log_dir
        _install_hook()


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
    kind = entry[0]
    if kind == "observed":
        _, observed_kind, name = entry
        sink.observe((observed_kind, name))
    elif kind == "file":
        _, name, token, sha256 = entry
        sink.file(name, None if token is None else tuple(token), sha256)
    elif kind == "directory":
        _, name, token = entry
        sink.directory(name, None if token is None else tuple(token))
    elif kind == "probe":
        _, name, outcome = entry
        sink.probe(name, None if outcome is None else (outcome[0], outcome[1]))
    elif kind == "refuse":
        sink.refuse(entry[1])
    else:
        sink.refuse(f"unknown worker fact {kind!r}")


def active() -> bool:
    """True while this process records a build or logs facts for one."""
    return _SINK is not None or _WORKER_LOG_DIR is not None


def _report_file_fact(
    path: Path | str, token: StatToken | None, sha256: str | None
) -> None:
    sink = _SINK
    if sink is not None:
        sink.file(path, token, sha256)
    else:
        _worker_log(["file", os.fspath(path), token, sha256])


def _report_probe_fact(path: Path | str, outcome: ProbeOutcome) -> None:
    sink = _SINK
    if sink is not None:
        sink.probe(path, outcome)
    else:
        _worker_log(["probe", os.fspath(path), outcome])


def report_read(
    path: Path,
    *,
    sha256: str,
    before: os.stat_result,
    after: os.stat_result,
) -> None:
    """Report bytes read from an open file whose fstat was taken before and
    after the read; a token change during the read refuses the record."""
    if not active():
        return
    first = stat_token(path, before)
    if first != stat_token(path, after):
        refuse(f"file changed while it was read: {path}")
        return
    _report_file_fact(path, first, sha256)


def report_file(path: Path | str, sha256: str | None = None) -> None:
    """Report a file whose bytes the build used without a captured read token."""
    if active():
        _report_file_fact(path, stat_token(path), sha256)


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
        _worker_log(["directory", os.fspath(path), token])


def report_probe(path: Path | str) -> None:
    if active():
        _report_probe_fact(path, resolved_probe(path))


def refuse(reason: str) -> None:
    sink = _SINK
    if sink is not None:
        sink.refuse(reason)
    else:
        _worker_log(["refuse", reason])
