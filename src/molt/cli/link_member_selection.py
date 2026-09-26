"""Bind actual linker extraction evidence to ordered archive member custody.

Producer inventory is only the universe of possible members. This parser never
infers selection from symbols or from an archive's presence on the command line.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

from molt.cli.native_symbol_inspection import _NativeGlobalSymbolFacts


class LinkMemberSelectionError(ValueError):
    """A linker trace cannot uniquely identify its selected archive members."""


@dataclass(frozen=True, slots=True)
class _ArchiveMemberIndex:
    members: dict[Path, dict[str, tuple[int, ...]]]
    member_basenames: dict[Path, dict[str, tuple[int, ...]]]
    full_labels: dict[str, tuple[Path, str]]
    canonical_paths: dict[str, Path]
    archive_basenames: frozenset[str]
    resolved_prefixes: dict[str, str] = field(default_factory=dict)

    @classmethod
    def from_facts(
        cls, facts_by_path: Mapping[Path, _NativeGlobalSymbolFacts]
    ) -> _ArchiveMemberIndex:
        archives: dict[Path, dict[str, tuple[int, ...]]] = {}
        basename_members: dict[Path, dict[str, tuple[int, ...]]] = {}
        labels: dict[str, tuple[Path, str]] = {}
        canonical_paths: dict[str, Path] = {}
        for supplied_path, facts in facts_by_path.items():
            path = supplied_path.resolve(strict=False)
            spelling = str(path)
            if (
                not supplied_path.is_absolute()
                or spelling != str(supplied_path)
                or spelling in canonical_paths
                or path in archives
            ):
                raise LinkMemberSelectionError(
                    "archive selection needs distinct resolved absolute paths: "
                    f"{supplied_path}"
                )
            if facts.members is None:
                raise LinkMemberSelectionError(
                    f"archive has no ordered member facts: {path}"
                )
            names: dict[str, list[int]] = {}
            basenames: dict[str, list[int]] = {}
            for ordinal, item in enumerate(facts.members):
                if item.identity.ordinal != ordinal or not item.identity.member.name:
                    raise LinkMemberSelectionError(
                        f"archive has invalid member ordinal/name custody: {path}"
                    )
                name = item.identity.member.name
                names.setdefault(name, []).append(ordinal)
                basenames.setdefault(
                    name.replace("\\", "/").rsplit("/", 1)[-1], []
                ).append(ordinal)
                labels[f"{path}({name})"] = (path, name)
            archives[path] = {name: tuple(values) for name, values in names.items()}
            basename_members[path] = {
                name: tuple(values) for name, values in basenames.items()
            }
            canonical_paths[spelling] = path
        return cls(
            archives,
            basename_members,
            labels,
            canonical_paths,
            frozenset(path.name for path in archives),
        )

    def _resolve_prefix(self, raw: str) -> str:
        if raw not in self.resolved_prefixes:
            self.resolved_prefixes[raw] = str(Path(raw).resolve(strict=False))
        return self.resolved_prefixes[raw]

    def _unique_member(
        self,
        path: Path,
        name: str,
        *,
        basename: bool = False,
        require_unique: bool = True,
    ) -> tuple[Path, int] | None:
        names = self.member_basenames[path] if basename else self.members[path]
        ordinals = names.get(name)
        if ordinals is None:
            raise LinkMemberSelectionError(
                f"unknown selected member of tracked archive {path}: {name!r}"
            )
        if len(ordinals) != 1:
            if not require_unique:
                return None
            raise LinkMemberSelectionError(
                f"ambiguous selected member of tracked archive {path}: {name!r}"
            )
        return path, ordinals[0]

    def full_member(
        self, token: str, *, require_unique: bool = True
    ) -> tuple[Path, int] | None:
        """Match exact labels first, then resolve a path before its delimiter.

        The fallback follows host filesystem path identity without Unicode
        case-folding. It scans delimiter positions, not tracked archives, so
        parentheses in the archive path or member name remain unambiguous.
        """
        exact = self.full_labels.get(token)
        if exact is not None:
            return self._unique_member(*exact, require_unique=require_unique)
        position = token.rfind("(")
        while position >= 0:
            raw_path = Path(token[:position])
            if raw_path.is_absolute():
                path = self.canonical_paths.get(self._resolve_prefix(token[:position]))
                if path is not None:
                    if not token.endswith(")"):
                        raise LinkMemberSelectionError(
                            f"malformed selected member of tracked archive {path}: {token!r}"
                        )
                    name = token[position + 1 : -1]
                    return self._unique_member(
                        path, name, require_unique=require_unique
                    )
            position = token.rfind("(", 0, position)
        return None

    def reject_unrecognized_tracked_label(self, token: str) -> None:
        """Do not reinterpret a malformed tracked-member row as dormant."""
        if token in self.canonical_paths:
            raise LinkMemberSelectionError(
                f"tracked archive lacks a selected member label: {token!r}"
            )
        plain_path = "(" not in token and "[" not in token and token.rfind(":") <= 1
        if (
            plain_path
            and Path(token).is_absolute()
            and self._resolve_prefix(token) in self.canonical_paths
        ):
            raise LinkMemberSelectionError(
                f"tracked archive lacks a selected member label: {token!r}"
            )
        parent = Path(token).parent
        if (
            parent.is_absolute()
            and self._resolve_prefix(str(parent)) in self.canonical_paths
        ):
            raise LinkMemberSelectionError(
                f"unrecognized selected member spelling of tracked archive: {token!r}"
            )
        for marker in ("[", ":"):
            position = token.rfind(marker)
            while position >= 0:
                raw = token[:position]
                if (
                    Path(raw).is_absolute()
                    and self._resolve_prefix(raw) in self.canonical_paths
                ):
                    raise LinkMemberSelectionError(
                        f"unrecognized selected member spelling of tracked archive: {token!r}"
                    )
                position = token.rfind(marker, 0, position)
        position = token.rfind("(")
        while position >= 0:
            if token[:position] in self.archive_basenames:
                raise LinkMemberSelectionError(
                    f"non-absolute selected member label is ambiguous: {token!r}"
                )
            position = token.rfind("(", 0, position)


def _absolute_stdout_selection(
    index: _ArchiveMemberIndex, *, stdout: str, dialect: str
) -> dict[Path, set[int]]:
    """The GNU/WASM --trace and Mach-O -t streams share one input grammar."""
    saw_input = False
    selected: dict[Path, set[int]] = {path: set() for path in index.members}
    for line in stdout.splitlines():
        if not Path(line).is_absolute():
            index.reject_unrecognized_tracked_label(line)
            continue
        saw_input = True
        member = index.full_member(line)
        if member is not None:
            path, ordinal = member
            selected[path].add(ordinal)
        elif not (
            Path(line).suffix.lower() in {".a", ".lib"}
            and index._resolve_prefix(line) in index.canonical_paths
        ):
            index.reject_unrecognized_tracked_label(line)
    if not saw_input:
        raise LinkMemberSelectionError(
            f"{dialect} trace has no absolute input-read evidence"
        )
    return selected


def _gnu_selection(
    index: _ArchiveMemberIndex, *, stdout: str, why_extract: str | None
) -> dict[Path, set[int]]:
    if why_extract is None:
        raise LinkMemberSelectionError(
            "GNU/WASM selection requires --why-extract evidence"
        )
    rows = why_extract.splitlines()
    if not rows or rows[0] != "reference\textracted\tsymbol":
        raise LinkMemberSelectionError("--why-extract header is missing or invalid")
    selected = _absolute_stdout_selection(index, stdout=stdout, dialect="GNU/WASM")
    why_selected: dict[Path, set[int]] = {path: set() for path in index.members}
    for line in rows[1:]:
        parts = line.split("\t")
        if len(parts) != 3 or not all(parts):
            raise LinkMemberSelectionError(f"malformed --why-extract row: {line!r}")
        member = index.full_member(parts[1])
        if member is None:
            index.reject_unrecognized_tracked_label(parts[1])
        else:
            path, ordinal = member
            why_selected[path].add(ordinal)
    if why_selected != selected:
        raise LinkMemberSelectionError("--why-extract and input trace disagree")
    return selected


def _coff_selection(index: _ArchiveMemberIndex, *, stderr: str) -> dict[Path, set[int]]:
    archives = index.members
    selected: dict[Path, set[int]] = {path: set() for path in archives}
    by_basename: dict[str, list[Path]] = {}
    for path in archives:
        by_basename.setdefault(path.name, []).append(path)

    read_archives: set[Path] = set()
    foreign_basenames: set[str] = set()
    reading_member_tokens: list[str] = []
    loaded_member_tokens: list[str] = []
    saw_read = False

    def basename_member(token: str, *, require_unique: bool) -> tuple[Path, int] | None:
        position = token.rfind("(")
        while position >= 0:
            basename = token[:position]
            candidates = by_basename.get(basename)
            if candidates is not None:
                if not token.endswith(")"):
                    raise LinkMemberSelectionError(
                        f"malformed COFF member of tracked archive: {token!r}"
                    )
                tracked_reads = [path for path in candidates if path in read_archives]
                if not tracked_reads:
                    if basename in foreign_basenames:
                        return None
                    raise LinkMemberSelectionError(
                        f"COFF member lacks full archive Reading row: {token!r}"
                    )
                if basename in foreign_basenames or len(tracked_reads) != 1:
                    if not require_unique:
                        return None
                    raise LinkMemberSelectionError(
                        f"ambiguous selected COFF archive basename: {basename!r}"
                    )
                path = tracked_reads[0]
                name = token[position + 1 : -1]
                return index._unique_member(
                    path, name, basename=True, require_unique=require_unique
                )
            position = token.rfind("(", 0, position)
        return None

    for line in stderr.splitlines():
        if line.startswith("lld-link: Reading "):
            token = line.removeprefix("lld-link: Reading ")
            if not token:
                raise LinkMemberSelectionError("empty COFF Reading row")
            path = Path(token)
            if path.suffix.lower() not in {".a", ".lib"}:
                reading_member_tokens.append(token)
                continue
            saw_read = True
            if not path.is_absolute():
                if path.name in by_basename:
                    foreign_basenames.add(path.name)
                continue
            resolved = str(path.resolve(strict=False))
            tracked = index.canonical_paths.get(resolved)
            if tracked is not None:
                read_archives.add(tracked)
            elif path.name in by_basename:
                foreign_basenames.add(path.name)
        elif line.startswith("lld-link: Loaded "):
            row = line.removeprefix("lld-link: Loaded ")
            token, separator, symbol = row.rpartition(" for ")
            if not separator or not token or not symbol:
                raise LinkMemberSelectionError(f"malformed COFF Loaded row: {line!r}")
            loaded_member_tokens.append(token)
    if not saw_read:
        raise LinkMemberSelectionError(
            "COFF /verbose trace has no archive input-read evidence"
        )
    read_members: dict[Path, set[int]] = {path: set() for path in archives}
    for token in reading_member_tokens:
        member = index.full_member(token, require_unique=False)
        if member is None:
            member = basename_member(token, require_unique=False)
        if member is not None:
            path, ordinal = member
            read_members[path].add(ordinal)
    for token in loaded_member_tokens:
        member = index.full_member(token)
        if member is None:
            member = basename_member(token, require_unique=True)
        if member is None:
            position = token.rfind("(")
            foreign_only = False
            while position >= 0:
                basename = token[:position]
                if basename in foreign_basenames and not any(
                    path in read_archives for path in by_basename.get(basename, ())
                ):
                    foreign_only = True
                    break
                position = token.rfind("(", 0, position)
            if not foreign_only:
                index.reject_unrecognized_tracked_label(token)
            continue
        path, ordinal = member
        if path not in read_archives or ordinal not in read_members[path]:
            raise LinkMemberSelectionError(
                f"COFF Loaded member lacks matching full archive/member Reading rows: {token!r}"
            )
        selected[path].add(ordinal)
    return selected


def selected_archive_members(
    facts_by_path: Mapping[Path, _NativeGlobalSymbolFacts],
    *,
    dialect: str,
    stdout: str,
    stderr: str,
    why_extract: str | None = None,
) -> Mapping[Path, tuple[int, ...]]:
    """Return only link-selected ordinals from one actual linker invocation.

    The caller owns tool-capability admission and binds these facts to the
    invocation's candidate output, logical plan and exact tool identity.
    """
    index = _ArchiveMemberIndex.from_facts(facts_by_path)
    if dialect in {"elf-gnu", "wasm"}:
        selected = _gnu_selection(index, stdout=stdout, why_extract=why_extract)
    elif dialect == "macho":
        selected = _absolute_stdout_selection(index, stdout=stdout, dialect="Mach-O -t")
    elif dialect == "coff-msvc":
        selected = _coff_selection(index, stderr=stderr)
    else:
        raise LinkMemberSelectionError(f"unsupported selection dialect: {dialect!r}")
    return {path: tuple(sorted(ordinals)) for path, ordinals in selected.items()}
