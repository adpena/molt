"""Minimal intrinsic-gated `zipapp` subset for Molt."""

from __future__ import annotations

import os
import zipfile
from collections.abc import Callable

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_ZIPAPP_RUNTIME_READY = _require_intrinsic("molt_zipapp_runtime_ready", globals())

# TODO(stdlib, owner:runtime, milestone:TL3, priority:P2, status:planned):
# Extend `zipapp` coverage to full CPython semantics (interpreter shebangs,
# custom entry-points, and in-memory target handling) via Rust intrinsics.


def create_archive(
    source: str | os.PathLike[str],
    target: str | os.PathLike[str] | None = None,
    interpreter: str | None = None,
    main: str | None = None,
    filter: Callable[[str], bool] | None = None,
    compressed: bool = False,
) -> None:
    del interpreter, main
    src_path = os.fspath(source)
    dst_path = os.fspath(target) if target is not None else src_path + ".pyz"
    compression = zipfile.ZIP_DEFLATED if compressed else zipfile.ZIP_STORED

    def _iter_files(path: str) -> list[str]:
        if not os.path.isdir(path):
            return [path]
        stack = [path]
        out: list[str] = []
        while stack:
            current = stack.pop()
            for child in os.listdir(current):
                absolute = os.path.join(current, child)
                if os.path.isdir(absolute):
                    stack.append(absolute)
                else:
                    out.append(absolute)
        return out

    with zipfile.ZipFile(dst_path, mode="w", compression=compression) as archive:
        if os.path.isdir(src_path):
            for absolute in _iter_files(src_path):
                relative = os.path.relpath(absolute, src_path)
                if filter is not None and not filter(relative):
                    continue
                with open(absolute, "rb") as handle:
                    archive.writestr(relative, handle.read())
            return None
        if filter is not None and not filter(os.path.basename(src_path)):
            return None
        with open(src_path, "rb") as handle:
            archive.writestr(os.path.basename(src_path), handle.read())
    return None


def is_archive(path: str | os.PathLike[str]) -> bool:
    return bool(zipfile.is_zipfile(os.fspath(path)))


__all__ = ["create_archive", "is_archive"]
