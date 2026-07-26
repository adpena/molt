from __future__ import annotations

from pathlib import Path

from molt.cli.native_link_manifest import write_native_link_dependency_manifest


SOURCE_FINGERPRINT = {
    "hash": "1" * 64,
    "inputs_digest": "2" * 64,
    "meta_digest": "3" * 64,
    "rustc": "rustc test toolchain",
}


def static_archive_bytes(payload: bytes = b"object") -> bytes:
    name = b"object.o/".ljust(16)
    header = b"".join(
        (
            name,
            b"0".ljust(12),
            b"0".ljust(6),
            b"0".ljust(6),
            b"100644".ljust(8),
            str(len(payload)).encode("ascii").ljust(10),
            b"`\n",
        )
    )
    return b"!<arch>\n" + header + payload + (b"\n" if len(payload) & 1 else b"")


def write_test_static_archive(path: Path, payload: bytes = b"object") -> None:
    path.write_bytes(static_archive_bytes(payload))


def write_test_native_link_manifest(
    runtime_lib: Path,
    *,
    source_root: Path,
    target_triple: str | None = None,
    native_arguments: str = "-lc",
) -> None:
    """Attach the minimal strict manifest required by production link plans."""
    write_native_link_dependency_manifest(
        "",
        cargo_stderr=f"note: native-static-libs: {native_arguments}\n",
        runtime_lib=runtime_lib,
        cargo_profile=runtime_lib.parent.name,
        target_triple=target_triple,
        source_root=source_root,
        source_fingerprint=SOURCE_FINGERPRINT,
    )
