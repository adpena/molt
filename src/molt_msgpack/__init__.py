"""Molt MsgPack/CBOR parsing via runtime intrinsics."""

from __future__ import annotations

from typing import Any

from molt import intrinsics as _intrinsics


def parse_msgpack(data: bytes) -> Any:
    parse = _intrinsics.require("molt_msgpack_parse_scalar_obj", globals())
    return parse(data)


def parse_cbor(data: bytes) -> Any:
    parse = _intrinsics.require("molt_cbor_parse_scalar_obj", globals())
    return parse(data)


def parse(data: bytes) -> Any:
    return parse_msgpack(data)
