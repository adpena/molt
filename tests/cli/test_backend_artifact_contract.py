from __future__ import annotations

from contextlib import contextmanager
from dataclasses import replace
from typing import BinaryIO

import pytest

from molt.cli import backend_artifact_contract as contracts
from molt.cli import runtime_wasm_validation
from molt.cli import static_archive_identity as archive_identity
from molt.cli.native_link_plan import NativeArtifactKind, _host_target_triple
from molt.cli.static_archive_identity import (
    StaticArchiveMember,
    static_archive_identity,
    visit_static_archive_members,
)
from tests.cli.native_link_test_support import static_archive_bytes
from tests.native_artifact_fixtures import elf_header, native_relocatable_object


@pytest.mark.parametrize(
    "target,emit,kind,suffix",
    [
        ("x86_64-pc-windows-msvc", "obj", "NATIVE_OBJECT", ".obj"),
        ("x86_64-pc-windows-msvc", "bin", "NATIVE_ARCHIVE", ".lib"),
        ("x86_64-pc-windows-gnu", "bin", "NATIVE_ARCHIVE", ".lib"),
        ("x86_64-unknown-linux-gnu", "obj", "NATIVE_OBJECT", ".o"),
        ("aarch64-unknown-linux-gnu", "bin", "NATIVE_ARCHIVE", ".a"),
        ("aarch64-apple-darwin", "obj", "NATIVE_OBJECT", ".o"),
        ("aarch64-apple-darwin", "bin", "NATIVE_ARCHIVE", ".a"),
        ("wasm", "wasm", "WASM", ".wasm"),
        ("wasm-freestanding", "wasm", "WASM", ".wasm"),
        ("wasm32-wasip1", "wasm", "WASM", ".wasm"),
        ("rust", "bin", "RUST", ".rs"),
        ("luau", "bin", "LUAU", ".luau"),
        ("mlir", "bin", "MLIR", ".mlir"),
    ],
)
def test_contract_resolves_requested_kind_suffix_and_flags(target, emit, kind, suffix):
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode=emit
    )
    assert contract.kind is contracts.BackendArtifactKind[kind]
    assert contract.suffix == suffix
    assert sum((contract.is_native, contract.is_wasm, contract.is_text)) == 1
    assert contract.is_native == kind.startswith("NATIVE_")
    assert contract.is_wasm == (kind == "WASM")
    if contract.is_native:
        assert contract.native_kind is NativeArtifactKind.for_emit_mode(emit)
        assert contract.native_target is not None
    else:
        assert contract.native_kind is None
        assert contract.native_target is None


def test_cache_identity_distinguishes_all_requested_output_families_and_targets():
    requests = (
        ("native", "obj"),
        ("native", "bin"),
        ("rust", "bin"),
        ("luau", "bin"),
        ("mlir", "bin"),
        ("wasm", "wasm"),
        ("wasm-freestanding", "wasm"),
    )
    identities = {
        contracts.resolve_backend_artifact_contract(
            target=target, emit_mode=emit
        ).cache_identity
        for target, emit in requests
    }
    assert len(identities) == len(requests)
    assert all(
        len(value) == 64 and set(value) <= set("0123456789abcdef")
        for value in identities
    )
    host = contracts.resolve_backend_artifact_contract(target="native", emit_mode="bin")
    explicit = contracts.resolve_backend_artifact_contract(
        target="native", emit_mode="bin", target_triple=_host_target_triple()
    )
    assert host.cache_identity == explicit.cache_identity
    normalized = contracts.resolve_backend_artifact_contract(
        target=" NATIVE ",
        emit_mode="bin",
        target_triple=" " + _host_target_triple().upper() + " ",
    )
    assert normalized.cache_identity == explicit.cache_identity
    linux = contracts.resolve_backend_artifact_contract(
        target="native", emit_mode="bin", target_triple="x86_64-unknown-linux-gnu"
    )
    windows = contracts.resolve_backend_artifact_contract(
        target="native", emit_mode="bin", target_triple="x86_64-pc-windows-msvc"
    )
    assert linux.cache_identity != windows.cache_identity


@pytest.mark.parametrize(
    "target,emit,triple",
    [
        ("native", "wasm", None),
        ("wasm", "bin", None),
        ("wasm", "obj", None),
        ("rust", "obj", None),
        ("luau", "wasm", None),
        ("mlir", "obj", None),
        ("native", "bin", ""),
        ("native", "obj", "wasm32-wasip1"),
        ("wasm", "wasm", "x86_64-pc-windows-msvc"),
        ("wasm32-wasip1", "wasm", "wasm32-unknown-unknown"),
        ("aarch64-apple-darwin", "bin", "x86_64-unknown-linux-gnu"),
        ("unrecognized-target", "bin", None),
    ],
)
def test_incompatible_request_is_rejected_before_any_file_read(target, emit, triple):
    with pytest.raises((ValueError, RuntimeError)):
        contracts.resolve_backend_artifact_contract(
            target=target, emit_mode=emit, target_triple=triple
        )


