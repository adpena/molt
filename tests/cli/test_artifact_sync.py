"""Content receipt custody; external symbol/WASM tools are transport stubs only."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from molt.cli import artifact_sync as sync
from molt.cli import backend_cache as cache
from molt.cli import runtime_wasm_validation
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt.toolchain_identity import stable_regular_file_identity
from tests.cli.native_link_test_support import static_archive_bytes
from tests.native_artifact_fixtures import native_relocatable_object


@pytest.fixture(autouse=True)
def isolated_receipt_caches():
    sync._ARTIFACT_SYNC_STATE_CACHE.clear()
    cache._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    cache._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    yield
    sync._ARTIFACT_SYNC_STATE_CACHE.clear()
    cache._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    cache._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()


_NATIVE_TARGETS = (
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)
_CONTRACTS = [
    BackendArtifactContract(kind, target)
    for kind in (BackendArtifactKind.NATIVE_OBJECT, BackendArtifactKind.NATIVE_ARCHIVE)
    for target in _NATIVE_TARGETS
] + [
    BackendArtifactContract(kind)
    for kind in (
        BackendArtifactKind.WASM,
        BackendArtifactKind.RUST,
        BackendArtifactKind.LUAU,
        BackendArtifactKind.MLIR,
    )
]


def _artifact_pair(contract: BackendArtifactContract) -> tuple[bytes, bytes]:
    if contract.is_native:
        pair = tuple(
            native_relocatable_object(
                target_triple=contract.target_triple,
                symbols=("molt_main", name),
            )
            for name in ("value_old", "value_new")
        )
        if contract.kind is BackendArtifactKind.NATIVE_ARCHIVE:
            pair = tuple(static_archive_bytes(data) for data in pair)
        return pair[0], pair[1]
    if contract.is_wasm:
        # Two valid empty WASM modules with different equal-size custom sections.
        return b"\0asm\x01\0\0\0\0\x02\x01a", b"\0asm\x01\0\0\0\0\x02\x01b"
    return b"return 1\n", b"return 2\n"


def _replace_restoring_mtime(path: Path, data: bytes, *, in_place: bool) -> None:
    before = path.stat()
    assert len(data) == before.st_size
    if in_place:
        path.write_bytes(data)
    else:
        replacement = path.with_name(path.name + ".replacement")
        replacement.write_bytes(data)
        replacement.replace(path)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    after = path.stat()
    assert (after.st_size, after.st_mtime_ns) == (before.st_size, before.st_mtime_ns)


@pytest.mark.parametrize(
    "contract", _CONTRACTS, ids=lambda c: f"{c.kind.value}-{c.target_triple}"
)
@pytest.mark.parametrize("in_place", [False, True], ids=["replace", "rewrite"])
def test_receipt_rejects_valid_same_size_restored_mtime_replacement(
    tmp_path: Path, contract: BackendArtifactContract, in_place: bool
):
    original, replacement = _artifact_pair(contract)
    artifact = tmp_path / ("output" + contract.suffix)
    artifact.write_bytes(original)
    receipt = tmp_path / "receipt.json"
    sync._write_artifact_sync_state(
        receipt, source_key="key", tier="module", artifact=artifact
    )
    state = sync._read_artifact_sync_state(receipt)
    assert sync._artifact_sync_state_matches(
        state, source_key="key", tier="module", artifact=artifact
    )
    _replace_restoring_mtime(artifact, replacement, in_place=in_place)
    assert not sync._artifact_sync_state_matches(
        state, source_key="key", tier="module", artifact=artifact
    )


@pytest.mark.parametrize("in_place", [False, True], ids=["replace", "rewrite"])
def test_warm_payload_cache_rejects_restored_mtime_sidecar_substitution(
    tmp_path, in_place
):
    receipt = tmp_path / "receipt.json"
    receipt.write_text('{"source_key":"old"}', encoding="utf-8")
    first = sync._read_artifact_sync_state(receipt)
    assert first == {"source_key": "old"}
    _replace_restoring_mtime(receipt, b'{"source_key":"new"}', in_place=in_place)
    second = sync._read_artifact_sync_state(receipt)
    assert second == {"source_key": "new"}
    assert second is not first


def test_warm_payload_cache_checks_generation_without_rehash(tmp_path, monkeypatch):
    receipt = tmp_path / "receipt.json"
    receipt.write_text('{"source_key":"key"}', encoding="utf-8")
    first = sync._read_artifact_sync_state(receipt)

    def unexpected_hash(*args, **kwargs):
        raise AssertionError("warm receipt payload must not rehash")

    with monkeypatch.context() as warm:
        warm.setattr(sync, "stable_regular_file_identity", unexpected_hash)
        assert sync._read_artifact_sync_state(receipt) is first
    receipt.unlink()
    assert sync._read_artifact_sync_state(receipt) is None


def test_publishing_payload_does_not_bind_callers_dict_to_a_peer_generation(
    tmp_path, monkeypatch
):
    receipt = tmp_path / "receipt.json"
    original_write = sync._atomic_write_json

    def publish_then_peer_replace(path, payload, **kwargs):
        original_write(path, payload, **kwargs)
        _replace_restoring_mtime(
            path, path.read_bytes().replace(b"old", b"new"), in_place=False
        )

    monkeypatch.setattr(sync, "_atomic_write_json", publish_then_peer_replace)
    sync._write_artifact_sync_payload(receipt, {"source_key": "old"})
    assert sync._read_artifact_sync_state(receipt) == {"source_key": "new"}


def test_legacy_stat_receipt_is_a_miss_even_for_identical_bytes(tmp_path):
    artifact = tmp_path / "artifact.rs"
    artifact.write_bytes(b"return 1\n")
    stat = artifact.stat()
    state = {
        "version": 1,
        "source_key": "key",
        "tier": "module",
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }
    assert not sync._artifact_sync_state_matches(
        state, source_key="key", tier="module", artifact=artifact
    )


def test_receipt_reuses_path_bound_validation_identity_without_rehash(
    tmp_path, monkeypatch
):
    artifact = tmp_path / "artifact.rs"
    artifact.write_bytes(b"return 1\n")
    identity = stable_regular_file_identity(artifact, label="test output")
    receipt = tmp_path / "receipt.json"

    def unexpected_hash(*args, **kwargs):
        raise AssertionError("receipt must reuse transaction content identity")

    monkeypatch.setattr(sync, "stable_regular_file_identity", unexpected_hash)
    sync._write_artifact_sync_state(
        receipt, source_key="key", tier="module", artifact=artifact, identity=identity
    )
    state = json.loads(receipt.read_bytes())
    assert sync._artifact_sync_state_matches(
        state, source_key="key", tier="module", artifact=artifact, identity=identity
    )
    other = tmp_path / "other.rs"
    other.write_bytes(artifact.read_bytes())
    assert not sync._artifact_sync_state_matches(
        state, source_key="key", tier="module", artifact=other, identity=identity
    )
    _replace_restoring_mtime(artifact, b"return 2\n", in_place=True)
    assert not sync._artifact_sync_state_matches(
        state, source_key="key", tier="module", artifact=artifact, identity=identity
    )
    with pytest.raises(OSError, match="Cannot attest backend artifact sync receipt"):
        sync._write_artifact_sync_state(
            receipt,
            source_key="key",
            tier="module",
            artifact=artifact,
            identity=identity,
        )


def _stub_external_inspectors(monkeypatch):
    # Shape admission, stable hashing and receipt matching remain real. Only
    # external tool execution is controlled; these are custody, not nm/WASM proofs.
    monkeypatch.setattr(
        cache,
        "_read_native_global_symbol_facts",
        lambda *args, **kwargs: cache._NativeGlobalSymbolFacts(
            frozenset({"molt_main"}), frozenset(), frozenset({"molt_main"})
        ),
    )
    monkeypatch.setattr(
        runtime_wasm_validation,
        "_reusable_wasm_artifact_validation_error",
        lambda path: None,
    )


@pytest.mark.parametrize(
    "contract", _CONTRACTS, ids=lambda c: f"{c.kind.value}-{c.target_triple}"
)
def test_all_backend_sync_consumers_reject_replacement_and_materialize_requested_bytes(
    tmp_path, monkeypatch, contract
):
    _stub_external_inspectors(monkeypatch)
    original, replacement = _artifact_pair(contract)
    output = tmp_path / ("output" + contract.suffix)
    output.write_bytes(original)
    source_key = cache._backend_artifact_source_key(
        "key", stdlib_object_cache_key=None, artifact_contract=contract
    )
    state_path = sync._artifact_sync_state_path(tmp_path, output)
    sync._write_artifact_sync_state(
        state_path, source_key=source_key, tier="module", artifact=output
    )
    state = sync._read_artifact_sync_state(state_path)
    _replace_restoring_mtime(output, replacement, in_place=False)
    assert (
        cache._synced_backend_output_cache_hit(
            state,
            output,
            output.stat(),
            artifact_contract=contract,
            cache_key="key",
            function_cache_key="functions",
            stdlib_object_cache_key=None,
        )
        is None
    )
    assert cache._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output,
        cache_key="key",
        function_cache_key="functions",
        artifact_contract=contract,
        state_path=state_path,
        state=state,
        output_stat=output.stat(),
    ) == (False, False)
    candidate = tmp_path / ("candidate" + contract.suffix)
    candidate.write_bytes(original)
    warnings = []
    assert cache._materialize_cached_backend_artifact(
        tmp_path,
        candidate,
        output,
        tier="module",
        source_key=source_key,
        cache_path=None,
        warnings=warnings,
        state_path=state_path,
        state=state,
        output_stat=output.stat(),
        artifact_contract=contract,
    )
    assert warnings == []
    assert output.read_bytes() == original
    # Caller-supplied True is a hint, never authority to bypass the receipt.
    _replace_restoring_mtime(output, replacement, in_place=False)
    backend_output = tmp_path / ("backend" + contract.suffix)
    backend_output.write_bytes(original)
    assert (
        cache._stage_backend_output_and_caches(
            tmp_path,
            backend_output,
            output,
            cache_path=None,
            cache_key="key",
            stdlib_object_cache_key=None,
            function_cache_path=None,
            warnings=warnings,
            output_already_synced=True,
            state_path=state_path,
            state=state,
            output_stat=output.stat(),
            artifact_contract=contract,
        )
        is None
    )
    assert output.read_bytes() == original
    assert warnings == []


def test_backend_receipt_decision_reuses_exactly_one_validation_identity(
    tmp_path, monkeypatch
):
    contract = BackendArtifactContract(BackendArtifactKind.RUST)
    artifact = tmp_path / "artifact.rs"
    artifact.write_bytes(b"return 1\n")
    source_key = cache._backend_artifact_source_key(
        "key", stdlib_object_cache_key=None, artifact_contract=contract
    )
    receipt = tmp_path / "receipt.json"
    sync._write_artifact_sync_state(
        receipt, source_key=source_key, tier="module", artifact=artifact
    )
    state = sync._read_artifact_sync_state(receipt)
    original_validate = cache._validate_backend_cache_artifact
    calls = []

    def validate(path, **kwargs):
        calls.append(path)
        return original_validate(path, **kwargs)

    def unexpected_receipt_hash(*args, **kwargs):
        raise AssertionError("backend receipt lookup rehashed validated bytes")

    monkeypatch.setattr(cache, "_validate_backend_cache_artifact", validate)
    monkeypatch.setattr(sync, "stable_regular_file_identity", unexpected_receipt_hash)
    hit = cache._synced_backend_output_cache_hit(
        state,
        artifact,
        artifact.stat(),
        artifact_contract=contract,
        cache_key="key",
        function_cache_key="functions",
        stdlib_object_cache_key=None,
    )
    assert hit is not None and hit.tier == "module"
    assert hit.identity.path == artifact
    assert calls == [artifact]


def test_daemon_sync_reuses_validation_identity_for_module_chunk_closure(
    tmp_path, monkeypatch
):
    _stub_external_inspectors(monkeypatch)
    contract = BackendArtifactContract(BackendArtifactKind.NATIVE_OBJECT)
    original, _replacement = _artifact_pair(contract)
    artifact = tmp_path / ("artifact" + contract.suffix)
    artifact.write_bytes(original)
    source_key = cache._backend_artifact_source_key(
        "key", stdlib_object_cache_key=None, artifact_contract=contract
    )
    receipt = sync._artifact_sync_state_path(tmp_path, artifact)
    sync._write_artifact_sync_state(
        receipt, source_key=source_key, tier="module", artifact=artifact
    )
    state = sync._read_artifact_sync_state(receipt)
    original_validate = cache._validate_backend_cache_artifact
    validated = []
    closure_identities = []

    def validate(path, **kwargs):
        identity = original_validate(path, **kwargs)
        validated.append(identity)
        return identity

    def check_chunks(path, stdlib_object_path, *, target_triple, identity):
        assert path == artifact
        assert target_triple == contract.target_triple
        closure_identities.append(identity)
        return False

    monkeypatch.setattr(cache, "_validate_backend_cache_artifact", validate)
    monkeypatch.setattr(
        cache, "_native_object_has_unresolved_module_chunks", check_chunks
    )
    assert cache._backend_daemon_skip_output_sync_flags(
        tmp_path,
        artifact,
        cache_key="key",
        function_cache_key="functions",
        artifact_contract=contract,
        state_path=receipt,
        state=state,
    ) == (True, False)
    assert len(validated) == 1
    assert closure_identities == validated
    assert closure_identities[0] is validated[0]


def test_disabled_cache_never_admits_empty_key_receipt(tmp_path, monkeypatch):
    artifact = tmp_path / "artifact.rs"
    artifact.write_bytes(b"return 1\n")
    receipt = sync._artifact_sync_state_path(tmp_path, artifact)
    sync._write_artifact_sync_state(
        receipt, source_key="", tier="module", artifact=artifact
    )
    state = sync._read_artifact_sync_state(receipt)

    def unexpected_validation(*args, **kwargs):
        raise AssertionError("cache-disabled receipt must not inspect output")

    monkeypatch.setattr(
        cache, "_validate_backend_cache_artifact", unexpected_validation
    )
    assert cache._backend_daemon_skip_output_sync_flags(
        tmp_path,
        artifact,
        cache_key=None,
        function_cache_key=None,
        artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
        state_path=receipt,
        state=state,
    ) == (False, False)


def test_sync_matching_preserves_native_inspection_failure(tmp_path, monkeypatch):
    artifact = tmp_path / "artifact.o"
    artifact.write_bytes(b"artifact")
    contract = BackendArtifactContract(BackendArtifactKind.NATIVE_OBJECT)
    source_key = cache._backend_artifact_source_key(
        "key", stdlib_object_cache_key=None, artifact_contract=contract
    )
    state = {"version": 2, "source_key": source_key, "tier": "module"}

    def fail_inspection(*args, **kwargs):
        raise cache.NativeSymbolInspectionError(artifact, ["reader unavailable"])

    monkeypatch.setattr(cache, "_validate_backend_cache_artifact", fail_inspection)
    with pytest.raises(cache.NativeSymbolInspectionError, match="reader unavailable"):
        cache._backend_daemon_skip_output_sync_flags(
            tmp_path,
            artifact,
            cache_key="key",
            function_cache_key=None,
            artifact_contract=contract,
            state_path=tmp_path / "receipt.json",
            state=state,
        )


@pytest.mark.parametrize("stage", ["materialize", "stage"])
@pytest.mark.parametrize(
    "contract", _CONTRACTS, ids=lambda c: f"{c.kind.value}-{c.target_triple}"
)
def test_self_consistent_receipt_cannot_hide_same_key_content_conflict(
    tmp_path, monkeypatch, stage, contract
):
    _stub_external_inspectors(monkeypatch)
    original, conflicting = _artifact_pair(contract)
    output = tmp_path / ("output" + contract.suffix)
    candidate = tmp_path / ("candidate" + contract.suffix)
    output.write_bytes(original)
    candidate.write_bytes(conflicting)
    source_key = cache._backend_artifact_source_key(
        "key", stdlib_object_cache_key=None, artifact_contract=contract
    )
    receipt = sync._artifact_sync_state_path(tmp_path, output)
    sync._write_artifact_sync_state(
        receipt, source_key=source_key, tier="module", artifact=output
    )
    warnings = []
    if stage == "materialize":
        assert not cache._materialize_cached_backend_artifact(
            tmp_path,
            candidate,
            output,
            tier="module",
            source_key=source_key,
            cache_path=None,
            warnings=warnings,
            artifact_contract=contract,
        )
        assert len(warnings) == 1 and "conflicting content" in warnings[0]
    else:
        error = cache._stage_backend_output_and_caches(
            tmp_path,
            candidate,
            output,
            cache_path=None,
            cache_key="key",
            stdlib_object_cache_key=None,
            function_cache_path=None,
            warnings=warnings,
            output_already_synced=True,
            artifact_contract=contract,
        )
        assert error is not None and "conflicting content" in error
    assert output.read_bytes() == original
    assert candidate.read_bytes() == conflicting


@pytest.mark.parametrize("tier", ["module", "function"])
def test_warm_output_and_daemon_share_native_chunk_closure(tmp_path, monkeypatch, tier):
    _stub_external_inspectors(monkeypatch)
    contract = BackendArtifactContract(BackendArtifactKind.NATIVE_OBJECT)
    output = tmp_path / ("output" + contract.suffix)
    output.write_bytes(_artifact_pair(contract)[0])
    source_key = cache._backend_artifact_source_key(
        "key", stdlib_object_cache_key=None, artifact_contract=contract
    )
    receipt = sync._artifact_sync_state_path(tmp_path, output)
    sync._write_artifact_sync_state(
        receipt, source_key=source_key, tier=tier, artifact=output
    )
    monkeypatch.setattr(
        cache,
        "_read_native_global_symbol_facts",
        lambda *a, **kw: cache._NativeGlobalSymbolFacts(
            frozenset({"molt_main"}),
            frozenset({"user__molt_module_chunk_1"}),
            frozenset({"molt_main"}),
        ),
    )
    assert cache._backend_daemon_skip_output_sync_flags(
        tmp_path,
        output,
        cache_key="key",
        function_cache_key="key",
        artifact_contract=contract,
    ) == (False, False)
    assert cache._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[],
        output_artifact=output,
        cache_key="key",
        function_cache_key="key",
        cache_path=None,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        warnings=[],
        artifact_contract=contract,
    ) == (False, None)
