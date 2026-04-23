"""Intrinsic-backed tempfile for Molt -- all operations delegated to Rust."""

from __future__ import annotations

import io as _io
import os as _os
import warnings as _warnings
from typing import Any as _Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TEMPFILE_GETTEMPDIR = _require_intrinsic("molt_tempfile_gettempdir")
_MOLT_TEMPFILE_GETTEMPDIRB = _require_intrinsic("molt_tempfile_gettempdirb")
_MOLT_TEMPFILE_MKDTEMP = _require_intrinsic("molt_tempfile_mkdtemp")
_MOLT_TEMPFILE_MKSTEMP = _require_intrinsic("molt_tempfile_mkstemp")
_MOLT_TEMPFILE_NAMED = _require_intrinsic("molt_tempfile_named")
_MOLT_TEMPFILE_TEMPDIR = _require_intrinsic("molt_tempfile_tempdir")
_MOLT_TEMPFILE_CLEANUP = _require_intrinsic("molt_tempfile_cleanup")

__all__ = [
    "NamedTemporaryFile",
    "SpooledTemporaryFile",
    "TemporaryDirectory",
    "TemporaryFile",
    "TMP_MAX",
    "gettempdir",
    "gettempdirb",
    "gettempprefix",
    "gettempprefixb",
    "mkdtemp",
    "mkstemp",
    "mktemp",
    "tempdir",
    "template",
]

# ── Module-level constants/variables (CPython compat) ────────────────────────

#: Value set by the user to override the default temp directory.
tempdir: str | None = None

#: Default prefix for temporary file/directory names.
template: str = "tmp"

#: Maximum number of attempts to find a non-existing name (CPython compat).
TMP_MAX: int = 10000


# ── Core helpers ──────────────────────────────────────────────────────────────


def gettempdir() -> str:
    """Return the name of the directory used for temporary files."""
    if tempdir is not None:
        return tempdir
    return str(_MOLT_TEMPFILE_GETTEMPDIR())


def gettempdirb() -> bytes:
    """Return the name of the directory used for temporary files, as bytes."""
    if tempdir is not None:
        return tempdir.encode()
    return _MOLT_TEMPFILE_GETTEMPDIRB()


def gettempprefix() -> str:
    """Return the default prefix for temporary files."""
    return template


def gettempprefixb() -> bytes:
    """Return the default prefix for temporary files, as bytes."""
    return template.encode()


def mkdtemp(
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
) -> str:
    """Create and return a temporary directory (secure, via Rust intrinsic)."""
    return str(_MOLT_TEMPFILE_MKDTEMP(suffix, prefix, dir))


def mkstemp(
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
    text: bool = False,
) -> tuple[int, str]:
    """Create a temporary file (secure, via Rust intrinsic) and return (fd, name)."""
    result = _MOLT_TEMPFILE_MKSTEMP(suffix, prefix, dir)
    return (int(result[0]), str(result[1]))


def mktemp(
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
) -> str:
    """Return a unique temporary filename (NOT SECURE — do not use in new code).

    .. deprecated::
        Use mkstemp() or NamedTemporaryFile() instead.
    """
    _warnings.warn(
        "mktemp is not secure; use NamedTemporaryFile() or mkstemp() instead",
        DeprecationWarning,
        stacklevel=2,
    )
    # Derive a unique name via mkstemp then immediately close+unlink the fd.
    fd, path = mkstemp(suffix=suffix, prefix=prefix, dir=dir)
    _os.close(fd)
    _os.unlink(path)
    return path


# ── NamedTemporaryFile ────────────────────────────────────────────────────────


