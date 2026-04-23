"""IFF chunk reader — CPython 3.12 parity for Molt.

This module provides a class ``Chunk`` that allows reading IFF/RIFF/AIFF
chunked file formats.  It is a pure-Python implementation; no intrinsics
are required.

class Chunk:
    def __init__(self, file, align=True, bigendian=True, inclheader=False)

The ``file`` argument must be an open binary-mode file-like object that
supports ``read()``, ``seek()``, and ``tell()``.

``align``      -- if True, chunk sizes are padded to even byte boundaries
                  (the IFF convention); default True.
``bigendian``  -- if True, the chunk size is read as a big-endian 32-bit
                  unsigned integer; if False, little-endian (RIFF convention);
                  default True.
``inclheader`` -- if True, the 8-byte chunk header is counted as part of
                  the chunk size; default False.
"""

from __future__ import annotations

import struct

__all__ = ["Chunk"]


class Chunk:
    """Class that implements reading of IFF/AIFF chunks.

    A chunk has the following structure:
        4 bytes: chunk id (ASCII string)
        4 bytes: chunk size (big-endian or little-endian unsigned int)
        n bytes: chunk data
        0 or 1 bytes: pad byte (if align=True and chunk size is odd)

    Usage::

        file = open("filename.aiff", "rb")
        chunk = Chunk(file)
        while True:
            chunk_id = chunk.getname()
            chunk_data = chunk.read()
            try:
                chunk.skip()
            except EOFError:
                break
    """

    def __init__(
        self, file, align: bool = True, bigendian: bool = True, inclheader: bool = False
    ) -> None:
        self.closed = False
        self.align = align  # whether to align to word (2-byte) boundaries
        if bigendian:
            strflag = ">"
        else:
            strflag = "<"
        self.file = file
        self.chunkname = file.read(4)
        if len(self.chunkname) < 4:
            raise EOFError
        try:
            strh = struct.unpack_from(strflag + "L", file.read(4))
        except struct.error:
            raise EOFError from None
        self.chunksize = strh[0]
        self.inclheader = inclheader
        if inclheader:
            self.chunksize = self.chunksize - 8  # subtract the 8-byte header
        self.size_read = 0
        try:
            self.offset = self.file.tell()
        except (AttributeError, OSError):
            self.seekable = False
        else:
            self.seekable = True

    def getname(self) -> bytes:
        """Return the name (ID) of the current chunk."""
        return self.chunkname

    def getsize(self) -> int:
        """Return the size of the current chunk, excluding the header."""
        return self.chunksize

    def close(self) -> None:
        if not self.closed:
            try:
                self.skip()
            finally:
                self.closed = True

    def __enter__(self) -> "Chunk":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()

    def isatty(self) -> bool:
        return False

    def seek(self, pos: int, whence: int = 0) -> None:
        """Set the chunk's current position.

        The seek is relative to the start of the chunk data (not the header).
        whence defaults to 0 (absolute), 1 (relative), 2 (from end).
        Raises OSError if the file is not seekable.
        """
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if not self.seekable:
            raise OSError("underlying stream is not seekable")
        if whence == 1:
            pos = pos + self.size_read
        elif whence == 2:
            pos = pos + self.chunksize
        if pos < 0 or pos > self.chunksize:
            raise RuntimeError("chunk seek out of range")
        self.file.seek(self.offset + pos, 0)
        self.size_read = pos

    def tell(self) -> int:
        if self.closed:
            raise ValueError("I/O operation on closed file")
        return self.size_read

    def read(self, size: int = -1) -> bytes:
        """Read at most *size* bytes from the chunk.

        If *size* is omitted or negative, read until the end of the chunk.
        An empty bytes object is returned when at end of chunk data.
        """
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if self.size_read >= self.chunksize:
            return b""
        if size < 0:
            size = self.chunksize - self.size_read
        if size > self.chunksize - self.size_read:
            size = self.chunksize - self.size_read
        data = self.file.read(size)
        self.size_read += len(data)
        if self.size_read == self.chunksize and self.align and (self.chunksize & 1):
            dummy = self.file.read(1)
            self.size_read += len(dummy)
        return data

    def skip(self) -> None:
        """Skip the rest of the chunk.

        If the file is not seekable, the remaining data is read and discarded.
        """
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if self.seekable:
            try:
                n = self.chunksize - self.size_read
                # Move the file pointer to the end of this chunk
                self.file.seek(n, 1)
                self.size_read = self.chunksize
                # Skip the pad byte, if any
                if self.align and (self.chunksize & 1):
                    self.file.seek(1, 1)
                return
            except OSError:
                pass
        while self.size_read < self.chunksize:
            n = min(8192, self.chunksize - self.size_read)
            dummy = self.read(n)
            if not dummy:
                raise EOFError
