"""Canonical frontend authority for try-region handler identity."""

from __future__ import annotations

from typing import Any

from molt.frontend._types import MoltOp


def try_region_id(op: MoltOp) -> Any:
    """Return the handler label carried by any frontend try marker.

    Ordinary ``try`` nodes carry the label in ``args[0]`` while ``with`` and
    ``async with`` preserve the same identity in ``metadata`` because their
    marker operands are otherwise empty.  All consumers must use this single
    authority so CFG pairing and serialized exception edges cannot diverge.
    """

    metadata = op.metadata or {}
    if "try_region_id" in metadata:
        return metadata["try_region_id"]
    if op.args:
        return op.args[0]
    return None
