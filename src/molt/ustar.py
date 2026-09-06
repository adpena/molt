"""Header admission for Molt's deterministic regular-file USTAR custody."""

from __future__ import annotations

import tarfile
from typing import Self


class RegularUstarTarInfo(tarfile.TarInfo):
    """Reject extension records before tarfile can allocate their payloads."""

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
