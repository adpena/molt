"""Header admission for Molt's deterministic regular-file USTAR custody."""

from __future__ import annotations

import tarfile
from tarfile import BLOCKSIZE, TarFile
from typing import Self


class RegularUstarTarInfo(tarfile.TarInfo):
    """Reject extension records before tarfile can allocate their payloads."""

    @classmethod
    def fromtarfile(cls, tarfile: TarFile) -> Self:
        """Own header admission before the base decoder can process extensions.

        Recent CPython patch releases route TarInfo.fromtarfile through a
        private _frombuf that bypasses this class's frombuf, so the header
        read is owned here on every patch release. _proc_member is untyped
        private stdlib API.
        """
        raw = tarfile.fileobj.read(BLOCKSIZE)
        member = cls.frombuf(raw, tarfile.encoding, tarfile.errors)
        member.offset = tarfile.fileobj.tell() - BLOCKSIZE
        return getattr(member, "_proc_member")(tarfile)

    @classmethod
    def frombuf(cls, buf: bytes | bytearray, encoding: str, errors: str) -> Self:
        member = super().frombuf(buf, encoding, errors)
        # ReadError is public and fatal at every offset; TarFile.next can treat
        # an invalid subsequent header as end-of-archive instead of rejecting it.
        if member.type != tarfile.REGTYPE:
            raise tarfile.ReadError("archive member is not a regular file")
        if buf[257:265] != tarfile.POSIX_MAGIC:
            raise tarfile.ReadError("archive member is not canonical USTAR")
        return member
