from __future__ import annotations

from molt._wasm_abi_generated import (
    ATTACHED_EXECUTION_CONTEXT_ABI_SECTION,
    ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT,
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


def with_execution_context_abi(module: bytes) -> bytes:
    """Attach the current attached execution ABI section to a test-only module."""
    return module + _custom_section(
        ATTACHED_EXECUTION_CONTEXT_ABI_SECTION,
        ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT.to_bytes(8, "little"),
    )
