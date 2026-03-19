"""Intrinsic-backed glob for Molt -- all operations delegated to Rust."""

from __future__ import annotations

from typing import Any
import sys
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["glob", "iglob", "escape"]
if sys.version_info >= (3, 13):
    __all__.append("translate")


_MOLT_GLOB_HAS_MAGIC = _require_intrinsic("molt_glob_has_magic")
_MOLT_GLOB_ESCAPE = _require_intrinsic("molt_glob_escape")
_MOLT_GLOB_GLOB = _require_intrinsic("molt_glob_glob")
_MOLT_GLOB_IGLOB = _require_intrinsic("molt_glob_iglob")
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir")
if sys.version_info >= (3, 13):
    _MOLT_GLOB_TRANSLATE = _require_intrinsic("molt_glob_translate")

_DEPRECATED_FUNCTION_MESSAGE = (
    "{name} is deprecated and will be removed in Python {remove}. Use "
    "glob.glob and pass a directory to its root_dir argument instead."
)


def has_magic(pathname: str) -> bool:
    """Return True if the pathname contains glob magic characters."""
    return bool(_MOLT_GLOB_HAS_MAGIC(pathname))


def glob(
    pathname: Any,
    *,
    root_dir: Any | None = None,
    dir_fd: Any | None = None,
    recursive: Any = False,
    include_hidden: Any = False,
) -> list[str] | list[bytes]:
    """Return a list of paths matching a pathname pattern (via Rust intrinsic)."""
    matches = _MOLT_GLOB_GLOB(pathname, root_dir, recursive)
    if not isinstance(matches, list):
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
    """Return an iterator yielding paths matching a pathname pattern (via Rust intrinsic)."""
    matches = _MOLT_GLOB_IGLOB(pathname, root_dir, recursive)
    if not isinstance(matches, list):
        raise RuntimeError("iglob intrinsic returned invalid value")
    yield from matches


def escape(pathname: Any) -> str | bytes:
    """Escape all special characters in pathname (via Rust intrinsic)."""
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
        """Translate a pathname with shell wildcards to a regular expression."""
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
    """Deprecated: use glob.glob() with root_dir instead."""
    _warn_deprecated("glob.glob0")
    if pattern:
        return glob(escape(pattern), root_dir=dirname)
    is_dir = bool(_MOLT_PATH_ISDIR(dirname))
    return [pattern] if is_dir else []


def glob1(dirname: Any, pattern: Any):
    """Deprecated: use glob.glob() with root_dir instead."""
    _warn_deprecated("glob.glob1")
    return glob(pattern, root_dir=dirname)
