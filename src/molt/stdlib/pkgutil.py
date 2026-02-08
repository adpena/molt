"""Intrinsic-backed pkgutil subset for Molt."""

from __future__ import annotations

from typing import Iterable, Iterator
import sys

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["ModuleInfo", "iter_modules", "walk_packages"]


_MOLT_PKGUTIL_ITER_MODULES = _require_intrinsic("molt_pkgutil_iter_modules", globals())
_MOLT_PKGUTIL_WALK_PACKAGES = _require_intrinsic(
    "molt_pkgutil_walk_packages", globals()
)


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


def _rows_to_module_info(rows) -> list[ModuleInfo]:
    if not isinstance(rows, list):
        raise RuntimeError("pkgutil intrinsic returned invalid value")
    out: list[ModuleInfo] = []
    for row in rows:
        if (
            not isinstance(row, (list, tuple))
            or len(row) != 3
            or not isinstance(row[0], str)
            or not isinstance(row[1], str)
            or not isinstance(row[2], bool)
        ):
            raise RuntimeError("pkgutil intrinsic returned invalid value")
        out.append(ModuleInfo(row[0], row[1], row[2]))
    return out


def iter_modules(
    path: Iterable[str] | None = None, prefix: str = ""
) -> Iterator[ModuleInfo]:
    source = sys.path if path is None else path
    rows = _MOLT_PKGUTIL_ITER_MODULES(source, prefix)
    yield from _rows_to_module_info(rows)


def walk_packages(
    path: Iterable[str] | None = None,
    prefix: str = "",
    onerror=None,
) -> Iterator[ModuleInfo]:
    del onerror
    source = sys.path if path is None else path
    rows = _MOLT_PKGUTIL_WALK_PACKAGES(source, prefix)
    yield from _rows_to_module_info(rows)
