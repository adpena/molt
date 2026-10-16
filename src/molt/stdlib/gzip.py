"""Gzip streams over the runtime compression and I/O authorities."""

from __future__ import annotations

import io
import builtins
import os
import weakref
import warnings
import time
import struct
import zlib

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_GZIP_OPEN = _require_intrinsic("molt_gzip_open")
_MOLT_GZIP_READ = _require_intrinsic("molt_gzip_read")
_MOLT_GZIP_CLOSE = _require_intrinsic("molt_gzip_close")
_TARGET_VERSION = _require_intrinsic("molt_sys_version_info")()[:2]
_TARGET_313 = _TARGET_VERSION >= (3, 13)


class BadGzipFile(OSError):
    """Exception raised for invalid gzip files."""

    pass


_GZIP_ERROR = BadGzipFile


def _create_simple_gzip_header(
    compresslevel: int, mtime=None, _time=time, _struct=struct
) -> bytes:
    """
    Write a simple gzip header with no extra fields.
    :param compresslevel: Compresslevel used to determine the xfl bytes.
    :param mtime: The mtime (must support conversion to a 32-bit integer).
    :return: A bytes object representing the gzip header.
    """
    if mtime is None:
        mtime = _time.time()
    if compresslevel == 9:
        xfl = 2
    elif compresslevel == 1:
        xfl = 4
    else:
        xfl = 0
    # Pack ID1 and ID2 magic bytes, method (8=deflate), header flags (no extra
    # fields added to header), mtime, xfl and os (255 for unknown OS).
    return _struct.pack("<BBBBLBB", 0x1f, 0x8b, 8, 0, int(mtime), xfl, 255)


def _compress_payload(
    data, compresslevel, mtime, _compress=zlib.compress,
    _time=time, _struct=struct, _zlib=zlib, _header=_create_simple_gzip_header,
    _modern_target=_TARGET_313,
):
    if not _modern_target and mtime != 0:
        header = _header(compresslevel, mtime)
        trailer = _struct.pack("<LL", _zlib.crc32(data), len(data) & 0xffffffff)
        return header + _compress(data, compresslevel, wbits=-15) + trailer
    payload = _compress(data, compresslevel, wbits=31)
    if not _modern_target:
        return payload
    if mtime is None:
        mtime = _time.time()
    header = _struct.pack("<4sLBB", payload, int(mtime), payload[8], 255)
    return header + payload[10:]


def _write32u(output, value, _pack=struct.pack):
    output.write(_pack("<L", value))


class _WriteBufferStream(io.RawIOBase):
    """CPython's bounded BufferedWriter adapter; its gzip edge is weak."""

    def __init__(self, gzip_file, _ref=weakref.ref):
        self.gzip_file = _ref(gzip_file)

    def write(self, data):
        gzip_file = self.gzip_file()
        if gzip_file is None:
            raise RuntimeError("lost gzip_file")
        return gzip_file._write_raw(data)

    def writable(self):
        return True

    def seekable(self):
        return False


