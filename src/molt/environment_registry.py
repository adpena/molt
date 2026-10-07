"""The one registry of ``MOLT_*`` environment names, and the one diagnostic.

Authority: ``src/molt/environment_registry.toml`` (hand-edited). This module
loads the generated projection ``molt._environment_registry`` so the shipped
CLI never parses TOML on start. ``tools/gen_environment_registry.py`` renders
the projection and the reference page; ``tools/check_environment_registry.py``
proves that every ``MOLT_*`` name the source reads is registered.

Three kinds of entry:

* ``variable`` — one fixed name, one fact.
* ``stem`` + ``family`` — names built at run time as ``<STEM><SUFFIX>`` (for
  example ``MOLT_BUILD_TIMEOUT_SEC``). A family may name a ``root_fallback``:
  the fixed name that is read when no stem-specific name is set.
* ``retired`` — a removed name and its replacement. Setting a retired name
  fails closed; Molt keeps no aliases.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping
from dataclasses import dataclass
import difflib
import functools
import os
import sys
from typing import Any, TextIO

AUDIENCES: tuple[str, ...] = ("user", "developer", "ci", "internal")
KINDS: tuple[str, ...] = (
    "bool",
    "int",
    "float",
    "path",
    "path-list",
    "enum",
    "string",
    "list",
)
NAME_PREFIX = "MOLT_"


class EnvironmentRegistryError(RuntimeError):
    """A retired ``MOLT_*`` name is set, or the registry data is malformed."""


@dataclass(frozen=True, slots=True)
class EnvironmentVariable:
    name: str
    audience: str
    kind: str
    default: str
    values: tuple[str, ...]
    owner: str
    summary: str


@dataclass(frozen=True, slots=True)
class EnvironmentStem:
    name: str
    owner: str
    summary: str


@dataclass(frozen=True, slots=True)
class EnvironmentFamily:
    suffix: str
    root_fallback: str
    audience: str
    kind: str
    default: str
    values: tuple[str, ...]
    owner: str
    summary: str

    @property
    def display_name(self) -> str:
        return f"MOLT_<STEM>{self.suffix}"


@dataclass(frozen=True, slots=True)
class EnvironmentPrefixFamily:
    """Names built from a fixed prefix and an open member set (one per crate)."""

    prefix: str
    audience: str
    kind: str
    owner: str
    summary: str

    @property
    def display_name(self) -> str:
        return f"{self.prefix}<MEMBER>"


@dataclass(frozen=True, slots=True)
class RetiredVariable:
    name: str
    replacement: str  # the registered name to set instead, or "" with a note
    retired: str
    note: str = ""
    # Files of a separate process that rejects the name itself (plus its tests).
    rejected_by: tuple[str, ...] = ()

    @property
    def advice(self) -> str:
        if self.replacement:
            return f"set {self.replacement} instead (Molt keeps no aliases)"
        return self.note


@dataclass(frozen=True, slots=True)
class RetiredSuffix:
    """A retired family suffix: ``<STEM><suffix>`` for every stem and the root."""

    suffix: str
    replacement_suffix: str
    retired: str


@dataclass(frozen=True, slots=True)
class FamilyMatch:
    stem: EnvironmentStem | None
    family: EnvironmentFamily


@dataclass(frozen=True)  # cached_property needs __dict__; one instance per process
class EnvironmentRegistry:
    variables: tuple[EnvironmentVariable, ...]
    stems: tuple[EnvironmentStem, ...]
    families: tuple[EnvironmentFamily, ...]
    prefix_families: tuple[EnvironmentPrefixFamily, ...]
    retired: tuple[RetiredVariable, ...]
    retired_suffixes: tuple[RetiredSuffix, ...]

    @functools.cached_property
    def variable_by_name(self) -> dict[str, EnvironmentVariable]:
        return {row.name: row for row in self.variables}

    @functools.cached_property
    def stem_by_name(self) -> dict[str, EnvironmentStem]:
        return {row.name: row for row in self.stems}

    @functools.cached_property
    def retired_by_name(self) -> dict[str, RetiredVariable]:
        return {row.name: row for row in self.retired}

    @functools.cached_property
    def root_fallback_families(self) -> dict[str, EnvironmentFamily]:
        return {row.root_fallback: row for row in self.families if row.root_fallback}

    def lookup_variable(self, name: str) -> EnvironmentVariable | None:
        return self.variable_by_name.get(name)

    def lookup_family(self, name: str) -> FamilyMatch | None:
        """Resolve a composed name to its stem and family, or ``None``."""

        family = self.root_fallback_families.get(name)
        if family is not None:
            return FamilyMatch(stem=None, family=family)
        for family in self.families:
            if not name.endswith(family.suffix):
                continue
            stem = self.stem_by_name.get(name[: -len(family.suffix)])
            if stem is not None:
                return FamilyMatch(stem=stem, family=family)
        return None

    def lookup_prefix_family(self, name: str) -> EnvironmentPrefixFamily | None:
        for family in self.prefix_families:
            if name.startswith(family.prefix) and len(name) > len(family.prefix):
                return family
        return None

    def lookup_retired(self, name: str) -> RetiredVariable | None:
        """Resolve a fixed retired name, or a retired family suffix on a stem."""

        fixed = self.retired_by_name.get(name)
        if fixed is not None:
            return fixed
        for row in self.retired_suffixes:
            if not name.endswith(row.suffix):
                continue
            stem_part = name[: -len(row.suffix)]
            if stem_part == NAME_PREFIX[:-1] or stem_part in self.stem_by_name:
                return RetiredVariable(
                    name=name,
                    replacement=f"{stem_part}{row.replacement_suffix}",
                    retired=row.retired,
                )
        return None

    def is_registered(self, name: str) -> bool:
        return (
            name in self.variable_by_name
            or self.lookup_family(name) is not None
            or self.lookup_prefix_family(name) is not None
        )

    def registered_names(self) -> Iterator[str]:
        """Every fixed name plus every stem/family expansion, in a stable order."""

        yield from (row.name for row in self.variables)
        for family in self.families:
            if family.root_fallback:
                yield family.root_fallback
            for stem in self.stems:
                yield f"{stem.name}{family.suffix}"


def _tuple_of_str(value: object, *, context: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise EnvironmentRegistryError(f"{context}: expected a list of strings")
    return tuple(value)


def _str(row: Mapping[str, object], key: str, *, context: object) -> str:
    """Read one required string field; `context` names the row in errors."""
    value = row.get(key)
    if not isinstance(value, str):
        raise EnvironmentRegistryError(f"{context}: field {key!r} must be a string")
    return value


def registry_from_payload(
    payload: Mapping[str, object], *, validate: bool = True
) -> EnvironmentRegistry:
    """Build the typed registry from a payload.

    ``validate=True`` checks every field (the generator's path, for hand-edited
    data). The shipped projection was validated when it was generated, so the
    CLI loads it with ``validate=False`` and pays only object construction.
    """

    if not validate:
        return _trusted_registry(payload)
    variables = tuple(
        EnvironmentVariable(
            name=_str(row, "name", context="variable"),
            audience=_str(row, "audience", context=row.get("name", "variable")),
            kind=_str(row, "kind", context=row.get("name", "variable")),
            default=_str(row, "default", context=row.get("name", "variable")),
            values=_tuple_of_str(row.get("values", []), context=str(row.get("name"))),
            owner=_str(row, "owner", context=row.get("name", "variable")),
            summary=_str(row, "summary", context=row.get("name", "variable")),
        )
        for row in _rows(payload, "variables")
    )
    stems = tuple(
        EnvironmentStem(
            name=_str(row, "name", context="stem"),
            owner=_str(row, "owner", context=row.get("name", "stem")),
            summary=_str(row, "summary", context=row.get("name", "stem")),
        )
        for row in _rows(payload, "stems")
    )
    families = tuple(
        EnvironmentFamily(
            suffix=_str(row, "suffix", context="family"),
            root_fallback=_str(row, "root_fallback", context=row.get("suffix", "")),
            audience=_str(row, "audience", context=row.get("suffix", "family")),
            kind=_str(row, "kind", context=row.get("suffix", "family")),
            default=_str(row, "default", context=row.get("suffix", "family")),
            values=_tuple_of_str(row.get("values", []), context=str(row.get("suffix"))),
            owner=_str(row, "owner", context=row.get("suffix", "family")),
            summary=_str(row, "summary", context=row.get("suffix", "family")),
        )
        for row in _rows(payload, "families")
    )
    prefix_families = tuple(
        EnvironmentPrefixFamily(
            prefix=_str(row, "prefix", context="prefix_family"),
            audience=_str(row, "audience", context=row.get("prefix", "prefix_family")),
            kind=_str(row, "kind", context=row.get("prefix", "prefix_family")),
            owner=_str(row, "owner", context=row.get("prefix", "prefix_family")),
            summary=_str(row, "summary", context=row.get("prefix", "prefix_family")),
        )
        for row in _rows(payload, "prefix_families")
    )
    retired = tuple(
        RetiredVariable(
            name=_str(row, "name", context="retired"),
            replacement=_str(row, "replacement", context=row.get("name", "retired")),
            retired=_str(row, "retired", context=row.get("name", "retired")),
            note=_str(row, "note", context=row.get("name", "retired")),
            rejected_by=_tuple_of_str(
                row.get("rejected_by", []), context=str(row.get("name"))
            ),
        )
        for row in _rows(payload, "retired")
    )
    retired_suffixes = tuple(
        RetiredSuffix(
            suffix=_str(row, "suffix", context="retired_suffix"),
            replacement_suffix=_str(
                row, "replacement_suffix", context=row.get("suffix", "retired_suffix")
            ),
            retired=_str(row, "retired", context=row.get("suffix", "retired_suffix")),
        )
        for row in _rows(payload, "retired_suffixes")
    )
    return EnvironmentRegistry(
        variables=variables,
        stems=stems,
        families=families,
        prefix_families=prefix_families,
        retired=retired,
        retired_suffixes=retired_suffixes,
    )


def _rows(payload: Mapping[str, object], key: str) -> list[Mapping[str, object]]:
    rows = payload.get(key)
    if not isinstance(rows, list):
        raise EnvironmentRegistryError(f"registry payload: {key!r} must be a list")
    for row in rows:
        if not isinstance(row, Mapping):
            raise EnvironmentRegistryError(
                f"registry payload: {key!r} rows must be tables"
            )
    return rows


def _trusted_registry(payload: Mapping[str, Any]) -> EnvironmentRegistry:
    """Build the registry from the generated projection, validated at generation."""
    rows = payload["variables"]
    return EnvironmentRegistry(
        variables=tuple(
            EnvironmentVariable(
                r["name"],
                r["audience"],
                r["kind"],
                r["default"],
                tuple(r["values"]),
                r["owner"],
                r["summary"],
            )
            for r in rows
        ),
        stems=tuple(
            EnvironmentStem(r["name"], r["owner"], r["summary"])
            for r in payload["stems"]
        ),
        families=tuple(
            EnvironmentFamily(
                r["suffix"],
                r["root_fallback"],
                r["audience"],
                r["kind"],
                r["default"],
                tuple(r["values"]),
                r["owner"],
                r["summary"],
            )
            for r in payload["families"]
        ),
        prefix_families=tuple(
            EnvironmentPrefixFamily(
                r["prefix"], r["audience"], r["kind"], r["owner"], r["summary"]
            )
            for r in payload["prefix_families"]
        ),
        retired=tuple(
            RetiredVariable(
                r["name"],
                r["replacement"],
                r["retired"],
                r["note"],
                tuple(r["rejected_by"]),
            )
            for r in payload["retired"]
        ),
        retired_suffixes=tuple(
            RetiredSuffix(r["suffix"], r["replacement_suffix"], r["retired"])
            for r in payload["retired_suffixes"]
        ),
    )


@functools.cache
def load_registry() -> EnvironmentRegistry:
    """Load the shipped projection once per process (validated at generation)."""

    from molt import _environment_registry

    return registry_from_payload(
        _environment_registry.environment_registry(), validate=False
    )


# --------------------------------------------------------------------------
# The diagnostic
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class EnvironmentFinding:
    name: str
    message: str


def _molt_keys(env: Mapping[str, str]) -> list[tuple[str, str]]:
    """``(key, canonical_upper)`` for every key that names a ``MOLT_*`` fact."""

    found: list[tuple[str, str]] = []
    for key in env:
        upper = key.upper()
        if upper.startswith(NAME_PREFIX):
            found.append((key, upper))
    found.sort()
    return found


def inspect_environment(
    env: Mapping[str, str] | None = None,
) -> tuple[list[EnvironmentFinding], list[EnvironmentFinding]]:
    """Return ``(errors, warnings)`` for the ``MOLT_*`` names set in ``env``.

    Errors name retired variables (fail closed: the replacement is in the
    message). Warnings name unknown variables with close matches, and, on
    case-sensitive platforms, names whose case cannot be read by Molt.
    The registry is loaded only when at least one ``MOLT_*`` key is set.
    """

    source = os.environ if env is None else env
    keys = _molt_keys(source)
    if not keys:
        return [], []
    registry = load_registry()
    errors: list[EnvironmentFinding] = []
    warnings: list[EnvironmentFinding] = []
    case_insensitive = os.name == "nt"
    for key, upper in keys:
        if key != upper and not case_insensitive:
            warnings.append(
                EnvironmentFinding(
                    key,
                    f"{key} is not read by Molt: environment names are "
                    f"case-sensitive on this platform; set {upper} instead.",
                )
            )
            continue
        retired = registry.lookup_retired(upper)
        if retired is not None:
            errors.append(
                EnvironmentFinding(
                    key, f"{upper} was retired on {retired.retired}; {retired.advice}."
                )
            )
            continue
        if registry.is_registered(upper):
            continue
        close = difflib.get_close_matches(
            upper, list(registry.registered_names()), n=3, cutoff=0.8
        )
        hint = f" Did you mean {', '.join(close)}?" if close else ""
        warnings.append(
            EnvironmentFinding(
                key, f"{upper} is not a Molt environment variable; it is ignored.{hint}"
            )
        )
    return errors, warnings


_REPORTED_WARNINGS: set[str] = set()


def check_process_environment(
    env: Mapping[str, str] | None = None,
    *,
    stream: TextIO | None = None,
    program: str = "molt",
) -> None:
    """Warn once per unknown name on ``stream``; raise for a retired name.

    Called from the CLI entry point and from ``RunContext`` so users and
    developers both see the same diagnostic.
    """

    errors, warnings = inspect_environment(env)
    out = sys.stderr if stream is None else stream
    for finding in warnings:
        if finding.message in _REPORTED_WARNINGS:
            continue
        _REPORTED_WARNINGS.add(finding.message)
        print(f"{program}: warning: {finding.message}", file=out)
    if errors:
        raise EnvironmentRegistryError("; ".join(finding.message for finding in errors))
