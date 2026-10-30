"""One host store for the answers of toolchain probes (HF-357).

A probe runs a toolchain executable to ask a question the compiler cannot
answer itself: rustc's target cfg, rustup's selected binary, Cargo's version,
xcrun's SDK path, a C driver's linker. Each answer is a function of the
executables that run, the argument vector, the working directory, a typed set
of environment variables those executables read, and the files they consult.
This module keys an answer by exactly those inputs and keeps it on the host,
so an unchanged toolchain is asked once per key, not once per build.

Design: ``docs/design/toolchain_fact_cache.md``. Cargo keeps the same kind of
store in ``target/.rustc_info.json`` (``src/util/rustc.rs``), keyed by a
rustc fingerprint; Go keys its action cache on each tool's ``-V=full`` build
ID (``cmd/go/internal/work/buildid.go``). Molt's key is stricter than
Cargo's: every executable carries its stat token and content digest, and the
files a question reads are named by their own tokens.

The store is one JSON file under the host cache namespace
(`molt.default_paths.toolchain_fact_store_path`), read once per process and
replaced atomically under the shared cache lock. A corrupt or foreign file is
discarded, never trusted. Only zero-exit answers are kept.
"""

from __future__ import annotations

from collections.abc import Callable, Generator, Iterable, Mapping
from contextlib import contextmanager
from dataclasses import dataclass, field
import os
from molt.durable_file_identity import StatToken
from pathlib import Path
import stat
import subprocess
import threading
import time
from typing import Any

from molt.default_paths import (
    _default_molt_cache,
    file_digest_cache_path,
    resolve_budget,
    toolchain_fact_store_path,
)
from molt.exact_json import canonical_json_sha256, dumps_exact, loads_exact
from molt.process_environment import ProcessEnvironment
from molt.toolchain_identity import (
    FileDigestCache,
    StableRegularFileIdentity,
    _executable_paths,
    file_digest_cache,
    stable_executable_probe,
)


STORE_SCHEMA = "molt.toolchain-facts.v3"

# The environment each probe class reads, decided from the tool's own source
# and documented in the design. A name outside its set cannot change the
# answer; a name inside it is part of the key, present or absent.

# rustc (rustc_session, rustc_target, rustc_driver_impl; 1.99): RUST_TARGET_PATH
# selects custom target specs; RUSTC_BOOTSTRAP admits unstable flags;
# RUSTC_FORCE_RUSTC_VERSION and RUSTC_OVERRIDE_VERSION_STRING change `-vV`;
# the Apple targets read SDKROOT and the deployment targets; the dynamic
# loader variables select which librustc_driver the launcher loads; the
# rustup variables cover a proxy that was not recognised as one.
RUSTC_PROBE_ENVIRONMENT = (
    "RUST_TARGET_PATH",
    "RUSTC_BOOTSTRAP",
    "RUSTC_FORCE_RUSTC_VERSION",
    "RUSTC_OVERRIDE_VERSION_STRING",
    "RUSTUP_TOOLCHAIN",
    "RUSTUP_HOME",
    "SDKROOT",
    "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
    "TVOS_DEPLOYMENT_TARGET",
    "WATCHOS_DEPLOYMENT_TARGET",
    "XROS_DEPLOYMENT_TARGET",
    "VISIONOS_DEPLOYMENT_TARGET",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
)

# rustup `which` (rustup-rs config.rs): the explicit toolchain, the rustup
# home (default `$HOME/.rustup`, `%USERPROFILE%` on Windows) and whether a
# missing toolchain is installed on demand. The Unix fallback settings file
# is keyed by the path it resolves to, so its selector needs no entry here.
RUSTUP_PROBE_ENVIRONMENT = (
    "RUSTUP_TOOLCHAIN",
    "RUSTUP_HOME",
    "RUSTUP_AUTO_INSTALL",
    "HOME",
    "USERPROFILE",
)

# `cargo --version` reads nothing; the rustup pair covers an unrecognised proxy.
CARGO_VERSION_ENVIRONMENT = ("RUSTUP_TOOLCHAIN", "RUSTUP_HOME")

# xcode-select and xcrun: the developer directory, an explicit SDK and an
# alternate toolchain identifier.
DARWIN_SELECTOR_ENVIRONMENT = ("DEVELOPER_DIR", "SDKROOT", "TOOLCHAINS")