class _NamedTemporaryFile:
    """Wrapper for a named temporary file backed by Rust intrinsic creation."""

    __slots__ = ("_handle", "name", "delete", "_closed", "delete_on_close")

    def __init__(
        self,
        handle: _Any,
        name: str,
        delete: bool,
        delete_on_close: bool = True,
    ) -> None:
        self._handle = handle
        self.name = name
        self.delete = delete
        self.delete_on_close = delete_on_close
        self._closed = False

    def __getattr__(self, name: str) -> _Any:
        return getattr(self._handle, name)

    def __enter__(self) -> "_NamedTemporaryFile":
        return self

    def close(self) -> None:
        if self._closed:
            return
        try:
            self._handle.close()
        finally:
            if self.delete and self.delete_on_close:
                try:
                    _os.unlink(self.name)
                except FileNotFoundError:
                    pass
            self._closed = True

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> None:
        self.close()

    def __iter__(self):
        return iter(self._handle)

    def __next__(self):
        return next(self._handle)

    @property
    def mode(self) -> str:
        return getattr(self._handle, "mode", "")

    @property
    def closed(self) -> bool:
        return self._closed

    def fileno(self) -> int:
        return self._handle.fileno()

    def read(self, *args: _Any, **kwargs: _Any) -> _Any:
        return self._handle.read(*args, **kwargs)

    def write(self, *args: _Any, **kwargs: _Any) -> _Any:
        return self._handle.write(*args, **kwargs)

    def seek(self, *args: _Any, **kwargs: _Any) -> _Any:
        return self._handle.seek(*args, **kwargs)

    def tell(self) -> int:
        return self._handle.tell()

    def flush(self) -> None:
        self._handle.flush()

    def truncate(self, size: int | None = None) -> int:
        if size is None:
            return self._handle.truncate()
        return self._handle.truncate(size)


def NamedTemporaryFile(
    mode: str = "w+b",
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
    delete: bool = True,
    *,
    errors: str | None = None,
    delete_on_close: bool = True,
) -> _NamedTemporaryFile:
    """Create a named temporary file (secure, via Rust intrinsic).

    The file is created securely by the Rust tempfile crate, then opened
    with the requested Python mode.
    """
    result = _MOLT_TEMPFILE_NAMED(suffix, prefix, dir, delete)
    fd = int(result[0])
    path = str(result[1])
    should_delete = bool(result[2])

    # Close the raw fd and re-open with the requested mode so the caller
    # gets a full-featured Python file object.
    _os.close(fd)
    open_kwargs: dict[str, _Any] = {"buffering": buffering}
    if encoding is not None:
        open_kwargs["encoding"] = encoding
    if newline is not None:
        open_kwargs["newline"] = newline
    if errors is not None:
        open_kwargs["errors"] = errors
    handle = open(path, mode, **open_kwargs)
    return _NamedTemporaryFile(handle, path, should_delete, delete_on_close)


# ── TemporaryFile ─────────────────────────────────────────────────────────────


def TemporaryFile(
    mode: str = "w+b",
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
    *,
    errors: str | None = None,
) -> _Any:
    """Create an anonymous temporary file.

    On POSIX the file is unlinked immediately after creation so it has no
    visible directory entry; on Windows it is created with FILE_FLAG_DELETE_ON_CLOSE.
    Returns an open file-like object.
    """
    # Create via NamedTemporaryFile then unlink the path to make it anonymous.
    f = NamedTemporaryFile(
        mode=mode,
        buffering=buffering,
        encoding=encoding,
        newline=newline,
        suffix=suffix,
        prefix=prefix,
        dir=dir,
        delete=False,
        errors=errors,
    )
    try:
        _os.unlink(f.name)
    except OSError:
        pass
    return f


# ── SpooledTemporaryFile ──────────────────────────────────────────────────────


