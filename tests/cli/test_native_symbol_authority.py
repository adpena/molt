"""Symbol reader, bitcode-reader protocol and admission tests.

Native objects carry real symbol tables from an independent fixture writer and
go through the in-process reader. Only LLVM bitcode reaches the llvm-nm
ladder; those tests fabricate its output, without invoking a compiler or nm.
"""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
import subprocess
import sys
from dataclasses import replace

from molt.cli import native_symbol_inspection
from molt.cli.static_archive_identity import (
    StaticArchiveMember,
    StaticArchiveMemberIdentity,
    static_archive_member_identities,
)
import pytest
from tools.command_execution import CommandExecutor

from molt.cli import backend_cache as cache
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
    BackendArtifactValidationError,
)
from tests.cli.native_link_test_support import (
    LLVM_BITCODE_STAND_IN,
    mock_symbol_reader_admission,
    static_archive_bytes,
)
from tests.llvm_sdk_test_support import verified_llvm_tools
from tests.native_artifact_fixtures import native_relocatable_object
from molt.native_symbol_table import NativeSymbolRow


_COMMANDS = CommandExecutor.for_file(__file__)


@pytest.mark.parametrize("archive", [False, True])
@pytest.mark.parametrize("tier", ["cold", "memory", "persistent"])
def test_native_content_admission_rejects_old_digest_with_current_metadata(
    tmp_path, monkeypatch, archive, tier
):
    artifact = tmp_path / "input.a"
    artifact.write_bytes(_archive("before"))
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    read = (
        native_symbol_inspection._native_archive_global_symbol_facts
        if archive
        else native_symbol_inspection._native_object_global_symbol_facts
    )
    old = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    if tier != "cold":
        assert read(artifact, identity=old).defined == {"before"}
    if tier == "persistent":
        native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
        native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    stamp = artifact.stat()
    artifact.write_bytes(_archive("afterx"))
    os.utime(artifact, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    current = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    supplied = replace(current, sha256=old.sha256)
    monkeypatch.setattr(
        native_symbol_inspection,
        "read_symbol_rows",
        lambda *args, **kwargs: pytest.fail("unadmitted content reached the reader"),
    )
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="content changed"
    ):
        read(artifact, identity=supplied)


@pytest.mark.parametrize("archive", [False, True])
def test_backend_supplied_digest_is_checked_before_native_shape(
    tmp_path, monkeypatch, archive
):
    target = "x86_64-unknown-linux-gnu"
    artifact = tmp_path / "application.a"
    before = native_relocatable_object(target_triple=target, symbols=("before",))
    after = native_relocatable_object(target_triple=target, symbols=("afterx",))
    if archive:
        before, after = static_archive_bytes(before), static_archive_bytes(after)
    assert len(before) == len(after)
    artifact.write_bytes(before)
    old = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    stamp = artifact.stat()
    artifact.write_bytes(after)
    os.utime(artifact, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    current = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    monkeypatch.setattr(
        BackendArtifactContract,
        "validate_native_shape",
        lambda *args, **kwargs: pytest.fail("old digest admitted new shape bytes"),
    )
    contract = BackendArtifactContract(
        BackendArtifactKind.NATIVE_ARCHIVE
        if archive
        else BackendArtifactKind.NATIVE_OBJECT,
        target,
    )
    with pytest.raises(BackendArtifactValidationError, match="content changed"):
        cache._validate_backend_cache_artifact(
            artifact,
            artifact_contract=contract,
            identity=replace(current, sha256=old.sha256),
        )


@pytest.mark.parametrize("archive", [False, True])
def test_native_shape_members_and_symbol_reader_share_one_owned_artifact_handle(
    tmp_path, monkeypatch, archive
):
    from molt.cli import backend_artifact_contract, static_archive_identity

    target = "x86_64-unknown-linux-gnu"
    artifact = tmp_path / "application.a"
    payload = native_relocatable_object(target_triple=target, symbols=("application",))
    if archive:
        payload = static_archive_bytes(payload)
    artifact.write_bytes(payload)
    opened = []
    original = native_symbol_inspection.open_stable_regular_file

    @contextmanager
    def own(path, **kwargs):
        with original(path, **kwargs) as handle:
            opened.append(handle)
            yield handle
        assert handle.stream.closed

    monkeypatch.setattr(native_symbol_inspection, "open_stable_regular_file", own)
    monkeypatch.setattr(backend_artifact_contract, "open_stable_regular_file", own)
    monkeypatch.setattr(static_archive_identity, "open_stable_regular_file", own)
    original_read = native_symbol_inspection.read_symbol_rows
    reads = []

    def read(reader, **kwargs):
        assert len(opened) == 1 and not opened[0].stream.closed
        assert opened[0].path == artifact
        reads.append(reader.size)
        return original_read(reader, **kwargs)

    monkeypatch.setattr(native_symbol_inspection, "read_symbol_rows", read)
    contract = BackendArtifactContract(
        BackendArtifactKind.NATIVE_ARCHIVE
        if archive
        else BackendArtifactKind.NATIVE_OBJECT,
        target,
    )
    identity = cache._validate_backend_cache_artifact(
        artifact, artifact_contract=contract
    )
    assert len(opened) == 1 and opened[0].stream.closed
    assert len(reads) == 1
    assert identity.sha256 == hashlib.sha256(payload).hexdigest()


def test_projection_checks_reject_old_digest_with_current_metadata(
    tmp_path, monkeypatch
):
    from molt.cli import runtime_callable_symbols

    artifact = tmp_path / "runtime.a"
    artifact.write_bytes(static_archive_bytes(b"before"))
    old = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    symbols = ("molt_example",)
    content = runtime_callable_symbols._runtime_callable_projection_content(symbols)
    digest = hashlib.sha256(content).hexdigest()
    projection = artifact.with_name(
        runtime_callable_symbols._runtime_callable_projection_name(
            artifact.name, archive_sha256=old.sha256, projection_sha256=digest
        )
    )
    projection.write_bytes(content)
    artifact.write_bytes(static_archive_bytes(b"after!"))
    current = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    supplied = replace(current, sha256=old.sha256)
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="content changed"
    ):
        runtime_callable_symbols._admit_runtime_callable_projection(
            projection,
            runtime_lib=artifact,
            archive_identity=supplied,
            expected_sha256=digest,
        )


def test_callable_projection_reuses_one_owned_archive_admission_per_call(
    tmp_path, monkeypatch
):
    from molt.cli import runtime_callable_symbols

    artifact = tmp_path / "runtime.a"
    artifact.write_bytes(_archive("molt_fixture"))
    identity = native_symbol_inspection._native_symbol_artifact_identity(artifact)
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    captured = []
    original_hash = native_symbol_inspection.stable_regular_file_handle_identity
    original_read = native_symbol_inspection.read_symbol_rows
    inspected = []

    def capture(opened, **kwargs):
        captured.append(opened.path)
        return original_hash(opened, **kwargs)

    def read(reader, **kwargs):
        inspected.append(reader.size)
        return original_read(reader, **kwargs)

    monkeypatch.setattr(
        native_symbol_inspection, "stable_regular_file_handle_identity", capture
    )
    monkeypatch.setattr(native_symbol_inspection, "read_symbol_rows", read)
    for _ in range(2):
        captured.clear()
        projection, failure = runtime_callable_symbols._runtime_callable_symbols_file(
            artifact, identity=identity
        )
        assert failure is None and projection is not None
        assert captured == [artifact]
        assert projection.identity.path.read_bytes() == b"molt_fixture\n"
    assert len(inspected) == 1


@pytest.fixture(autouse=True)
def isolated_symbol_cache(
    monkeypatch: pytest.MonkeyPatch, request, tmp_path_factory
) -> Iterator[None]:
    if request.node.get_closest_marker("slow") is not None:
        yield
        return
    with mock_symbol_reader_admission(
        monkeypatch, tmp_path_factory.mktemp("symbol-facts")
    ):
        yield


# The fabricated llvm-nm output owns the symbol evidence for this stand-in.
_BITCODE = LLVM_BITCODE_STAND_IN


def _archive(*functions: str, **kwargs) -> bytes:
    """One real relocatable object with these functions, framed as an archive."""
    return static_archive_bytes(native_relocatable_object(symbols=functions, **kwargs))


def _bitcode_tool(monkeypatch, *, code=0, stdout="", stderr=""):
    """Fabricate the llvm-nm run that only LLVM bitcode reaches."""
    monkeypatch.setattr(
        native_symbol_inspection, "_nm_candidate_binaries", lambda: ["llvm-nm"]
    )

    def run(argv, **kwargs):
        assert kwargs["errors"] == "strict"
        assert argv[1:-1] == ["-g"]
        return subprocess.CompletedProcess(argv, code, stdout, stderr)

    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", run)


