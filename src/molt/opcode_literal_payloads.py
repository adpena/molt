"""Owned literal projections from the opcode registry, including kind aliases."""

from __future__ import annotations

# (Rust variant, owns a variable-length payload). Both generated IR enums and
# backend projections consume this vocabulary; opcode membership lives in TOML.
LITERAL_PAYLOAD_KINDS = {
    "int": ("Int", False),
    "float": ("Float", False),
    "none": ("None", False),
    "bool": ("Bool", False),
    "string": ("String", True),
    "bytes": ("Bytes", True),
    "bigint_decimal": ("BigintDecimal", True),
}


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
