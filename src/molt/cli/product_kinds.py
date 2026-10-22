"""The closed set of product kinds the host's product store holds.

A leaf module, so the CLI parser names the kinds without importing the store.
"""

from __future__ import annotations

from typing import Literal, get_args

ProductKind = Literal["native-runtime", "host-tool", "jit-runtime-image"]
PRODUCT_KINDS: tuple[ProductKind, ...] = get_args(ProductKind)


def product_kind(name: str) -> ProductKind:
    """``name`` as a product kind; ValueError when the store holds no such kind."""
    for kind in PRODUCT_KINDS:
        if kind == name:
            return kind
    raise ValueError(f"unknown product kind: {name!r}")