@pytest.mark.parametrize(
    "code,stdout,stderr",
    [
        (1, "00000000 T present\n", "archive.a(member.o): no symbols\n"),
        (1, "00000000 T present\n", "archive.a:member.o: no symbols\n"),
        (1, "", "archive.a: no symbols\nllvm-nm: error: unreadable member\n"),
        (124, "", "archive.a: no symbols\n"),
        (0, "", "llvm-nm: error: failed to decode\n"),
        (0, "not a symbol row\n", ""),
        (0, "00000000 ? unknown\n", ""),
        (0, "0000 T present\n", "foreign.a:member.o: no symbols\n"),
        (0, "0000 T present\n", "archive.a.backup:member.o: no symbols\n"),
        (0, "0000 T present\n", "foreign-nm: archive.a:member.o: no symbols\n"),
        (0, "0000 T present\n", "archive.a:: no symbols\n"),
        (0, "0000 T present\n", "archive.a(): no symbols\n"),
        (0, "0000 T present\n", "archive.a: error: no symbols\n"),
        (0, "0000 T present\n", "archive.a:member.o: error: no symbols\n"),
        (
            0,
            "0000 T present\n",
            "archive.a:empty.o: no symbols\nllvm-nm: error: unreadable member\n",
        ),
    ],
)
def test_incomplete_or_malformed_tool_evidence_is_never_symbol_success(
    tmp_path, monkeypatch, code, stdout, stderr
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(_BITCODE)
    _bitcode_tool(monkeypatch, code=code, stdout=stdout, stderr=stderr)
    with pytest.raises(native_symbol_inspection.NativeSymbolInspectionError) as caught:
        native_symbol_inspection._read_native_global_symbol_facts(artifact)
    assert caught.value.path == artifact
    assert "llvm-nm" in str(caught.value)
    assert caught.value.attempts


@pytest.mark.parametrize(
    "code,stdout,stderr",
    [
        (0, "", ""),
        (1, "", "archive.a: no symbols\n"),
        (1, "", "llvm-nm: archive.a: no symbols\n"),
        (1, "", "llvm-nm: archive.a:empty.o: no symbols\n"),
    ],
)
def test_legitimate_empty_artifact_has_successful_empty_facts(
    tmp_path, monkeypatch, code, stdout, stderr
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(_BITCODE)
    _bitcode_tool(monkeypatch, code=code, stdout=stdout, stderr=stderr)
    facts = native_symbol_inspection._read_native_global_symbol_facts(artifact)
    assert facts == native_symbol_inspection._NativeGlobalSymbolFacts(
        frozenset(), frozenset(), frozenset()
    )


@pytest.mark.parametrize(
    "target",
    [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "wasm32-wasip1",
    ],
)
@pytest.mark.parametrize("archive", ["archive.a", "dir/archive.a", r"C:\Molt\libc.a"])
@pytest.mark.parametrize(
    "member",
    ["(empty.o)", ":empty.o", r":C:\build\empty.obj", ":dir with spaces/empty.o"],
)
@pytest.mark.parametrize("channel", ["stdout", "stderr"])
def test_successful_archive_can_contain_empty_members(
    monkeypatch, target, archive, member, channel
):
    path = Path(archive)
    diagnostic = f"llvm-nm: {path}{member}: no symbols\n"
    empty_name = member[1:-1] if member.startswith("(") else member[1:]
    members = (
        StaticArchiveMemberIdentity(
            0, StaticArchiveMember("member.o", 68, 1), "a" * 64
        ),
        StaticArchiveMemberIdentity(
            1, StaticArchiveMember(empty_name, 130, 1), "b" * 64
        ),
    )
    result = subprocess.CompletedProcess(
        ["llvm-nm", str(path)],
        0,
        stdout=f"member.o:\n00000000 T provider\n{empty_name}:\n"
        + (diagnostic if channel == "stdout" else ""),
        stderr=diagnostic if channel == "stderr" else "",
    )
    names = frozenset(item.member.name for item in members)
    tables = native_symbol_inspection._bind_nm_archive_tables(
        native_symbol_inspection._validated_nm_output(
            result, archive_member_names=names
        ),
        path=path,
        members=members,
    )
    facts = [
        native_symbol_inspection._parse_native_nm_global_symbol_facts(
            table, target_triple=target
        )
        for table in tables
    ]
    assert facts[0].defined == {"provider"}
    assert facts[1].defined == facts[1].undefined == frozenset()


@pytest.mark.parametrize(
    "reader",
    [
        native_symbol_inspection._native_object_global_symbol_facts,
        native_symbol_inspection._native_archive_global_symbol_facts,
    ],
)
def test_object_and_archive_caches_share_parsing_protocol_identity(
    tmp_path, monkeypatch, reader
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(_archive("before"))
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    assert reader(artifact).defined == {"before"}
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    # A different reader generation would read other facts from the same bytes.
    monkeypatch.setattr(
        native_symbol_inspection,
        "read_symbol_rows",
        lambda *args, **kwargs: (NativeSymbolRow("T", "after"),),
    )
    # Persistent facts remain reusable under the exact existing protocol.
    assert reader(artifact).defined == {"before"}
    monkeypatch.setattr(
        native_symbol_inspection, "_NATIVE_SYMBOL_FACTS_PROTOCOL", "test.next-protocol"
    )
    assert reader(artifact).defined == {"after"}


@pytest.mark.parametrize("change", ["environment", "metadata", "location"])
@pytest.mark.parametrize("persistent", [False, True])
@pytest.mark.parametrize("archive", [False, True])
def test_symbol_facts_reuse_exact_bytes_and_reader_across_incidental_changes(
    tmp_path, monkeypatch, change, persistent, archive
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(
        _archive("preserved")
        if archive
        else native_relocatable_object(symbols=("preserved",))
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    read = (
        native_symbol_inspection._native_archive_global_symbol_facts
        if archive
        else native_symbol_inspection._native_object_global_symbol_facts
    )
    assert (read(artifact) if archive else read(artifact, publish=True)).defined == {
        "preserved"
    }
    if persistent:
        native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
        native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    if change == "environment":
        monkeypatch.setenv(
            "PATH", os.environ.get("PATH", "") + os.pathsep + "unrelated"
        )
    elif change == "metadata":
        before = artifact.stat()
        os.utime(artifact, ns=(before.st_atime_ns, before.st_mtime_ns + 1000000000))
    else:
        other = tmp_path / "retained" / "archive.a"
        other.parent.mkdir()
        other.write_bytes(artifact.read_bytes())
        if persistent and not archive:
            sidecar = native_symbol_inspection._native_object_symbol_facts_sidecar_path
            sidecar(other).write_bytes(sidecar(artifact).read_bytes())
        artifact = other

    def unexpected_extraction(*args, **kwargs):
        raise AssertionError("unchanged admitted artifact/reader was re-extracted")

    monkeypatch.setattr(
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        unexpected_extraction,
    )
    assert read(artifact).defined == {"preserved"}


@pytest.mark.parametrize("archive", [False, True])
def test_symbol_fact_retention_bounds_persistent_and_memory_admission(
    tmp_path, monkeypatch, archive
):
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    read = (
        native_symbol_inspection._native_archive_global_symbol_facts
        if archive
        else native_symbol_inspection._native_object_global_symbol_facts
    )
    cache = (
        native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE
        if archive
        else native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT"
        if archive
        else "_NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT",
        2,
    )
    artifacts = []
    for index in range(4):
        path = tmp_path / f"{index}.a"
        path.write_bytes(
            _archive("preserved", f"input_{index}")
            if archive
            else native_relocatable_object(symbols=("preserved", f"input_{index}"))
        )
        artifacts.append(path)
        assert (
            "preserved" in (read(path) if archive else read(path, publish=True)).defined
        )
        assert len(cache) <= 2
    cache.clear()

    def unexpected_extraction(*args, **kwargs):
        raise AssertionError("persistent facts were unnecessarily re-extracted")

    monkeypatch.setattr(
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        unexpected_extraction,
    )
    # Persistent admissions obey the same bound; a memory hit retains its entry.
    for index in (0, 1, 0, 2, 3):
        assert read(artifacts[index]).defined == {"preserved", f"input_{index}"}
        assert len(cache) <= 2
        if index == 2:
            assert {key.artifact_digest for key in cache} == {
                hashlib.sha256(artifacts[i].read_bytes()).hexdigest() for i in (0, 2)
            }
    assert {key.artifact_digest for key in cache} == {
        hashlib.sha256(artifacts[i].read_bytes()).hexdigest() for i in (2, 3)
    }


def test_missing_tools_and_decode_failures_preserve_typed_diagnostics(
    tmp_path, monkeypatch
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(_BITCODE)
    monkeypatch.setattr(native_symbol_inspection, "_nm_candidate_binaries", lambda: [])
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="no llvm-nm candidate",
    ) as missing:
        native_symbol_inspection._read_native_global_symbol_facts(artifact)
    assert not isinstance(
        missing.value, native_symbol_inspection.NativeSymbolArtifactError
    )
    primary = UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid tool output")
    monkeypatch.setattr(
        native_symbol_inspection,
        "_nm_candidate_binaries",
        lambda: ["first-nm", "second-nm"],
    )

    def fail(argv, **kwargs):
        if argv[0] == "first-nm":
            raise primary
        raise subprocess.TimeoutExpired(argv, 1)

    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", fail)
    with pytest.raises(native_symbol_inspection.NativeSymbolInspectionError) as caught:
        native_symbol_inspection._read_native_global_symbol_facts(artifact)
    assert caught.value.__cause__ is primary
    assert len(caught.value.attempts) == 2
    assert "UnicodeDecodeError" in str(caught.value)
    assert "TimeoutExpired" in str(caught.value)


def test_failed_candidate_does_not_hide_later_valid_candidate(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(_BITCODE)
    monkeypatch.setattr(
        native_symbol_inspection,
        "_nm_candidate_binaries",
        lambda: ["bad-nm", "good-nm"],
    )
    calls = []

    def run(argv, **kwargs):
        calls.append(argv[0])
        return subprocess.CompletedProcess(
            argv, 0, "bad output" if argv[0] == "bad-nm" else "0000 T provider\n", ""
        )

    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", run)
    assert native_symbol_inspection._read_native_global_symbol_facts(
        artifact
    ).defined == {"provider"}
    assert calls == ["bad-nm", "good-nm"]


def test_weak_undefined_and_indirect_facts_have_explicit_semantics():
    facts = native_symbol_inspection._parse_native_nm_global_symbol_facts(
        " U required\n w optional_function\n v optional_object\n"
        "0000 w weak_address\n0001 V weak_defined_object\n0002 W weak_defined_function\n"
        "0003 i resolver\n0004 u unique_global\n0005 I alias (indirect for provider)\n",
        target_triple="x86_64-unknown-linux-gnu",
    )
    assert facts.undefined == {"required", "provider"}
    assert facts.weak_undefined == {
        "optional_function",
        "optional_object",
        "weak_address",
    }
    assert facts.defined == {
        "weak_defined_object",
        "weak_defined_function",
        "resolver",
        "unique_global",
        "alias",
    }
    assert facts.defined_functions == {"weak_defined_function", "resolver"}
    assert facts.weak_defined == {"weak_defined_function", "weak_defined_object"}


def test_symbol_normalization_uses_requested_target_not_host():
    from molt.cli.native_link_plan import _host_target_triple

    assert native_symbol_inspection._symbol_normalization_target(None) == (
        native_symbol_inspection._symbol_normalization_target(_host_target_triple())
    )
    output = "0000 T _molt_init_sys\n U _required\n w _optional\n"
    macho = native_symbol_inspection._parse_native_nm_global_symbol_facts(
        output, target_triple="aarch64-apple-darwin"
    )
    elf = native_symbol_inspection._parse_native_nm_global_symbol_facts(
        output, target_triple="aarch64-unknown-linux-gnu"
    )
    assert macho.defined == {"molt_init_sys"}
    assert macho.undefined == {"required"}
    assert macho.weak_undefined == {"optional"}
    assert elf.defined == {"_molt_init_sys"}


def _shared_metadata(path, *, target_triple="x86_64-unknown-linux-gnu", payload=None):
    # Container evidence is real; the mocked leaf tool owns symbol evidence.
    if payload is None:
        payload = static_archive_bytes(
            native_relocatable_object(
                target_triple=target_triple, symbols=("molt_init_sys",)
            )
        )
    path.write_bytes(payload)
    cache._stdlib_object_key_sidecar_path(path).write_text("key", encoding="utf-8")
    cache._stdlib_object_manifest_sidecar_path(path).write_text(
        "manifest", encoding="utf-8"
    )
    cache._stdlib_object_partition_manifest_sidecar_path(path).write_text(
        json.dumps(
            {
                "schema": cache._SHARED_STDLIB_PARTITION_SCHEMA_VERSION,
                "functions": ["molt_init_sys"],
                "function_count": 1,
            }
        ),
        encoding="utf-8",
    )
    cache._stdlib_object_digest_sidecar_path(path).write_text(
        hashlib.sha256(path.read_bytes()).hexdigest(), encoding="utf-8"
    )


def _unavailable_symbol_evidence(path, **kwargs):
    # An operational reader failure, as an unreadable bitcode member raises.
    raise native_symbol_inspection.NativeSymbolInspectionError(
        path, ["LLVM bitcode reader unavailable"]
    )


def test_failed_symbol_read_cannot_mint_or_reuse_success_token(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    _shared_metadata(artifact)
    original = native_symbol_inspection._read_native_global_symbol_facts
    monkeypatch.setattr(
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        _unavailable_symbol_evidence,
    )
    for _ in range(2):
        with pytest.raises(native_symbol_inspection.NativeSymbolInspectionError):
            cache._shared_stdlib_cache_matches_key(
                artifact,
                "key",
                stdlib_object_manifest="manifest",
                target_triple="x86_64-unknown-linux-gnu",
            )
        assert not cache._stdlib_object_symbol_contract_sidecar_path(artifact).exists()
    assert not native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE
    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", original
    )
    assert cache._shared_stdlib_cache_matches_key(
        artifact,
        "key",
        stdlib_object_manifest="manifest",
        target_triple="x86_64-unknown-linux-gnu",
    )
    assert cache._stdlib_object_symbol_contract_sidecar_path(artifact).exists()


@pytest.mark.parametrize("malformed", ["object", "wrong-target-archive", "truncated"])
@pytest.mark.parametrize("receipt", ["none", "symbol", "generation"])
def test_shared_stdlib_shape_cannot_be_authorized_by_symbol_or_generation_receipts(
    tmp_path, monkeypatch, malformed, receipt
):
    artifact = tmp_path / "stdlib.a"
    target = "x86_64-unknown-linux-gnu"
    payload = native_relocatable_object(
        target_triple=(
            "aarch64-unknown-linux-gnu"
            if malformed == "wrong-target-archive"
            else target
        ),
        symbols=("molt_init_sys",),
    )
    if malformed == "wrong-target-archive":
        payload = static_archive_bytes(payload)
    elif malformed == "truncated":
        payload = b"!<arch>\ntruncated"
    _shared_metadata(artifact, payload=payload)

    def unexpected_symbols(*args, **kwargs):
        raise AssertionError("invalid shared artifact must not reach symbol inspection")

    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", unexpected_symbols
    )
    if receipt == "symbol":
        cache._write_shared_stdlib_symbol_contract(
            artifact,
            stdlib_object_cache_key="key",
            stdlib_object_manifest="manifest",
            stdlib_module_symbols=None,
            object_digest=hashlib.sha256(payload).hexdigest(),
            partition_manifest_digest=hashlib.sha256(
                cache._stdlib_object_partition_manifest_sidecar_path(
                    artifact
                ).read_bytes()
            ).hexdigest(),
            target_triple=target,
        )
    # The generation observer is deliberately not an admission authority.
    previous = None
    if receipt == "generation":
        previous = cache._shared_stdlib_cache_generation_token(
            artifact, "key", stdlib_object_manifest="manifest", target_triple=target
        )
        assert previous is not None
    assert not cache._shared_stdlib_cache_matches_key(
        artifact, "key", stdlib_object_manifest="manifest", target_triple=target
    )
    assert (
        cache._shared_stdlib_cache_validation_token(
            artifact,
            "key",
            stdlib_object_manifest="manifest",
            target_triple=target,
            previous_token=previous,
        )
        is None
    )
    detail = cache._shared_stdlib_cache_mismatch_detail(
        artifact, "key", stdlib_object_manifest="manifest", target_triple=target
    )
    assert "Invalid backend native-archive artifact" in detail
    assert artifact.exists()


@pytest.mark.parametrize(
    "kind", [BackendArtifactKind.NATIVE_OBJECT, BackendArtifactKind.NATIVE_ARCHIVE]
)
def test_all_native_admission_siblings_reject_unavailable_symbol_evidence(
    tmp_path, monkeypatch, kind
):
    target = "x86_64-unknown-linux-gnu"
    contract = BackendArtifactContract(kind, target)
    artifact = tmp_path / f"application{contract.suffix}"
    payload = native_relocatable_object(target_triple=target)
    if kind is BackendArtifactKind.NATIVE_ARCHIVE:
        payload = static_archive_bytes(payload)
    if kind is BackendArtifactKind.NATIVE_ARCHIVE:
        _shared_metadata(artifact, payload=payload)
    else:
        artifact.write_bytes(payload)
    monkeypatch.setattr(
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        _unavailable_symbol_evidence,
    )
    checks = [
        lambda: cache._is_valid_cached_backend_artifact(
            artifact, artifact_contract=contract
        ),
        lambda: cache._backend_artifact_sync_identity(
            {"source_key": "key", "tier": "module"},
            source_key="key",
            tier="module",
            artifact=artifact,
            artifact_contract=contract,
        ),
        lambda: cache._materialize_cached_backend_artifact(
            tmp_path,
            artifact,
            tmp_path / "output.o",
            tier="module",
            source_key="key",
            cache_path=None,
            warnings=[],
            artifact_contract=contract,
        ),
        lambda: cache._native_object_has_unresolved_module_chunks(artifact, None),
        lambda: cache._shared_stdlib_native_symbol_closure_issue(
            artifact, stdlib_module_symbols=None
        ),
    ]
    if kind is BackendArtifactKind.NATIVE_ARCHIVE:
        checks.append(
            lambda: cache._shared_stdlib_cache_matches_key(
                artifact, "key", stdlib_object_manifest="manifest", target_triple=target
            )
        )
    for check in checks:
        with pytest.raises(
            native_symbol_inspection.NativeSymbolInspectionError
        ) as caught:
            check()
        assert not isinstance(
            caught.value, native_symbol_inspection.NativeSymbolArtifactError
        )


def test_old_or_cross_target_success_tokens_do_not_authorize(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    args = dict(
        stdlib_object_cache_key="key",
        stdlib_object_manifest="manifest",
        stdlib_module_symbols={"sys"},
        object_digest="digest",
        partition_manifest_digest="partition",
        target_triple="aarch64-apple-darwin",
    )
    payload = cache._shared_stdlib_symbol_contract_payload(**args)
    path = cache._stdlib_object_symbol_contract_sidecar_path(artifact)
    path.write_text(json.dumps({**payload, "schema": 1}), encoding="utf-8")
    assert not cache._shared_stdlib_symbol_contract_matches(artifact, **args)
    path.write_text(json.dumps(payload), encoding="utf-8")
    assert cache._shared_stdlib_symbol_contract_matches(artifact, **args)
    assert not cache._shared_stdlib_symbol_contract_matches(
        artifact, **{**args, "target_triple": "aarch64-unknown-linux-gnu"}
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_NATIVE_SYMBOL_FACTS_PROTOCOL", "test.next-protocol"
    )
    assert not cache._shared_stdlib_symbol_contract_matches(artifact, **args)


def test_old_symbol_facts_are_misses_and_new_weak_facts_roundtrip(tmp_path):
    artifact = tmp_path / "archive.a"
    facts = native_symbol_inspection._NativeGlobalSymbolFacts(
        frozenset({"provider"}),
        frozenset({"required"}),
        frozenset({"provider"}),
        frozenset({"optional"}),
        "digest",
    )
    payload = native_symbol_inspection._native_object_symbol_facts_payload(
        object_digest="digest",
        facts=facts,
        target_triple=None,
        reader_identity=("llvm-nm",),
    )
    path = native_symbol_inspection._native_object_symbol_facts_sidecar_path(artifact)
    path.write_text(json.dumps({**payload, "schema": 3}), encoding="utf-8")
    assert (
        native_symbol_inspection._read_native_object_symbol_facts(
            artifact,
            object_digest="digest",
            target_triple=None,
            reader_identity=("llvm-nm",),
        )
        is None
    )
    path.write_text(json.dumps(payload), encoding="utf-8")
    assert (
        native_symbol_inspection._read_native_object_symbol_facts(
            artifact,
            object_digest="digest",
            target_triple=None,
            reader_identity=("different-llvm-nm",),
        )
        is None
    )
    assert (
        native_symbol_inspection._read_native_object_symbol_facts(
            artifact,
            object_digest="digest",
            target_triple=None,
            reader_identity=("llvm-nm",),
        )
        == facts
    )


@pytest.mark.parametrize(
    "reader",
    [
        native_symbol_inspection._native_object_global_symbol_facts,
        native_symbol_inspection._native_archive_global_symbol_facts,
    ],
)
def test_symbol_read_replacement_cannot_publish_facts_for_previous_bytes(
    tmp_path, monkeypatch, reader
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(static_archive_bytes(b"generation-A"))
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )

    def inspect(path, **kwargs):
        path.write_bytes(b"generation-B")
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            frozenset({"B"}), frozenset(), frozenset({"B"})
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", inspect
    )
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="changed during symbol inspection",
    ):
        reader(artifact)
    assert not native_symbol_inspection._native_object_symbol_facts_sidecar_path(
        artifact
    ).exists()
    assert not native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE
    assert not native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE


def test_provider_facts_reject_changed_bytes_with_restored_mtime(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(static_archive_bytes(b"A"))
    timestamp = artifact.stat().st_mtime_ns
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )

    def inspect(path, **kwargs):
        name = path.read_bytes()[68:69].decode("ascii")
        symbols = native_symbol_inspection._NativeGlobalSymbolFacts(
            frozenset({name}), frozenset(), frozenset({name})
        )
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            frozenset(),
            frozenset(),
            frozenset(),
            members=(
                native_symbol_inspection._NativeArchiveMemberSymbolFacts(
                    kwargs["archive_members"][0],
                    symbols,
                ),
            ),
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", inspect
    )
    assert native_symbol_inspection._native_archive_global_symbol_facts(
        artifact
    ).defined == {"A"}
    artifact.write_bytes(static_archive_bytes(b"B"))
    os.utime(artifact, ns=(timestamp, timestamp))
    assert native_symbol_inspection._native_archive_global_symbol_facts(
        artifact
    ).defined == {"B"}


def test_validation_and_token_mint_share_one_generation_lock(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    _shared_metadata(artifact)
    held = False

    @contextmanager
    def locked(path):
        nonlocal held
        assert not held
        held = True
        try:
            yield
        finally:
            held = False

    def validate(path, *args, **kwargs):
        assert held
        path.write_bytes(b"changed-after-validation")
        return True

    monkeypatch.setattr(cache, "_shared_stdlib_cache_lock", locked)
    monkeypatch.setattr(cache, "_shared_stdlib_cache_matches_key", validate)
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="changed during locked validation",
    ):
        cache._shared_stdlib_cache_validation_token(
            artifact, "key", stdlib_object_manifest="manifest"
        )
    assert not held


def test_same_size_restored_mtime_cannot_reuse_validated_token(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    _shared_metadata(artifact)
    token = cache._shared_stdlib_cache_validation_token(
        artifact,
        "key",
        stdlib_object_manifest="manifest",
        target_triple="x86_64-unknown-linux-gnu",
    )
    assert token is not None
    timestamp = artifact.stat().st_mtime_ns
    artifact.write_bytes(b"X" * artifact.stat().st_size)
    os.utime(artifact, ns=(timestamp, timestamp))
    assert not cache._shared_stdlib_cache_validation_token_matches(
        artifact,
        "key",
        token,
        stdlib_object_manifest="manifest",
        target_triple="x86_64-unknown-linux-gnu",
    )


@pytest.mark.parametrize(
    "kind", [BackendArtifactKind.NATIVE_OBJECT, BackendArtifactKind.NATIVE_ARCHIVE]
)
def test_empty_leaf_is_not_an_application_cache_hit(tmp_path, monkeypatch, kind):
    target = "x86_64-unknown-linux-gnu"
    contract = BackendArtifactContract(kind, target)
    artifact = tmp_path / f"application{contract.suffix}"
    payload = native_relocatable_object(target_triple=target)
    if kind is BackendArtifactKind.NATIVE_ARCHIVE:
        payload = static_archive_bytes(payload)
    artifact.write_bytes(payload)
    assert (
        native_symbol_inspection._native_object_global_symbol_facts(artifact).defined
        == frozenset()
    )
    assert not cache._is_valid_cached_backend_artifact(
        artifact,
        artifact_contract=contract,
    )


@pytest.mark.parametrize("reader_name", ["object", "archive"])
@pytest.mark.parametrize("tier", ["memory", "persistent", "publication"])
def test_symbol_fact_generation_is_rechecked_on_every_return(
    tmp_path, monkeypatch, reader_name, tier
):
    archive = reader_name == "archive"
    artifact = tmp_path / ("input.a" if archive else "input.o")
    payload = native_relocatable_object(symbols=("original_symbol",))
    artifact.write_bytes(static_archive_bytes(payload) if archive else payload)
    stamp = artifact.stat()
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    reader = getattr(
        native_symbol_inspection, f"_native_{reader_name}_global_symbol_facts"
    )
    storage_name = f"_NATIVE_{reader_name.upper()}_SYMBOL_SETS_CACHE"
    storage = getattr(native_symbol_inspection, storage_name)

    def replace_generation():
        artifact.write_bytes(b"replaced")
        os.utime(artifact, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))

    if tier != "publication":
        assert (
            reader(artifact) if archive else reader(artifact, publish=True)
        ).defined == {"original_symbol"}
    if tier == "memory":

        class ReplacingCache(dict):
            def get(self, key, default=None):
                result = super().get(key, default)
                if result is not None:
                    replace_generation()
                return result

        monkeypatch.setattr(
            native_symbol_inspection, storage_name, ReplacingCache(storage)
        )
    else:
        storage.clear()
        hook_name = (
            f"_read_native_{reader_name}_symbol_"
            + ("facts" if reader_name == "object" else "cache")
            if tier == "persistent"
            else f"_write_native_{reader_name}_symbol_"
            + ("facts" if reader_name == "object" else "cache")
        )
        original = getattr(native_symbol_inspection, hook_name)

        def replace_after_operation(*args, **kwargs):
            result = original(*args, **kwargs)
            replace_generation()
            return result

        monkeypatch.setattr(
            native_symbol_inspection, hook_name, replace_after_operation
        )
    if tier == "publication":
        facts = reader(artifact) if archive else reader(artifact, publish=True)
        assert facts.defined == {"original_symbol"}
        assert (
            facts.artifact_digest != hashlib.sha256(artifact.read_bytes()).hexdigest()
        )
    else:
        with pytest.raises(
            native_symbol_inspection.NativeSymbolInspectionError, match="changed"
        ):
            reader(artifact)


def test_native_cache_shape_and_symbols_share_one_generation(tmp_path, monkeypatch):
    artifact = tmp_path / "application.o"
    artifact.write_bytes(
        native_relocatable_object(
            target_triple="x86_64-unknown-linux-gnu", symbols=("application",)
        )
    )
    contract = BackendArtifactContract(
        BackendArtifactKind.NATIVE_OBJECT, "x86_64-unknown-linux-gnu"
    )
    original = BackendArtifactContract.validate_native_shape

    def replace_after_shape(self, path, *, opened=None):
        original(self, path, opened=opened)
        path.write_bytes(
            native_relocatable_object(
                target_triple="aarch64-unknown-linux-gnu", symbols=("application",)
            )
        )

    monkeypatch.setattr(
        BackendArtifactContract, "validate_native_shape", replace_after_shape
    )
    with pytest.raises(BackendArtifactValidationError, match="changed"):
        cache._validate_backend_cache_artifact(artifact, artifact_contract=contract)


def test_native_cache_admission_hashes_once_and_returns_reusable_generation(
    tmp_path, monkeypatch
):
    artifact = tmp_path / "application.o"
    artifact.write_bytes(native_relocatable_object(symbols=("application",)))
    contract = BackendArtifactContract(BackendArtifactKind.NATIVE_OBJECT)
    captures = []
    original = native_symbol_inspection.stable_regular_file_handle_identity

    def capture(opened, *, label):
        captures.append(opened.path)
        return original(opened, label=label)

    monkeypatch.setattr(
        native_symbol_inspection, "stable_regular_file_handle_identity", capture
    )
    identity = cache._validate_backend_cache_artifact(
        artifact, artifact_contract=contract
    )
    assert captures == [artifact]
    assert identity.sha256 == hashlib.sha256(artifact.read_bytes()).hexdigest()
    assert native_symbol_inspection._native_object_global_symbol_facts(
        artifact, target_triple=contract.target_triple, identity=identity
    ).defined == {"application"}
    assert captures == [artifact, artifact], (
        "each detached cache hit admits current bytes once"
    )
    other = tmp_path / "other.o"
    other.write_bytes(artifact.read_bytes())
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="changed"
    ):
        native_symbol_inspection._native_object_global_symbol_facts(
            other, identity=identity
        )


@pytest.mark.parametrize("stage", ["publish", "materialize", "stage"])
def test_backend_publication_rejects_swapped_source_before_copy(
    tmp_path, monkeypatch, stage
):
    source = tmp_path / "source.rs"
    source.write_text("fn alpha() {}\n", encoding="utf-8")
    destination = tmp_path / "out.rs"
    destination.write_text("prior output", encoding="utf-8")
    original_copy = cache._atomic_copy_file
    contract = BackendArtifactContract(BackendArtifactKind.RUST)

    def replace_before_copy(src, dst, **kwargs):
        src.write_text("fn bravo() {}\n", encoding="utf-8")
        return original_copy(src, dst, **kwargs)

    monkeypatch.setattr(cache, "_atomic_copy_file", replace_before_copy)
    warnings = []
    if stage == "publish":
        with pytest.raises(
            cache.BackendArtifactValidationError, match="source changed"
        ):
            cache._publish_immutable_backend_cache_artifact(
                source,
                tmp_path / "cache.rs",
                artifact_contract=contract,
                warnings=warnings,
            )
        assert not (tmp_path / "cache.rs").exists()
    elif stage == "materialize":
        assert not cache._materialize_cached_backend_artifact(
            tmp_path,
            source,
            destination,
            tier="module",
            source_key="key",
            cache_path=None,
            warnings=warnings,
            artifact_contract=contract,
        )
        assert warnings and "source changed" in warnings[0]
    else:
        error = cache._stage_backend_output_and_caches(
            tmp_path,
            source,
            destination,
            cache_path=None,
            cache_key=None,
            stdlib_object_cache_key=None,
            function_cache_path=None,
            warnings=warnings,
            artifact_contract=contract,
        )
        assert error and "source changed" in error
    assert destination.read_text(encoding="utf-8") == "prior output"


def test_immutable_cache_publication_rejects_conflicting_valid_peer(tmp_path):
    source = tmp_path / "source.rs"
    destination = tmp_path / "cache.rs"
    source.write_text("fn alpha() {}\n", encoding="utf-8")
    destination.write_text("fn bravo() {}\n", encoding="utf-8")
    with pytest.raises(
        cache.BackendArtifactValidationError, match="conflicting content"
    ):
        cache._publish_immutable_backend_cache_artifact(
            source,
            destination,
            artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
            warnings=[],
        )
    assert destination.read_text(encoding="utf-8") == "fn bravo() {}\n"


def test_immutable_publication_returns_admitted_identity_without_source_alias(tmp_path):
    source = tmp_path / "source.rs"
    destination = tmp_path / "cache.rs"
    payload = "fn alpha() {}\n"
    source.write_bytes(payload.encode())
    identity = cache._publish_immutable_backend_cache_artifact(
        source,
        destination,
        artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
        warnings=[],
    )
    assert identity.path == destination
    assert identity.sha256 == hashlib.sha256(payload.encode()).hexdigest()
    cache.verify_stable_regular_file_identity(identity, label="published cache")
    assert not source.samefile(destination)
    source.write_text("fn bravo() {}\n", encoding="utf-8")
    assert destination.read_text(encoding="utf-8") == payload


def test_typed_callable_requirement_admits_facts_and_partitions_cache(
    tmp_path, monkeypatch
):
    archive = tmp_path / "runtime.a"
    archive.write_bytes(_archive("molt_borrowed", "irrelevant"))
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    read = native_symbol_inspection._native_archive_global_symbol_facts
    assert read(archive).defined_functions == {"molt_borrowed", "irrelevant"}
    requirement = native_symbol_inspection.NativeSymbolRequirement(
        function_prefix="molt_", excluded_functions=frozenset({"molt_borrowed"})
    )
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="consumer requirement",
    ) as caught:
        read(archive, requirement=requirement)
    assert not isinstance(
        caught.value, native_symbol_inspection.NativeSymbolArtifactError
    )
    archive.write_bytes(_archive("molt_borrowed", "molt_ready"))
    facts = read(archive, requirement=requirement)
    assert facts.defined_functions == {"molt_borrowed", "molt_ready"}
    original = native_symbol_inspection.read_symbol_rows
    reads = []

    def counted(reader, **kwargs):
        reads.append(reader.size)
        return original(reader, **kwargs)

    monkeypatch.setattr(native_symbol_inspection, "read_symbol_rows", counted)
    assert read(archive, requirement=requirement) == facts
    assert reads == []
    # A rejected read publishes nothing; each requirement owns its cache entry.
    keys = list(native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE)
    assert len(keys) == 2
    assert len({key.reader_identity for key in keys}) == 2


_ELF = "x86_64-unknown-linux-gnu"
_FIRST_MEMBER = native_relocatable_object(
    target_triple=_ELF,
    symbols=("root",),
    undefined_symbols=("callback", "optional"),
    weak_symbols=("optional",),
)
_SECOND_MEMBER = native_relocatable_object(
    target_triple=_ELF,
    symbols=("callback",),
    undefined_symbols=("external",),
    weak_symbols=("callback",),
)


def _two_member_archive(path, first=_FIRST_MEMBER, second=_SECOND_MEMBER):
    # Equal names, different payloads: identity must include ordinal and bytes.
    path.write_bytes(static_archive_bytes(first) + static_archive_bytes(second)[8:])
    return static_archive_member_identities(path)


def _two_bitcode_member_archive(path):
    return _two_member_archive(path, _BITCODE + b"first", _BITCODE + b"second")


@pytest.mark.parametrize("cache_kind", ["object", "archive"])
def test_member_custody_and_aggregate_projections_survive_both_cache_forms(
    tmp_path, monkeypatch, cache_kind
):
    path = tmp_path / "duplicate.a"
    members = _two_member_archive(path)
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    read = getattr(
        native_symbol_inspection, f"_native_{cache_kind}_global_symbol_facts"
    )
    facts = read(path)
    assert facts.members is not None
    assert tuple(item.identity for item in facts.members) == members
    assert members[0].sha256 == hashlib.sha256(_FIRST_MEMBER).hexdigest()
    assert members[1].sha256 == hashlib.sha256(_SECOND_MEMBER).hexdigest()
    assert facts.members[0].symbols.undefined == {"callback"}
    assert facts.members[1].symbols.undefined == {"external"}
    assert facts.defined == {"root", "callback"}
    # Aggregate evidence is not lazy extraction or whole-archive link closure.
    assert facts.undefined == {"callback", "external"}
    assert facts.weak_undefined == {"optional"}
    assert facts.weak_defined == {"callback"}
    getattr(
        native_symbol_inspection, f"_NATIVE_{cache_kind.upper()}_SYMBOL_SETS_CACHE"
    ).clear()

    def no_second_read(*args, **kwargs):
        pytest.fail("content-bound cache should retain ordered facts without a read")

    monkeypatch.setattr(native_symbol_inspection, "read_symbol_rows", no_second_read)
    assert read(path) == facts


@pytest.mark.parametrize(
    "output",
    [
        "0000 T orphan\n",
        "object.o:\n0000 T first\n",
        "object.o:\nobject.o:\nobject.o:\n",
        "foreign.o:\nobject.o:\n",
        "object.o:\nwarning:\n",
    ],
)
def test_member_facts_reject_missing_extra_or_unbound_tables(
    tmp_path, monkeypatch, output
):
    path = tmp_path / "archive.a"
    _two_bitcode_member_archive(path)
    _bitcode_tool(monkeypatch, stdout=output)
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="member"
    ):
        native_symbol_inspection._native_archive_global_symbol_facts(path)
    assert not native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE
    assert not (tmp_path / "cache").exists()


