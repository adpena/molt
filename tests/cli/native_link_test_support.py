from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path
from contextlib import contextmanager
import sys
import hashlib
from typing import Sequence

from molt.cli import native_symbol_inspection
from molt.cli.native_symbol_inspection import (
    NativeSymbolInspectionError,
    NativeSymbolRequirement,
    _NativeArchiveMemberSymbolFacts,
    _NativeGlobalSymbolFacts,
    _NativeSymbolReader,
    _NativeSymbolReaderCandidate,
)
from molt.toolchain_identity import (
    StableRegularFileHandle,
    stable_regular_file_identity,
)
from molt.cli.static_archive_identity import (
    StaticArchiveMember,
    StaticArchiveMemberIdentity,
)

from molt.cli.native_link_manifest import write_native_link_dependency_manifest
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.cli.native_link_plan import _host_target_triple
from tests.runtime_build_identity_helper import native_runtime_staticlib_identity
from tests.native_artifact_fixtures import (
    NativeSymbolFixture,
    native_relocatable_object,
)


@contextmanager
def mock_symbol_reader_admission(monkeypatch, facts_cache: Path) -> Iterator[None]:
    """Admit every bitcode-reader command under the interpreter's identity.

    Only LLVM bitcode reaches an external reader. Tests that drive that ladder
    fabricate its llvm-nm output, so each command is admitted as llvm-nm.

    The facts a test reads may be synthetic, so the persistent symbol-facts
    cache lives in ``facts_cache`` for the test: synthetic facts must never
    reach a developer or CI MOLT_CACHE, where a later test with the same
    archive bytes and reader identity would hit them instead of running its
    own reader.
    """
    identity = stable_regular_file_identity(
        Path(sys.executable).resolve(strict=True), label="test symbol reader"
    )

    @contextmanager
    def admitted_reader(path, *, label, identity=None):
        del label
        assert identity is not None
        yield path, identity

    monkeypatch.setattr(
        native_symbol_inspection,
        "_native_symbol_reader_candidate",
        lambda command: _NativeSymbolReaderCandidate(
            tuple(command), executable_identity=identity
        ),
    )
    monkeypatch.setattr(
        native_symbol_inspection, "stable_executable_probe", admitted_reader
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: facts_cache
    )
    # Within a test the production content caches stay live across siblings.
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    try:
        yield
    finally:
        native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
        native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()


def stub_native_symbol_admission(monkeypatch, read_facts) -> None:
    """Synthetic symbol tables for projection tests, retaining real file custody.

    These tests prove the callable projection protocol. Native symbol/cache
    admission has its own suite; this fixture makes no nm or execution claim.
    """
    from molt.cli import native_symbol_inspection as symbols

    @contextmanager
    def admit(path, *, archive, identity, target_triple, requirement):
        assert archive
        with symbols._open_native_symbol_artifact(path, identity) as (opened, current):
            facts = read_facts(
                path,
                identity=current,
                target_triple=target_triple,
                requirement=requirement,
            )
            yield opened, current, facts

    monkeypatch.setattr(symbols, "_native_symbol_facts_admission", admit)


RUNTIME_BUILD_IDENTITY = native_runtime_staticlib_identity(
    cargo_profile="dev-fast",
    target_triple=None,
    family_seed="native-link-test-family",
)


# Raw LLVM bitcode magic. Only bitcode reaches the llvm-nm ladder, so tests of
# that ladder frame this stand-in and fabricate the reader's output.
LLVM_BITCODE_STAND_IN = b"BC\xc0\xde" + b"\x35\x14\x00\x00" * 4


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


def single_member_archive_symbol_facts(
    archive_members: tuple[StaticArchiveMemberIdentity, ...],
    object_facts: _NativeGlobalSymbolFacts,
) -> _NativeGlobalSymbolFacts:
    if len(archive_members) != 1:
        raise ValueError("synthetic archive must contain exactly one member")
    if object_facts.members is not None:
        raise ValueError("synthetic archive member facts must describe an object")
    return _NativeGlobalSymbolFacts(
        defined=frozenset(),
        undefined=frozenset(),
        defined_functions=frozenset(),
        members=(_NativeArchiveMemberSymbolFacts(archive_members[0], object_facts),),
    )


