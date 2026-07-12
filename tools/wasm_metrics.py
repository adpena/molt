"""Machine-readable WebAssembly section and data-segment metrics."""

from __future__ import annotations

import hashlib
from pathlib import Path

SECTION_NAMES = {
    0: "custom", 1: "type", 2: "import", 3: "function", 4: "table",
    5: "memory", 6: "global", 7: "export", 8: "start", 9: "element",
    10: "code", 11: "data", 12: "data_count", 13: "tag",
}


def _read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("unexpected EOF while reading varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 63:
            raise ValueError("varuint too large")


def _skip_const_expr(data: bytes, offset: int) -> int:
    while True:
        if offset >= len(data):
            raise ValueError("unexpected EOF while reading const expression")
        opcode = data[offset]
        offset += 1
        if opcode == 0x0B:
            return offset
        if opcode in {0x41, 0x42, 0x23, 0xD2}:
            _, offset = _read_varuint(data, offset)
        elif opcode == 0x43:
            offset += 4
        elif opcode == 0x44:
            offset += 8
        elif opcode == 0xD0:
            offset += 1
        else:
            raise ValueError(f"unsupported data-segment const opcode 0x{opcode:02x}")


def _data_segment_metrics(payload: bytes) -> dict[str, int]:
    count, offset = _read_varuint(payload, 0)
    metrics = {"count": count, "active_count": 0, "passive_count": 0,
               "payload_bytes": 0, "zero_bytes": 0}
    for _ in range(count):
        flags, offset = _read_varuint(payload, offset)
        if flags == 0:
            metrics["active_count"] += 1
            offset = _skip_const_expr(payload, offset)
        elif flags == 1:
            metrics["passive_count"] += 1
        elif flags == 2:
            metrics["active_count"] += 1
            _, offset = _read_varuint(payload, offset)
            offset = _skip_const_expr(payload, offset)
        else:
            raise ValueError(f"unsupported data segment flags {flags}")
        size, offset = _read_varuint(payload, offset)
        end = offset + size
        if end > len(payload):
            raise ValueError("unexpected EOF while reading data segment")
        body = payload[offset:end]
        metrics["payload_bytes"] += size
        metrics["zero_bytes"] += body.count(0)
        offset = end
    if offset != len(payload):
        raise ValueError("trailing bytes in data section")
    return metrics


def wasm_metrics(source: bytes | Path) -> dict[str, object]:
    data = source.read_bytes() if isinstance(source, Path) else source
    if len(data) < 8 or data[:8] != b"\0asm\x01\0\0\0":
        raise ValueError("invalid WebAssembly header")
    offset = 8
    sections: dict[str, int] = {}
    data_segments = {"count": 0, "active_count": 0, "passive_count": 0,
                     "payload_bytes": 0, "zero_bytes": 0}
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_varuint(data, offset)
        section_end = offset + section_size
        if section_end > len(data):
            raise ValueError("unexpected EOF while reading section")
        name = SECTION_NAMES.get(section_id, f"unknown({section_id})")
        sections[name] = sections.get(name, 0) + section_size
        if section_id == 11:
            current = _data_segment_metrics(data[offset:section_end])
            for key, value in current.items():
                data_segments[key] += value
        offset = section_end
    return {
        "file_bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "sections": dict(sorted(sections.items())),
        "data_segments": data_segments,
    }