@pytest.mark.parametrize(
    "mutation", ["reorder", "hash", "offset", "bool", "drop", "projection", "weak"]
)
def test_member_cache_codec_cannot_replace_current_content_custody(tmp_path, mutation):
    path = tmp_path / "archive.a"
    members = _two_member_archive(path)
    parse = native_symbol_inspection._parse_native_nm_global_symbol_facts
    facts = native_symbol_inspection._NativeGlobalSymbolFacts(
        frozenset(),
        frozenset(),
        frozenset(),
        members=(
            native_symbol_inspection._NativeArchiveMemberSymbolFacts(
                members[0], parse("0000 T root\n", target_triple=_ELF)
            ),
            native_symbol_inspection._NativeArchiveMemberSymbolFacts(
                members[1], parse("0000 W leaf\n", target_triple=_ELF)
            ),
        ),
    )
    payload = native_symbol_inspection._symbol_facts_payload(facts)
    rows = payload["members"]
    if mutation == "reorder":
        rows.reverse()
    elif mutation == "hash":
        rows[0]["sha256"] = rows[1]["sha256"]
    elif mutation == "offset":
        rows[0]["offset"] += 1
    elif mutation == "bool":
        rows[0]["ordinal"] = False
    elif mutation == "drop":
        rows.pop()
    elif mutation == "projection":
        payload["defined"] = ["invented"]
    else:
        rows[0]["symbols"]["weak_defined"] = ["invented"]
    assert (
        native_symbol_inspection._decode_symbol_facts(
            payload, artifact_digest="digest", members=members
        )
        is None
    )


