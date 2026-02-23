"""Minimal intrinsic-gated `lzma` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- One-shot compress / decompress ---
_MOLT_LZMA_COMPRESS = _require_intrinsic("molt_lzma_compress", globals())
_MOLT_LZMA_DECOMPRESS = _require_intrinsic("molt_lzma_decompress", globals())

# --- Format constants ---
_MOLT_LZMA_FORMAT_XZ = _require_intrinsic("molt_lzma_format_xz", globals())
_MOLT_LZMA_FORMAT_ALONE = _require_intrinsic("molt_lzma_format_alone", globals())
_MOLT_LZMA_FORMAT_RAW = _require_intrinsic("molt_lzma_format_raw", globals())
_MOLT_LZMA_FORMAT_AUTO = _require_intrinsic("molt_lzma_format_auto", globals())

FORMAT_XZ: int = int(_MOLT_LZMA_FORMAT_XZ())
FORMAT_ALONE: int = int(_MOLT_LZMA_FORMAT_ALONE())
FORMAT_RAW: int = int(_MOLT_LZMA_FORMAT_RAW())
FORMAT_AUTO: int = int(_MOLT_LZMA_FORMAT_AUTO())

# --- Check constants ---
_MOLT_LZMA_CHECK_NONE = _require_intrinsic("molt_lzma_check_none", globals())
_MOLT_LZMA_CHECK_CRC32 = _require_intrinsic("molt_lzma_check_crc32", globals())
_MOLT_LZMA_CHECK_CRC64 = _require_intrinsic("molt_lzma_check_crc64", globals())
_MOLT_LZMA_CHECK_SHA256 = _require_intrinsic("molt_lzma_check_sha256", globals())

CHECK_NONE: int = int(_MOLT_LZMA_CHECK_NONE())
CHECK_CRC32: int = int(_MOLT_LZMA_CHECK_CRC32())
CHECK_CRC64: int = int(_MOLT_LZMA_CHECK_CRC64())
CHECK_SHA256: int = int(_MOLT_LZMA_CHECK_SHA256())

# --- Preset constants ---
_MOLT_LZMA_PRESET_DEFAULT = _require_intrinsic("molt_lzma_preset_default", globals())
_MOLT_LZMA_PRESET_EXTREME = _require_intrinsic("molt_lzma_preset_extreme", globals())

PRESET_DEFAULT: int = int(_MOLT_LZMA_PRESET_DEFAULT())
PRESET_EXTREME: int = int(_MOLT_LZMA_PRESET_EXTREME())

# --- Incremental compressor ---
_MOLT_LZMA_COMPRESSOR_NEW = _require_intrinsic("molt_lzma_compressor_new", globals())
_MOLT_LZMA_COMPRESSOR_COMPRESS = _require_intrinsic(
    "molt_lzma_compressor_compress", globals()
)
_MOLT_LZMA_COMPRESSOR_FLUSH = _require_intrinsic(
    "molt_lzma_compressor_flush", globals()
)
_MOLT_LZMA_COMPRESSOR_DROP = _require_intrinsic("molt_lzma_compressor_drop", globals())

# --- Incremental decompressor ---
_MOLT_LZMA_DECOMPRESSOR_NEW = _require_intrinsic(
    "molt_lzma_decompressor_new", globals()
)
_MOLT_LZMA_DECOMPRESSOR_DECOMPRESS = _require_intrinsic(
    "molt_lzma_decompressor_decompress", globals()
)
_MOLT_LZMA_DECOMPRESSOR_EOF = _require_intrinsic(
    "molt_lzma_decompressor_eof", globals()
)
_MOLT_LZMA_DECOMPRESSOR_NEEDS_INPUT = _require_intrinsic(
    "molt_lzma_decompressor_needs_input", globals()
)
_MOLT_LZMA_DECOMPRESSOR_UNUSED_DATA = _require_intrinsic(
    "molt_lzma_decompressor_unused_data", globals()
)
_MOLT_LZMA_DECOMPRESSOR_DROP = _require_intrinsic(
    "molt_lzma_decompressor_drop", globals()
)


class LZMAError(Exception):
    """Exception raised for LZMA-related errors."""

    pass


class LZMACompressor:
    """Incremental LZMA compressor."""

    def __init__(
        self,
        format: int = FORMAT_XZ,
        check: int = -1,
        preset: int | None = None,
        filters=None,
    ) -> None:
        if check == -1:
            check = CHECK_CRC64 if format == FORMAT_XZ else CHECK_NONE
        if preset is None:
            preset = PRESET_DEFAULT
        self._handle = _MOLT_LZMA_COMPRESSOR_NEW(format, check, preset)
        self._flushed = False

    def compress(self, data: bytes) -> bytes:
        if self._flushed:
            raise ValueError("Compressor has been flushed")
        return bytes(_MOLT_LZMA_COMPRESSOR_COMPRESS(self._handle, data))

    def flush(self) -> bytes:
        if self._flushed:
            raise ValueError("Repeated call to flush()")
        self._flushed = True
        return bytes(_MOLT_LZMA_COMPRESSOR_FLUSH(self._handle))

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_LZMA_COMPRESSOR_DROP(handle)
            except Exception:
                pass


class LZMADecompressor:
    """Incremental LZMA decompressor."""

    def __init__(
        self,
        format: int = FORMAT_AUTO,
        memlimit: int | None = None,
        filters=None,
    ) -> None:
        effective_memlimit = memlimit if memlimit is not None else 0
        self._handle = _MOLT_LZMA_DECOMPRESSOR_NEW(format, effective_memlimit)

    def decompress(self, data: bytes, max_length: int = -1) -> bytes:
        if self.eof:
            raise EOFError("Already at end of stream")
        return bytes(_MOLT_LZMA_DECOMPRESSOR_DECOMPRESS(self._handle, data, max_length))

    @property
    def eof(self) -> bool:
        return bool(_MOLT_LZMA_DECOMPRESSOR_EOF(self._handle))

    @property
    def needs_input(self) -> bool:
        return bool(_MOLT_LZMA_DECOMPRESSOR_NEEDS_INPUT(self._handle))

    @property
    def unused_data(self) -> bytes:
        return bytes(_MOLT_LZMA_DECOMPRESSOR_UNUSED_DATA(self._handle))

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_LZMA_DECOMPRESSOR_DROP(handle)
            except Exception:
                pass


def compress(
    data: bytes,
    format: int = FORMAT_XZ,
    check: int = -1,
    preset: int | None = None,
    filters=None,
) -> bytes:
    """Compress *data* in one shot, returning the compressed bytes."""
    if check == -1:
        check = CHECK_CRC64 if format == FORMAT_XZ else CHECK_NONE
    if preset is None:
        preset = PRESET_DEFAULT
    try:
        return bytes(_MOLT_LZMA_COMPRESS(data, format, check, preset))
    except Exception as exc:
        raise LZMAError(str(exc)) from exc


def decompress(
    data: bytes,
    format: int = FORMAT_AUTO,
    memlimit: int | None = None,
    filters=None,
) -> bytes:
    """Decompress *data* in one shot, returning the decompressed bytes."""
    effective_memlimit = memlimit if memlimit is not None else 0
    try:
        return bytes(_MOLT_LZMA_DECOMPRESS(data, format, effective_memlimit))
    except Exception as exc:
        raise LZMAError(str(exc)) from exc


def open(
    filename: str,
    mode: str = "rb",
    *,
    format: int | None = None,
    check: int = -1,
    preset: int | None = None,
    filters=None,
    encoding: str | None = None,
    errors: str | None = None,
    newline: str | None = None,
) -> "_LZMAFile":
    """Open an LZMA-compressed file in binary mode."""
    if format is None:
        format = FORMAT_XZ
    return _LZMAFile(filename, mode, format=format, check=check, preset=preset)


class _LZMAFile:
    """Minimal LZMA file wrapper backed by one-shot compress/decompress."""

    def __init__(
        self,
        filename: str,
        mode: str = "rb",
        *,
        format: int = FORMAT_XZ,
        check: int = -1,
        preset: int | None = None,
    ) -> None:
        if mode not in ("rb", "wb", "ab", "r", "w", "a"):
            raise ValueError(f"Invalid mode: {mode!r}")
        self._name = filename
        self._mode = mode
        self._format = format
        self._check = check
        self._preset = preset
        self._closed = False
        self._write_buf = bytearray()
        self._read_buf: bytes | None = None
        self._read_pos = 0
        self._writing = "w" in mode or "a" in mode
        self._reading = "r" in mode

    def write(self, data: bytes) -> int:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if not self._writing:
            raise OSError("File not open for writing")
        self._write_buf.extend(data)
        return len(data)

    def read(self, size: int = -1) -> bytes:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if not self._reading:
            raise OSError("File not open for reading")
        if self._read_buf is None:
            _open = (
                __builtins__["open"]  # type: ignore[index]
                if isinstance(__builtins__, dict)
                else __builtins__.open  # type: ignore[union-attr]
            )
            with _open(self._name, "rb") as f:
                raw = f.read()
            self._read_buf = decompress(raw, format=self._format)
            self._read_pos = 0
        if size < 0:
            result = self._read_buf[self._read_pos :]
            self._read_pos = len(self._read_buf)
            return result
        result = self._read_buf[self._read_pos : self._read_pos + size]
        self._read_pos += len(result)
        return result

    def close(self) -> None:
        if self._closed:
            return
        if self._writing and self._write_buf:
            compressed = compress(
                bytes(self._write_buf),
                format=self._format,
                check=self._check,
                preset=self._preset,
            )
            _open = (
                __builtins__["open"]  # type: ignore[index]
                if isinstance(__builtins__, dict)
                else __builtins__.open  # type: ignore[union-attr]
            )
            with _open(self._name, "wb") as f:
                f.write(compressed)
        self._closed = True

    def __enter__(self) -> "_LZMAFile":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


__all__ = [
    "compress",
    "decompress",
    "open",
    "LZMACompressor",
    "LZMADecompressor",
    "LZMAError",
    "FORMAT_XZ",
    "FORMAT_ALONE",
    "FORMAT_RAW",
    "FORMAT_AUTO",
    "CHECK_NONE",
    "CHECK_CRC32",
    "CHECK_CRC64",
    "CHECK_SHA256",
    "PRESET_DEFAULT",
    "PRESET_EXTREME",
]
