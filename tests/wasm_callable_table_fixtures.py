from __future__ import annotations

from molt._wasm_abi_generated import (
    WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME,
    WASM_CALLABLE_TABLE_LAYOUT_VERSION,
    WASM_CALLABLE_TABLE_SECTION_NAME,
    WASM_CALLABLE_TABLE_SECTION_VERSION,
    WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT,
)


def _wasm_u32(value: int) -> bytes:
    encoded = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        encoded.append(byte | 0x80 if value else byte)
        if not value:
            return bytes(encoded)


def _wasm_string(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _wasm_u32(len(encoded)) + encoded


def _custom_section(name: str, payload: bytes) -> bytes:
    encoded = _wasm_string(name) + payload
    return b"\0" + _wasm_u32(len(encoded)) + encoded


def attested_empty_callable_table(module: bytes) -> bytes:
    """Attach the canonical zero-entry shape to a test-only core module."""
    layout = b"".join(
        _wasm_u32(value) for value in (WASM_CALLABLE_TABLE_LAYOUT_VERSION, 0, 0, 0, 0)
    )
    attestation = b"".join(
        _wasm_u32(value)
        for value in (
            WASM_CALLABLE_TABLE_SECTION_VERSION,
            WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT,
            0,
            0,
        )
    )
    return (
        module
        + _custom_section(WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME, layout)
        + _custom_section(WASM_CALLABLE_TABLE_SECTION_NAME, attestation)
    )
