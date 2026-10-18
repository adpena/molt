"""Operation-local custody for the filesystem facts Python imports consume."""

from __future__ import annotations

import os
import stat
import time
from dataclasses import dataclass, field
from pathlib import Path
from collections.abc import Callable
from typing import Literal, NamedTuple, TypeVar

from molt import input_report as _input_report
from molt.cli.module_source import PythonSourceChangedError
from molt.file_hashing import DIGEST_RACY_WINDOW_NS, content_change_time_ns


class PythonImportTopologyChangedError(PythonSourceChangedError):
    """An import lookup no longer belongs to its captured filesystem facts."""


_Read = TypeVar("_Read")


class PythonImportPath(NamedTuple):
    path: Path
    mode: int


class _DirectoryState(NamedTuple):
    path: Path
    token: tuple[int, int, int, int, int | None]


_NameQuery = Literal["package", "source", "directories"]
# A read retries while its directory changes underneath it, as a seqlock
# reader does: a shared temporary or project directory gains unrelated
# entries at any moment, and only a changed import fact ends an operation.
_STABLE_READ_ATTEMPTS = 64
# The entry kind an import consumes under an exact spelling: a package or
# namespace portion is a directory, a module or initializer is a file.
ExactEntryKind = Literal["directory", "file"]


@dataclass
class _DirectoryObservation:
    state: _DirectoryState | None
    racy: bool
    entries: tuple[os.DirEntry[str], ...] | None = None
    entries_by_name: dict[str, os.DirEntry[str]] = field(default_factory=dict)
    exact_names: dict[tuple[str, ExactEntryKind], bool] = field(default_factory=dict)
    name_queries: dict[_NameQuery, frozenset[str]] = field(default_factory=dict)
    probes: set[Path] = field(default_factory=set)
    has_links: bool = False


def _lexical_path(path: Path) -> Path:
    # Keep the selector, including aliases, rather than replacing it by the
    # first resolved target. Relative selectors are bound to the current cwd.
    return path if path.is_absolute() else Path.cwd() / path


def _probe(path: Path) -> PythonImportPath | None:
    try:
        metadata = path.stat()
        return PythonImportPath(
            path.resolve(strict=True), stat.S_IFMT(metadata.st_mode)
        )
    except (FileNotFoundError, NotADirectoryError):
        return None


def _directory_state(path: Path) -> _DirectoryState | None:
    try:
        metadata = path.stat()
        if not stat.S_ISDIR(metadata.st_mode):
            return None
        return _DirectoryState(
            path.resolve(strict=True),
            (
                metadata.st_dev,
                metadata.st_ino,
                metadata.st_size,
                metadata.st_mtime_ns,
                content_change_time_ns(path, metadata),
            ),
        )
    except (FileNotFoundError, NotADirectoryError):
        return None


def _read_entries(path: Path) -> tuple[os.DirEntry[str], ...]:
    try:
        with os.scandir(path) as entries:
            return tuple(entries)
    except (FileNotFoundError, NotADirectoryError):
        return ()


def _holds_exact(
    entries: dict[str, os.DirEntry[str]], name: str, kind: ExactEntryKind
) -> bool:
    entry = entries.get(name)
    if entry is None:
        return False
    return entry.is_dir() if kind == "directory" else entry.is_file()


def _names(entries: tuple[os.DirEntry[str], ...], query: _NameQuery) -> frozenset[str]:
    names: set[str] = set()
    for entry in entries:
        if query == "directories":
            if entry.is_dir():
                names.add(entry.name)
        elif query == "package":
            name = entry.name if entry.is_dir() else entry.name.partition(".")[0]
            if name.isidentifier() and name != "__init__":
                names.add(name)
        else:
            name = entry.name
            if entry.is_file() and name.endswith(".py"):
                name = name[:-3]
            elif not entry.is_dir() or name == "__pycache__":
                continue
            if name and "." not in name:
                names.add(name)
    return frozenset(names)