def _symbol_codec_payload(table, members):
    if members is None:
        return {"object": table}
    return {
        "members": [
            {
                "ordinal": item.ordinal,
                "name": item.member.name,
                "offset": item.member.content_offset,
                "size": item.member.size,
                "sha256": item.sha256,
                "symbols": table,
            }
            for item in members
        ]
    }


def _symbol_codec_table(symbols):
    return {
        "defined": list(symbols),
        "undefined": [],
        "defined_functions": list(symbols),
        "weak_undefined": [],
        "weak_defined": [],
    }


@pytest.mark.parametrize("archive", [False, True])
def test_symbol_cache_codec_preserves_unicode_whitespace_semantics(tmp_path, archive):
    members = _two_member_archive(tmp_path / "archive.a") if archive else None
    # Python's Unicode whitespace includes the four ASCII information separators
    # as well as non-ASCII separators; ASCII-only matching would admit bad names.
    whitespace = (
        "\t\n\v\f\r\x1c\x1d\x1e\x1f \x85\xa0\u1680"
        "\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007"
        "\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000"
    )
    for character in whitespace:
        assert character.isspace()
        table = _symbol_codec_table([f"left{character}right"])
        assert (
            native_symbol_inspection._decode_symbol_facts(
                _symbol_codec_payload(table, members),
                artifact_digest="digest",
                members=members,
            )
            is None
        )
    # These code points are not whitespace and must retain the existing codec's
    # acceptance, including format characters and the embedded NUL boundary.
    names = sorted(["plain", "文", "a\u200bb", "a\ufeffb", "a\x00b"])
    facts = native_symbol_inspection._decode_symbol_facts(
        _symbol_codec_payload(_symbol_codec_table(names), members),
        artifact_digest="digest",
        members=members,
    )
    assert facts is not None
    assert facts.defined == facts.defined_functions == frozenset(names)