# clang/gcc `-print-prog-name`: program search covers COMPILER_PATH, the GCC
# prefix and PATH; the Apple shim dispatches on the developer selection.
LINK_DRIVER_PROBE_ENVIRONMENT = (
    "PATH",
    "COMPILER_PATH",
    "GCC_EXEC_PREFIX",
    "DEVELOPER_DIR",
    "SDKROOT",
    "TOOLCHAINS",
)

# The symlink `xcode-select` and the `/usr/bin` shims dispatch on.
DARWIN_DEVELOPER_SELECTION_LINK = Path("/var/db/xcode_select_link")


@dataclass(frozen=True, slots=True)
class FileToken:
    node: StatToken
    link_text: str | None = None
    target: StatToken | None = None

    def payload(self) -> list[object]:
        wire: list[object] = list(self.node.payload())
        if self.link_text is not None:
            wire.extend(
                [self.link_text, None if self.target is None else self.target.payload()]
            )
        return wire


FileInput = tuple[str, FileToken | None]


@dataclass(frozen=True, slots=True)
class ProbeTool:
    """One executable a probe runs: its invoked path and content generation.

    ``identity`` is the captured generation the key fields project; a miss
    runs the probe under `fenced`, which proves that generation unchanged
    before and after the process, so a recorded answer never belongs to
    other bytes than its key names.
    """

    entrypoint: str
    content_path: str
    size: int
    sha256: str
    token: StatToken
    identity: StableRegularFileIdentity = field(repr=False, compare=False)

    @classmethod
    def from_identity(
        cls, entrypoint: Path, identity: StableRegularFileIdentity
    ) -> ProbeTool:
        return cls(
            os.fspath(entrypoint),
            os.fspath(identity.path),
            identity.size,
            identity.sha256,
            identity.stat_token(),
            identity,
        )

    @contextmanager
    def fenced(self, *, label: str) -> Generator[Path]:
        """Yield the entrypoint while its captured generation is proven current."""
        with stable_executable_probe(
            Path(self.entrypoint), label=label, identity=self.identity
        ) as (entrypoint, _identity):
            yield entrypoint

    def as_json(self) -> dict[str, object]:
        return {
            "entrypoint": self.entrypoint,
            "content_path": self.content_path,
            "size": self.size,
            "sha256": self.sha256,
            "token": self.token.payload(),
        }


def host_file_digest_cache() -> FileDigestCache:
    """This process's stat-keyed digest cache over the host cache namespace."""
    _max_gb, max_age_days = resolve_budget(None, None, os.environ)
    return file_digest_cache(
        file_digest_cache_path(_default_molt_cache()),
        max_age_ns=max_age_days * 86_400 * 1_000_000_000,
    )


def capture_probe_tool(path: Path, *, label: str) -> ProbeTool:
    """Identify an executable through the host digest cache.

    On POSIX an unchanged stat token answers from the cache without reading
    the file; a new generation is hashed once and recorded.
    """
    entrypoint, content = _executable_paths(path, label=label)
    digests = host_file_digest_cache()
    identity = digests.identity(content, label=label)
    digests.flush()
    return ProbeTool.from_identity(entrypoint, identity)


def file_input(path: Path | str) -> FileInput:
    """Name one file a probe consults by its no-follow token.

    An absent path is a lookup outcome and part of the key: a file that
    appears later changes the answer. A symlink carries its text and the
    token of the file it resolves to, because the probe reads through it: an
    edit behind the link changes the answer (a dangling link keys as absent
    behind its text).
    """
    name = os.fspath(path)
    try:
        metadata = os.lstat(name)
    except (FileNotFoundError, NotADirectoryError):
        return name, None
    node = StatToken(
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )
    if not stat.S_ISLNK(metadata.st_mode):
        return name, FileToken(node)
    link_text = os.readlink(name)
    try:
        target = os.stat(name)
    except (FileNotFoundError, NotADirectoryError):
        return name, FileToken(node, link_text)
    return name, FileToken(
        node,
        link_text,
        StatToken(
            target.st_ino,
            target.st_mode,
            target.st_size,
            target.st_mtime_ns,
            target.st_ctime_ns,
        ),
    )


_DYNAMIC_LIBRARY_SUFFIXES = frozenset({".dll", ".dylib", ".so"})


