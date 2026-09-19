"""Canonical frontend authority for try-region handler identity."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from molt.frontend.cfg_analysis import OpLike


def try_region_id(op: OpLike) -> Any:
    """Return the handler label carried by any frontend try marker.

    Every named region carries its label in ``args[0]``, including context
    managers and resumed generators. Empty operands denote anonymous IR regions.
    CFG and serialization consume the same operand authority. A region
    may close on several alternative paths; the identity is not a textual
    bracket and must never close an enclosing IF or LOOP.
    """

    if op.args:
        return op.args[0]
    return None
