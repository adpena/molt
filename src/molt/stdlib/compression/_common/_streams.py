"""``compression._common._streams`` — base stream classes for compression I/O."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_COMPRESSION_STREAMS_BUFFER_SIZE = _require_intrinsic(
    "molt_compression_streams_buffer_size")

import io

BUFFER_SIZE = int(_MOLT_COMPRESSION_STREAMS_BUFFER_SIZE())

__all__ = ["BUFFER_SIZE", "BaseStream", "DecompressReader"]


class BaseStream(io.BufferedIOBase):
    """Base class for compressed file stream wrappers."""

    def readable(self) -> bool:
        return False

    def writable(self) -> bool:
        return False

    def seekable(self) -> bool:
        return False


class DecompressReader(io.RawIOBase):
    """Adapts a decompressor object to a RawIOBase reader interface."""

    def __init__(
        self,
        fp: io.RawIOBase | io.BufferedIOBase,
        decomp_factory: object,
        trailing_error: type[Exception] = Exception,
        **decomp_args: object,
    ) -> None:
        self._fp = fp
        self._decomp_factory = decomp_factory
        self._trailing_error = trailing_error
        self._decomp_args = decomp_args
        self._decompressor: object = decomp_factory(**decomp_args)  # type: ignore[operator]
        self._eof = False
        self._pos = 0
        self._size = -1

    def readable(self) -> bool:
        return True

    def readinto(self, b: bytearray | memoryview) -> int:
        with memoryview(b) as view, view.cast("B") as byte_view:
            data = self.read(len(byte_view))
            byte_view[: len(data)] = data
        return len(data)

    def read(self, size: int = -1) -> bytes:
        if size < 0:
            return self.readall()
        if not size or self._eof:
            return b""
        buf = b""
        while len(buf) < size:
            raw = self._fp.read(BUFFER_SIZE)
            if not raw:
                self._eof = True
                break
            decomp = self._decompressor
            if decomp is None:
                break
            try:
                data = decomp.decompress(raw, size - len(buf))  # type: ignore[union-attr]
            except self._trailing_error:
                self._eof = True
                break
            buf += data
            if hasattr(decomp, "eof") and decomp.eof:  # type: ignore[union-attr]
                self._eof = True
                unused = getattr(decomp, "unused_data", b"")
                if unused:
                    self._fp.prepend(unused)  # type: ignore[union-attr]
                break
        self._pos += len(buf)
        return buf

    def readall(self) -> bytes:
        chunks: list[bytes] = []
        while True:
            chunk = self.read(BUFFER_SIZE)
            if not chunk:
                break
            chunks.append(chunk)
        return b"".join(chunks)

    @property
    def _needsInput(self) -> bool:
        decomp = self._decompressor
        if decomp is None:
            return False
        return getattr(decomp, "needs_input", True)  # type: ignore[union-attr]
