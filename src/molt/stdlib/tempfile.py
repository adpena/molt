"""Temporary-file policy over secure runtime filesystem primitives."""

from __future__ import annotations

from _thread import allocate_lock as _allocate_lock
import io as _io
import os as _os
import warnings as _warnings
from typing import Any as _Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_REQUIRE_OPERATION = _require_intrinsic("molt_capabilities_require_operation")
_MOLT_TEMPFILE_GETTEMPDIR = _require_intrinsic("molt_tempfile_gettempdir")
_MOLT_TEMPFILE_MKDTEMP = _require_intrinsic("molt_tempfile_mkdtemp")
_MOLT_TEMPFILE_MKSTEMP = _require_intrinsic("molt_tempfile_mkstemp")
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
tempdir: str | bytes | None = None
_once_lock = _allocate_lock()

#: Default prefix for temporary file/directory names.
template: str = "tmp"

#: Maximum number of attempts to find a non-existing name (CPython compat).
TMP_MAX: int = 10000


# ── Core helpers ──────────────────────────────────────────────────────────────


def _candidate_tempdir_list():
    candidates = []
    for name in ("TMPDIR", "TEMP", "TMP"):
        value = _os.getenv(name)
        if value:
            candidates.append(value)
    if _os.name == "nt":
        candidates.extend(
            [
                _os.path.expanduser(r"~\AppData\Local\Temp"),
                _os.path.expandvars(r"%SYSTEMROOT%\Temp"),
                r"c:\temp",
                r"c:\tmp",
                r"\temp",
                r"\tmp",
            ]
        )
    else:
        candidates.extend(["/tmp", "/var/tmp", "/usr/tmp"])
    try:
        candidates.append(_os.getcwd())
    except (AttributeError, OSError):
        candidates.append(_os.curdir)
    return candidates


def _gettempdir():
    global tempdir
    if tempdir is None:
        with _once_lock:
            if tempdir is None:
                _MOLT_REQUIRE_OPERATION("tempfile.gettempdir")
                candidates = _candidate_tempdir_list()
                selected = _MOLT_TEMPFILE_GETTEMPDIR(candidates)
                if selected is None:
                    raise FileNotFoundError(
                        2, "No usable temporary directory found in %s" % candidates
                    )
                tempdir = selected
    return tempdir


def gettempdir() -> str:
    """Return the selected temporary directory as a filesystem string."""
    return _os.fsdecode(_gettempdir())


def gettempdirb() -> bytes:
    """Return the same selected temporary directory as filesystem bytes."""
    return _os.fsencode(_gettempdir())


def gettempprefix() -> str:
    return _os.fsdecode(template)


def gettempprefixb() -> bytes:
    return _os.fsencode(template)


def _sanitize_params(suffix, prefix, dir):
    output_type = None
    for value in (prefix, suffix, dir):
        if value is None:
            continue
        if isinstance(value, _os.PathLike):
            value = _os.fspath(value)
        kind = bytes if isinstance(value, bytes) else str
        if output_type is not None and kind is not output_type:
            raise TypeError("Can't mix bytes and non-bytes in path components.")
        output_type = kind
    if output_type is None:
        output_type = str if tempdir is None or isinstance(tempdir, str) else bytes
    if suffix is None:
        suffix = output_type()
    if prefix is None:
        prefix = template if output_type is str else _os.fsencode(template)
    if dir is None:
        dir = gettempdir() if output_type is str else gettempdirb()
    # Creation makes the path absolute in the target filesystem. Keep bytes
    # intact here rather than sending them through a string-only path projection.
    return suffix, prefix, dir


def mkdtemp(suffix=None, prefix=None, dir=None):
    """Create and return a secure temporary directory."""
    suffix, prefix, dir = _sanitize_params(suffix, prefix, dir)
    return _MOLT_TEMPFILE_MKDTEMP(suffix, prefix, dir)


def mkstemp(suffix=None, prefix=None, dir=None, text=False):
    """Create a secure temporary file and return its descriptor and path."""
    suffix, prefix, dir = _sanitize_params(suffix, prefix, dir)
    return _MOLT_TEMPFILE_MKSTEMP(suffix, prefix, dir, text)


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

    __slots__ = ("file", "name", "delete", "_closed", "delete_on_close")

    def __init__(
        self,
        handle: _Any,
        name: str,
        delete: bool,
        delete_on_close: bool = True,
    ) -> None:
        self.file = handle
        self.name = name
        self.delete = delete
        self.delete_on_close = delete_on_close
        self._closed = False

    def __getattr__(self, name: str) -> _Any:
        return getattr(self.file, name)

    def __enter__(self) -> "_NamedTemporaryFile":
        return self

    def close(self) -> None:
        if self._closed:
            return
        try:
            self.file.close()
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
        return iter(self.file)

    def __next__(self):
        return next(self.file)

    @property
    def mode(self) -> str:
        return getattr(self.file, "mode", "")

    @property
    def closed(self) -> bool:
        return self._closed

    def fileno(self) -> int:
        return self.file.fileno()

    def read(self, *args: _Any, **kwargs: _Any) -> _Any:
        return self.file.read(*args, **kwargs)

    def write(self, *args: _Any, **kwargs: _Any) -> _Any:
        return self.file.write(*args, **kwargs)

    def seek(self, *args: _Any, **kwargs: _Any) -> _Any:
        return self.file.seek(*args, **kwargs)

    def tell(self) -> int:
        return self.file.tell()

    def flush(self) -> None:
        self.file.flush()

    def truncate(self, size: int | None = None) -> int:
        if size is None:
            return self.file.truncate()
        return self.file.truncate(size)


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
    suffix, prefix, dir = _sanitize_params(suffix, prefix, dir)
    path = None

    def opener(_path, _flags):
        nonlocal path
        fd, path = _MOLT_TEMPFILE_MKSTEMP(suffix, prefix, dir, False)
        return fd

    try:
        handle = _io.open(
            dir,
            mode,
            buffering=buffering,
            encoding=encoding,
            errors=errors,
            newline=newline,
            opener=opener,
        )
        try:
            raw = getattr(handle, "buffer", handle)
            raw = getattr(raw, "raw", raw)
            raw.name = path
            return _NamedTemporaryFile(handle, path, delete, delete_on_close)
        except BaseException:
            handle.close()
            raise
    except BaseException:
        if path is not None:
            _os.unlink(path)
        raise


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
        self.name = mkdtemp(suffix=suffix, prefix=prefix, dir=dir)
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