@pytest.mark.parametrize("kind", list(contracts.BackendArtifactKind))
def test_only_native_archive_can_have_shared_stdlib(kind):
    contract = contracts.BackendArtifactContract(kind)
    contract.validate_shared_stdlib(enabled=False)
    if kind is contracts.BackendArtifactKind.NATIVE_ARCHIVE:
        contract.validate_shared_stdlib(enabled=True)
    else:
        with pytest.raises(ValueError, match="Shared stdlib extraction"):
            contract.validate_shared_stdlib(enabled=True)


@pytest.mark.parametrize(
    "target",
    [
        "x86_64-pc-windows-msvc",
        "x86_64-pc-windows-gnu",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ],
)
@pytest.mark.parametrize("emit", ["obj", "bin"])
def test_native_admission_uses_requested_shape_not_path_suffix(tmp_path, target, emit):
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode=emit
    )
    artifact = tmp_path / "misleading.rs"
    # Empty symbol tables are legitimate object shape, not an inspection failure.
    object_bytes = native_relocatable_object(target_triple=target)
    artifact.write_bytes(
        object_bytes if emit == "obj" else static_archive_bytes(object_bytes)
    )
    contract.validate(artifact)
    artifact.write_bytes(
        static_archive_bytes(object_bytes) if emit == "obj" else object_bytes
    )
    with pytest.raises(contracts.BackendArtifactValidationError):
        contract.validate(artifact)


@pytest.mark.parametrize("emit", ["obj", "bin"])
@pytest.mark.parametrize(
    "invalid_kind", ["wrong_arch", "wrong_format", "image", "truncated"]
)
def test_native_admission_rejects_every_incompatible_member(
    tmp_path, emit, invalid_kind
):
    target = "x86_64-unknown-linux-gnu"
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode=emit
    )
    invalid = {
        "wrong_arch": native_relocatable_object(
            target_triple="aarch64-unknown-linux-gnu"
        ),
        "wrong_format": native_relocatable_object(
            target_triple="x86_64-pc-windows-msvc"
        ),
        "image": bytes(elf_header(machine=62, kind=2)),
        "truncated": b"\x7fELF",
    }[invalid_kind]
    path = tmp_path / "artifact"
    if emit == "bin":
        good = static_archive_bytes(native_relocatable_object(target_triple=target))
        path.write_bytes(good + static_archive_bytes(invalid)[8:])
    else:
        path.write_bytes(invalid)
    with pytest.raises(contracts.BackendArtifactValidationError) as caught:
        contract.validate(path)
    if emit == "bin":
        assert "archive member" in str(caught.value)


@pytest.mark.parametrize(
    "data", [b"!<arch>\n", b"!<thin>\n", static_archive_bytes(b"!<arch>\n")]
)
def test_backend_archive_requires_self_contained_relocatable_members(tmp_path, data):
    contract = contracts.resolve_backend_artifact_contract(
        target="native", emit_mode="bin"
    )
    path = tmp_path / "application.a"
    path.write_bytes(data)
    with pytest.raises(contracts.BackendArtifactValidationError):
        contract.validate(path)


def _member(name: str, payload: bytes) -> bytes:
    # Reuse the archive fixture's framing/padding rather than another ar writer.
    member = bytearray(static_archive_bytes(payload)[8:])
    member[:16] = name.encode("ascii").ljust(16)
    return bytes(member)


@pytest.mark.parametrize("style", ["gnu", "coff", "bsd"])
def test_shared_archive_visitor_resolves_metadata_and_exact_payload_extent(
    tmp_path, style
):
    target = "x86_64-unknown-linux-gnu"
    payload = native_relocatable_object(target_triple=target, symbols=("molt_main",))
    name = "long_relocatable_object_member.o"
    if style == "bsd":
        parts = [
            _member("__.SYMDEF/", b"index metadata, not an object"),
            _member(f"#1/{len(name)}", name.encode("ascii") + payload),
        ]
    else:
        terminator = b"/\n" if style == "gnu" else b"\0"
        parts = [
            _member("/", b"index metadata, not an object"),
            _member("//", name.encode("ascii") + terminator),
            _member("/0", payload),
        ]
    path = tmp_path / "archive.bin"
    path.write_bytes(b"!<arch>\n" + b"".join(parts))
    observed = []

    def visit(member: StaticArchiveMember, stream: BinaryIO) -> None:
        stream.seek(member.content_offset)
        observed.append((member.name, member.size, stream.read(member.size)))

    plain_identity = static_archive_identity(path)
    visited_identity = static_archive_identity(path, visit_member=visit)
    assert visited_identity == plain_identity
    assert observed == [(name, len(payload), payload)]
    observed.clear()
    assert visit_static_archive_members(path, visit_member=visit) == 1
    assert observed == [(name, len(payload), payload)]
    contracts.resolve_backend_artifact_contract(
        target=target, emit_mode="bin"
    ).validate(path)


