"""Owned literal projections from the opcode registry, including kind aliases."""

from __future__ import annotations

import ast
import copy

# (Rust variant, owns a variable-length payload). Both generated IR enums and
# backend projections consume this vocabulary; opcode membership lives in TOML.
LITERAL_PAYLOAD_KINDS = {
    "int": ("Int", False),
    "float": ("Float", False),
    "none": ("None", False),
    "bool": ("Bool", False),
    "string": ("String", True),
    "bytes": ("Bytes", True),
    "bigint_bytes": ("BigintBytes", True),
}


def integer_literal_bytes(value: int) -> bytes:
    """Minimal signed big-endian compiler transport, independent of digit policy."""
    if type(value) is not int:
        raise TypeError("integer literal requires an exact int")
    magnitude_bits = value.bit_length() if value >= 0 else (~value).bit_length()
    return value.to_bytes((magnitude_bits + 8) // 8, "big", signed=True)


def integer_literal_payload(payload: bytes) -> bytes:
    """Admit only the canonical compiler carrier; old decimal IR is invalid."""
    if type(payload) is not bytes or not payload:
        raise ValueError("integer literal requires nonempty signed bytes")
    if len(payload) > 1 and (
        payload[0] == 0
        and payload[1] < 0x80
        or payload[0] == 0xFF
        and payload[1] >= 0x80
    ):
        raise ValueError("integer literal has redundant sign extension")
    return payload


def integer_literal_value(payload: bytes) -> int:
    return int.from_bytes(integer_literal_payload(payload), "big", signed=True)


class _IntegerLiteralPayload(ast.AST):
    """Typed AST-dump leaf; cannot collide with a parsed Python AST node."""

    _fields = ("signed_bytes",)

    def __init__(self, payload: bytes) -> None:
        self.signed_bytes = payload


def literal_ast_dump(node: ast.AST) -> str:
    """Use the existing AST identity, with binary leaves for large integers.

    Only ancestors of a changed leaf are copied. The caller's live AST and its
    Python values remain untouched, and ordinary ASTs retain their exact dump.
    """

    def project(value: object) -> object:
        if type(value) is int and not -(1 << 63) <= value < (1 << 63):
            return _IntegerLiteralPayload(integer_literal_bytes(value))
        if isinstance(value, ast.AST):
            updates = None
            for name, child in ast.iter_fields(value):
                replacement = project(child)
                if replacement is not child:
                    if updates is None:
                        updates = {}
                    updates[name] = replacement
            if updates is not None:
                value = copy.copy(value)
                for name, replacement in updates.items():
                    setattr(value, name, replacement)
        elif isinstance(value, list):
            replacement_list = None
            for index, child in enumerate(value):
                replacement = project(child)
                if replacement is not child:
                    if replacement_list is None:
                        replacement_list = list(value)
                    replacement_list[index] = replacement
            if replacement_list is not None:
                return replacement_list
        return value

    projected = project(node)
    assert isinstance(projected, ast.AST)
    return ast.dump(projected, include_attributes=False)


def owned_literal_payloads_by_kind(data: dict) -> dict[str, str]:
    opcodes = {row["name"] for row in data["opcode"]}
    payloads: dict[str, str] = {}
    for row in data["literal_payload_opcodes"]:
        opcode, literal = row["opcode"], row["literal"]
        if opcode not in opcodes or opcode in payloads:
            raise ValueError(f"invalid or duplicate literal opcode {opcode!r}")
        if literal not in LITERAL_PAYLOAD_KINDS:
            raise ValueError(f"unknown literal payload {literal!r}")
        payloads[opcode] = literal
    result: dict[str, str] = {}
    for row in data["kind"]:
        literal = payloads.get(row.get("mapper_opcode"))
        if literal is None or not LITERAL_PAYLOAD_KINDS[literal][1]:
            continue
        for kind in (row["canonical"], *row.get("aliases", ())):
            if kind in result:
                raise ValueError(f"duplicate owned literal kind {kind!r}")
            result[kind] = literal
    return result