class GzipFile:
    """Gzip writing over the shared buffered/raw and zlib stream owners."""

    def __init__(self, filename=None, mode=None, compresslevel=9, fileobj=None, mtime=None):
        _open, _path, _io, _raw = builtins.open, os, io, _WriteBufferStream
        _compressobj, _crc32, _native_open = zlib.compressobj, zlib.crc32, _MOLT_GZIP_OPEN
        compression_options = (zlib.DEFLATED, -zlib.MAX_WBITS, zlib.DEF_MEM_LEVEL, zlib.Z_DEFAULT_STRATEGY)
        self._handle = None
        self.fileobj = None
        self.myfileobj = None
        self._buffer = None
        if mode and ("t" in mode or "U" in mode):
            raise ValueError("Invalid mode: {!r}".format(mode))
        if mode and "b" not in mode:
            mode += "b"
        try:
            if fileobj is None and (mode is None or mode.startswith("r")):
                self._mode = mode or "rb"
                self._handle = _native_open(filename, self._mode, compresslevel)
                self.name = filename
                return
            if fileobj is None:
                fileobj = self.myfileobj = _open(filename, mode or "rb")
            if filename is None:
                filename = getattr(fileobj, "name", "")
                if not isinstance(filename, (str, bytes)):
                    filename = ""
            else:
                filename = _path.fspath(filename)
            selected_mode = mode if mode is not None else getattr(fileobj, "mode", "rb")
            self._mode = selected_mode
            self.name = filename
            if selected_mode.startswith("r"):
                # Fileobj READ retains its existing separate one-shot owner.
                self.fileobj = fileobj
                return
            if not selected_mode.startswith(("w", "a", "x")):
                raise ValueError("Invalid mode: {!r}".format(selected_mode))
            if mode is None:
                warnings.warn("GzipFile was opened for writing, but this will change in future Python releases.  Specify the mode argument for opening it for writing.", FutureWarning, 2)
            self.crc = _crc32(b"")
            self.size = 0
            self.offset = 0
            self.compress = _compressobj(compresslevel, *compression_options)
            self._write_mtime = mtime
            self._buffer = _io.BufferedWriter(_raw(self), buffer_size=4 * _io.DEFAULT_BUFFER_SIZE)
            self.fileobj = fileobj
            self._write_gzip_header(compresslevel)
        except:
            self._close()
            raise

    def _write_gzip_header(self, compresslevel, _path=os.path, _time=time.time, _write32=_write32u):
        self.fileobj.write(b"\037\213")
        self.fileobj.write(b"\010")
        try:
            filename = _path.basename(self.name)
            if not isinstance(filename, bytes):
                filename = filename.encode("latin-1")
            if filename.endswith(b".gz"):
                filename = filename[:-3]
        except UnicodeEncodeError:
            filename = b""
        self.fileobj.write(bytes((8 if filename else 0,)))
        mtime = self._write_mtime
        if mtime is None:
            mtime = _time()
        _write32(self.fileobj, int(mtime))
        self.fileobj.write(b"\002" if compresslevel == 9 else b"\004" if compresslevel == 1 else b"\000")
        self.fileobj.write(b"\377")
        if filename:
            self.fileobj.write(filename + b"\000")

    def _check_not_closed(self):
        if self.closed:
            raise ValueError("I/O operation on closed file")

    def read(self, size=-1):
        self._check_not_closed()
        if not self.readable():
            raise OSError(9, "read() on write-only GzipFile object")
        if self.fileobj is not None:
            return decompress(self.fileobj.read())
        return bytes(_MOLT_GZIP_READ(self._handle, size))

    def write(self, data):
        self._check_not_closed()
        if not self._mode.startswith(("w", "a", "x")):
            raise OSError(9, "write() on read-only GzipFile object")
        return self._buffer.write(data)

    def _write_raw(self, data, _crc32=zlib.crc32):
        if isinstance(data, (bytes, bytearray)):
            length = len(data)
        else:
            data = memoryview(data)
            length = data.nbytes
        if length:
            self.fileobj.write(self.compress.compress(data))
            self.size += length
            self.crc = _crc32(data, self.crc)
            self.offset += length
        return length

    def flush(self, zlib_mode=zlib.Z_SYNC_FLUSH):
        self._check_not_closed()
        if self._mode.startswith(("w", "a", "x")):
            self._buffer.flush()
            self.fileobj.write(self.compress.flush(zlib_mode))
            self.fileobj.flush()

    def readable(self):
        return self._mode.startswith("r")

    def writable(self):
        return self._mode.startswith(("w", "a", "x"))

    def seekable(self):
        return False

    @property
    def closed(self):
        return self.fileobj is None and self._handle is None

    def _release_handle(self, _close=_MOLT_GZIP_CLOSE, _write32=_write32u, _version=_TARGET_VERSION):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            object.__setattr__(self, "_handle", None)
            _close(handle)
            return
        fileobj = getattr(self, "fileobj", None)
        if fileobj is None:
            return
        if self._mode.startswith(("w", "a", "x")) and _version >= (3, 13):
            if _version >= (3, 14) and self._buffer is None:
                return
            if self._buffer.closed:
                return
        try:
            if self._mode.startswith(("w", "a", "x")):
                self._buffer.flush()
                fileobj.write(self.compress.flush())
                _write32(fileobj, self.crc)
                _write32(fileobj, self.size & 0xffffffff)
        finally:
            self._close()

    def _close(self):
        self.fileobj = None
        owned_file = getattr(self, "myfileobj", None)
        if owned_file is not None:
            self.myfileobj = None
            owned_file.close()

    def close(self):
        self._release_handle()

    def __enter__(self):
        self._check_not_closed()
        return self

    def __exit__(self, exc_type, exc, tb):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def compress(
    data: bytes, compresslevel: int = 9, *, mtime: float | None = (0 if _TARGET_VERSION >= (3, 14) else None)
) -> bytes:
    """Compress *data* in one shot, returning the gzip-compressed bytes."""
    effective_mtime = mtime
    return _compress_payload(data, compresslevel, effective_mtime)