@pytest.mark.parametrize("archive", [False, True])
@pytest.mark.parametrize(
    "change",
    [
        {"defined": ["z", "a"]},
        {"defined": ["a", "a"]},
        {"defined": [""]},
        {"defined": [1]},
        {"defined": ["a", None]},
        {"defined": "a"},
        {"defined_functions": ["missing"]},
        {"weak_defined": ["missing"]},
        {"unexpected": []},
    ],
)
def test_symbol_cache_codec_rejects_noncanonical_tables(tmp_path, archive, change):
    members = _two_member_archive(tmp_path / "archive.a") if archive else None
    table = _symbol_codec_table(["a"]) | change
    assert (
        native_symbol_inspection._decode_symbol_facts(
            _symbol_codec_payload(table, members),
            artifact_digest="digest",
            members=members,
        )
        is None
    )


@pytest.mark.parametrize("archive", [False, True])
def test_symbol_cache_codec_reuses_lexical_checks_only_within_payload(
    tmp_path, monkeypatch, archive
):
    members = _two_member_archive(tmp_path / "archive.a") if archive else None
    pattern = native_symbol_inspection._SYMBOL_WHITESPACE
    checked = []

    class ObservedWhitespace:
        def search(self, symbol):
            checked.append(symbol)
            return pattern.search(symbol)

    monkeypatch.setattr(
        native_symbol_inspection, "_SYMBOL_WHITESPACE", ObservedWhitespace()
    )
    names = ["a", "shared_" + "long_name_" * 32, "文"]
    payload = _symbol_codec_payload(_symbol_codec_table(names), members)
    for admission in (1, 2):
        facts = native_symbol_inspection._decode_symbol_facts(
            payload, artifact_digest="digest", members=members
        )
        assert facts is not None
        assert facts.defined == facts.defined_functions == frozenset(names)
        # Each string appears in two fields and, for archives, two members. Its
        # immutable lexical property is checked once in each payload admission.
        assert checked == names * admission


