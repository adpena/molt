from __future__ import annotations

from tools import wasm_link_edit
from tools.wasm_link_format import (
    WASM_MAGIC,
    WASM_VERSION,
    _parse_sections,
    _read_varuint,
    _write_varuint,
)


def _write_varsint(value: int) -> bytes:
    parts: list[int] = []
    more = True
    while more:
        byte = value & 0x7F
        value >>= 7
        sign_bit_set = bool(byte & 0x40)
        more = not (
            (value == 0 and not sign_bit_set) or (value == -1 and sign_bit_set)
        )
        if more:
            byte |= 0x80
        parts.append(byte)
    return bytes(parts)


def _section(section_id: int, payload: bytes) -> bytes:
    return bytes([section_id]) + _write_varuint(len(payload)) + payload


def _active_segment(start: int, funcs: list[int]) -> bytes:
    payload = bytearray()
    payload.append(0x00)
    payload.append(0x41)
    payload.extend(_write_varsint(start))
    payload.append(0x0B)
    payload.extend(_write_varuint(len(funcs)))
    for func in funcs:
        payload.extend(_write_varuint(func))
    return bytes(payload)


def _passive_segment(funcs: list[int]) -> bytes:
    payload = bytearray([0x01, 0x00])
    payload.extend(_write_varuint(len(funcs)))
    for func in funcs:
        payload.extend(_write_varuint(func))
    return bytes(payload)


def _declarative_segment(funcs: list[int]) -> bytes:
    payload = bytearray([0x03, 0x00])
    payload.extend(_write_varuint(len(funcs)))
    for func in funcs:
        payload.extend(_write_varuint(func))
    return bytes(payload)


def _module_with_elements(*segments: bytes) -> bytes:
    element_payload = bytearray()
    element_payload.extend(_write_varuint(len(segments)))
    for segment in segments:
        element_payload.extend(segment)
    return WASM_MAGIC + WASM_VERSION + _section(9, bytes(element_payload))


def _element_payload(data: bytes) -> bytes:
    for section_id, payload in _parse_sections(data):
        if section_id == 9:
            return payload
    raise AssertionError("element section missing")


def test_linked_table_cleanup_keeps_runtime_prefix_and_drops_app_active_segments() -> None:
    runtime_prefix = _active_segment(1, [10, 11])
    relocated_app_segment = _active_segment(-6384, [12])
    positive_app_segment = _active_segment(256, [13])
    passive_declaration = _passive_segment([14])
    declarative_segment = _declarative_segment([15])
    data = _module_with_elements(
        runtime_prefix,
        relocated_app_segment,
        passive_declaration,
        positive_app_segment,
        declarative_segment,
    )

    updated = wasm_link_edit._drop_linked_app_active_table_elements(data)

    assert updated is not None
    payload = _element_payload(updated)
    count, offset = _read_varuint(payload, 0)
    assert count == 3
    remaining = payload[offset:]
    assert runtime_prefix in remaining
    assert passive_declaration in remaining
    assert declarative_segment in remaining
    assert relocated_app_segment not in remaining
    assert positive_app_segment not in remaining


def test_linked_table_cleanup_is_noop_without_app_active_segments() -> None:
    data = _module_with_elements(_active_segment(1, [10]), _passive_segment([11]))

    assert wasm_link_edit._drop_linked_app_active_table_elements(data) is None
