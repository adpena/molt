"""Bounded WASM member projection over the shared static-archive authority."""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

from molt.cli.static_archive_identity import (
    _ARCHIVE_MAGIC as AR_MAGIC,
    _THIN_ARCHIVE_MAGIC as THIN_AR_MAGIC,
    open_static_archive_members,
)
from molt.toolchain_identity import open_stable_regular_file

WASM_HEADER = b"\0asm\x01\0\0\0"
_MAX_ARCHIVE_MEMBERS = 65_536
_MAX_WASM_OBJECT_BYTES = 256 * 1024 * 1024


@dataclass(frozen=True, slots=True)
class WasmArchiveMember:
    name: str
    data: bytes


def iter_wasm_archive_members(path: Path) -> Iterator[WasmArchiveMember]:
    """Project framed members without introducing another archive parser."""
    with open_static_archive_members(path) as (members, stream):
        if len(members) > _MAX_ARCHIVE_MEMBERS:
            raise ValueError("WASM archive member count exceeds the supported bound")
        for member in members:
            if member.size > _MAX_WASM_OBJECT_BYTES:
                raise ValueError(
                    f"WASM archive member exceeds the size bound: {member.name}"
                )
            stream.seek(member.content_offset)
            data = stream.read(member.size)
            if len(data) != member.size:
                raise ValueError(f"WASM archive member is truncated: {member.name}")
            if not data.startswith(WASM_HEADER):
                raise ValueError(
                    f"WASM archive contains a non-WASM object member: {member.name}"
                )
            yield WasmArchiveMember(member.name, data)


def iter_wasm_object_members(path: Path) -> Iterator[WasmArchiveMember]:
    """Yield a raw WASM object or members read through archive source custody."""
    with open_stable_regular_file(path, label="native WASM input") as opened:
        prefix = opened.stream.read(len(WASM_HEADER))
        if prefix == WASM_HEADER:
            size = opened.stat.st_size
            if size > _MAX_WASM_OBJECT_BYTES:
                raise ValueError(f"WASM object exceeds the size bound: {path}")
            opened.stream.seek(0)
            data = opened.stream.read(size)
            if len(data) != size:
                raise ValueError(f"WASM object is truncated: {path}")
            yield WasmArchiveMember(path.name, data)
            return
        if prefix == THIN_AR_MAGIC:
            raise ValueError("thin WASM archives are outside linker custody")
        if prefix != AR_MAGIC:
            raise ValueError(f"native linker input is neither WASM nor ar: {path}")
    yield from iter_wasm_archive_members(path)
