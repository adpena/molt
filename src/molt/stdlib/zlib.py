"""zlib compression for Molt — all computation delegated to Rust intrinsics."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- runtime gate ---
_MOLT_ZLIB_RUNTIME_READY = _require_intrinsic("molt_zlib_runtime_ready", globals())

# --- one-shot functions ---
_MOLT_ZLIB_COMPRESS = _require_intrinsic("molt_zlib_compress", globals())
_MOLT_ZLIB_DECOMPRESS = _require_intrinsic("molt_zlib_decompress", globals())
_MOLT_ZLIB_CRC32 = _require_intrinsic("molt_zlib_crc32", globals())
_MOLT_ZLIB_ADLER32 = _require_intrinsic("molt_zlib_adler32", globals())

# --- compressobj handle operations ---
_MOLT_ZLIB_COMPRESSOBJ_NEW = _require_intrinsic("molt_zlib_compressobj_new", globals())
_MOLT_ZLIB_COMPRESSOBJ_COMPRESS = _require_intrinsic(
    "molt_zlib_compressobj_compress", globals()
)
_MOLT_ZLIB_COMPRESSOBJ_FLUSH = _require_intrinsic(
    "molt_zlib_compressobj_flush", globals()
)
_MOLT_ZLIB_COMPRESSOBJ_DROP = _require_intrinsic(
    "molt_zlib_compressobj_drop", globals()
)

# --- decompressobj handle operations ---
_MOLT_ZLIB_DECOMPRESSOBJ_NEW = _require_intrinsic(
    "molt_zlib_decompressobj_new", globals()
)
_MOLT_ZLIB_DECOMPRESSOBJ_DECOMPRESS = _require_intrinsic(
    "molt_zlib_decompressobj_decompress", globals()
)
_MOLT_ZLIB_DECOMPRESSOBJ_FLUSH = _require_intrinsic(
    "molt_zlib_decompressobj_flush", globals()
)
_MOLT_ZLIB_DECOMPRESSOBJ_DROP = _require_intrinsic(
    "molt_zlib_decompressobj_drop", globals()
)
_MOLT_ZLIB_DECOMPRESSOBJ_EOF = _require_intrinsic(
    "molt_zlib_decompressobj_eof", globals()
)
_MOLT_ZLIB_DECOMPRESSOBJ_UNCONSUMED_TAIL = _require_intrinsic(
    "molt_zlib_decompressobj_unconsumed_tail", globals()
)

# --- constants from Rust ---
DEF_BUF_SIZE: int = _require_intrinsic("molt_zlib_def_buf_size", globals())()
DEF_MEM_LEVEL: int = _require_intrinsic("molt_zlib_def_mem_level", globals())()
MAX_WBITS: int = _require_intrinsic("molt_zlib_max_wbits", globals())()
Z_BEST_COMPRESSION: int = _require_intrinsic(
    "molt_zlib_z_best_compression", globals()
)()
Z_BEST_SPEED: int = _require_intrinsic("molt_zlib_z_best_speed", globals())()
Z_DEFAULT_COMPRESSION: int = _require_intrinsic(
    "molt_zlib_z_default_compression", globals()
)()
Z_DEFAULT_STRATEGY: int = _require_intrinsic(
    "molt_zlib_z_default_strategy", globals()
)()
Z_FILTERED: int = _require_intrinsic("molt_zlib_z_filtered", globals())()
Z_FINISH: int = _require_intrinsic("molt_zlib_z_finish", globals())()
Z_FULL_FLUSH: int = _require_intrinsic("molt_zlib_z_full_flush", globals())()
Z_HUFFMAN_ONLY: int = _require_intrinsic("molt_zlib_z_huffman_only", globals())()
Z_NO_COMPRESSION: int = _require_intrinsic("molt_zlib_z_no_compression", globals())()
Z_NO_FLUSH: int = _require_intrinsic("molt_zlib_z_no_flush", globals())()
Z_SYNC_FLUSH: int = _require_intrinsic("molt_zlib_z_sync_flush", globals())()

# DEFLATED is the only supported method constant (matches CPython)
DEFLATED: int = 8


class error(Exception):
    """Exception raised on compression and decompression errors."""


def compress(
    data: bytes, /, level: int = Z_DEFAULT_COMPRESSION, wbits: int = MAX_WBITS
) -> bytes:
    """Compress *data* in one shot, returning compressed bytes."""
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError(f"a bytes-like object is required, not {type(data).__name__!r}")
    if not isinstance(level, int):
        raise TypeError("an integer is required")
    if not isinstance(wbits, int):
        raise TypeError("an integer is required")
    try:
        return bytes(_MOLT_ZLIB_COMPRESS(data, level))
    except Exception as exc:
        raise error(str(exc)) from exc


def decompress(
    data: bytes, /, wbits: int = MAX_WBITS, bufsize: int = DEF_BUF_SIZE
) -> bytes:
    """Decompress *data* in one shot, returning uncompressed bytes."""
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError(f"a bytes-like object is required, not {type(data).__name__!r}")
    if not isinstance(wbits, int):
        raise TypeError("an integer is required")
    if not isinstance(bufsize, int):
        raise TypeError("an integer is required")
    try:
        return bytes(_MOLT_ZLIB_DECOMPRESS(data, wbits, bufsize))
    except Exception as exc:
        raise error(str(exc)) from exc


def crc32(data: bytes, value: int = 0) -> int:
    """Compute CRC-32 checksum of *data*, optionally continuing from *value*."""
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError(f"a bytes-like object is required, not {type(data).__name__!r}")
    if not isinstance(value, int):
        raise TypeError("an integer is required")
    return int(_MOLT_ZLIB_CRC32(data, value)) & 0xFFFFFFFF


def adler32(data: bytes, value: int = 1) -> int:
    """Compute Adler-32 checksum of *data*, optionally continuing from *value*."""
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError(f"a bytes-like object is required, not {type(data).__name__!r}")
    if not isinstance(value, int):
        raise TypeError("an integer is required")
    return int(_MOLT_ZLIB_ADLER32(data, value)) & 0xFFFFFFFF


def compressobj(
    level: int = Z_DEFAULT_COMPRESSION,
    method: int = DEFLATED,
    wbits: int = MAX_WBITS,
    memLevel: int = DEF_MEM_LEVEL,
    strategy: int = Z_DEFAULT_STRATEGY,
    zdict: bytes | None = None,
) -> "Compress":
    """Return a streaming compression object.

    The *zdict* parameter is accepted for API compatibility but is currently
    not forwarded to the Rust intrinsic (which silently ignores it).
    """
    if not isinstance(level, int):
        raise TypeError("an integer is required")
    if not isinstance(method, int):
        raise TypeError("an integer is required")
    if not isinstance(wbits, int):
        raise TypeError("an integer is required")
    if not isinstance(memLevel, int):
        raise TypeError("an integer is required")
    if not isinstance(strategy, int):
        raise TypeError("an integer is required")
    if zdict is not None and not isinstance(zdict, (bytes, bytearray)):
        raise TypeError(
            f"a bytes-like object is required, not {type(zdict).__name__!r}"
        )
    return Compress(level, method, wbits, memLevel, strategy)


def decompressobj(wbits: int = MAX_WBITS, zdict: bytes | None = None) -> "Decompress":
    """Return a streaming decompression object.

    The *zdict* parameter is accepted for API compatibility but is currently
    not forwarded to the Rust intrinsic (which silently ignores it).
    """
    if not isinstance(wbits, int):
        raise TypeError("an integer is required")
    if zdict is not None and not isinstance(zdict, (bytes, bytearray)):
        raise TypeError(
            f"a bytes-like object is required, not {type(zdict).__name__!r}"
        )
    return Decompress(wbits)


class Compress:
    """Streaming compressor backed by a Rust handle."""

    def __init__(
        self,
        level: int,
        method: int,
        wbits: int,
        memLevel: int,
        strategy: int,
    ) -> None:
        try:
            self._handle = _MOLT_ZLIB_COMPRESSOBJ_NEW(
                level, method, wbits, memLevel, strategy
            )
        except Exception as exc:
            raise error(str(exc)) from exc
        self._flushed = False

    def compress(self, data: bytes) -> bytes:
        """Compress *data*, returning a partial compressed bytes object."""
        if self._flushed:
            raise error("compressobj finished")
        if not isinstance(data, (bytes, bytearray)):
            raise TypeError(
                f"a bytes-like object is required, not {type(data).__name__!r}"
            )
        try:
            return bytes(_MOLT_ZLIB_COMPRESSOBJ_COMPRESS(self._handle, data))
        except Exception as exc:
            raise error(str(exc)) from exc

    def flush(self, mode: int = Z_FINISH) -> bytes:
        """Flush remaining compressed output.

        *mode* may be Z_NO_FLUSH, Z_SYNC_FLUSH, Z_FULL_FLUSH, or Z_FINISH
        (default).  After a Z_FINISH flush, compress() must not be called again.
        """
        if self._flushed:
            raise error("compressobj finished")
        if not isinstance(mode, int):
            raise TypeError("an integer is required")
        try:
            result = bytes(_MOLT_ZLIB_COMPRESSOBJ_FLUSH(self._handle, mode))
        except Exception as exc:
            raise error(str(exc)) from exc
        if mode == Z_FINISH:
            self._flushed = True
        return result

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_ZLIB_COMPRESSOBJ_DROP(handle)
            except Exception:
                pass


class Decompress:
    """Streaming decompressor backed by a Rust handle."""

    def __init__(self, wbits: int) -> None:
        try:
            self._handle = _MOLT_ZLIB_DECOMPRESSOBJ_NEW(wbits)
        except Exception as exc:
            raise error(str(exc)) from exc
        self._flushed = False

    def decompress(self, data: bytes, max_length: int = 0) -> bytes:
        """Decompress *data*, returning uncompressed bytes.

        If *max_length* is non-zero the returned buffer is at most that many
        bytes; any remaining input is accessible via *unconsumed_tail*.
        """
        if self.eof:
            raise error("end of stream already reached")
        if not isinstance(data, (bytes, bytearray)):
            raise TypeError(
                f"a bytes-like object is required, not {type(data).__name__!r}"
            )
        if not isinstance(max_length, int):
            raise TypeError("an integer is required")
        try:
            return bytes(
                _MOLT_ZLIB_DECOMPRESSOBJ_DECOMPRESS(self._handle, data, max_length)
            )
        except Exception as exc:
            raise error(str(exc)) from exc

    def flush(self, length: int = DEF_BUF_SIZE) -> bytes:
        """Process all pending input and return remaining uncompressed output."""
        if not isinstance(length, int):
            raise TypeError("an integer is required")
        try:
            result = bytes(_MOLT_ZLIB_DECOMPRESSOBJ_FLUSH(self._handle, length))
        except Exception as exc:
            raise error(str(exc)) from exc
        self._flushed = True
        return result

    @property
    def eof(self) -> bool:
        """True if the end of the compressed data stream has been reached."""
        return bool(_MOLT_ZLIB_DECOMPRESSOBJ_EOF(self._handle))

    @property
    def unconsumed_tail(self) -> bytes:
        """Data not consumed by the last decompress() call due to max_length."""
        return bytes(_MOLT_ZLIB_DECOMPRESSOBJ_UNCONSUMED_TAIL(self._handle))

    @property
    def unused_data(self) -> bytes:
        """Bytes past the end of the compressed data stream."""
        # The Rust intrinsic returns the same buffer as unconsumed_tail when
        # the stream is complete; re-use the same intrinsic for now.
        # TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): expose a dedicated molt_zlib_decompressobj_unused_data intrinsic
        return b""

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_ZLIB_DECOMPRESSOBJ_DROP(handle)
            except Exception:
                pass


__all__ = [
    # exception
    "error",
    # one-shot functions
    "adler32",
    "compress",
    "compressobj",
    "crc32",
    "decompress",
    "decompressobj",
    # streaming classes
    "Compress",
    "Decompress",
    # constants
    "DEF_BUF_SIZE",
    "DEF_MEM_LEVEL",
    "DEFLATED",
    "MAX_WBITS",
    "Z_BEST_COMPRESSION",
    "Z_BEST_SPEED",
    "Z_DEFAULT_COMPRESSION",
    "Z_DEFAULT_STRATEGY",
    "Z_FILTERED",
    "Z_FINISH",
    "Z_FULL_FLUSH",
    "Z_HUFFMAN_ONLY",
    "Z_NO_COMPRESSION",
    "Z_NO_FLUSH",
    "Z_SYNC_FLUSH",
]
