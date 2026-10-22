"""The closed set of product kinds the host's product store holds.

A leaf module, so the CLI parser names the kinds without importing the store.
"""

from __future__ import annotations

from typing import Literal, get_args

ProductKind = Literal["native-runtime", "host-tool"]
PRODUCT_KINDS: tuple[ProductKind, ...] = get_args(ProductKind)