def test_empty_archive_is_distinct_from_object_and_missing_member_output(
    tmp_path, monkeypatch
):
    path = tmp_path / "empty.a"
    path.write_bytes(b"!<arch>\n")
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    facts = native_symbol_inspection._native_archive_global_symbol_facts(path)
    assert facts.members == ()
    assert facts.defined == facts.undefined == frozenset()


def test_archive_empty_diagnostic_must_name_a_framed_member(tmp_path, monkeypatch):
    path = tmp_path / "archive.a"
    _two_bitcode_member_archive(path)
    _bitcode_tool(
        monkeypatch,
        stdout="object.o:\nobject.o:\n",
        stderr=f"{path}(foreign.o): no symbols\n",
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="foreign.o"
    ):
        native_symbol_inspection._native_archive_global_symbol_facts(path)


def test_object_parser_does_not_silently_discard_archive_or_architecture_headers():
    with pytest.raises(ValueError, match="member custody"):
        native_symbol_inspection._parse_native_nm_global_symbol_facts(
            "member.o:\n0000 T root\n"
        )


@pytest.mark.slow
@pytest.mark.parametrize("object_format", ["coff", "elf", "macho"])
def test_member_symbols_real_native_archive(tmp_path, monkeypatch, object_format):
    tools = verified_llvm_tools()
    _assert_real_archive_member_symbols(
        tmp_path, monkeypatch, tools, tools.native_target(object_format)
    )


