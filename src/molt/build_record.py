"""Early cutoff for unchanged builds (docs/design/build_early_cutoff.md).

Each input authority reports what it reads through `molt.input_report` when it
reads it; an audit hook observes the reads no authority reports. This module
owns the ledger those facts reach, the request key and the record. A successful
build whose ledger holds no refusal and no racy input records those facts with
the token of the binary it published. The next build of the same request
re-checks them by stat: when every input matches and the binary is untouched,
the build is complete. A changed token on a file with a recorded digest falls
back to a digest compare, so a `touch` costs one hash. Anything unproven means
a full build.
"""

from __future__ import annotations

import hashlib
import json
import os
import stat
import threading
import time
from collections.abc import Generator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from molt import file_publication, input_report
from molt.file_hashing import DIGEST_RACY_WINDOW_NS
from molt.input_report import ProbeOutcome, StatToken, resolved_probe, stat_token

_SCHEMA = "molt-build-record-v6"
# Keys that name the terminal or login session. No build step reads them for
# its artifact (`TERM` only styles progress output); they change between
# shells and would defeat every hit. Everything else is keyed, `PWD` included:
# compilers may take the debug-info directory from it.
_SESSION_ENVIRONMENT_KEYS = frozenset(
    {
        "_",
        "COLORTERM",
        "COLUMNS",
        "ITERM_PROFILE",
        "ITERM_SESSION_ID",
        "LINES",
        "OLDPWD",
        # pytest's temporary root, which the memory guard rebinds to each
        # guarded child's scratch lease; only pytest reads it.
        "PYTEST_DEBUG_TEMPROOT",
        "SECURITYSESSIONID",
        "SHLVL",
        "SSH_AGENT_PID",
        "SSH_AUTH_SOCK",
        "TERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
        "TMUX",
        "TMUX_PANE",
        "__CFBundleIdentifier",
        "__CF_USER_TEXT_ENCODING",
    }
)
# Records unused this long are trimmed; a hit refreshes its record's mtime at
# most once per interval, and trimming runs at most once per interval.
_RECORD_UNUSED_LIMIT_NS = 7 * 24 * 3600 * 1_000_000_000
_RECORD_REFRESH_INTERVAL_NS = 3600 * 1_000_000_000
_TRIM_MARKER = ".trimmed"
# Each recorded build's own transient files, beside the records it writes.
_TRANSIENT_DIRNAME = ".transient"


def _file_sha256(path: Path | str) -> str | None:
    try:
        with open(path, "rb") as handle:
            return hashlib.file_digest(handle, "sha256").hexdigest()
    except OSError:
        return None


@dataclass
class InputLedger:
    """What one build read, reported by each authority when it read it."""

    # No input may have changed at or after this time minus the racy window:
    # the start of the recording process, which may hold earlier reads.
    started_ns: int
    # path -> (token of the bytes read, their sha256 or None)
    files: dict[str, tuple[StatToken, str | None]] = field(default_factory=dict)
    # path -> token; adding, removing or renaming an entry changes it
    directories: dict[str, StatToken] = field(default_factory=dict)
    probes: dict[str, ProbeOutcome] = field(default_factory=dict)
    refusals: list[str] = field(default_factory=list)
    # ("file" | "directory", absolute path) seen by the audit
    # hook; resolved against the excluded roots when the record is written.
    observed: set[tuple[str, str]] = field(default_factory=set)
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def file(
        self, path: Path | str, token: StatToken | None, sha256: str | None
    ) -> None:
        name = os.fspath(path)
        with self._lock:
            if token is None:
                self.refusals.append(f"unreportable file {name}")
                return
            prior = self.files.get(name)
            if prior is not None and prior[0] != token:
                self.refusals.append(f"file changed during the build: {name}")
            self.files[name] = (token, sha256 or (prior[1] if prior else None))

    def directory(self, path: Path | str, token: StatToken | None) -> None:
        name = os.fspath(path)
        with self._lock:
            if token is None:
                self.refusals.append(f"unreportable directory {name}")
                return
            prior = self.directories.get(name)
            if prior is not None and prior != token:
                self.refusals.append(f"directory changed during the build: {name}")
            self.directories[name] = token

    def probe(self, path: Path | str, outcome: ProbeOutcome) -> None:
        name = os.fspath(path)
        with self._lock:
            if name in self.probes and self.probes[name] != outcome:
                self.refusals.append(f"lookup changed during the build: {name}")
            self.probes[name] = outcome

    def refuse(self, reason: str) -> None:
        with self._lock:
            self.refusals.append(reason)

    def observe(self, entry: tuple[str, str]) -> None:
        with self._lock:
            self.observed.add(entry)

    def racy_inputs(self) -> list[str]:
        """Inputs whose content changed within the racy window of the
        recording process's start: their token cannot prove what it read."""
        limit = self.started_ns - DIGEST_RACY_WINDOW_NS
        racy = [name for name, (token, _) in self.files.items() if token[5] >= limit]
        racy.extend(
            name for name, token in self.directories.items() if token[5] >= limit
        )
        return racy


