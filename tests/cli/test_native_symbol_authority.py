"""Symbol-tool protocol and admission tests, without invoking a compiler or nm."""

from __future__ import annotations

import hashlib
import json
import os
from contextlib import contextmanager
from pathlib import Path
import subprocess
import sys

from molt.cli import native_symbol_inspection
import pytest

from molt.cli import backend_cache as cache
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from tests.cli.native_link_test_support import static_archive_bytes
from tests.native_artifact_fixtures import native_relocatable_object


@pytest.fixture(autouse=True)
def isolated_symbol_cache(monkeypatch: pytest.MonkeyPatch):
    identity = cache.stable_regular_file_identity(
        Path(sys.executable), label="test symbol reader"
    )

    @contextmanager
    def admitted_reader(path, *, label, identity=None):
        del label
        assert identity is not None
        yield path, identity

    monkeypatch.setattr(
        native_symbol_inspection,
        "_native_symbol_reader_candidate",
        lambda command: native_symbol_inspection._NativeSymbolReaderCandidate(
            tuple(command), executable_identity=identity
        ),
    )
    monkeypatch.setattr(
        native_symbol_inspection, "stable_executable_probe", admitted_reader
    )
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    yield
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()


def _tool(monkeypatch, *, code=0, stdout="", stderr=""):
    monkeypatch.setattr(
        native_symbol_inspection, "_nm_candidate_binaries", lambda: ["llvm-nm"]
    )

    def run(argv, **kwargs):
        assert kwargs["errors"] == "strict"
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
    monkeypatch, code, stdout, stderr
):
    _tool(monkeypatch, code=code, stdout=stdout, stderr=stderr)
    with pytest.raises(native_symbol_inspection.NativeSymbolInspectionError) as caught:
        native_symbol_inspection._read_native_global_symbol_facts(
            Path("archive.a"), timeout=1
        )
    assert caught.value.path == Path("archive.a")
    assert "llvm-nm" in str(caught.value)
    assert caught.value.attempts


@pytest.mark.parametrize(
    "code,stdout,stderr",
    [
        (0, "", ""),
        (1, "", "archive.a: no symbols\n"),
        (1, "", "llvm-nm: archive.a: no symbols\n"),
        (1, "", "llvm-nm: archive.a:empty.o: no symbols\n"),
        (0, "member.o:\n", "archive.a(member.o): no symbols\n"),
    ],
)
def test_legitimate_empty_artifact_has_successful_empty_facts(
    monkeypatch, code, stdout, stderr
):
    _tool(monkeypatch, code=code, stdout=stdout, stderr=stderr)
    facts = native_symbol_inspection._read_native_global_symbol_facts(
        Path("archive.a"), timeout=1
    )
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
    _tool(
        monkeypatch,
        stdout="member.o:\n00000000 T provider\n"
        + (diagnostic if channel == "stdout" else ""),
        stderr=diagnostic if channel == "stderr" else "",
    )
    reader = native_symbol_inspection._native_symbol_reader(
        nm_command=("llvm-nm",), target_triple=None
    )
    assert native_symbol_inspection._read_native_global_symbol_facts(
        path, timeout=1, target_triple=target, _reader=reader
    ).defined == {"provider"}


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
    artifact.write_bytes(b"symbol-protocol-input")
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    _tool(monkeypatch, stdout="0000 T before\n")
    assert reader(artifact).defined == {"before"}
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    _tool(monkeypatch, stdout="0000 T after\n")
    # Persistent facts remain reusable under the exact existing protocol.
    assert reader(artifact).defined == {"before"}
    monkeypatch.setattr(
        native_symbol_inspection, "_NATIVE_SYMBOL_FACTS_PROTOCOL", "test.next-protocol"
    )
    assert reader(artifact).defined == {"after"}


def test_missing_tools_and_decode_failures_preserve_typed_diagnostics(monkeypatch):
    monkeypatch.setattr(native_symbol_inspection, "_nm_candidate_binaries", lambda: [])
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError,
        match="no nm/llvm-nm candidate",
    ):
        native_symbol_inspection._read_native_global_symbol_facts(
            Path("archive.a"), timeout=1
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
        native_symbol_inspection._read_native_global_symbol_facts(
            Path("archive.a"), timeout=1
        )
    assert caught.value.__cause__ is primary
    assert len(caught.value.attempts) == 2
    assert "UnicodeDecodeError" in str(caught.value)
    assert "TimeoutExpired" in str(caught.value)


def test_failed_candidate_does_not_hide_later_valid_candidate(monkeypatch):
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
        Path("archive.a"), timeout=1
    ).defined == {"provider"}
    assert calls == ["bad-nm", "good-nm"]