def directory_inputs(
    directory: Path, *, dynamic_libraries_only: bool = False
) -> tuple[FileInput, ...]:
    """The directory's own token, then each direct non-directory entry's token.

    The listing is what a loader or a spec search sees; it never descends.
    """
    own = file_input(directory)
    mode = None if own[1] is None else own[1].node.mode  # the token's st_mode
    if not isinstance(mode, int) or not stat.S_ISDIR(mode):
        return (own,)
    entries: list[FileInput] = [own]
    with os.scandir(directory) as iterator:
        names = sorted(
            entry.name
            for entry in iterator
            if not entry.is_dir(follow_symlinks=False)
            and (
                not dynamic_libraries_only
                or Path(entry.name).suffix.casefold() in _DYNAMIC_LIBRARY_SUFFIXES
                or ".so." in entry.name
            )
        )
    entries.extend(file_input(directory / name) for name in names)
    return tuple(entries)


def environment_inputs(
    env: Mapping[str, str], names: Iterable[str]
) -> tuple[tuple[str, str | None], ...]:
    """The typed subset of ``env`` a probe reads, absent names included."""
    view = env if isinstance(env, ProcessEnvironment) else ProcessEnvironment(env)
    return tuple((name, view.get(name)) for name in sorted(set(names)))


@dataclass(frozen=True, slots=True)
class ProbeKey:
    """Every input that can change a probe's answer."""

    probe: str
    tools: tuple[ProbeTool, ...]
    argv: tuple[str, ...]
    cwd: str | None
    environment: tuple[tuple[str, str | None], ...]
    inputs: tuple[FileInput, ...]

    def as_json(self) -> dict[str, object]:
        return {
            "probe": self.probe,
            "tools": [tool.as_json() for tool in self.tools],
            "argv": list(self.argv),
            "cwd": self.cwd,
            "environment": dict(self.environment),
            "inputs": [
                [path, None if token is None else token.payload()]
                for path, token in self.inputs
            ],
        }

    def digest(self) -> str:
        return canonical_json_sha256(self.as_json())


@dataclass(frozen=True, slots=True)
class ProbeAnswer:
    stdout: str
    stderr: str


@dataclass(frozen=True, slots=True)
class _Record:
    key: dict[str, object]
    answer: ProbeAnswer
    recorded_ns: int

    def as_json(self) -> dict[str, object]:
        return {
            "key": self.key,
            "stdout": self.answer.stdout,
            "stderr": self.answer.stderr,
            "recorded_ns": self.recorded_ns,
        }


def _record_from_json(digest: str, value: object) -> _Record | None:
    """One stored record, or None when its shape or digest is not ours."""
    if not isinstance(value, dict):
        return None
    key = value.get("key")
    stdout = value.get("stdout")
    stderr = value.get("stderr")
    recorded_ns = value.get("recorded_ns")
    if (
        not isinstance(key, dict)
        or not isinstance(stdout, str)
        or not isinstance(stderr, str)
        or type(recorded_ns) is not int
        or canonical_json_sha256(key) != digest
    ):
        return None
    return _Record(key, ProbeAnswer(stdout, stderr), recorded_ns)