class PythonImportTopology:
    """Retain only queried import facts; changed facts end the operation.

    Listings are shared, but a single-name lookup observes that name alone.
    Unrelated build output can therefore change a directory without changing
    an import fact. Portable cache keys never contain these live stat tokens.
    """

    def __init__(self) -> None:
        self._directories: dict[Path, _DirectoryObservation] = {}
        self._probes: dict[Path, PythonImportPath | None] = {}
        self._failure: str | None = None

    def _check(self) -> None:
        if self._failure is not None:
            raise PythonImportTopologyChangedError(self._failure)

    def _changed(self, path: Path) -> None:
        self._failure = f"Python import topology changed during operation: {path}"
        raise PythonImportTopologyChangedError(self._failure)

    def _stable_read(
        self, directory: Path, read: Callable[[], _Read]
    ) -> tuple[_DirectoryState | None, _Read]:
        """Read while ``directory`` holds still; return its state with the value.

        A change between the two stats means the read may mix two states, so
        it is retried. Facts derive only from a coherent read and are checked
        against their capture at validation.
        """
        for _ in range(_STABLE_READ_ATTEMPTS):
            before = _directory_state(directory)
            value = read()
            if before == _directory_state(directory):
                return before, value
        self._failure = (
            "Python import directory never held still for "
            f"{_STABLE_READ_ATTEMPTS} reads during operation: {directory}"
        )
        raise PythonImportTopologyChangedError(self._failure)

    def _directory(self, path: Path) -> _DirectoryObservation:
        self._check()
        cached = self._directories.get(path)
        if cached is not None:
            return cached
        started = time.time_ns()
        state = _directory_state(path)
        change = state.token[4] if state is not None else None
        observation = _DirectoryObservation(
            state,
            state is None
            or change is None
            or max(state.token[3], change) >= started - DIGEST_RACY_WINDOW_NS,
        )
        self._directories[path] = observation
        if state is not None:
            _input_report.report_directory(path)
        # An absent parent is itself a negative lookup. Retain its existing
        # ancestor so creating or retargeting that path cannot grant old facts.
        if state is None and path.parent != path:
            self.probe(path)
        return observation

    def _entries(
        self, path: Path, observation: _DirectoryObservation
    ) -> tuple[os.DirEntry[str], ...]:
        if observation.entries is None:
            before, entries = self._stable_read(path, lambda: _read_entries(path))
            observation.entries = entries
            observation.entries_by_name = {entry.name: entry for entry in entries}
            observation.has_links = any(
                entry.is_symlink() or getattr(entry, "is_junction", lambda: False)()
                for entry in entries
            )
            observation.racy |= before != observation.state
        return observation.entries

    def probe(self, path: Path) -> PythonImportPath | None:
        self._check()
        path = _lexical_path(path)
        if path not in self._probes:
            parent = self._directory(path.parent)
            _state, value = self._stable_read(path.parent, lambda: _probe(path))
            self._probes[path] = value
            _input_report.report_probe(path)
            parent.probes.add(path)
            # A symlink target may change without changing the selector's
            # parent. Verify that binding at every fence, even on POSIX.
            parent.has_links |= (
                path.is_symlink() or value is not None and value.path != path
            )
        return self._probes[path]

    def is_dir(self, path: Path) -> bool:
        result = self.probe(path)
        return result is not None and stat.S_ISDIR(result.mode)

    def is_file(self, path: Path) -> bool:
        result = self.probe(path)
        return result is not None and stat.S_ISREG(result.mode)

    def contains_exact(self, directory: Path, name: str, kind: ExactEntryKind) -> bool:
        """Whether ``directory`` holds an entry spelled ``name`` of ``kind``.

        CPython's FileFinder matches the spelling, then uses a package
        candidate only as a directory and a module only as a file. The kind is
        therefore part of the fact: a binary published under its module's name
        beside the source changes no import.
        """
        directory = _lexical_path(directory)
        observation = self._directory(directory)
        key = (name, kind)
        if key not in observation.exact_names:
            self._entries(directory, observation)
            observation.exact_names[key] = _holds_exact(
                observation.entries_by_name, name, kind
            )
        return observation.exact_names[key]

    def _child_names(self, directory: Path, query: _NameQuery) -> frozenset[str]:
        directory = _lexical_path(directory)
        observation = self._directory(directory)
        if observation.state is None:
            raise ValueError(f"cannot list Python package directory: {directory}")
        if query not in observation.name_queries:
            observation.name_queries[query] = _names(
                self._entries(directory, observation), query
            )
        return observation.name_queries[query]

    def package_child_names(self, directory: Path) -> frozenset[str]:
        return self._child_names(directory, "package")

    def source_child_names(self, directory: Path) -> frozenset[str]:
        return self._child_names(directory, "source")

    def directory_names(self, directory: Path) -> frozenset[str]:
        return self._child_names(directory, "directories")

    def validate(self) -> None:
        self._check()
        try:
            for path, observation in self._directories.items():
                before = _directory_state(path)
                if before != observation.state:
                    if (
                        before is None
                        or observation.state is None
                        or before.path != observation.state.path
                    ):
                        self._changed(path)
                if (
                    os.name != "nt"
                    and not observation.racy
                    and not observation.has_links
                    and before == observation.state
                ):
                    continue
                listed = bool(observation.exact_names or observation.name_queries)
                state, (entries, probes) = self._stable_read(
                    path,
                    lambda: (
                        _read_entries(path) if listed else (),
                        {
                            candidate: _probe(candidate)
                            for candidate in observation.probes
                        },
                    ),
                )
                # The directory must still be the one captured, or still absent.
                if (state is None) != (observation.state is None) or (
                    state is not None
                    and observation.state is not None
                    and state.path != observation.state.path
                ):
                    self._changed(path)
                by_name = {entry.name: entry for entry in entries}
                if any(
                    _holds_exact(by_name, name, kind) != found
                    for (name, kind), found in observation.exact_names.items()
                ):
                    self._changed(path)
                if any(
                    _names(entries, query) != expected
                    for query, expected in observation.name_queries.items()
                ):
                    self._changed(path)
                for candidate, value in probes.items():
                    if value != self._probes[candidate]:
                        self._changed(candidate)
        except OSError as exc:
            self._failure = (
                f"Python import topology is unavailable during operation: {exc}"
            )
            raise PythonImportTopologyChangedError(self._failure) from exc