@pytest.mark.slow
def test_member_symbols_real_wasm_archive(tmp_path, monkeypatch):
    _assert_real_archive_member_symbols(
        tmp_path, monkeypatch, verified_llvm_tools(), "wasm32-wasip1"
    )


def _assert_real_archive_member_symbols(tmp_path, monkeypatch, tools, target):
    cc, ar, nm = tools.clang, tools.ar, tools.nm
    if target.startswith("wasm"):
        # Native LLVM and the WASI SDK have independent pinned releases. Never
        # substitute a native reader for the SDK selected by the WASM job.
        from molt.llvm_toolchain import verify_wasm_llvm_nm
        from molt.source_root import compiler_source_root

        nm = str(verify_wasm_llvm_nm(compiler_source_root()).path)
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    sources = (
        "extern int callback(void); int root(void) { return callback(); }\n",
        "extern int external(void); int callback(void) { return external(); }\n",
        "/* An object with no global symbols still has member identity. */\n",
    )
    objects = []
    for index, source in enumerate(sources):
        directory = tmp_path / str(index)
        directory.mkdir()
        source_path = directory / "input.c"
        source_path.write_text(source, encoding="utf-8")
        # Deliberate duplicate archive names in three different directories.
        output = directory / "object.o"
        result = _COMMANDS.run(
            [cc, f"--target={target}", "-c", str(source_path), "-o", str(output)],
            capture_output=True,
            text=True,
            timeout=30,
            encoding="utf-8",
        )
        assert result.returncode == 0, result.stderr
        objects.append(output)
    archive = tmp_path / "members.a"
    result = _COMMANDS.run(
        [ar, "qcD", str(archive), *map(str, objects)],
        capture_output=True,
        text=True,
        timeout=30,
        encoding="utf-8",
    )
    assert result.returncode == 0, result.stderr
    facts = native_symbol_inspection._native_archive_global_symbol_facts(
        archive, nm_command=(nm,), target_triple=target
    )
    assert facts.members is not None
    assert [member.identity.ordinal for member in facts.members] == [0, 1, 2]
    assert [member.identity.member.name for member in facts.members] == ["object.o"] * 3
    assert [member.identity.sha256 for member in facts.members] == [
        hashlib.sha256(path.read_bytes()).hexdigest() for path in objects
    ]
    assert facts.members[0].symbols.defined_functions == {"root"}
    assert facts.members[0].symbols.undefined == {"callback"}
    assert facts.members[1].symbols.defined_functions == {"callback"}
    assert facts.members[1].symbols.undefined == {"external"}
    assert (
        facts.members[2].symbols.defined
        == facts.members[2].symbols.undefined
        == frozenset()
    )
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    # The disk projection must preserve the same member content and weak facts.
    assert (
        native_symbol_inspection._native_archive_global_symbol_facts(
            archive, nm_command=(nm,), target_triple=target
        )
        == facts
    )


@pytest.mark.parametrize("target", ["wasm32-wasip1", "wasm32-unknown-unknown"])
def test_wasm_symbol_family_preserves_names_and_cache_identity(target):
    assert (
        native_symbol_inspection._symbol_normalization_target(target)
        == f"target:{target}"
    )
    assert (
        native_symbol_inspection._normalize_native_symbol_name(
            "_compiler_rt_helper", target_triple=target
        )
        == "_compiler_rt_helper"
    )
    assert (
        native_symbol_inspection._symbol_normalization_target(target.upper())
        == f"target:{target}"
    )


@pytest.mark.parametrize("archive", [False, True])
def test_explicit_symbol_publication_materializes_content_hit_without_reextracting(
    tmp_path, monkeypatch, archive
):
    source = tmp_path / "source.o"
    payload = native_relocatable_object(symbols=("published_function",))
    source.write_bytes(static_archive_bytes(payload) if archive else payload)
    original = native_symbol_inspection._native_object_global_symbol_facts(source)
    destination = tmp_path / "destination.o"
    destination.write_bytes(source.read_bytes())
    sidecar = native_symbol_inspection._native_object_symbol_facts_sidecar_path(
        destination
    )

    def unexpected(*args, **kwargs):
        raise AssertionError("byte-identical publication must reuse admitted facts")

    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", unexpected
    )
    assert (
        native_symbol_inspection._native_object_global_symbol_facts(destination)
        == original
    )
    assert not sidecar.exists()
    assert (
        native_symbol_inspection._native_object_global_symbol_facts(
            destination, publish=True
        )
        == original
    )
    assert sidecar.exists() is (not archive)
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    assert (
        native_symbol_inspection._native_object_global_symbol_facts(destination)
        == original
    )


@pytest.mark.parametrize(
    "consumer", ["backend", "synced-output", "materialize", "shared"]
)
@pytest.mark.parametrize(
    "failure", ["missing", "nonregular", "unreadable", "opening-race", "closing-fence"]
)
def test_cached_artifact_input_failures_are_misses(
    tmp_path, monkeypatch, consumer, failure
):
    from molt.toolchain_identity import StableRegularFileChangedError

    target = "x86_64-unknown-linux-gnu"
    artifact = tmp_path / "stdlib.a"
    _shared_metadata(artifact)
    original_open = native_symbol_inspection.open_stable_regular_file

    if failure in {"missing", "nonregular"}:
        artifact.unlink()
        if failure == "nonregular":
            artifact.mkdir()

    @contextmanager
    def fail_artifact(path, **kwargs):
        if path == artifact:
            if failure == "unreadable":
                raise PermissionError("cached artifact read denied")
            if failure == "opening-race":
                artifact.unlink()
        with original_open(path, **kwargs) as opened:
            yield opened
        if path == artifact and failure == "closing-fence":
            raise StableRegularFileChangedError(
                "cached artifact changed at closing fence"
            )

    monkeypatch.setattr(
        native_symbol_inspection, "open_stable_regular_file", fail_artifact
    )
    contract = BackendArtifactContract(BackendArtifactKind.NATIVE_ARCHIVE, target)
    if consumer == "backend":
        assert not cache._is_valid_cached_backend_artifact(
            artifact, artifact_contract=contract
        )
    elif consumer == "synced-output":
        assert (
            cache._backend_artifact_sync_identity(
                {"source_key": "key", "tier": "module"},
                source_key="key",
                tier="module",
                artifact=artifact,
                artifact_contract=contract,
            )
            is None
        )
    elif consumer == "materialize":
        output = tmp_path / "output.o"
        output.write_bytes(b"previous output")
        warnings = []
        assert not cache._materialize_cached_backend_artifact(
            tmp_path,
            artifact,
            output,
            tier="module",
            source_key="key",
            cache_path=None,
            warnings=warnings,
            artifact_contract=contract,
        )
        assert output.read_bytes() == b"previous output"
        assert len(warnings) == 1 and "Cache candidate admission failed" in warnings[0]
    else:
        assert not cache._shared_stdlib_cache_matches_key(
            artifact, "key", stdlib_object_manifest="manifest", target_triple=target
        )
    assert not cache._stdlib_object_symbol_contract_sidecar_path(artifact).exists()
    assert not native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE
    assert not native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE


