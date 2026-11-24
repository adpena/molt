"""Operation-local custody for the filesystem facts Python imports consume."""

from __future__ import annotations

import os
import stat
import time
from dataclasses import dataclass, field
from pathlib import Path
from collections.abc import Callable, Iterable
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


class _ProbeObservation(NamedTuple):
    value: PythonImportPath | None
    has_link: bool


def _probe(path: Path, *, parent: _DirectoryState | None) -> _ProbeObservation:
    if os.name != "nt" and parent is not None and path.name not in ("", ".."):
        # The enclosing read fences the resolved parent. A direct entry
        # needs only its own mode; resolving every ancestor again adds no
        # observation. Do the lookup on the lexical path so the filesystem
        # still decides case and Unicode spelling.
        try:
            kind = stat.S_IFMT(path.lstat().st_mode)
        except (FileNotFoundError, NotADirectoryError):
            return _ProbeObservation(None, False)
        if kind != stat.S_IFLNK:
            resolved = parent.path / path.name
            return _ProbeObservation(PythonImportPath(resolved, kind), resolved != path)

    # A link's target may become valid without changing its containing
    # directory. Windows junctions and drive/UNC roots use full resolution.
    link = path.is_symlink()
    try:
        metadata = path.stat()
        resolved = path.resolve(strict=True)
        return _ProbeObservation(
            PythonImportPath(resolved, stat.S_IFMT(metadata.st_mode)),
            link or resolved != path,
        )
    except (FileNotFoundError, NotADirectoryError):
        return _ProbeObservation(None, link)


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