def test_weak_undefined_and_indirect_facts_have_explicit_semantics():
    facts = native_symbol_inspection._parse_native_nm_global_symbol_facts(
        "member.o:\n U required\n w optional_function\n v optional_object\n"
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


def test_symbol_normalization_uses_requested_target_not_host():
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
    cache._stdlib_object_key_sidecar_path(path).write_text("key")
    cache._stdlib_object_manifest_sidecar_path(path).write_text("manifest")
    cache._stdlib_object_partition_manifest_sidecar_path(path).write_text(
        json.dumps(
            {
                "schema": cache._SHARED_STDLIB_PARTITION_SCHEMA_VERSION,
                "functions": ["molt_init_sys"],
                "function_count": 1,
            }
        )
    )
    cache._stdlib_object_digest_sidecar_path(path).write_text(
        hashlib.sha256(path.read_bytes()).hexdigest()
    )


def test_failed_symbol_read_cannot_mint_or_reuse_success_token(tmp_path, monkeypatch):
    artifact = tmp_path / "archive.a"
    _shared_metadata(artifact)
    monkeypatch.setattr(native_symbol_inspection, "_nm_candidate_binaries", lambda: [])
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
    _tool(monkeypatch, stdout="0000 T molt_init_sys\n")
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
    artifact.write_bytes(payload)
    monkeypatch.setattr(native_symbol_inspection, "_nm_candidate_binaries", lambda: [])
    checks = [
        lambda: cache._is_valid_cached_backend_artifact(
            artifact, artifact_contract=contract
        ),
        lambda: cache._native_object_has_unresolved_module_chunks(artifact, None),
        lambda: cache._shared_stdlib_native_symbol_closure_issue(
            artifact, stdlib_module_symbols=None
        ),
    ]
    for check in checks:
        with pytest.raises(native_symbol_inspection.NativeSymbolInspectionError):
            check()


def test_old_or_cross_target_success_tokens_do_not_authorize(tmp_path):
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
    path.write_text(json.dumps({**payload, "schema": 1}))
    assert not cache._shared_stdlib_symbol_contract_matches(artifact, **args)
    path.write_text(json.dumps(payload))
    assert cache._shared_stdlib_symbol_contract_matches(artifact, **args)
    assert not cache._shared_stdlib_symbol_contract_matches(
        artifact, **{**args, "target_triple": "aarch64-unknown-linux-gnu"}
    )


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
    path.write_text(json.dumps({**payload, "schema": 3}))
    assert (
        native_symbol_inspection._read_native_object_symbol_facts(
            artifact,
            object_digest="digest",
            target_triple=None,
            reader_identity=("llvm-nm",),
        )
        is None
    )
    path.write_text(json.dumps(payload))
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
    artifact.write_bytes(b"generation-A")
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


def test_provider_cache_identity_includes_content_not_only_windows_metadata(
    tmp_path, monkeypatch
):
    artifact = tmp_path / "archive.a"
    artifact.write_bytes(b"A")
    timestamp = artifact.stat().st_mtime_ns
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    monkeypatch.setattr(
        native_symbol_inspection, "content_change_time_ns", lambda path, stat: 42
    )

    def inspect(path, **kwargs):
        name = path.read_text()
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            frozenset({name}), frozenset(), frozenset({name})
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_read_native_global_symbol_facts", inspect
    )
    assert native_symbol_inspection._native_archive_global_symbol_facts(
        artifact
    ).defined == {"A"}
    artifact.write_bytes(b"B")
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
    _tool(monkeypatch, stdout="0000 T molt_init_sys\n")
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
    _tool(monkeypatch)
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
    artifact = tmp_path / "input.a"
    artifact.write_bytes(b"original")
    stamp = artifact.stat()
    _tool(monkeypatch, stdout="0000 T original_symbol\n")
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
        assert reader(artifact).defined == {"original_symbol"}
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
    original = BackendArtifactContract.validate

    def replace_after_shape(self, path):
        original(self, path)
        path.write_bytes(
            native_relocatable_object(
                target_triple="aarch64-unknown-linux-gnu", symbols=("application",)
            )
        )

    monkeypatch.setattr(BackendArtifactContract, "validate", replace_after_shape)
    _tool(monkeypatch, stdout="0000 T application\n")
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="changed"
    ):
        cache._validate_backend_cache_artifact(artifact, artifact_contract=contract)


