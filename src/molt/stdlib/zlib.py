"""zlib compression for Molt — all computation delegated to Rust intrinsics."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- runtime gate ---

# --- one-shot functions ---
_MOLT_ZLIB_COMPRESS = _require_intrinsic("molt_zlib_compress")
_MOLT_ZLIB_DECOMPRESS = _require_intrinsic("molt_zlib_decompress")
_MOLT_ZLIB_CRC32 = _require_intrinsic("molt_zlib_crc32")
_MOLT_ZLIB_ADLER32 = _require_intrinsic("molt_zlib_adler32")

# --- compressobj handle operations ---
_MOLT_ZLIB_COMPRESSOBJ_NEW = _require_intrinsic("molt_zlib_compressobj_new")
_MOLT_ZLIB_COMPRESSOBJ_COMPRESS = _require_intrinsic("molt_zlib_compressobj_compress")
_MOLT_ZLIB_COMPRESSOBJ_FLUSH = _require_intrinsic("molt_zlib_compressobj_flush")

# --- decompressobj handle operations ---
_MOLT_ZLIB_DECOMPRESSOBJ_NEW = _require_intrinsic("molt_zlib_decompressobj_new")
_MOLT_ZLIB_DECOMPRESSOBJ_DECOMPRESS = _require_intrinsic(
    "molt_zlib_decompressobj_decompress"
)
_MOLT_ZLIB_DECOMPRESSOBJ_FLUSH = _require_intrinsic("molt_zlib_decompressobj_flush")
_MOLT_ZLIB_DECOMPRESSOBJ_EOF = _require_intrinsic("molt_zlib_decompressobj_eof")
_MOLT_ZLIB_DECOMPRESSOBJ_UNCONSUMED_TAIL = _require_intrinsic(
    "molt_zlib_decompressobj_unconsumed_tail"
)
_MOLT_ZLIB_DECOMPRESSOBJ_UNUSED_DATA = _require_intrinsic(
    "molt_zlib_decompressobj_unused_data"
)

# --- constants from Rust ---
DEF_BUF_SIZE: int = _require_intrinsic("molt_zlib_def_buf_size")()
DEF_MEM_LEVEL: int = _require_intrinsic("molt_zlib_def_mem_level")()
MAX_WBITS: int = _require_intrinsic("molt_zlib_max_wbits")()
Z_BEST_COMPRESSION: int = _require_intrinsic("molt_zlib_z_best_compression")()
Z_BEST_SPEED: int = _require_intrinsic("molt_zlib_z_best_speed")()
Z_DEFAULT_COMPRESSION: int = _require_intrinsic("molt_zlib_z_default_compression")()
Z_DEFAULT_STRATEGY: int = _require_intrinsic("molt_zlib_z_default_strategy")()
Z_FILTERED: int = _require_intrinsic("molt_zlib_z_filtered")()
Z_FINISH: int = _require_intrinsic("molt_zlib_z_finish")()
Z_FULL_FLUSH: int = _require_intrinsic("molt_zlib_z_full_flush")()
Z_HUFFMAN_ONLY: int = _require_intrinsic("molt_zlib_z_huffman_only")()
Z_NO_COMPRESSION: int = _require_intrinsic("molt_zlib_z_no_compression")()
Z_NO_FLUSH: int = _require_intrinsic("molt_zlib_z_no_flush")()
Z_SYNC_FLUSH: int = _require_intrinsic("molt_zlib_z_sync_flush")()

# DEFLATED is the only supported method constant (matches CPython)
DEFLATED: int = 8


class error(Exception):
    """Exception raised on compression and decompression errors."""


_ZLIB_ERROR = error
_ZDICT_OMITTED = object()


def compress(
    data: bytes, /, level: int = Z_DEFAULT_COMPRESSION, wbits: int = MAX_WBITS
) -> bytes:
    """Compress *data* in one shot, returning compressed bytes."""
    return bytes(_MOLT_ZLIB_COMPRESS(_ZLIB_ERROR, data, level, wbits))


def decompress(
    data: bytes, /, wbits: int = MAX_WBITS, bufsize: int = DEF_BUF_SIZE
) -> bytes:
    """Decompress *data* in one shot, returning uncompressed bytes."""
    return bytes(_MOLT_ZLIB_DECOMPRESS(_ZLIB_ERROR, data, wbits, bufsize))


def crc32(data: bytes, value: int = 0) -> int:
    """Compute CRC-32 checksum of *data*, optionally continuing from *value*."""
    return int(_MOLT_ZLIB_CRC32(data, value)) & 0xFFFFFFFF


def adler32(data: bytes, value: int = 1) -> int:
    """Compute Adler-32 checksum of *data*, optionally continuing from *value*."""
    return int(_MOLT_ZLIB_ADLER32(data, value)) & 0xFFFFFFFF


def compressobj(
    level: int = Z_DEFAULT_COMPRESSION,
    method: int = DEFLATED,
    wbits: int = MAX_WBITS,
    memLevel: int = DEF_MEM_LEVEL,
    strategy: int = Z_DEFAULT_STRATEGY,
    zdict=_ZDICT_OMITTED,
) -> "Compress":
    """Return a streaming compression object."""
    return Compress(level, method, wbits, memLevel, strategy, zdict)


def decompressobj(wbits: int = MAX_WBITS, zdict: bytes | None = None) -> "Decompress":
    """Return a streaming decompression object.

    The *zdict* parameter is accepted for API compatibility but is currently
    not forwarded to the Rust intrinsic (which silently ignores it).
    """
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
        zdict=_ZDICT_OMITTED,
    ) -> None:
        self._handle = _MOLT_ZLIB_COMPRESSOBJ_NEW(
            _ZLIB_ERROR,
            level,
            method,
            wbits,
            memLevel,
            strategy,
            zdict,
            zdict is not _ZDICT_OMITTED,
        )

    def compress(self, data: bytes) -> bytes:
        """Compress *data*, returning a partial compressed bytes object."""
        return bytes(_MOLT_ZLIB_COMPRESSOBJ_COMPRESS(_ZLIB_ERROR, self._handle, data))

    def flush(self, mode: int = Z_FINISH) -> bytes:
        """Flush remaining compressed output.

        *mode* may be Z_NO_FLUSH, Z_SYNC_FLUSH, Z_FULL_FLUSH, or Z_FINISH
        (default).  After a Z_FINISH flush, compress() must not be called again.
        """
        return bytes(_MOLT_ZLIB_COMPRESSOBJ_FLUSH(_ZLIB_ERROR, self._handle, mode))


class Decompress:
    """Streaming decompressor backed by a Rust handle."""

    def __init__(self, wbits: int) -> None:
        self._handle = _MOLT_ZLIB_DECOMPRESSOBJ_NEW(wbits)

    def decompress(self, data: bytes, max_length: int = 0) -> bytes:
        """Decompress *data*, returning uncompressed bytes.

        If *max_length* is non-zero the returned buffer is at most that many
        bytes; any remaining input is accessible via *unconsumed_tail*.
        """
        return bytes(
            _MOLT_ZLIB_DECOMPRESSOBJ_DECOMPRESS(_ZLIB_ERROR, self._handle, data, max_length)
        )

    def flush(self, length: int = DEF_BUF_SIZE) -> bytes:
        """Process all pending input and return remaining uncompressed output."""
        result = bytes(_MOLT_ZLIB_DECOMPRESSOBJ_FLUSH(_ZLIB_ERROR, self._handle, length))
        return result

    @property
    def eof(self) -> bool:
        """True if the end of the compressed data stream has been reached."""
        return bool(_MOLT_ZLIB_DECOMPRESSOBJ_EOF(_ZLIB_ERROR, self._handle))

    @property
    def unconsumed_tail(self) -> bytes:
        """Data not consumed by the last decompress() call due to max_length."""
        return bytes(_MOLT_ZLIB_DECOMPRESSOBJ_UNCONSUMED_TAIL(_ZLIB_ERROR, self._handle))

    @property
    def unused_data(self) -> bytes:
        """Bytes past the end of the compressed data stream.

        Matches CPython zlib: this is ``b""`` until the last byte of
        compressed data has been processed, after which it holds any trailing
        bytes found after the compressed stream ended (e.g. concatenated
        streams). It is distinct from :attr:`unconsumed_tail`.
        """
        return bytes(_MOLT_ZLIB_DECOMPRESSOBJ_UNUSED_DATA(_ZLIB_ERROR, self._handle))



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

globals().pop("_require_intrinsic", None)
