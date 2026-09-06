from __future__ import annotations

from pathlib import Path

from molt.cli.native_link_manifest import write_native_link_dependency_manifest
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.native_link_plan import _host_target_triple
from tests.runtime_build_identity_helper import native_runtime_staticlib_identity


RUNTIME_BUILD_IDENTITY = native_runtime_staticlib_identity(
    cargo_profile="dev-fast",
    target_triple=None,
    family_seed="native-link-test-family",
)


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
    build_identity: RuntimeBuildIdentity | None = None,
    target_triple: str | None = None,
    native_arguments: str = "-lc",
) -> RuntimeBuildIdentity:
    """Attach the minimal strict manifest required by production link plans."""
    if build_identity is None:
        build_identity = native_runtime_staticlib_identity(
            cargo_profile=runtime_lib.parent.name,
            target_triple=target_triple,
            family_seed="native-link-test-family",
            host_target=_host_target_triple(),
        )
    write_native_link_dependency_manifest(
        "",
        cargo_stderr=f"note: native-static-libs: {native_arguments}\n",
        runtime_lib=runtime_lib,
        cargo_profile=runtime_lib.parent.name,
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )
    return build_identity