def _read_entries(path: Path, *, recorded: bool) -> tuple[os.DirEntry[str], ...]:
    scan = _input_report.scan_import_directory if recorded else os.scandir
    try:
        with scan(path) as entries:
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

    def record(self) -> dict[str, object]:
        """The semantic facts consumed, excluding disposable directory epochs."""
        self.validate()
        return {
            "probes": [
                [str(path), None if value is None else [str(value.path), value.mode]]
                for path, value in sorted(self._probes.items())
            ],
            "directories": [
                [
                    str(path),
                    None if observed.state is None else str(observed.state.path),
                    [
                        [name, kind, value]
                        for (name, kind), value in sorted(observed.exact_names.items())
                    ],
                    [
                        [kind, sorted(names)]
                        for kind, names in sorted(observed.name_queries.items())
                    ],
                ]
                for path, observed in sorted(self._directories.items())
            ],
        }

    @classmethod
    def from_record(cls, payload: object) -> PythonImportTopology:
        """Reobserve a persisted import topology; changed facts refuse admission."""
        if not isinstance(payload, dict) or set(payload) != {"probes", "directories"}:
            raise ValueError("invalid import topology record")
        probes, directories = payload["probes"], payload["directories"]
        if not isinstance(probes, list) or not isinstance(directories, list):
            raise ValueError("invalid import topology rows")
        result = cls()
        probe_rows: list[tuple[Path, PythonImportPath | None]] = []
        for row in probes:
            if (
                not isinstance(row, list)
                or len(row) != 2
                or not isinstance(row[0], str)
            ):
                raise ValueError("invalid import topology probe")
            expected = row[1]
            if expected is not None and (
                not isinstance(expected, list)
                or len(expected) != 2
                or not isinstance(expected[0], str)
                or type(expected[1]) is not int
            ):
                raise ValueError("invalid import topology outcome")
            probe_rows.append(
                (
                    _lexical_path(Path(row[0])),
                    None
                    if expected is None
                    else PythonImportPath(Path(expected[0]), expected[1]),
                )
            )
        result._reobserve_probes(path for path, _ in probe_rows)
        for path, expected_path in probe_rows:
            if result._probes[path] != expected_path:
                raise PythonImportTopologyChangedError(
                    f"recorded import lookup changed: {path}"
                )
        for row in directories:
            if (
                not isinstance(row, list)
                or len(row) != 4
                or not isinstance(row[0], str)
            ):
                raise ValueError("invalid import topology directory")
            path = Path(row[0])
            observed = result._directory(path)
            actual_path = None if observed.state is None else str(observed.state.path)
            if (
                actual_path != row[1]
                or not isinstance(row[2], list)
                or not isinstance(row[3], list)
            ):
                raise PythonImportTopologyChangedError(
                    f"recorded import directory changed: {path}"
                )
            for exact in row[2]:
                if (
                    not isinstance(exact, list)
                    or len(exact) != 3
                    or not isinstance(exact[0], str)
                    or exact[1] not in ("file", "directory")
                    or type(exact[2]) is not bool
                ):
                    raise ValueError("invalid exact import observation")
                if result.contains_exact(path, exact[0], exact[1]) != exact[2]:
                    raise PythonImportTopologyChangedError(
                        f"recorded import selection changed: {path}"
                    )
            for names in row[3]:
                if (
                    not isinstance(names, list)
                    or len(names) != 2
                    or names[0] not in ("package", "source", "directories")
                    or not isinstance(names[1], list)
                    or any(not isinstance(name, str) for name in names[1])
                ):
                    raise ValueError("invalid import listing observation")
                if result._child_names(path, names[0]) != frozenset(names[1]):
                    raise PythonImportTopologyChangedError(
                        f"recorded import listing changed: {path}"
                    )
        if result.record() != payload:
            raise ValueError("noncanonical import topology record")
        return result

    def _check(self) -> None:
        if self._failure is not None:
            raise PythonImportTopologyChangedError(self._failure)

    def _changed(self, path: Path) -> None:
        self._failure = f"Python import topology changed during operation: {path}"
        raise PythonImportTopologyChangedError(self._failure)

    def _stable_read(
        self, directory: Path, read: Callable[[_DirectoryState | None], _Read]
    ) -> tuple[_DirectoryState | None, _Read]:
        """Read while ``directory`` holds still; return its state with the value.

        A change between the two stats means the read may mix two states, so
        it is retried. Facts derive only from a coherent read and are checked
        against their capture at validation.
        """
        for _ in range(_STABLE_READ_ATTEMPTS):
            before = _directory_state(directory)
            value = read(before)
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
        # An absent parent is itself a negative lookup. Retain its existing
        # ancestor so creating or retargeting that path cannot grant old facts.
        if state is None and path.parent != path:
            if self.probe(
                path
            ) is not None and not _input_report.report_import_topology(self):
                # ENOTDIR: the blocking entry exists but is not a directory.
                # Its containing directory is the generation boundary.
                _input_report.report_directory(path.parent)
        return observation

    def _entries(
        self, path: Path, observation: _DirectoryObservation
    ) -> tuple[os.DirEntry[str], ...]:
        if observation.entries is None:
            recorded = _input_report.report_import_topology(self)
            if recorded:
                _input_report.report_use(
                    path, kind=_input_report.WorkerFactKind.DIRECTORY
                )
            else:
                _input_report.report_directory(path)
            before, entries = self._stable_read(
                path, lambda _: _read_entries(path, recorded=recorded)
            )
            observation.entries = entries
            observation.entries_by_name = {entry.name: entry for entry in entries}
            observation.has_links = any(
                entry.is_symlink() or getattr(entry, "is_junction", lambda: False)()
                for entry in entries
            )
            observation.racy |= before != observation.state
        return observation.entries

    def _remember_probe(
        self, path: Path, probe: _ProbeObservation, parent: _DirectoryObservation
    ) -> None:
        self._probes[path] = probe.value
        if _input_report.report_import_topology(self):
            _input_report.report_use(path, kind=_input_report.WorkerFactKind.PROBE)
        else:
            _input_report.report_probe(path)
            if probe.value is None and parent.state is not None:
                # A worker records its nearest existing boundary through
                # the recursive probes of missing parents.
                _input_report.report_directory(path.parent)
        parent.probes.add(path)
        # A symlink target may change without changing the selector's
        # parent. Verify that binding at every fence, even on POSIX.
        parent.has_links |= probe.has_link

    def _reobserve_probes(self, paths: Iterable[Path]) -> None:
        # Persisted queries are already known. Bracket siblings with one
        # directory fence, and retain the individual facts for validation.
        by_directory: dict[Path, dict[Path, None]] = {}
        for path in paths:
            by_directory.setdefault(path.parent, {})[path] = None
        for directory, children in by_directory.items():
            parent = self._directory(directory)
            pending = tuple(path for path in children if path not in self._probes)
            if pending:
                _, values = self._stable_read(
                    directory,
                    lambda state: tuple(_probe(path, parent=state) for path in pending),
                )
                for path, probe in zip(pending, values, strict=True):
                    self._remember_probe(path, probe, parent)

    def probe(self, path: Path) -> PythonImportPath | None:
        self._check()
        path = _lexical_path(path)
        if path not in self._probes:
            parent = self._directory(path.parent)
            _state, probe = self._stable_read(
                path.parent, lambda state: _probe(path, parent=state)
            )
            self._remember_probe(path, probe, parent)
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
                recorded = listed and _input_report.report_import_topology(self)
                state, (entries, probes) = self._stable_read(
                    path,
                    lambda state: (
                        _read_entries(path, recorded=recorded) if listed else (),
                        {
                            candidate: _probe(candidate, parent=state).value
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
