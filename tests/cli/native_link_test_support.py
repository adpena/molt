from __future__ import annotations

from pathlib import Path
from typing import Sequence

from molt.cli.native_symbol_inspection import (
    NativeSymbolInspectionError,
    NativeSymbolRequirement,
    _NativeGlobalSymbolFacts,
    _NativeSymbolReader,
)

from molt.cli.native_link_manifest import write_native_link_dependency_manifest
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.native_link_plan import _host_target_triple
from tests.runtime_build_identity_helper import native_runtime_staticlib_identity
from tests.native_artifact_fixtures import (
    NativeSymbolFixture,
    native_relocatable_object,
)


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


class NativeArchiveFixtureCatalog:
    """Pytest-owned expected reader results for independently emitted bytes.

    Only the external reader is replaced. Production artifact snapshots, reader
    identity, digest stamping and caches consume these results unchanged.
    """

    def __init__(self) -> None:
        self._descriptors: dict[bytes, NativeSymbolFixture] = {}

    def archive(
        self,
        symbols: NativeSymbolFixture,
        *,
        target_triple: str | None = None,
        revision: bytes = b"",
    ) -> bytes:
        payload = static_archive_bytes(
            native_relocatable_object(
                target_triple=target_triple,
                symbols=symbols.functions,
                data_symbols=symbols.data,
            )
            + revision
        )
        previous = self._descriptors.setdefault(payload, symbols)
        if previous != symbols:
            raise ValueError(
                "one native fixture image cannot assert two symbol descriptors"
            )
        return payload

    def read_symbols(
        self,
        path: Path,
        *,
        timeout: float,
        nm_command: Sequence[str] | None = None,
        target_triple: str | None = None,
        _reader: _NativeSymbolReader | None = None,
        requirement: NativeSymbolRequirement | None = None,
    ) -> _NativeGlobalSymbolFacts:
        del timeout, nm_command, target_triple
        descriptor = self._descriptors.get(path.read_bytes())
        if descriptor is None:
            raise NativeSymbolInspectionError(
                path, ["unregistered synthetic native artifact bytes"]
            )
        facts = _NativeGlobalSymbolFacts(
            defined=frozenset(descriptor.defined),
            undefined=frozenset(),
            defined_functions=frozenset(descriptor.functions),
        )
        selected = requirement or (
            _reader.requirement if _reader is not None else NativeSymbolRequirement()
        )
        if not selected.accepts(facts):
            raise NativeSymbolInspectionError(
                path, ["synthetic native artifact does not satisfy reader requirements"]
            )
        return facts


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
