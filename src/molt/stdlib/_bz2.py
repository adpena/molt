"""Minimal _bz2 shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement
# bz2 compression/decompression parity or runtime-backed hooks.


class BZ2Compressor:
    def __init__(self, *args, **kwargs) -> None:
        raise NotImplementedError("_bz2.BZ2Compressor is not supported yet")


class BZ2Decompressor:
    def __init__(self, *args, **kwargs) -> None:
        raise NotImplementedError("_bz2.BZ2Decompressor is not supported yet")


__all__ = ["BZ2Compressor", "BZ2Decompressor"]
