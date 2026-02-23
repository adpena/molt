"""Minimal intrinsic-gated `gzip` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_GZIP_COMPRESS = _require_intrinsic("molt_gzip_compress", globals())
_MOLT_GZIP_DECOMPRESS = _require_intrinsic("molt_gzip_decompress", globals())
_MOLT_GZIP_OPEN = _require_intrinsic("molt_gzip_open", globals())
_MOLT_GZIP_READ = _require_intrinsic("molt_gzip_read", globals())
_MOLT_GZIP_WRITE = _require_intrinsic("molt_gzip_write", globals())
_MOLT_GZIP_CLOSE = _require_intrinsic("molt_gzip_close", globals())
_MOLT_GZIP_DROP = _require_intrinsic("molt_gzip_drop", globals())


class BadGzipFile(OSError):
    """Exception raised for invalid gzip files."""

    pass


class GzipFile:
    """Minimal gzip file object backed by intrinsics."""

    def __init__(
        self,
        filename: str | None = None,
        mode: str | None = None,
        compresslevel: int = 9,
        fileobj=None,
        mtime: float | None = None,
    ) -> None:
        if fileobj is not None and filename is None:
            # When using fileobj, operate in one-shot mode via compress/decompress
            self._handle = None
            self._fileobj = fileobj
            self._mode = mode or "rb"
            self._compresslevel = compresslevel
            self._mtime = mtime
            self._closed = False
            self._write_buf = bytearray()
            return

        if mode is None:
            mode = "rb"
        if filename is None:
            raise TypeError("filename is required when fileobj is not provided")

        self._handle = _MOLT_GZIP_OPEN(filename, mode, compresslevel)
        self._fileobj = None
        self._mode = mode
        self._compresslevel = compresslevel
        self._mtime = mtime
        self._closed = False
        self._write_buf = bytearray()

    def read(self, size: int = -1) -> bytes:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if self._fileobj is not None:
            # fileobj mode: decompress in one shot
            raw = self._fileobj.read()
            return decompress(raw)
        return bytes(_MOLT_GZIP_READ(self._handle, size))

    def write(self, data: bytes) -> int:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if self._fileobj is not None:
            self._write_buf.extend(data)
            return len(data)
        return int(_MOLT_GZIP_WRITE(self._handle, data))

    def close(self) -> None:
        if self._closed:
            return
        if self._fileobj is not None and self._write_buf:
            mtime = self._mtime if self._mtime is not None else 0
            compressed = compress(
                bytes(self._write_buf), self._compresslevel, mtime=mtime
            )
            self._fileobj.write(compressed)
        if self._handle is not None:
            _MOLT_GZIP_CLOSE(self._handle)
            _MOLT_GZIP_DROP(self._handle)
            self._handle = None
        self._closed = True

    def __enter__(self) -> "GzipFile":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


def compress(
    data: bytes, compresslevel: int = 9, *, mtime: float | None = None
) -> bytes:
    """Compress *data* in one shot, returning the gzip-compressed bytes."""
    effective_mtime = mtime if mtime is not None else 0
    try:
        return bytes(_MOLT_GZIP_COMPRESS(data, compresslevel, effective_mtime))
    except Exception as exc:
        raise BadGzipFile(str(exc)) from exc


def decompress(data: bytes) -> bytes:
    """Decompress *data* in one shot, returning the decompressed bytes."""
    try:
        return bytes(_MOLT_GZIP_DECOMPRESS(data))
    except Exception as exc:
        raise BadGzipFile(str(exc)) from exc


def open(
    filename: str,
    mode: str = "rb",
    compresslevel: int = 9,
    encoding: str | None = None,
    errors: str | None = None,
    newline: str | None = None,
) -> GzipFile:
    """Open a gzip-compressed file in binary mode."""
    return GzipFile(filename=filename, mode=mode, compresslevel=compresslevel)


__all__ = [
    "compress",
    "decompress",
    "open",
    "GzipFile",
    "BadGzipFile",
]
