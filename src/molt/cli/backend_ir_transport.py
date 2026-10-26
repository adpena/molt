"""Content-addressed backend IR leases (backend daemon protocol 5, HF-516).

A lease is one MessagePack map with lexically sorted keys: the program header
(``code_slots``, ``module_registry``, ``profile``), ``function_digests`` (one
``bin`` of 32-byte body digests in program order) and ``function_bodies`` (one
``bin`` per shipped body, its exact sorted MessagePack bytes). The receiver
hashes each body, so a body can only fill the slot its own bytes name. A daemon
already holds most bodies; the CLI ships the ones it reports missing. One-shot
backends, the fact graph and the JIT image receive every body. The Rust reader
is ``runtime/molt-ir/src/addressed_program.rs``.
"""

from __future__ import annotations

from collections.abc import Collection, Mapping, Sequence
from typing import TYPE_CHECKING, Any, Protocol

import msgspec

from molt.cli.function_records import encode_function_body

if TYPE_CHECKING:
    from molt.cli.cache_keys import FunctionDigests

# The header fields the backend admits; the lease carries no other key.
_LEASE_HEADER_KEYS = ("code_slots", "module_registry", "profile")


def _container_header(count: int, *, array: bool) -> bytes:
    """MessagePack fix/16/32 framing; values use the existing C encoder."""
    if count < 16:
        return bytes([(0x90 if array else 0x80) | count])
    if count < 1 << 16:
        return bytes([0xDC if array else 0xDE]) + count.to_bytes(2, "big")
    return bytes([0xDD if array else 0xDF]) + count.to_bytes(4, "big")


def _bin_header(length: int) -> bytes:
    """MessagePack bin 8/16/32 framing."""
    if length < 1 << 8:
        return bytes([0xC4, length])
    if length < 1 << 16:
        return b"\xc5" + length.to_bytes(2, "big")
    return b"\xc6" + length.to_bytes(4, "big")


class ByteSink(Protocol):
    """Anything the transport can stream encoded bytes into."""

    def write(self, data: bytes, /) -> object: ...


class BackendIrProgram:
    """One backend IR program: its header and each function's body digest.

    The digests come from the one ``FunctionDigests`` owner, so seeded bodies
    cost no encode. They are read on the first transport use, so a build that
    hits an artifact cache reads none. The functions must stay unchanged while
    this exists.
    """

    __slots__ = ("_digests", "_function_digests", "_ir")

    def __init__(self, ir: Mapping[str, Any], function_digests: FunctionDigests):
        self._ir = ir
        self._function_digests = function_digests
        self._digests: tuple[bytes, ...] | None = None

    @property
    def _functions(self) -> list[dict[str, Any]]:
        functions = self._ir.get("functions")
        if not isinstance(functions, list):
            raise TypeError("backend IR requires a functions list")
        return functions

    @property
    def digests(self) -> tuple[bytes, ...]:
        """Each function's body digest, in program order."""
        if self._digests is None:
            digest = self._function_digests.digest
            self._digests = tuple(digest(function) for function in self._functions)
        return self._digests

    def __len__(self) -> int:
        return len(self.digests)

    def digest_hex(self) -> str:
        """The program's digests as one lowercase hex string, in order."""
        return b"".join(self.digests).hex()

    def write(self, handle: ByteSink, shipped: Collection[int] | None = None) -> None:
        """Stream the lease, shipping the bodies at ``shipped`` (every body when None).

        At most one encoded body is resident at a time.
        """
        positions: Sequence[int] = (
            range(len(self.digests)) if shipped is None else sorted(set(shipped))
        )
        if positions and not 0 <= positions[0] <= positions[-1] < len(self.digests):
            raise ValueError("shipped function position outside the program")
        encode = msgspec.msgpack.Encoder(order="sorted").encode
        functions = self._functions
        header = sorted(
            (key, self._ir[key]) for key in _LEASE_HEADER_KEYS if key in self._ir
        )
        handle.write(_container_header(len(header) + 2, array=False))
        fields = [
            *header,
            ("function_bodies", None),
            ("function_digests", None),
        ]
        for key, value in sorted(fields, key=lambda field: field[0]):
            handle.write(encode(key))
            if key == "function_bodies":
                handle.write(_container_header(len(positions), array=True))
                for position in positions:
                    body = encode_function_body(functions[position])
                    handle.write(_bin_header(len(body)))
                    handle.write(body)
            elif key == "function_digests":
                digests = b"".join(self.digests)
                handle.write(_bin_header(len(digests)))
                handle.write(digests)
            else:
                handle.write(encode(value))
