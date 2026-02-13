"""Intrinsic-backed glob support for Molt."""

from __future__ import annotations

from typing import Any
import sys
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["glob", "iglob", "escape"]
if sys.version_info >= (3, 13):
    __all__.append("translate")


_MOLT_GLOB_HAS_MAGIC = _require_intrinsic("molt_glob_has_magic", globals())
_MOLT_GLOB_ESCAPE = _require_intrinsic("molt_glob_escape", globals())
_MOLT_GLOB = _require_intrinsic("molt_glob", globals())
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir", globals())
if sys.version_info >= (3, 13):
    _MOLT_GLOB_TRANSLATE = _require_intrinsic("molt_glob_translate", globals())

_DEPRECATED_FUNCTION_MESSAGE = (
    "{name} is deprecated and will be removed in Python {remove}. Use "
    "glob.glob and pass a directory to its root_dir argument instead."
)


def has_magic(pathname: str) -> bool:
    return bool(_MOLT_GLOB_HAS_MAGIC(pathname))


def glob(
    pathname: Any,
    *,
    root_dir: Any | None = None,
    dir_fd: Any | None = None,
    recursive: Any = False,
    include_hidden: Any = False,
) -> list[str] | list[bytes]:
    matches = _MOLT_GLOB(pathname, root_dir, dir_fd, recursive, include_hidden)
    if not isinstance(matches, list):
        raise RuntimeError("glob intrinsic returned invalid value")
    for match in matches:
        if not isinstance(match, (str, bytes)):
            raise RuntimeError("glob intrinsic returned invalid value")
    return matches


def iglob(
    pathname: Any,
    *,
    root_dir: Any | None = None,
    dir_fd: Any | None = None,
    recursive: Any = False,
    include_hidden: Any = False,
):
    yield from glob(
        pathname,
        root_dir=root_dir,
        dir_fd=dir_fd,
        recursive=recursive,
        include_hidden=include_hidden,
    )


def escape(pathname: Any) -> str | bytes:
    out = _MOLT_GLOB_ESCAPE(pathname)
    if not isinstance(out, (str, bytes)):
        raise RuntimeError("glob escape intrinsic returned invalid value")
    return out


if sys.version_info >= (3, 13):

    def translate(
        pathname: Any,
        *,
        recursive: Any = False,
        include_hidden: Any = False,
        seps: Any | None = None,
    ) -> str:
        out = _MOLT_GLOB_TRANSLATE(pathname, recursive, include_hidden, seps)
        if not isinstance(out, str):
            raise RuntimeError("glob translate intrinsic returned invalid value")
        return out


def _warn_deprecated(name: str, remove: tuple[int, int] = (3, 15)) -> None:
    warnings.warn(
        _DEPRECATED_FUNCTION_MESSAGE.format(
            name=name,
            remove=f"{remove[0]}.{remove[1]}",
        ),
        DeprecationWarning,
        stacklevel=2,
    )


def glob0(dirname: Any, pattern: Any):
    _warn_deprecated("glob.glob0")
    if pattern:
        return glob(escape(pattern), root_dir=dirname)
    is_dir = bool(_MOLT_PATH_ISDIR(dirname))
    return [pattern] if is_dir else []


def glob1(dirname: Any, pattern: Any):
    _warn_deprecated("glob.glob1")
    return glob(pattern, root_dir=dirname)
