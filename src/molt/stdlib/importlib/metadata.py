"""Minimal importlib.metadata implementation for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full metadata parsing, dependency fields, and entry point resolution semantics.

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable
import os
import sys

from molt import capabilities

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
    return "".join("-" if ch in "._" else ch for ch in name).lower()


def _ensure_fs_read() -> None:
    if not capabilities.trusted():
        capabilities.require("fs.read")


class _Metadata:
    def __init__(self, mapping: dict[str, str]) -> None:
        self._raw = mapping
        self._lower = {k.lower(): v for k, v in mapping.items()}

    def __getitem__(self, key: str) -> str:
        return self._lower[key.lower()]

    def get(self, key: str, default: str | None = None) -> str | None:
        return self._lower.get(key.lower(), default)

    def keys(self):
        return self._raw.keys()


class Distribution:
    def __init__(self, name: str, version: str, path: str, metadata: _Metadata) -> None:
        self._name = name
        self._version = version
        self._path = path
        self._metadata = metadata

    @property
    def metadata(self) -> _Metadata:
        return self._metadata

    @property
    def version(self) -> str:
        return self._version

    def read_text(self, filename: str) -> str | None:
        target = os.path.join(self._path, filename)
        if not os.path.exists(target):
            return None
        with open(target, "r", encoding="utf-8", errors="surrogateescape") as handle:
            return handle.read()


@dataclass(frozen=True)
class EntryPoint:
    name: str
    value: str
    group: str

    def __repr__(self) -> str:
        return f"EntryPoint(name={self.name!r}, value={self.value!r}, group={self.group!r})"


class EntryPoints(list):
    def select(
        self, *, group: str | None = None, name: str | None = None
    ) -> "EntryPoints":
        items = [
            ep
            for ep in self
            if (group is None or ep.group == group)
            and (name is None or ep.name == name)
        ]
        return EntryPoints(items)


_DIST_CACHE: dict[str, Distribution] | None = None
_DIST_PATH_SNAPSHOT: tuple[str, ...] | None = None


def _iter_dist_paths() -> Iterable[str]:
    for base in sys.path:
        if not base:
            continue
        if not os.path.isdir(base):
            continue
        for name in os.listdir(base):
            if name.endswith(".dist-info") or name.endswith(".egg-info"):
                yield os.path.join(base, name)


def _parse_metadata(path: str) -> _Metadata:
    metadata_file = os.path.join(path, "METADATA")
    if not os.path.exists(metadata_file):
        metadata_file = os.path.join(path, "PKG-INFO")
    mapping: dict[str, str] = {}
    if not os.path.exists(metadata_file):
        return _Metadata(mapping)
    with open(metadata_file, "r", encoding="utf-8", errors="surrogateescape") as handle:
        text = handle.read()
    current_key: str | None = None
    for raw_line in text.splitlines():
        if not raw_line:
            current_key = None
            continue
        if raw_line[0] in " \t" and current_key is not None:
            mapping[current_key] = (
                mapping.get(current_key, "") + "\n" + raw_line.strip()
            )
            continue
        if ":" not in raw_line:
            continue
        key, value = raw_line.split(":", 1)
        current_key = key.strip()
        mapping[current_key] = value.strip()
    return _Metadata(mapping)


def _build_cache() -> dict[str, Distribution]:
    _ensure_fs_read()
    cache: dict[str, Distribution] = {}
    for path in _iter_dist_paths():
        metadata = _parse_metadata(path)
        name = metadata.get("Name")
        if not name:
            name = os.path.basename(path).split("-", 1)[0]
        version_val = metadata.get("Version", "")
        dist = Distribution(name, version_val or "", path, metadata)
        cache[_normalize(name)] = dist
    return cache


def _ensure_cache() -> dict[str, Distribution]:
    global _DIST_CACHE, _DIST_PATH_SNAPSHOT
    snapshot = tuple(sys.path)
    if _DIST_CACHE is None or snapshot != _DIST_PATH_SNAPSHOT:
        _DIST_PATH_SNAPSHOT = snapshot
        _DIST_CACHE = _build_cache()
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


def entry_points() -> EntryPoints:
    _ensure_fs_read()
    items: list[EntryPoint] = []
    for dist in distributions():
        text = dist.read_text("entry_points.txt")
        if not text:
            continue
        group: str | None = None
        for line in text.splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            if stripped.startswith("[") and stripped.endswith("]"):
                group = stripped[1:-1].strip()
                continue
            if group is None:
                continue
            if "=" not in stripped:
                continue
            name, value = stripped.split("=", 1)
            items.append(EntryPoint(name.strip(), value.strip(), group))
    return EntryPoints(items)