def test_native_cache_admission_hashes_once_and_returns_reusable_generation(
    tmp_path, monkeypatch
):
    artifact = tmp_path / "application.o"
    artifact.write_bytes(native_relocatable_object(symbols=("application",)))
    contract = BackendArtifactContract(BackendArtifactKind.NATIVE_OBJECT)
    _tool(monkeypatch, stdout="0000 T application\n")
    captures = []
    original = cache.stable_regular_file_identity

    def capture(path, *, label):
        captures.append(path)
        return original(path, label=label)

    monkeypatch.setattr(cache, "stable_regular_file_identity", capture)
    identity = cache._validate_backend_cache_artifact(
        artifact, artifact_contract=contract
    )
    assert captures == [artifact]
    assert identity.sha256 == hashlib.sha256(artifact.read_bytes()).hexdigest()
    assert native_symbol_inspection._native_object_global_symbol_facts(
        artifact, target_triple=contract.target_triple, identity=identity
    ).defined == {"application"}
    assert captures == [artifact], "reuse must verify mutation identity, not rehash"
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
    source.write_text("fn alpha() {}\n")
    destination = tmp_path / "out.rs"
    destination.write_text("prior output")
    original_copy = cache._atomic_copy_file
    contract = BackendArtifactContract(BackendArtifactKind.RUST)

    def replace_before_copy(src, dst, **kwargs):
        src.write_text("fn bravo() {}\n")
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
    assert destination.read_text() == "prior output"


def test_immutable_cache_publication_rejects_conflicting_valid_peer(tmp_path):
    source = tmp_path / "source.rs"
    destination = tmp_path / "cache.rs"
    source.write_text("fn alpha() {}\n")
    destination.write_text("fn bravo() {}\n")
    with pytest.raises(
        cache.BackendArtifactValidationError, match="conflicting content"
    ):
        cache._publish_immutable_backend_cache_artifact(
            source,
            destination,
            artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
            warnings=[],
        )
    assert destination.read_text() == "fn bravo() {}\n"


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
    source.write_text("fn bravo() {}\n")
    assert destination.read_text() == payload


@pytest.mark.parametrize(
    "first_stdout", ["", "0000 T irrelevant\n", "0000 T molt_borrowed\n"]
)
def test_typed_callable_requirement_continues_reader_ladder_and_partitions_cache(
    tmp_path, monkeypatch, first_stdout
):
    archive = tmp_path / "runtime.a"
    archive.write_bytes(b"archive")
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "_nm_candidate_binaries",
        lambda: ["first-nm", "second-nm"],
    )
    calls = []

    def inspect(command, **kwargs):
        calls.append(command[0])
        return subprocess.CompletedProcess(
            command,
            0,
            first_stdout if command[0] == "first-nm" else "0000 T molt_ready\n",
            "",
        )

    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", inspect)
    native_symbol_inspection._native_archive_global_symbol_facts(archive)
    assert calls == ["first-nm"]
    calls.clear()
    requirement = native_symbol_inspection.NativeSymbolRequirement(
        function_prefix="molt_", excluded_functions=frozenset({"molt_borrowed"})
    )
    facts = native_symbol_inspection._native_archive_global_symbol_facts(
        archive, requirement=requirement
    )
    assert facts.defined_functions == {"molt_ready"}
    assert calls == ["first-nm", "second-nm"]
    calls.clear()
    assert (
        native_symbol_inspection._native_archive_global_symbol_facts(
            archive, requirement=requirement
        )
        == facts
    )
    assert calls == []


def test_typed_callable_requirement_reports_every_incompatible_reader(monkeypatch):
    monkeypatch.setattr(
        native_symbol_inspection,
        "_nm_candidate_binaries",
        lambda: ["first-nm", "second-nm"],
    )
    monkeypatch.setattr(
        native_symbol_inspection,
        "_run_completed_command",
        lambda command, **kwargs: subprocess.CompletedProcess(
            command, 0, "0000 T unrelated\n", ""
        ),
    )
    with pytest.raises(native_symbol_inspection.NativeSymbolInspectionError) as caught:
        native_symbol_inspection._read_native_global_symbol_facts(
            Path("runtime.a"),
            timeout=1,
            requirement=native_symbol_inspection.NativeSymbolRequirement(
                function_prefix="molt_"
            ),
        )
    assert len(caught.value.attempts) == 2
    assert all("consumer requirement" in item for item in caught.value.attempts)
    assert "first-nm" in caught.value.attempts[0]
    assert "second-nm" in caught.value.attempts[1]