# Header and member sequencing follow CPython v3.14.3 Lib/gzip.py (PSF license).
# zlib.decompressobj remains the sole deflate decoder; the Python facade owns
# the public buffer-then-subscript protocol and module exception identity.
def _read_exact(fp, n):
    '''Read exactly *n* bytes from `fp`

    This method is required because fp may be unbuffered,
    i.e. return short reads.
    '''
    data = fp.read(n)
    while len(data) < n:
        b = fp.read(n - len(data))
        if not b:
            raise EOFError("Compressed file ended before the "
                           "end-of-stream marker was reached")
        data += b
    return data


def _read_gzip_header(fp):
    '''Read a gzip header from `fp` and progress to the end of the header.

    Returns last mtime if header was present or None otherwise.
    '''
    magic = fp.read(2)
    if magic == b'':
        return None

    if magic != b'\037\213':
        raise _GZIP_ERROR('Not a gzipped file (%r)' % magic)

    (method, flag, last_mtime) = struct.unpack("<BBIxx", _read_exact(fp, 8))
    if method != 8:
        raise _GZIP_ERROR('Unknown compression method')

    if flag & FEXTRA:
        # Read & discard the extra field, if present
        extra_len, = struct.unpack("<H", _read_exact(fp, 2))
        _read_exact(fp, extra_len)
    if flag & FNAME:
        # Read and discard a null-terminated string containing the filename
        while True:
            s = fp.read(1)
            if not s or s==b'\000':
                break
    if flag & FCOMMENT:
        # Read and discard a null-terminated string containing a comment
        while True:
            s = fp.read(1)
            if not s or s==b'\000':
                break
    if flag & FHCRC:
        _read_exact(fp, 2)     # Read & discard the 16-bit header CRC
    return last_mtime


def decompress(data):
    """Decompress a gzip compressed string in one shot.
    Return the decompressed string.
    """
    decompressed_members = []
    while True:
        fp = io.BytesIO(data)
        if _read_gzip_header(fp) is None:
            return b"".join(decompressed_members)
        # Use a zlib raw deflate compressor
        do = zlib.decompressobj(wbits=-zlib.MAX_WBITS)
        # Read all the data except the header
        decompressed = do.decompress(data[fp.tell():])
        if not do.eof or len(do.unused_data) < 8:
            raise EOFError("Compressed file ended before the end-of-stream "
                           "marker was reached")
        crc, length = struct.unpack("<II", do.unused_data[:8])
        if crc != zlib.crc32(decompressed):
            raise _GZIP_ERROR("CRC check failed")
        if length != (len(decompressed) & 0xffffffff):
            raise _GZIP_ERROR("Incorrect length of data produced")
        decompressed_members.append(decompressed)
        data = do.unused_data[8:].lstrip(b"\x00")



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

globals().pop("_require_intrinsic", None)

FEXTRA = 4

FNAME = 8

FCOMMENT = 16

FHCRC = 2