_BUILDS_LOCK = threading.Lock()
_BUILDS_ENTERED = 0


def enter_build() -> bool:
    """Count one build in this process; True for the first.

    Only the first build of a process may record: process-global caches can
    serve a later build without a read to report, whether or not the earlier
    build was eligible for a record.
    """
    global _BUILDS_ENTERED
    with _BUILDS_LOCK:
        _BUILDS_ENTERED += 1
        return _BUILDS_ENTERED == 1


def transient_root(state_root: Path) -> Path:
    """Where recorded builds keep their transient files: a product root."""
    return state_root / "build_records" / _TRANSIENT_DIRNAME


@contextmanager
def recording(transient_parent: Path) -> Generator[InputLedger]:
    """Activate one ledger for the first build of this process.

    The racy bound is the process's start: modules, caches and worker state may
    hold bytes read before the build began.

    The build's temporary files and its workers' read logs live in one
    directory under ``transient_parent``, as Bazel gives each action a
    temporary directory in its output base. A file the build creates and
    removes in an input directory, such as the system temporary directory,
    changes that directory during the build and refuses the record. Normal
    scope exit removes only this allocation, fenced by its directory identity.
    An interrupted build can leave its allocation behind; without process-tree
    closure evidence, neither its age nor a missing parent authorizes deletion.
    """
    import tempfile

    from molt.process_birth import current_process_started_ns
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    started = current_process_started_ns()
    ledger = InputLedger(started if started is not None else time.time_ns())
    if started is None:
        ledger.refuse("process start time unavailable")
    transient_parent.mkdir(parents=True, exist_ok=True)
    with OwnedTemporaryDirectory(prefix="build-", dir=transient_parent) as transient:
        log_dir = os.path.join(transient, "reads")
        temporary = os.path.join(transient, "temporary")
        os.mkdir(log_dir)
        os.mkdir(temporary)
        previous_temporary = tempfile.tempdir
        tempfile.tempdir = temporary
        input_report.install_sink(ledger, log_dir)
        try:
            yield ledger
        finally:
            input_report.remove_sink()
            tempfile.tempdir = previous_temporary
            input_report.merge_worker_logs(ledger, log_dir)


def request_key(
    arguments: Mapping[str, Any],
    *,
    environment: Mapping[str, str],
    cwd: str,
    interpreter: str,
    interpreter_version: str,
    host: Mapping[str, str],
) -> str | None:
    """Identity of one build request, or None when it is not plain data.

    The environment is keyed whole except terminal-session names and the
    registry's output-neutral names: a guarded build carries a fresh guard
    token, marker and scratch lease, and memory budgets measured at launch.
    """
    from molt.environment_registry import load_registry

    # The registry loads only when a MOLT_* name is set, as at CLI start.
    registry = (
        load_registry() if any(key.startswith("MOLT_") for key in environment) else None
    )
    material = {
        "schema": _SCHEMA,
        "arguments": dict(arguments),
        "cwd": cwd,
        "interpreter": interpreter,
        "interpreter_version": interpreter_version,
        "host": dict(host),
        "environment": {
            key: value
            for key, value in environment.items()
            if key not in _SESSION_ENVIRONMENT_KEYS
            and not (
                registry is not None
                and key.startswith("MOLT_")
                and registry.is_output_neutral(key)
            )
        },
    }
    try:
        encoded = json.dumps(
            material, sort_keys=True, separators=(",", ":"), allow_nan=False
        )
    except (TypeError, ValueError):
        return None
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def record_path(state_root: Path, key: str) -> Path:
    return state_root / "build_records" / key[:2] / f"{key}.json"