@pytest.mark.parametrize("error_type", [OSError, ValueError])
def test_owned_native_artifact_preserves_unrelated_consumer_errors(
    tmp_path, error_type
):
    artifact = tmp_path / "input.o"
    artifact.write_bytes(b"consumer input")
    primary = error_type("consumer failure outside artifact admission")
    with pytest.raises(error_type) as caught:
        with native_symbol_inspection._open_native_symbol_artifact(artifact):
            raise primary
    assert caught.value is primary


@pytest.mark.parametrize("archive", [False, True])
@pytest.mark.parametrize("consumer", ["generic", "link-selection"])
def test_external_symbol_admission_uses_content_policy_without_input_sidecars(
    tmp_path, monkeypatch, archive, consumer
):
    from molt.cli.extension_scan_surface import _ExtensionScanSurface
    from molt.cli.link_selection_admission import LinkSelectionAdmission
    from molt.cli.source_extension_link_requirements import (
        SourceExtensionLinkRequirements,
        source_extension_link_file,
    )

    # The admitted suffix deliberately disagrees with the bytes.
    # External consumers own neither the input nor a sidecar beside it.
    artifact = tmp_path / ("external.o" if archive else "external.a")
    payload = native_relocatable_object(
        target_triple="x86_64-unknown-linux-gnu"
        if consumer == "link-selection"
        else None,
        symbols=("provider",),
    )
    artifact.write_bytes(static_archive_bytes(payload) if archive else payload)
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    original_read = native_symbol_inspection.read_symbol_rows
    calls = []

    def read(reader, **kwargs):
        calls.append(reader.size)
        return original_read(reader, **kwargs)

    monkeypatch.setattr(native_symbol_inspection, "read_symbol_rows", read)
    if consumer == "generic":
        facts = native_symbol_inspection._native_object_global_symbol_facts(artifact)
    else:
        requirements = SourceExtensionLinkRequirements(
            "x86_64-unknown-linux-gnu", (source_extension_link_file(artifact),)
        )
        surface = _ExtensionScanSurface(
            frozenset(), frozenset(), frozenset(), tmp_path / "Python.h"
        )
        selected = LinkSelectionAdmission.capture(requirements, surface=surface)
        facts = selected.facts[artifact.resolve()]
    assert facts.defined == {"provider"}
    assert (facts.members is not None) is archive
    assert calls == [len(payload)]
    assert not native_symbol_inspection._native_object_symbol_facts_sidecar_path(
        artifact
    ).exists()
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    if archive:
        again = native_symbol_inspection._native_object_global_symbol_facts(
            artifact,
            target_triple="x86_64-unknown-linux-gnu"
            if consumer == "link-selection"
            else None,
        )
        assert again == facts and calls == [len(payload)]


def test_nm_reader_family_is_classified_from_the_version_banner():
    classify = native_symbol_inspection.nm_reader_family_from_banner
    # LLVM 22.1.8 llvm-nm and Xcode's nm (an llvm-nm) print the same first line.
    assert classify("llvm-nm, compatible with GNU nm") == "llvm"
    assert classify("GNU nm (GNU Binutils for Ubuntu) 2.42") == "gnu"
    assert classify("Apple, Inc. version cctools-1010.6") is None
    assert classify(None) is None


@pytest.mark.parametrize(
    "banner,error",
    [
        (
            "Apple, Inc. version cctools-1010.6",
            "unrecognized nm reader banner: 'Apple, Inc. version cctools-1010.6'",
        ),
        (
            "GNU nm (GNU Binutils for Ubuntu) 2.42",
            "GNU nm cannot read LLVM bitcode: 'GNU nm (GNU Binutils for Ubuntu) 2.42'",
        ),
        (None, "nm reader printed no --version banner"),
    ],
)
def test_only_llvm_nm_passes_bitcode_reader_admission(monkeypatch, banner, error):
    monkeypatch.undo()  # Admit a real executable instead of the fixture's fake.
    probe = native_symbol_inspection._cached_nm_reader_family
    probe.cache_clear()
    monkeypatch.setattr(native_symbol_inspection, "_tool_version", lambda path: banner)
    try:
        candidate = native_symbol_inspection._native_symbol_reader_candidate(
            (str(Path(sys.executable).resolve(strict=True)),)
        )
    finally:
        probe.cache_clear()
    assert candidate.admission_error == error


def test_bitcode_reader_keeps_llvm_nm_bitcode_reader_enabled(tmp_path, monkeypatch):
    # Rows captured from llvm-nm 22.1.8 reading a dev-fast libmolt_runtime
    # staticlib on aarch64-apple-darwin: llvm-nm prints dashes, not an
    # address, for a defined bitcode symbol.
    artifact = tmp_path / "runtime.bc"
    artifact.write_bytes(_BITCODE)
    bitcode_rows = (
        "---------------- T __RINvMs5_NtNtCscEX5ZwinSox_3std2io5errorNtB6_"
        "5Error3newReEBa_\n000000000000274c T _molt_abs_builtin\n"
        "                 U _molt_required\n"
    )
    commands: list[list[str]] = []

    def run(argv, **kwargs):
        commands.append(list(argv))
        assert kwargs["timeout"] == (
            native_symbol_inspection._LLVM_NM_BITCODE_READ_TIMEOUT_S
        )
        return subprocess.CompletedProcess(argv, 0, bitcode_rows, "")

    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", run)
    monkeypatch.setattr(
        native_symbol_inspection, "_nm_candidate_binaries", lambda: ["llvm-nm"]
    )
    facts = native_symbol_inspection._read_native_global_symbol_facts(
        artifact, target_triple="aarch64-apple-darwin"
    )
    assert facts.defined == {
        "_RINvMs5_NtNtCscEX5ZwinSox_3std2io5errorNtB6_5Error3newReEBa_",
        "molt_abs_builtin",
    }
    assert facts.undefined == {"molt_required"}
    assert commands == [["llvm-nm", "-g", str(artifact)]]


def test_native_objects_never_reach_the_bitcode_reader(tmp_path, monkeypatch):
    artifact = tmp_path / "runtime.a"
    artifact.write_bytes(_archive("molt_native"))
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )

    def unexpected(*args, **kwargs):
        raise AssertionError("a native object reached the external reader")

    monkeypatch.setattr(native_symbol_inspection, "_native_symbol_reader", unexpected)
    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", unexpected)
    facts = native_symbol_inspection._native_archive_global_symbol_facts(
        artifact, nm_command=("ignored-nm",)
    )
    assert facts.defined_functions == {"molt_native"}


def test_mixed_archive_binds_llvm_nm_tables_only_to_bitcode_members(
    tmp_path, monkeypatch
):
    path = tmp_path / "mixed.a"
    members = _two_member_archive(path, _FIRST_MEMBER, _BITCODE + b"module")
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    # llvm-nm also prints the native member; that table never becomes a fact.
    _bitcode_tool(
        monkeypatch,
        stdout=(
            "object.o:\n0000 T invented_native\n"
            "object.o:\n---------------- T bitcode_root\n U root\n"
        ),
    )
    facts = native_symbol_inspection._native_archive_global_symbol_facts(
        path, target_triple=_ELF
    )
    assert facts.members is not None
    assert tuple(item.identity for item in facts.members) == members
    assert facts.members[0].symbols.defined == {"root"}
    assert facts.members[0].symbols.weak_undefined == {"optional"}
    assert facts.members[1].symbols.defined == {"bitcode_root"}
    assert facts.members[1].symbols.undefined == {"root"}
    key = next(iter(native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE))
    assert key.reader_identity[1] != "in-process:molt.native_symbol_table"


@pytest.mark.parametrize(
    "archive,payload",
    [
        (archive, payload)
        for archive in (False, True)
        for payload in (b"", b"\x7fEL", b"plain text, not an object")
    ]
    + [(True, b"!<arch>\n")],  # A nested archive is not an object.
)
def test_unreadable_objects_are_typed_artifact_errors(
    tmp_path, monkeypatch, archive, payload
):
    artifact = tmp_path / "input.o"
    if archive:
        payload = static_archive_bytes(payload)
    artifact.write_bytes(payload)
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    with pytest.raises(native_symbol_inspection.NativeSymbolArtifactError) as caught:
        native_symbol_inspection._native_object_global_symbol_facts(artifact)
    if archive:
        assert "archive member 0 ('object.o')" in str(caught.value)
    assert not native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE
    assert not native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE
