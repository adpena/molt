"""Molt JSON parsing via runtime intrinsics."""

from __future__ import annotations

from typing import Any

from molt import intrinsics as _intrinsics


def parse(data: str) -> Any:
    parse_scalar = _intrinsics.require("molt_json_parse_scalar_obj", globals())
    return parse_scalar(data)


__all__ = ["parse"]
