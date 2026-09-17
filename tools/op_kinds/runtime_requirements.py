"""Shared projections of callable requirements from the semantic role registry."""

from __future__ import annotations


def runtime_symbol_requirement_masks(data: dict) -> dict[str, int]:
    masks: dict[str, int] = {}
    for role in data["simpleir_runtime_requirement_roles"]:
        for symbol in role.get("runtime_symbols", []):
            masks[symbol] = masks.get(symbol, 0) | (1 << role["bit"])
    return masks


def runtime_callable_attribute_requirement_masks(data: dict) -> dict[str, int]:
    symbols = runtime_symbol_requirement_masks(data)
    attributes: dict[str, int] = {}
    protected = 0
    for row in data.get("simpleir_runtime_qualified_callable", []):
        bits = symbols[row["symbol"]]
        attr = row["qualified"].rsplit(".", 1)[1]
        attributes[attr] = attributes.get(attr, 0) | bits
        protected |= bits
    for attr in data.get("simpleir_runtime_protected_attribute_gateways", []):
        attributes[attr] = attributes.get(attr, 0) | protected
    return attributes