def resolve_observed(
    ledger: InputLedger,
    excluded_roots: tuple[Path, ...],
    *,
    published_output: Path | None = None,
) -> None:
    """Turn the audit hook's observations into inputs.

    Only named product roots and the successfully published output, with the
    receipt, journals, lock and stages its publication keeps beside it, are
    products. The publication lists the output's directory for its own
    journals, so an observed listing of that directory is custody; a reader
    that consumes the listing, such as the import topology, reports it
    explicitly. Pre-syscall audit events cannot prove write or
    directory-creation ownership. Explicit input reports are never excluded,
    even for the published output.
    """
    from molt.artifact_publication import is_publication_family_member
    from molt.path_custody import resolved_host_path_key

    output = (
        None if published_output is None else Path(os.path.realpath(published_output))
    )
    output_parent = None if output is None else resolved_host_path_key(output.parent)
    roots = tuple(
        {
            resolved_host_path_key(os.path.realpath(root)).rstrip(os.sep) + os.sep
            for root in excluded_roots
        }
    )
    # Only this resolution's completed chains prove that a failed lookup has
    # a generation boundary. Explicit probes may not have recorded one.
    completed_lookups: set[str] = set()

    def failed_lookup(name: str) -> None:
        chain: list[str] = []
        while name not in completed_lookups:
            outcome = resolved_probe(name)
            ledger.probe(name, outcome)
            chain.append(name)
            if outcome is not None and outcome[0]:
                # The path exists but its content-change token is unavailable.
                # A parent cannot substitute for that missing generation.
                ledger.refuse(f"unreportable lookup generation {name}")
                break
            parent = os.path.dirname(name)
            if parent == name:
                ledger.refuse(f"unreportable lookup boundary {name}")
                break
            if parent in completed_lookups:
                break
            token = stat_token(parent)
            if token is not None:
                if stat.S_ISDIR(token[2]):
                    if parent not in ledger.directories:
                        ledger.directory(parent, token)
                elif parent not in ledger.files:
                    # ENOTDIR can terminate at a file. Keep its typed token;
                    # it is not a directory listing dependency.
                    ledger.file(parent, token, None)
                chain.append(parent)
                break
            name = parent
        completed_lookups.update(chain)

    for kind, name in sorted(ledger.observed):
        resolved = os.path.realpath(name)
        real = resolved_host_path_key(resolved)
        if output is not None and (
            (kind == "file" and is_publication_family_member(Path(resolved), output))
            or (kind == "directory" and real == output_parent)
        ):
            continue
        if (real + os.sep).startswith(roots):
            continue
        if real.startswith(("/dev/", "/proc/", "/sys/")):
            continue
        token = stat_token(name)
        if token is None:
            if name in ledger.files:
                # A reported file keeps its original generation; validation
                # already refuses its removal or an unreadable replacement.
                ledger.probe(name, resolved_probe(name))
            else:
                # Missing intermediate components are probes too. The nearest
                # stat-able ancestor supplies the racy-removal boundary.
                failed_lookup(name)
            continue
        if kind == "directory":
            if name not in ledger.directories:
                ledger.directory(name, token)
        elif name not in ledger.files and stat.S_ISREG(token[2]):
            ledger.file(name, token, None)


def _first_change(
    files: list[list[Any]],
    directories: list[list[Any]],
    probes: list[list[Any]],
) -> str | None:
    """The first recorded input that no longer matches, or None."""
    for name, token in directories:
        if stat_token(name) != tuple(token):
            return f"directory {name}"
    for name, outcome in probes:
        expected = None if outcome is None else (outcome[0], outcome[1])
        if resolved_probe(name) != expected:
            return f"lookup {name}"
    for name, token, sha256 in files:
        current = stat_token(name)
        if current == tuple(token):
            continue
        # A rewritten file with equal bytes (a touch, a checkout) is unchanged
        # when its content digest was recorded.
        if (
            sha256 is None
            or current is None
            or current[3] != token[3]
            or _file_sha256(name) != sha256
        ):
            return f"file {name}"
    return None