@pytest.mark.parametrize("target", ["rust", "luau", "mlir"])
def test_text_admission_is_incremental_utf8_and_never_native(
    tmp_path, monkeypatch, target
):
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode="bin"
    )

    def native_reader_forbidden(*args, **kwargs):
        pytest.fail("text output must not invoke native admission")

    monkeypatch.setattr(
        contracts.BackendArtifactContract,
        "validate_native_shape",
        native_reader_forbidden,
    )
    path = tmp_path / "text_with_native_suffix.a"
    path.write_bytes(b" " * 65535 + "π\n".encode("utf-8"))
    contract.validate(path)


@pytest.mark.parametrize("target", ["rust", "luau", "mlir"])
@pytest.mark.parametrize(
    "data", [b"", b" \r\n\t", b"\xff", b"text\xe2\x82", b"text\0tail"]
)
def test_text_admission_rejects_empty_binary_or_truncated_utf8(tmp_path, target, data):
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode="bin"
    )
    path = tmp_path / "output"
    path.write_bytes(data)
    with pytest.raises(contracts.BackendArtifactValidationError):
        contract.validate(path)


@pytest.mark.parametrize("target", ["wasm", "wasm-freestanding"])
@pytest.mark.parametrize(
    "validation_error", [None, "WASM structural validator unavailable"]
)
def test_wasm_contract_uses_existing_structural_validation(
    tmp_path, monkeypatch, target, validation_error
):
    seen = []
    path = tmp_path / "wasm_with_misleading.rs"
    path.write_bytes(b"\0asm\x01\0\0\0")

    def validate(candidate):
        seen.append(candidate)
        return validation_error

    monkeypatch.setattr(
        runtime_wasm_validation, "_reusable_wasm_artifact_validation_error", validate
    )
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode="wasm"
    )
    if validation_error is None:
        contract.validate(path)
    else:
        with pytest.raises(
            contracts.BackendArtifactValidationError, match=validation_error
        ):
            contract.validate(path)
    assert seen == [path]


@pytest.mark.parametrize(
    "target,emit", [("native", "obj"), ("native", "bin"), ("rust", "bin")]
)
def test_missing_output_fails_with_artifact_context(tmp_path, target, emit):
    contract = contracts.resolve_backend_artifact_contract(
        target=target, emit_mode=emit
    )
    path = tmp_path / "missing"
    with pytest.raises(contracts.BackendArtifactValidationError, match="missing"):
        contract.validate(path)


def test_archive_shape_skips_payload_hash_and_reads_only_required_header_bytes(
    tmp_path, monkeypatch
):
    target = "x86_64-unknown-linux-gnu"
    payload = native_relocatable_object(
        target_triple=target, symbols=("molt_main",)
    ) + b"\0" * (1024 * 1024)
    path = tmp_path / "application.a"
    path.write_bytes(static_archive_bytes(payload))
    real_open = archive_identity.open_stable_regular_file
    real_hash = archive_identity._hash_exact
    streams = []
    hashed_sizes = []

    class ReadMeter:
        def __init__(self, stream):
            self.stream = stream
            self.bytes_read = 0

        def read(self, size=-1):
            data = self.stream.read(size)
            self.bytes_read += len(data)
            return data

        def tell(self):
            return self.stream.tell()

        def seek(self, offset, whence=0):
            return self.stream.seek(offset, whence)

    @contextmanager
    def metered_open(candidate, *, label):
        with real_open(candidate, label=label) as opened:
            meter = ReadMeter(opened.stream)
            streams.append(meter)
            yield replace(opened, stream=meter)

    def metered_hash(stream, size):
        hashed_sizes.append(size)
        return real_hash(stream, size)

    monkeypatch.setattr(archive_identity, "open_stable_regular_file", metered_open)
    monkeypatch.setattr(archive_identity, "_hash_exact", metered_hash)
    contracts.resolve_backend_artifact_contract(
        target=target, emit_mode="bin"
    ).validate(path)
    assert hashed_sizes == []
    assert len(streams) == 1
    assert 0 < streams[0].bytes_read < 4096
    identity = static_archive_identity(path)
    assert identity["member_count"] == 1
    assert identity["content_size_bytes"] == len(payload)
    assert hashed_sizes == [len(payload)]
    assert streams[1].bytes_read == len(path.read_bytes())
