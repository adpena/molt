from __future__ import annotations


def _encode_u32(value: int) -> bytes:
    out = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        if remaining:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            break
    return bytes(out)


def _encode_str(text: str) -> bytes:
    data = text.encode("utf-8")
    return _encode_u32(len(data)) + data


def _section(section_id: int, payload: bytes) -> bytes:
    return bytes([section_id]) + _encode_u32(len(payload)) + payload


def build_wasm_tag_import_before_memory() -> bytes:
    """Minimal module with a tag import immediately before an env memory import."""
    magic = b"\x00asm"
    version = b"\x01\x00\x00\x00"

    func_type = bytes([0x60]) + _encode_u32(0) + _encode_u32(0)
    type_section = _section(1, _encode_u32(1) + func_type)

    imports = bytearray()
    imports += _encode_str("env")
    imports += _encode_str("molt_exception")
    imports.append(0x04)  # tag import
    imports += _encode_u32(0)  # exception attribute
    imports += _encode_u32(0)  # type index

    imports += _encode_str("env")
    imports += _encode_str("memory")
    imports.append(0x02)  # memory import
    imports.append(0x00)  # limits: min only
    imports += _encode_u32(1)

    import_section = _section(2, _encode_u32(2) + bytes(imports))
    return magic + version + type_section + import_section
