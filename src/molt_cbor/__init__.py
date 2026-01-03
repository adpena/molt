"""CPython fallback for Molt CBOR parsing."""

from __future__ import annotations

from typing import Any

from molt_msgpack import parse_cbor


def parse(data: bytes) -> Any:
    return parse_cbor(data)
