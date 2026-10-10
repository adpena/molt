"""Minimal `sre_constants` subset for Molt."""

from __future__ import annotations


OPCODES: tuple[str, ...] = (
    "FAILURE",
    "SUCCESS",
    "LITERAL",
    "NOT_LITERAL",
    "IN",
    "ANY",
    "AT",
)

__all__ = ["OPCODES"]