def write_record(
    path: Path, key: str, ledger: InputLedger, *, output: Path
) -> str | None:
    """Publish the record of a build that produced ``output``.

    Returns why no record was written: a ledger refusal, a racy input, an
    input that no longer matches what the build read, or a failed write.
    """
    if ledger.refusals:
        return ledger.refusals[0]
    racy = ledger.racy_inputs()
    if racy:
        return f"input changed after the recording process started: {racy[0]}"
    files = [
        [name, list(token), sha256]
        for name, (token, sha256) in sorted(ledger.files.items())
    ]
    directories = [
        [name, list(token)] for name, token in sorted(ledger.directories.items())
    ]
    probes = [
        [name, None if outcome is None else list(outcome)]
        for name, outcome in sorted(ledger.probes.items())
    ]
    changed = _first_change(files, directories, probes)
    if changed is not None:
        return f"input changed during the build: {changed}"
    output_token = stat_token(output)
    if output_token is None or not stat.S_ISREG(output_token[2]):
        return f"output {output} is not a regular file"
    payload = {
        "schema": _SCHEMA,
        "key": key,
        "output": [os.fspath(output), list(output_token)],
        "files": files,
        "directories": directories,
        "probes": probes,
    }
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        file_publication.atomic_write_bytes(
            path,
            json.dumps(payload, separators=(",", ":")).encode("utf-8"),
            durability=file_publication.PublicationDurability.RECONSTRUCTIBLE,
        )
    except OSError as error:
        return f"record write failed: {error}"
    trim_if_due(path.parent.parent)
    return None


def check(path: Path, key: str, output: Path) -> str | None:
    """None when the recorded build is current, else the first difference.

    Current means every recorded input matches and ``output`` still holds the
    binary that build published. Never raises.
    """
    try:
        metadata = os.stat(path)
        with open(path, "rb") as handle:
            payload = json.loads(handle.read())
        if not isinstance(payload, dict):
            return "unreadable record: expected a JSON object"
        if payload.get("schema") != _SCHEMA or payload.get("key") != key:
            return "no record for this request"
        recorded_output, output_token = payload["output"]
        if recorded_output != os.fspath(output) or stat_token(output) != tuple(
            output_token
        ):
            return f"output {output} changed"
        changed = _first_change(
            payload["files"], payload["directories"], payload["probes"]
        )
        if changed is not None:
            return changed
        if time.time_ns() - metadata.st_mtime_ns > _RECORD_REFRESH_INTERVAL_NS:
            os.utime(path)
        return None
    except FileNotFoundError:
        return "no record for this request"
    except (
        OSError,
        ValueError,
        TypeError,
        KeyError,
        IndexError,
        OverflowError,
        RecursionError,
    ) as error:
        return f"unreadable record: {error}"


def trim_if_due(records_root: Path) -> None:
    """Delete records unused for a week, at most once an hour.

    Concurrent trims are harmless: each deletion is idempotent, and a reader
    that loses its record to a trim sees a miss.
    """
    marker = records_root / _TRIM_MARKER
    now = time.time_ns()
    try:
        if now - os.stat(marker).st_mtime_ns < _RECORD_REFRESH_INTERVAL_NS:
            return
    except FileNotFoundError:
        pass
    except OSError:
        return
    try:
        marker.touch()
        shards = list(os.scandir(records_root))
    except OSError:
        return
    for shard in shards:
        if shard.name == _TRANSIENT_DIRNAME:
            # Execution scratch is not a cache record. Age does not prove
            # that its writers have exited, including after parent death.
            continue
        if not shard.is_dir(follow_symlinks=False):
            continue
        try:
            entries = list(os.scandir(shard.path))
        except OSError:
            continue
        for entry in entries:
            try:
                if (
                    now - entry.stat(follow_symlinks=False).st_mtime_ns
                    > _RECORD_UNUSED_LIMIT_NS
                ):
                    os.unlink(entry.path)
            except OSError:
                continue