class NativeArchiveFixtureCatalog:
    """Pytest-owned expected reader results for independently emitted bytes.

    Only the symbol-table read is replaced. Production artifact snapshots,
    reader identity, digest stamping and caches consume these results unchanged.
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
        nm_command: Sequence[str] | None = None,
        target_triple: str | None = None,
        _reader: _NativeSymbolReader | None = None,
        requirement: NativeSymbolRequirement | None = None,
        archive_members: tuple[StaticArchiveMemberIdentity, ...] | None = None,
        _opened: StableRegularFileHandle | None = None,
    ) -> _NativeGlobalSymbolFacts:
        del nm_command, target_triple
        if _opened is None or _opened.stream.closed:
            raise NativeSymbolInspectionError(
                path, ["synthetic native reader requires a live owned descriptor"]
            )
        if _opened.path != path.expanduser().absolute():
            raise NativeSymbolInspectionError(
                path, ["synthetic native reader descriptor belongs to another path"]
            )
        position = _opened.stream.tell()
        try:
            _opened.stream.seek(0)
            payload = _opened.stream.read()
        finally:
            _opened.stream.seek(position)
        if len(payload) != _opened.stat.st_size:
            raise NativeSymbolInspectionError(
                path, ["synthetic native reader descriptor size changed"]
            )
        descriptor = self._descriptors.get(payload)
        if descriptor is None:
            raise NativeSymbolInspectionError(
                path, ["unregistered synthetic native artifact bytes"]
            )
        if archive_members is None:
            raise NativeSymbolInspectionError(
                path, ["synthetic native archive was read without member identities"]
            )
        # This fixture emits exactly one ordinary ar member. Check the reader's
        # input against those bytes, independently of the production ar parser.
        size = int(payload[56:66])
        expected_members = (
            StaticArchiveMemberIdentity(
                ordinal=0,
                member=StaticArchiveMember("object.o", 68, size),
                sha256=hashlib.sha256(payload[68 : 68 + size]).hexdigest(),
            ),
        )
        if archive_members != expected_members:
            raise NativeSymbolInspectionError(
                path,
                ["synthetic native reader member identities differ from owned bytes"],
            )
        object_facts = _NativeGlobalSymbolFacts(
            defined=frozenset(descriptor.defined),
            undefined=frozenset(),
            defined_functions=frozenset(descriptor.functions),
        )
        facts = single_member_archive_symbol_facts(archive_members, object_facts)
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


def native_codegen_binding(runtime_lib: Path, build_identity: RuntimeBuildIdentity):
    """Bind real fixture file generations for tests unrelated to symbol reading."""
    from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
    from molt.cli.runtime_callable_symbols import _runtime_callable_symbols_digest
    from molt.toolchain_identity import stable_regular_file_identity

    symbols = runtime_lib.with_name(runtime_lib.name + ".test-callables")
    if not symbols.exists():
        symbols.write_text("molt_test_intrinsic\n", encoding="utf-8")
    # Like the production producer (native_symbol_inspection), the archive
    # identity names the resolved generation; a symlinked fixture root (macOS
    # /tmp or /var) otherwise fails NativeRuntimeCodegenBinding.verify().
    return NativeRuntimeCodegenBinding(
        runtime_lib=runtime_lib,
        build_identity=build_identity,
        archive=stable_regular_file_identity(
            runtime_lib.resolve(strict=True), label="test codegen archive"
        ),
        callable_symbols=stable_regular_file_identity(
            symbols, label="test codegen symbols"
        ),
        semantic_digest=_runtime_callable_symbols_digest(
            tuple(sorted(set(symbols.read_text(encoding="utf-8").splitlines())))
        ),
    )


def transport_codegen_binding(root: Path, *, target_triple: str | None = None):
    """Real file generations for transport tests, without compiling a runtime."""
    root.mkdir(parents=True, exist_ok=True)
    archive = root / "transport-runtime.a"
    write_test_static_archive(archive)
    identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="transport-runtime",
        host_target=_host_target_triple(),
    )
    return native_codegen_binding(archive, identity)
