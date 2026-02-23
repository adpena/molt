"""Intrinsic-backed tarfile module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_tarfile_open = _require_intrinsic("molt_tarfile_open", globals())
_molt_tarfile_close = _require_intrinsic("molt_tarfile_close", globals())
_molt_tarfile_drop = _require_intrinsic("molt_tarfile_drop", globals())
_molt_tarfile_getmembers = _require_intrinsic("molt_tarfile_getmembers", globals())
_molt_tarfile_getnames = _require_intrinsic("molt_tarfile_getnames", globals())
_molt_tarfile_extract = _require_intrinsic("molt_tarfile_extract", globals())
_molt_tarfile_extractall = _require_intrinsic("molt_tarfile_extractall", globals())
_molt_tarfile_extractfile = _require_intrinsic("molt_tarfile_extractfile", globals())
_molt_tarfile_add = _require_intrinsic("molt_tarfile_add", globals())
_molt_tarfile_is_tarfile = _require_intrinsic("molt_tarfile_is_tarfile", globals())


class TarError(Exception):
    """Base exception for tarfile errors."""

    pass


class ReadError(TarError):
    """Exception for unreadable tar archives."""

    pass


class CompressionError(TarError):
    """Exception for unavailable compression methods."""

    pass


class ExtractError(TarError):
    """Exception for extract errors."""

    pass


class HeaderError(TarError):
    """Exception for invalid headers."""

    pass


class TarFile:
    """The TarFile class provides an interface to tar archives."""

    def __init__(self, name: str | None = None, mode: str = "r") -> None:
        self._name = name
        self._mode = mode
        self._handle = None
        if name is not None:
            self._handle = _molt_tarfile_open(str(name), str(mode))

    @classmethod
    def open(cls, name: str | None = None, mode: str = "r", **kwargs) -> "TarFile":
        obj = cls.__new__(cls)
        obj._name = name
        obj._mode = mode
        obj._handle = None
        if name is not None:
            obj._handle = _molt_tarfile_open(str(name), str(mode))
        return obj

    @property
    def name(self) -> str | None:
        return self._name

    def _require_handle(self):
        handle = self._handle
        if handle is None:
            raise RuntimeError("TarFile is closed")
        return handle

    def close(self) -> None:
        if self._handle is not None:
            _molt_tarfile_close(self._handle)
            _molt_tarfile_drop(self._handle)
            self._handle = None

    def __enter__(self) -> "TarFile":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass

    def getmembers(self) -> list:
        return _molt_tarfile_getmembers(self._require_handle())

    def getnames(self) -> list:
        return list(_molt_tarfile_getnames(self._require_handle()))

    def extract(self, member, path: str = ".", **kwargs) -> None:
        _molt_tarfile_extract(self._require_handle(), member, str(path))

    def extractall(self, path: str = ".", **kwargs) -> None:
        _molt_tarfile_extractall(self._require_handle(), str(path))

    def extractfile(self, member):
        return _molt_tarfile_extractfile(self._require_handle(), member)

    def add(self, name: str, arcname: str | None = None, **kwargs) -> None:
        effective_arcname = arcname if arcname is not None else name
        _molt_tarfile_add(self._require_handle(), str(name), str(effective_arcname))


def open(name: str | None = None, mode: str = "r", **kwargs) -> TarFile:
    return TarFile.open(name=name, mode=mode, **kwargs)


def is_tarfile(name: str) -> bool:
    return bool(_molt_tarfile_is_tarfile(str(name)))
