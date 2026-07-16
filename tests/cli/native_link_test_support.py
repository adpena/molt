from __future__ import annotations

from pathlib import Path

from molt.cli.native_link_manifest import write_native_link_dependency_manifest


SOURCE_FINGERPRINT = {
    "hash": "1" * 64,
    "inputs_digest": "2" * 64,
    "meta_digest": "3" * 64,
    "rustc": "rustc test toolchain",
}


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