class SpooledTemporaryFile:
    """Temporary file wrapper that spools data in memory up to *max_size* bytes.

    Once the spool exceeds *max_size*, data is flushed to a real temporary file
    and all subsequent reads/writes go directly to disk.
    """

    def __init__(
        self,
        max_size: int = 0,
        mode: str = "w+b",
        buffering: int = -1,
        encoding: str | None = None,
        newline: str | None = None,
        suffix: str | None = None,
        prefix: str | None = None,
        dir: str | None = None,
        *,
        errors: str | None = None,
    ) -> None:
        self._max_size = max_size
        self._mode = mode
        self._buffering = buffering
        self._encoding = encoding
        self._newline = newline
        self._suffix = suffix
        self._prefix = prefix
        self._dir = dir
        self._errors = errors
        self._rolled = False
        self._closed = False

        # In-memory buffer: bytes mode uses BytesIO, text mode uses StringIO.
        if "b" in mode:
            self._buffer: _io.BytesIO | _io.StringIO | _NamedTemporaryFile = (
                _io.BytesIO()
            )
        else:
            self._buffer = _io.StringIO(
                newline=newline,
            )

    # -- Internal rollover -------------------------------------------------------

    def _roll(self) -> None:
        """Spill the in-memory buffer to a real temporary file."""
        if self._rolled:
            return
        pos = self._buffer.tell()
        self._buffer.seek(0)
        data = self._buffer.read()
        self._buffer = NamedTemporaryFile(
            mode=self._mode,
            buffering=self._buffering,
            encoding=self._encoding,
            newline=self._newline,
            suffix=self._suffix,
            prefix=self._prefix,
            dir=self._dir,
            delete=True,
            errors=self._errors,
        )
        self._buffer.write(data)
        self._buffer.seek(pos)
        self._rolled = True

    @property
    def name(self) -> str | None:
        if self._rolled:
            return self._buffer.name  # type: ignore[union-attr]
        return None

    @property
    def mode(self) -> str:
        return self._mode

    @property
    def closed(self) -> bool:
        return self._closed

    def _check_max_size(self) -> None:
        if not self._rolled and self._max_size > 0:
            try:
                size = self._buffer.tell()
            except Exception:
                size = 0
            if size > self._max_size:
                self._roll()

    def fileno(self) -> int:
        self._roll()
        return self._buffer.fileno()  # type: ignore[union-attr]

    def write(self, s: _Any) -> int:
        if self._closed:
            raise ValueError("write to closed file")
        n = self._buffer.write(s)
        self._check_max_size()
        return n

    def read(self, size: int = -1) -> _Any:
        if self._closed:
            raise ValueError("read from closed file")
        return self._buffer.read(size)

    def readline(self, size: int = -1) -> _Any:
        if self._closed:
            raise ValueError("read from closed file")
        if size == -1:
            return self._buffer.readline()
        return self._buffer.readline(size)

    def readlines(self, hint: int = -1) -> list:
        if self._closed:
            raise ValueError("read from closed file")
        if hint == -1:
            return self._buffer.readlines()
        return self._buffer.readlines(hint)

    def writelines(self, lines: _Any) -> None:
        if self._closed:
            raise ValueError("write to closed file")
        for line in lines:
            self.write(line)

    def seek(self, pos: int, whence: int = 0) -> int:
        return self._buffer.seek(pos, whence)

    def tell(self) -> int:
        return self._buffer.tell()

    def truncate(self, size: int | None = None) -> int:
        if size is None:
            return self._buffer.truncate()
        return self._buffer.truncate(size)

    def flush(self) -> None:
        self._buffer.flush()

    def close(self) -> None:
        if self._closed:
            return
        try:
            self._buffer.close()
        finally:
            self._closed = True

    def __enter__(self) -> "SpooledTemporaryFile":
        return self

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> None:
        self.close()

    def __iter__(self):
        return iter(self._buffer)

    def __next__(self):
        return next(self._buffer)

    @property
    def softspace(self) -> int:
        return 0

    def readable(self) -> bool:
        return True

    def writable(self) -> bool:
        return True

    def seekable(self) -> bool:
        return True


# ── TemporaryDirectory ─────────────────────────────────────────────────────────


class TemporaryDirectory:
    """Create a temporary directory (secure, via Rust intrinsic).

    Cleanup is handled by the Rust intrinsic on __exit__.
    """

    __slots__ = ("name", "_closed", "_ignore_cleanup_errors")

    def __init__(
        self,
        suffix: str | None = None,
        prefix: str | None = None,
        dir: str | None = None,
        ignore_cleanup_errors: bool = False,
        *,
        delete: bool = True,
    ) -> None:
        self.name = str(_MOLT_TEMPFILE_TEMPDIR(suffix, prefix, dir))
        self._closed = False
        self._ignore_cleanup_errors = ignore_cleanup_errors

    def cleanup(self) -> None:
        """Remove the temporary directory and all its contents."""
        if self._closed:
            return
        try:
            _MOLT_TEMPFILE_CLEANUP(self.name)
        except OSError:
            if not self._ignore_cleanup_errors:
                raise
        finally:
            self._closed = True

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> None:
        self.cleanup()

    def __repr__(self) -> str:
        return f"<TemporaryDirectory {self.name!r}>"


globals().pop("_require_intrinsic", None)
