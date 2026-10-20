"""Stream executable IR without a whole-document transport buffer."""

from collections.abc import Mapping
from typing import Any, BinaryIO

import msgspec

from molt.cli.function_records import encode_function_body


def _container_header(count: int, *, array: bool) -> bytes:
    """MessagePack fix/16/32 framing; values use the existing C encoder."""
    if count < 16:
        return bytes([(0x90 if array else 0x80) | count])
    if count < 1 << 16:
        return bytes([0xDC if array else 0xDE]) + count.to_bytes(2, "big")
    return bytes([0xDD if array else 0xDF]) + count.to_bytes(4, "big")


def write_backend_ir(handle: BinaryIO, ir: Mapping[str, Any]) -> None:
    """Keep at most one top-level array element's encoded bytes resident.

    Function bodies use the same sorted, bit-preserving codec as persisted
    records. No rich-metadata projection or JSON value tree intervenes.
    """
    encode = msgspec.msgpack.Encoder(order="sorted").encode
    handle.write(_container_header(len(ir), array=False))
    for key, value in sorted(ir.items()):
        handle.write(encode(key))
        if isinstance(value, list):
            handle.write(_container_header(len(value), array=True))
            encode_element = encode_function_body if key == "functions" else encode
            for item in value:
                handle.write(encode_element(item))
        else:
            handle.write(encode(value))
