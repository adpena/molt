"""Utilities to support importers and packaging tools.

This is a minimal, deterministic subset focused on filesystem-based discovery.
"""

from __future__ import annotations

from typing import Iterable, Iterator
import os
import sys

__all__ = ["ModuleInfo", "iter_modules", "walk_packages"]


class ModuleInfo:
    __slots__ = ("module_finder", "name", "ispkg")

    def __init__(self, module_finder: object, name: str, ispkg: bool) -> None:
        self.module_finder = module_finder
        self.name = name
        self.ispkg = ispkg

    def __iter__(self):
        yield self.module_finder
        yield self.name
        yield self.ispkg

    def __repr__(self) -> str:
        return "ModuleInfo(module_finder={!r}, name={!r}, ispkg={!r})".format(
            self.module_finder, self.name, self.ispkg
        )


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement pkgutil loader APIs, zipimport parity, and full walk_packages semantics.


def iter_modules(
    path: Iterable[str] | None = None, prefix: str = ""
) -> Iterator[ModuleInfo]:
    """Yield ModuleInfo for all submodules on path.

    If path is None, iterate over sys.path. Only filesystem paths are supported.
    """
    if path is None:
        path_iter = sys.path
    else:
        path_iter = path

    yielded: set[str] = set()
    for entry in path_iter:
        for info in _iter_modules_in_path(entry, prefix):
            if info.name in yielded:
                continue
            yielded.add(info.name)
            yield info


def walk_packages(
    path: Iterable[str] | None = None,
    prefix: str = "",
    onerror=None,
) -> Iterator[ModuleInfo]:
    """Yield ModuleInfo for modules and packages recursively."""
    for info in iter_modules(path, prefix):
        yield info
        if not info.ispkg:
            continue
        base = info.module_finder
        pkg_name = info.name
        if prefix and pkg_name.startswith(prefix):
            pkg_name = pkg_name[len(prefix) :]
        subdir = _path_join(base, pkg_name)
        try:
            yield from walk_packages([subdir], info.name + ".", onerror)
        except OSError:
            if onerror:
                onerror(subdir)


def _iter_modules_in_path(path: str, prefix: str) -> list[ModuleInfo]:
    try:
        entries = os.listdir(path)
    except OSError:
        return []

    entries.sort()
    yielded: set[str] = set()
    results: list[ModuleInfo] = []

    for entry in entries:
        if entry == "__pycache__":
            continue
        full = _path_join(path, entry)
        if "." not in entry:
            try:
                dir_entries = os.listdir(full)
            except OSError:
                dir_entries = None
            if dir_entries is not None:
                if "__init__.py" in dir_entries:
                    if entry in yielded:
                        continue
                    yielded.add(entry)
                    results.append(ModuleInfo(path, prefix + entry, True))
                continue

        if not entry.endswith(".py"):
            continue
        modname = entry[:-3]
        if not modname or modname == "__init__" or "." in modname:
            continue
        if modname in yielded:
            continue
        yielded.add(modname)
        results.append(ModuleInfo(path, prefix + modname, False))

    return results


def _path_join(base: str, name: str) -> str:
    if not base:
        return name
    sep = os.sep
    if base.endswith(sep):
        return base + name
    return base + sep + name