def _read_records(path: Path) -> dict[str, _Record]:
    """The store's records; a missing, corrupt or foreign file reads as empty."""
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return {}
    try:
        document: Any = loads_exact(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        return {}
    if not isinstance(document, dict) or document.get("schema") != STORE_SCHEMA:
        return {}
    facts = document.get("facts")
    if not isinstance(facts, dict):
        return {}
    records: dict[str, _Record] = {}
    for digest, value in facts.items():
        record = _record_from_json(digest, value)
        if record is not None:
            records[digest] = record
    return records


def _replace_records(path: Path, records: Mapping[str, _Record]) -> None:
    """Publish the whole store in one atomic replace."""
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = dumps_exact(
        {
            "schema": STORE_SCHEMA,
            "facts": {
                digest: record.as_json() for digest, record in sorted(records.items())
            },
        },
        indent=None,
    ).encode("utf-8")
    staging = path.with_name(f"{path.name}.{os.getpid()}.{threading.get_ident()}.tmp")
    try:
        staging.write_bytes(payload)
        os.replace(staging, path)
    except BaseException:
        staging.unlink(missing_ok=True)
        raise


class ToolchainFactStore:
    """Probe answers keyed by `ProbeKey`, shared by every process on one host.

    A provisional answer lives in this process only until its owner persists
    it; the Cargo plan persists a rustc answer after the qualification replay
    proves it under the captured resource closure. Records unused for the
    cache age budget are dropped on the next write; a record read in the second
    half of that budget is refreshed, so a toolchain in use is never re-probed
    for age alone.
    """

    def __init__(self, cache_root: Path, *, max_age_ns: int) -> None:
        if type(max_age_ns) is not int or max_age_ns <= 0:
            raise ValueError("toolchain fact age budget must be a positive integer")
        self.cache_root = cache_root
        self.path = toolchain_fact_store_path(cache_root)
        self._max_age_ns = max_age_ns
        self._started_ns = time.time_ns()
        self._lock = threading.RLock()
        self._persisted: dict[str, _Record] | None = None
        self._provisional: dict[str, _Record] = {}

    def _host_lock(self):  # type: ignore[no-untyped-def]
        from molt.file_locks import _shared_cache_lock

        return _shared_cache_lock("toolchain-facts", cache_root=self.cache_root)

    def _records(self) -> dict[str, _Record]:
        if self._persisted is None:
            with self._host_lock():
                self._persisted = _read_records(self.path)
        return self._persisted

    def lookup(self, key: ProbeKey) -> ProbeAnswer | None:
        """The recorded answer for ``key``: provisional first, then persisted."""
        digest = key.digest()
        with self._lock:
            provisional = self._provisional.get(digest)
            if provisional is not None:
                return provisional.answer
            record = self._records().get(digest)
            if record is None:
                return None
            if record.recorded_ns < self._started_ns - self._max_age_ns // 2:
                self._write(
                    {digest: _Record(record.key, record.answer, self._started_ns)}
                )
            return record.answer

    def persisted(self, key: ProbeKey) -> bool:
        with self._lock:
            return key.digest() in self._records()

    def record(self, key: ProbeKey, answer: ProbeAnswer, *, persist: bool) -> None:
        """Keep ``answer`` for ``key``; ``persist`` publishes it to the host."""
        digest = key.digest()
        record = _Record(key.as_json(), answer, time.time_ns())
        with self._lock:
            if persist:
                self._provisional.pop(digest, None)
                self._write({digest: record})
            else:
                self._provisional[digest] = record

    def forget(self, key: ProbeKey) -> None:
        """Drop this process's provisional answer for ``key``."""
        with self._lock:
            self._provisional.pop(key.digest(), None)

    def _write(self, pending: Mapping[str, _Record]) -> None:
        # Merge over the current host generation so parallel writers lose
        # nothing, drop aged records, then replace the file atomically.
        floor = self._started_ns - self._max_age_ns
        with self._host_lock():
            merged = {
                digest: record
                for digest, record in {**_read_records(self.path), **pending}.items()
                if record.recorded_ns >= floor
            }
            _replace_records(self.path, merged)
            self._persisted = merged


_STORES: dict[Path, ToolchainFactStore] = {}
_STORES_LOCK = threading.Lock()


def toolchain_fact_store(cache_root: Path, *, max_age_ns: int) -> ToolchainFactStore:
    """This process's store over one cache namespace."""
    with _STORES_LOCK:
        store = _STORES.get(cache_root)
        if store is None:
            store = _STORES[cache_root] = ToolchainFactStore(
                cache_root, max_age_ns=max_age_ns
            )
        return store


def host_toolchain_facts() -> ToolchainFactStore:
    """The store for the host cache namespace, under the shared age budget."""
    _max_gb, max_age_days = resolve_budget(None, None, os.environ)
    return toolchain_fact_store(
        _default_molt_cache(), max_age_ns=max_age_days * 86_400 * 1_000_000_000
    )


def completed_probe(
    store: ToolchainFactStore,
    key: ProbeKey,
    run: Callable[[], subprocess.CompletedProcess[str]],
    *,
    persist: bool = True,
    accept: Callable[[ProbeAnswer], bool] | None = None,
) -> tuple[subprocess.CompletedProcess[str], bool]:
    """Answer ``key`` from the store, or run the probe and keep a zero-exit answer.

    ``run`` returns only after its own fence proved the executables unchanged
    across the process (`ProbeTool.fenced`), so the answer is recorded under
    the generation its key names. ``accept`` rejects a recorded answer that no
    longer holds (for example a selected path that was removed); the probe then
    runs again and its new answer replaces the old one. Returns the completed
    process and whether it came from the store.
    """
    cached = store.lookup(key)
    if cached is not None and (accept is None or accept(cached)):
        return (
            subprocess.CompletedProcess(
                list(key.argv), 0, cached.stdout, cached.stderr
            ),
            True,
        )
    result = run()
    if result.returncode == 0:
        if not isinstance(result.stdout, str) or not isinstance(result.stderr, str):
            raise TypeError("a toolchain probe must capture text stdout and stderr")
        store.record(key, ProbeAnswer(result.stdout, result.stderr), persist=persist)
    return result, False
