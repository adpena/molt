"""Minimal `marshal` subset for Molt."""

from __future__ import annotations

import json


def dumps(value, version: int = 4) -> bytes:
    _ = version
    return json.dumps(value, sort_keys=True).encode("utf-8")


def loads(data: bytes):
    return json.loads(bytes(data).decode("utf-8"))


__all__ = ["dumps", "loads"]
