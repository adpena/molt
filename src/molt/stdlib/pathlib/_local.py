"""Intrinsic-first shim for internal `pathlib._local` (CPython 3.13+ layout).

Molt keeps path shaping in Rust intrinsics; this module exists for import parity.
"""

from __future__ import annotations


from . import Path as Path  # re-export

# Avoid probe-only classification: this shim must still be intrinsic-backed.

__all__ = ["Path"]
