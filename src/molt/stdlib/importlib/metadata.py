"""Minimal importlib.metadata implementation for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full metadata version semantics and remaining entry point selection edge cases.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from dataclasses import dataclass
from typing import Iterable
import os
import sys
from types import ModuleType

_capabilities: ModuleType | None
try:
    from molt import capabilities as _capabilities_raw
except Exception:
    _capabilities = None
else:
    _capabilities = (
        _capabilities_raw if isinstance(_capabilities_raw, ModuleType) else None
    )

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_READ_FILE = _require_intrinsic("molt_importlib_read_file", globals())
_MOLT_IMPORTLIB_METADATA_DIST_PATHS = _require_intrinsic(
    "molt_importlib_metadata_dist_paths", globals()
)
_MOLT_IMPORTLIB_BOOTSTRAP_PAYLOAD = _require_intrinsic(
    "molt_importlib_bootstrap_payload", globals()
)
_MOLT_IMPORTLIB_METADATA_ENTRY_POINTS_SELECT_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_entry_points_select_payload", globals()
)
_MOLT_IMPORTLIB_METADATA_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_payload", globals()
)
_MOLT_IMPORTLIB_METADATA_NORMALIZE_NAME = _require_intrinsic(
    "molt_importlib_metadata_normalize_name", globals()
)

__all__ = [
    "PackageNotFoundError",
    "distribution",
    "distributions",
    "version",
    "entry_points",
    "EntryPoint",
    "EntryPoints",
]


class PackageNotFoundError(ModuleNotFoundError):
    pass


def _normalize(name: str) -> str:
    normalized = _MOLT_IMPORTLIB_METADATA_NORMALIZE_NAME(name)
    if not isinstance(normalized, str):
        raise RuntimeError("invalid importlib metadata normalize payload: str expected")
    return normalized


def _ensure_fs_read() -> None:
    if _capabilities is not None and not _capabilities.trusted():
        _capabilities.require("fs.read")


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


@dataclass(frozen=True)
class EntryPoint:
    name: str
    value: str
    group: str

    def __repr__(self) -> str:
        return f"EntryPoint(name={self.name!r}, value={self.value!r}, group={self.group!r})"


class EntryPoints:
    def __init__(self, entries: Iterable[EntryPoint] = ()) -> None:
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


_DIST_CACHE: dict[str, Distribution] | None = None
_DIST_PATH_SNAPSHOT: tuple[str, ...] | None = None


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


def _iter_dist_paths(search_paths: tuple[str, ...]) -> Iterable[str]:
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
    search_paths: tuple[str, ...], group: str | None = None, name: str | None = None
) -> list[tuple[str, str, str]]:
    payload = _MOLT_IMPORTLIB_METADATA_ENTRY_POINTS_SELECT_PAYLOAD(
        search_paths, _metadata_module_file(), group, name
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


def _metadata_payload(path: str) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_METADATA_PAYLOAD(path)
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


def _read_text_file(path: str) -> str | None:
    try:
        raw = _MOLT_IMPORTLIB_READ_FILE(path)
    except FileNotFoundError:
        return None
    if not isinstance(raw, bytes):
        raise RuntimeError("invalid importlib read payload: bytes expected")
    return raw.decode("utf-8", errors="surrogateescape")


def _build_cache(search_paths: tuple[str, ...]) -> dict[str, Distribution]:
    _ensure_fs_read()
    cache: dict[str, Distribution] = {}
    for path in _iter_dist_paths(search_paths):
        payload = _metadata_payload(path)
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


def entry_points(**params) -> EntryPoints:
    _ensure_fs_read()
    group = params.get("group")
    name = params.get("name")
    use_runtime_filter = (
        set(params).issubset({"group", "name"})
        and (group is None or isinstance(group, str))
        and (name is None or isinstance(name, str))
    )
    snapshot = _resolved_search_paths(_path_snapshot())
    payload_group = group if use_runtime_filter else None
    payload_name = name if use_runtime_filter else None
    items = [
        EntryPoint(name, value, group)
        for name, value, group in _entry_points_payload(
            snapshot, payload_group, payload_name
        )
    ]
    entry_points_obj = EntryPoints(items)
    if params:
        return entry_points_obj.select(**params)
    return entry_points_obj
