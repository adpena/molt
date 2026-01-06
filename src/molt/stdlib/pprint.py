"""Pretty-print helpers for Molt."""

from __future__ import annotations

from typing import Any

__all__ = ["pformat", "pprint", "pp"]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement width/indent formatting.


def pformat(
    obj: Any,
    indent: int = 1,
    width: int = 80,
    depth: int | None = None,
    compact: bool = False,
) -> str:
    _ = (indent, width, depth, compact)
    return repr(obj)


def pprint(
    obj: Any,
    indent: int = 1,
    width: int = 80,
    depth: int | None = None,
    compact: bool = False,
) -> None:
    print(pformat(obj, indent=indent, width=width, depth=depth, compact=compact))


pp = pprint
