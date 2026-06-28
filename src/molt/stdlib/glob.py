"""Intrinsic-backed glob for Molt -- all operations delegated to Rust."""

from __future__ import annotations

import sys
import warnings

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["glob", "iglob", "escape"]
if sys.version_info >= (3, 13):
    __all__.append("translate")


_MOLT_GLOB_HAS_MAGIC = _require_intrinsic("molt_glob_has_magic")
_MOLT_GLOB_ESCAPE = _require_intrinsic("molt_glob_escape")
_MOLT_GLOB = _require_intrinsic("molt_glob")
_MOLT_GLOB_ITER = _require_intrinsic("molt_glob_iter")
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir")
if sys.version_info >= (3, 13):
    _MOLT_GLOB_TRANSLATE = _require_intrinsic("molt_glob_translate")

# Module-level regex objects that CPython exposes for third-party magic-char
# detection. molt's glob does NOT use them internally (magic detection is the
# `molt_glob_has_magic` intrinsic), so they are pure API-compat surface.
#
# `magic_check` (str) and `magic_check_bytes` (bytes) are resolved lazily via
# PEP 562 `__getattr__`: molt's `re`
# engine does not yet support bytes patterns, and eagerly compiling it here
# would crash glob at import (the str-only `re.compile` raises TypeError on a
# bytes pattern). Deferring it keeps the whole module importable and every glob
# operation working — including byte-path globbing, which flows through the
# Rust `molt_glob`/`molt_glob_iter` intrinsics (full bytes support), NOT through
# this regex. Accessing `glob.magic_check_bytes` surfaces molt's real
# re-bytes-pattern limitation at the point of use rather than masking it.
# Keep both lazy so importing `glob` does not make `re` part of module-init reach.
def __getattr__(name: str):
    if name == "magic_check":
        import re

        value = re.compile(r"([*?[])")
        globals()["magic_check"] = value
        return value
    if name == "magic_check_bytes":
        import re

        value = re.compile(rb"([*?[])")
        globals()["magic_check_bytes"] = value
        return value
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


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
    """Return a list of paths matching a pathname pattern.

    Equivalent to ``list(iglob(...))`` — the eager intrinsic drains the same
    CPython-faithful matcher the lazy ``iglob`` streams, so the two agree.
    """
    matches = _MOLT_GLOB(pathname, root_dir, dir_fd, recursive, include_hidden)
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
    """Return an iterator which yields the paths matching a pathname pattern.

    The returned object is a native lazy iterator (CPython's ``glob.iglob`` is
    likewise a generator chain over ``os.scandir``): paths are produced on
    demand, so large or deep trees stream at bounded memory instead of being
    fully materialized. Like CPython, ``iglob`` itself returns the iterator
    (it is not a generator function).
    """
    return _MOLT_GLOB_ITER(pathname, root_dir, dir_fd, recursive, include_hidden)


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


globals().pop("_require_intrinsic", None)
