"""Minimal importlib.metadata implementation for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full metadata version semantics and remaining entry point selection edge cases.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from importlib import import_module
from typing import Iterable as _Iterable
from typing import cast
from ._collections import FreezableDefaultDict, Pair
from ._functools import method_cache, pass_none
from ._itertools import always_iterable, unique_everseen
from ._meta import PackageMetadata, SimplePath
import collections
import contextlib
import csv
import email
import functools
import importlib.abc as abc
import inspect
import itertools
import operator
import os
import pathlib
import posixpath
import re
import sys
import textwrap
import warnings
import zipfile

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_READ_FILE = _require_intrinsic("molt_importlib_read_file")
_MOLT_IMPORTLIB_METADATA_DIST_PATHS = _require_intrinsic(
    "molt_importlib_metadata_dist_paths"
)
_MOLT_IMPORTLIB_BOOTSTRAP_PAYLOAD = _require_intrinsic(
    "molt_importlib_bootstrap_payload"
)
_MOLT_IMPORTLIB_METADATA_ENTRY_POINTS_SELECT_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_entry_points_select_payload"
)
_MOLT_IMPORTLIB_METADATA_ENTRY_POINTS_FILTER_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_entry_points_filter_payload"
)
_MOLT_IMPORTLIB_METADATA_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_payload"
)
_MOLT_IMPORTLIB_METADATA_DISTRIBUTIONS_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_distributions_payload"
)
_MOLT_IMPORTLIB_METADATA_RECORD_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_record_payload"
)
_MOLT_IMPORTLIB_METADATA_PACKAGES_DISTRIBUTIONS_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_packages_distributions_payload"
)
_MOLT_IMPORTLIB_METADATA_NORMALIZE_NAME = _require_intrinsic(
    "molt_importlib_metadata_normalize_name"
)
_MOLT_IMPORTLIB_METADATA_TYPES_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_types_payload"
)
_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted")
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require")


def _load_types_payload() -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_METADATA_TYPES_PAYLOAD(
        __import__("typing"), abc, contextlib, itertools
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib.metadata types payload: dict expected")
    return payload


def _payload_get(payload: dict[str, object], name: str) -> object:
    if name not in payload:
        raise RuntimeError(f"invalid importlib.metadata types payload: missing {name}")
    return payload[name]


_TYPES_PAYLOAD = _load_types_payload()
List = _payload_get(_TYPES_PAYLOAD, "List")
Mapping = _payload_get(_TYPES_PAYLOAD, "Mapping")
MetaPathFinder = _payload_get(_TYPES_PAYLOAD, "MetaPathFinder")
Optional = _payload_get(_TYPES_PAYLOAD, "Optional")
suppress = _payload_get(_TYPES_PAYLOAD, "suppress")


class PackageNotFoundError(ModuleNotFoundError):
    pass


class DeprecatedNonAbstract:
    pass


class DeprecatedTuple(tuple):
    pass


class DistributionFinder(MetaPathFinder):
    @classmethod
    def find_distributions(cls, context=None):
        return ()


class FastPath(str):
    pass


class FileHash:
    def __init__(self, mode: str = "", value: str = "") -> None:
        self.mode = mode
        self.value = value

    def __repr__(self) -> str:
        return f"<FileHash mode={self.mode!r} value={self.value!r}>"


class Lookup(dict):
    pass


class MetadataPathFinder(DistributionFinder):
    @classmethod
    def find_distributions(cls, context=None):
        return ()


class Prepared:
    @staticmethod
    def normalize(name: str) -> str:
        return _normalize(name)


class Sectioned:
    pass


class PackagePath(str):
    def __new__(
        cls,
        path: str,
        dist: "Distribution" | None = None,
        hash_value: str | None = None,
        size_text: str | None = None,
    ):
        obj = str.__new__(cls, path)
        obj._path = path
        obj.dist = dist
        obj.hash = _parse_file_hash(hash_value)
        obj.size = _parse_file_size(size_text)
        base_dir = os.path.dirname(dist._path) if dist is not None else ""
        obj._base_dir = base_dir
        return obj

    def __str__(self) -> str:
        return self._path

    def __repr__(self) -> str:
        return repr(self._path)

    def __fspath__(self) -> str:
        return self._path

    def locate(self) -> pathlib.Path:
        return pathlib.Path(self._base_dir).joinpath(self._path)

    def read_text(self, encoding: str = "utf-8") -> str:
        _ensure_fs_read()
        target = str(self.locate())
        text = _read_text_file(target)
        if text is None:
            raise FileNotFoundError(target)
        return text

    def read_binary(self) -> bytes:
        _ensure_fs_read()
        target = str(self.locate())
        raw = _MOLT_IMPORTLIB_READ_FILE(target)
        if not isinstance(raw, bytes):
            raise RuntimeError("invalid importlib read payload: bytes expected")
        return raw


starmap = itertools.starmap


class _Metadata:
    def __init__(
        self, mapping: dict[str, str], multi_values: dict[str, list[str]] | None = None
    ) -> None:
        self._raw = mapping
        self._lower = {k.lower(): v for k, v in mapping.items()}
        self._multi_lower = {k.lower(): [v] for k, v in mapping.items()}
        if multi_values is not None:
            for key, values in multi_values.items():
                self._multi_lower[key.lower()] = list(values)

    def __getitem__(self, key: str) -> str:
        return self._lower[key.lower()]

    def get(self, key: str, default: str | None = None) -> str | None:
        return self._lower.get(key.lower(), default)

    def get_all(self, key: str) -> list[str] | None:
        values = self._multi_lower.get(key.lower())
        if values is None:
            return None
        return list(values)

    def keys(self):
        return self._raw.keys()


class Distribution:
    def __init__(
        self,
        name: str,
        version: str,
        path: str,
        metadata: _Metadata,
        entry_points_payload: list[tuple[str, str, str]],
        requires_dist: list[str],
        provides_extra: list[str],
        requires_python: str | None,
    ) -> None:
        self._name = name
        self._version = version
        self._path = path
        self._metadata = metadata
        self._entry_points_payload = entry_points_payload
        self._requires_dist = requires_dist
        self._provides_extra = provides_extra
        self._requires_python = requires_python

    @property
    def metadata(self) -> _Metadata:
        return self._metadata

    @property
    def version(self) -> str:
        return self._version

    @property
    def requires(self) -> list[str] | None:
        if not self._requires_dist:
            return None
        return list(self._requires_dist)

    def read_text(self, filename: str) -> str | None:
        target = os.path.join(self._path, filename)
        return _read_text_file(target)


class PathDistribution(Distribution):
    pass


class EntryPoint:
    __slots__ = ("name", "value", "group")

    def __init__(self, name: str, value: str, group: str) -> None:
        self.name = name
        self.value = value
        self.group = group

    def __eq__(self, other):
        if not isinstance(other, EntryPoint):
            return NotImplemented
        return (
            self.name == other.name
            and self.value == other.value
            and self.group == other.group
        )

    def __hash__(self) -> int:
        return hash((self.name, self.value, self.group))

    def __repr__(self) -> str:
        return f"EntryPoint(name={self.name!r}, value={self.value!r}, group={self.group!r})"


class EntryPoints:
    def __init__(self, entries: _Iterable[EntryPoint] = ()) -> None:
        self._entries = tuple(entries)

    def __iter__(self):
        return iter(self._entries)

    def __len__(self) -> int:
        return len(self._entries)

    def __getitem__(self, name):
        if isinstance(name, slice):
            return EntryPoints(self._entries[name])
        try:
            return next(iter(self.select(name=name)))
        except StopIteration:
            raise KeyError(name) from None

    @property
    def names(self) -> set[str]:
        return {entry.name for entry in self}

    @property
    def groups(self) -> set[str]:
        return {entry.group for entry in self}

    def select(self, **params) -> "EntryPoints":
        items = list(self._entries)
        for attr, expected in params.items():
            items = [ep for ep in items if getattr(ep, attr) == expected]
        return EntryPoints(items)


__all__ = [
    "DeprecatedNonAbstract",
    "DeprecatedTuple",
    "Distribution",
    "DistributionFinder",
    "EntryPoint",
    "EntryPoints",
    "FastPath",
    "FileHash",
    "FreezableDefaultDict",
    "List",
    "Lookup",
    "Mapping",
    "MetaPathFinder",
    "MetadataPathFinder",
    "Optional",
    "PackageMetadata",
    "PackageNotFoundError",
    "PackagePath",
    "Pair",
    "PathDistribution",
    "Prepared",
    "Sectioned",
    "SimplePath",
    "abc",
    "always_iterable",
    "cast",
    "collections",
    "contextlib",
    "csv",
    "distribution",
    "distributions",
    "email",
    "entry_points",
    "files",
    "functools",
    "import_module",
    "inspect",
    "itertools",
    "metadata",
    "method_cache",
    "operator",
    "os",
    "packages_distributions",
    "pass_none",
    "pathlib",
    "posixpath",
    "re",
    "requires",
    "starmap",
    "suppress",
    "sys",
    "textwrap",
    "unique_everseen",
    "version",
    "warnings",
    "zipfile",
]

_DIST_CACHE: dict[str, Distribution] | None = None
_DIST_PATH_SNAPSHOT: tuple[str, ...] | None = None


def _normalize(name: str) -> str:
    normalized = _MOLT_IMPORTLIB_METADATA_NORMALIZE_NAME(name)
    if not isinstance(normalized, str):
        raise RuntimeError("invalid importlib metadata normalize payload: str expected")
    return normalized


def _ensure_fs_read() -> None:
    if _MOLT_CAPABILITIES_TRUSTED():
        return
    _MOLT_CAPABILITIES_REQUIRE("fs.read")


def _metadata_module_file() -> str | None:
    module_file = globals().get("__file__")
    if isinstance(module_file, str) and module_file:
        return module_file
    return None


def _path_snapshot() -> tuple[str, ...]:
    try:
        return tuple(sys.path)
    except Exception:
        return ()


def _resolved_search_paths(snapshot: tuple[str, ...]) -> tuple[str, ...]:
    payload = _MOLT_IMPORTLIB_BOOTSTRAP_PAYLOAD(snapshot, _metadata_module_file())
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib bootstrap payload: dict expected")
    resolved = payload.get("resolved_search_paths")
    if not isinstance(resolved, (list, tuple)):
        raise RuntimeError("invalid importlib bootstrap payload: resolved_search_paths")
    out: list[str] = []
    for entry in resolved:
        if not isinstance(entry, str):
            raise RuntimeError("invalid importlib bootstrap payload: str entries")
        out.append(entry)
    return tuple(out)


def _iter_dist_paths(search_paths: tuple[str, ...]) -> _Iterable[str]:
    payload = _MOLT_IMPORTLIB_METADATA_DIST_PATHS(search_paths, _metadata_module_file())
    if not isinstance(payload, (list, tuple)):
        raise RuntimeError(
            "invalid importlib metadata dist paths payload: sequence expected"
        )
    for entry in payload:
        if not isinstance(entry, str):
            raise RuntimeError(
                "invalid importlib metadata dist paths payload: str entries"
            )
        yield entry


def _entry_points_payload(
    search_paths: tuple[str, ...],
    group: str | None = None,
    name: str | None = None,
    value: str | None = None,
) -> list[tuple[str, str, str]]:
    payload = _MOLT_IMPORTLIB_METADATA_ENTRY_POINTS_FILTER_PAYLOAD(
        search_paths, _metadata_module_file(), group, name, value
    )
    if not isinstance(payload, (list, tuple)):
        raise RuntimeError(
            "invalid importlib metadata entry points payload: sequence expected"
        )
    out: list[tuple[str, str, str]] = []
    for entry in payload:
        if (
            not isinstance(entry, (list, tuple))
            or len(entry) != 3
            or not isinstance(entry[0], str)
            or not isinstance(entry[1], str)
            or not isinstance(entry[2], str)
        ):
            raise RuntimeError(
                "invalid importlib metadata entry points payload: triplets expected"
            )
        out.append((entry[0], entry[1], entry[2]))
    return out


def _coerce_metadata_payload(payload: object) -> dict[str, object]:
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib metadata payload: dict expected")
    path_value = payload.get("path")
    name = payload.get("name")
    version = payload.get("version")
    metadata = payload.get("metadata")
    entry_points = payload.get("entry_points")
    requires_dist = payload.get("requires_dist")
    provides_extra = payload.get("provides_extra")
    requires_python = payload.get("requires_python")
    if not isinstance(path_value, str):
        raise RuntimeError("invalid importlib metadata payload: path")
    if not isinstance(name, str):
        raise RuntimeError("invalid importlib metadata payload: name")
    if not isinstance(version, str):
        raise RuntimeError("invalid importlib metadata payload: version")
    if not isinstance(metadata, dict):
        raise RuntimeError("invalid importlib metadata payload: metadata")
    for key, value in metadata.items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise RuntimeError("invalid importlib metadata payload: metadata values")
    if not isinstance(entry_points, (list, tuple)):
        raise RuntimeError("invalid importlib metadata payload: entry_points")
    for entry in entry_points:
        if (
            not isinstance(entry, (list, tuple))
            or len(entry) != 3
            or not isinstance(entry[0], str)
            or not isinstance(entry[1], str)
            or not isinstance(entry[2], str)
        ):
            raise RuntimeError(
                "invalid importlib metadata payload: entry_points values"
            )
    if not isinstance(requires_dist, (list, tuple)) or not all(
        isinstance(entry, str) for entry in requires_dist
    ):
        raise RuntimeError("invalid importlib metadata payload: requires_dist")
    if not isinstance(provides_extra, (list, tuple)) or not all(
        isinstance(entry, str) for entry in provides_extra
    ):
        raise RuntimeError("invalid importlib metadata payload: provides_extra")
    if requires_python is not None and not isinstance(requires_python, str):
        raise RuntimeError("invalid importlib metadata payload: requires_python")
    return {
        "path": path_value,
        "name": name,
        "version": version,
        "metadata": dict(metadata),
        "entry_points": [tuple(entry) for entry in entry_points],
        "requires_dist": list(requires_dist),
        "provides_extra": list(provides_extra),
        "requires_python": requires_python,
    }


def _metadata_payload(path: str) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_METADATA_PAYLOAD(path)
    return _coerce_metadata_payload(payload)


def _distributions_payload(search_paths: tuple[str, ...]) -> list[dict[str, object]]:
    payload = _MOLT_IMPORTLIB_METADATA_DISTRIBUTIONS_PAYLOAD(
        search_paths, _metadata_module_file()
    )
    if not isinstance(payload, (list, tuple)):
        raise RuntimeError(
            "invalid importlib metadata distributions payload: sequence expected"
        )
    out: list[dict[str, object]] = []
    for entry in payload:
        out.append(_coerce_metadata_payload(entry))
    return out


def _read_text_file(path: str) -> str | None:
    try:
        raw = _MOLT_IMPORTLIB_READ_FILE(path)
    except FileNotFoundError:
        return None
    if not isinstance(raw, bytes):
        raise RuntimeError("invalid importlib read payload: bytes expected")
    return raw.decode("utf-8", errors="surrogateescape")


def _parse_file_hash(value: str | None) -> FileHash | None:
    if not value:
        return None
    if "=" in value:
        mode, digest = value.split("=", 1)
    else:
        mode, digest = "", value
    return FileHash(mode, digest)


def _parse_file_size(value: str | None) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _record_payload(path: str) -> list[tuple[str, str | None, str | None]]:
    payload = _MOLT_IMPORTLIB_METADATA_RECORD_PAYLOAD(path)
    if not isinstance(payload, (list, tuple)):
        raise RuntimeError(
            "invalid importlib metadata record payload: sequence expected"
        )
    out: list[tuple[str, str | None, str | None]] = []
    for entry in payload:
        if (
            not isinstance(entry, (list, tuple))
            or len(entry) != 3
            or not isinstance(entry[0], str)
            or (entry[1] is not None and not isinstance(entry[1], str))
            or (entry[2] is not None and not isinstance(entry[2], str))
        ):
            raise RuntimeError(
                "invalid importlib metadata record payload: triplets expected"
            )
        out.append((entry[0], entry[1], entry[2]))
    return out


def _packages_distributions_payload(
    search_paths: tuple[str, ...],
) -> dict[str, list[str]]:
    payload = _MOLT_IMPORTLIB_METADATA_PACKAGES_DISTRIBUTIONS_PAYLOAD(
        search_paths, _metadata_module_file()
    )
    if not isinstance(payload, dict):
        raise RuntimeError(
            "invalid importlib metadata packages_distributions payload: dict expected"
        )
    out: dict[str, list[str]] = {}
    for package, providers in payload.items():
        if not isinstance(package, str):
            raise RuntimeError(
                "invalid importlib metadata packages_distributions payload: package key"
            )
        if not isinstance(providers, (list, tuple)) or not all(
            isinstance(entry, str) for entry in providers
        ):
            raise RuntimeError(
                "invalid importlib metadata packages_distributions payload: providers"
            )
        deduped: list[str] = []
        for entry in providers:
            if entry not in deduped:
                deduped.append(entry)
        out[package] = deduped
    return out


def _build_cache(search_paths: tuple[str, ...]) -> dict[str, Distribution]:
    _ensure_fs_read()
    cache: dict[str, Distribution] = {}
    for payload in _distributions_payload(search_paths):
        name = str(payload["name"])
        version_val = str(payload["version"])
        metadata_map = payload["metadata"]
        if not isinstance(metadata_map, dict):
            raise RuntimeError("invalid importlib metadata payload: metadata")
        metadata: dict[str, str] = {}
        for key, value in metadata_map.items():
            if not isinstance(key, str) or not isinstance(value, str):
                raise RuntimeError(
                    "invalid importlib metadata payload: metadata values"
                )
            metadata[key] = value
        entry_points_raw = payload["entry_points"]
        if not isinstance(entry_points_raw, list):
            raise RuntimeError("invalid importlib metadata payload: entry_points")
        entry_points_payload: list[tuple[str, str, str]] = []
        for entry in entry_points_raw:
            if (
                not isinstance(entry, tuple)
                or len(entry) != 3
                or not isinstance(entry[0], str)
                or not isinstance(entry[1], str)
                or not isinstance(entry[2], str)
            ):
                raise RuntimeError(
                    "invalid importlib metadata payload: entry_points values"
                )
            entry_points_payload.append((entry[0], entry[1], entry[2]))
        requires_dist_raw = payload["requires_dist"]
        if not isinstance(requires_dist_raw, list) or not all(
            isinstance(entry, str) for entry in requires_dist_raw
        ):
            raise RuntimeError("invalid importlib metadata payload: requires_dist")
        provides_extra_raw = payload["provides_extra"]
        if not isinstance(provides_extra_raw, list) or not all(
            isinstance(entry, str) for entry in provides_extra_raw
        ):
            raise RuntimeError("invalid importlib metadata payload: provides_extra")
        requires_python_raw = payload["requires_python"]
        if requires_python_raw is not None and not isinstance(requires_python_raw, str):
            raise RuntimeError("invalid importlib metadata payload: requires_python")
        multi_values: dict[str, list[str]] = {
            "Requires-Dist": list(requires_dist_raw),
            "Provides-Extra": list(provides_extra_raw),
        }
        if isinstance(requires_python_raw, str):
            multi_values["Requires-Python"] = [requires_python_raw]
        dist = Distribution(
            name,
            version_val,
            str(payload["path"]),
            _Metadata(metadata, multi_values),
            entry_points_payload,
            list(requires_dist_raw),
            list(provides_extra_raw),
            requires_python_raw,
        )
        cache[_normalize(name)] = dist
    return cache


def _ensure_cache() -> dict[str, Distribution]:
    global _DIST_CACHE, _DIST_PATH_SNAPSHOT
    snapshot = _resolved_search_paths(_path_snapshot())
    if _DIST_CACHE is None or snapshot != _DIST_PATH_SNAPSHOT:
        _DIST_PATH_SNAPSHOT = snapshot
        _DIST_CACHE = _build_cache(snapshot)
    return _DIST_CACHE


def distributions() -> list[Distribution]:
    return list(_ensure_cache().values())


def distribution(name: str) -> Distribution:
    dist = _ensure_cache().get(_normalize(name))
    if dist is None:
        raise PackageNotFoundError(name)
    return dist


def version(name: str) -> str:
    return distribution(name).version


def metadata(name: str) -> _Metadata:
    return distribution(name).metadata


def requires(name: str) -> list[str] | None:
    return distribution(name).requires


def files(name: str):
    dist = distribution(name)
    payload = _record_payload(dist._path)
    if not payload:
        return None
    return [
        PackagePath(path, dist, hash_value, size_text)
        for path, hash_value, size_text in payload
    ]


def packages_distributions() -> dict[str, list[str]]:
    _ensure_fs_read()
    snapshot = _resolved_search_paths(_path_snapshot())
    return _packages_distributions_payload(snapshot)


def entry_points(**params) -> EntryPoints:
    _ensure_fs_read()
    group = params.get("group")
    name = params.get("name")
    value = params.get("value")
    use_runtime_filter = (
        set(params).issubset({"group", "name", "value"})
        and (group is None or isinstance(group, str))
        and (name is None or isinstance(name, str))
        and (value is None or isinstance(value, str))
    )
    snapshot = _resolved_search_paths(_path_snapshot())
    payload_group = group if use_runtime_filter else None
    payload_name = name if use_runtime_filter else None
    payload_value = value if use_runtime_filter else None
    items = [
        EntryPoint(name, value, group)
        for name, value, group in _entry_points_payload(
            snapshot, payload_group, payload_name, payload_value
        )
    ]
    entry_points_obj = EntryPoints(items)
    if params and not use_runtime_filter:
        return cast(EntryPoints, entry_points_obj.select(**params))
    return entry_points_obj
