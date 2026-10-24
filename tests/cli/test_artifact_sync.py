"""Content receipt custody; external symbol/WASM tools are transport stubs only."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import replace
from pathlib import Path

from molt.cli import native_symbol_inspection
import pytest

from molt.backend_protocol import BackendEmissionOrigin, BackendStdlibOrigin
from molt.default_paths import backend_cache_sync_receipt_path
from molt.cli import artifact_sync as sync
from molt.cli import backend_cache as cache
from molt.cli import runtime_wasm_validation
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
    BackendArtifactValidationError,
)
from molt.toolchain_identity import stable_regular_file_identity
from tests.cli.native_link_test_support import static_archive_bytes
from tests.native_artifact_fixtures import native_relocatable_object
from tests.process_guard_common import install_module_view


def _receipt_payload(source_key: str) -> dict[str, object]:
    # Literal wire shape is independent of the writer under test.
    return {
        "version": 3,
        "source_key": source_key,
        "tier": "module",
        "size": 9,
        "sha256": hashlib.sha256(b"return 1\n").hexdigest(),
        "generation": [11, 22],
        "emission_origin": None,
    }


def test_artifact_sync_payload_bound_precedes_warm_parse_cache(tmp_path, monkeypatch):
    path = tmp_path / "receipt.json"
    payload = _receipt_payload("key")
    path.write_text(json.dumps(payload), encoding="utf-8")
    assert sync._read_artifact_sync_state(path) == payload
    monkeypatch.setattr(sync, "RUNTIME_ARTIFACT_METADATA_MAX_BYTES", 8)
    assert sync._read_artifact_sync_state(path) is None
    assert path not in sync._ARTIFACT_SYNC_STATE_CACHE


@pytest.fixture(autouse=True)
def isolated_receipt_caches():
    sync._ARTIFACT_SYNC_STATE_CACHE.clear()
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    yield
    sync._ARTIFACT_SYNC_STATE_CACHE.clear()
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()


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
    original = _receipt_payload("old")
    replacement = _receipt_payload("new")
    receipt.write_text(json.dumps(original), encoding="utf-8")
    first = sync._read_artifact_sync_state(receipt)
    assert first == original
    _replace_restoring_mtime(
        receipt, json.dumps(replacement).encode(), in_place=in_place
    )
    second = sync._read_artifact_sync_state(receipt)
    assert second == replacement
    assert second is not first


def test_warm_payload_cache_validates_bytes_and_reuses_only_parsing(
    tmp_path, monkeypatch
):
    receipt = tmp_path / "receipt.json"
    receipt.write_text(json.dumps(_receipt_payload("key")), encoding="utf-8")
    captures = []
    capture = sync.capture_stable_regular_file

    def counted_capture(path, **kwargs):
        captures.append(path)
        return capture(path, **kwargs)

    monkeypatch.setattr(sync, "capture_stable_regular_file", counted_capture)
    first = sync._read_artifact_sync_state(receipt)

    def unexpected_parse(*args, **kwargs):
        raise AssertionError("unchanged receipt payload must not be parsed twice")

    with monkeypatch.context() as warm:
        warm.setattr(sync.json, "loads", unexpected_parse)
        assert sync._read_artifact_sync_state(receipt) is first
    assert captures == [receipt, receipt]
    receipt.unlink()
    assert sync._read_artifact_sync_state(receipt) is None


def test_publishing_payload_does_not_bind_callers_dict_to_a_peer_generation(
    tmp_path, monkeypatch
):
    receipt = tmp_path / "receipt.json"
    artifact = tmp_path / "output.rs"
    artifact.write_bytes(b"return 1\n")
    original_write = sync._atomic_write_json

    def publish_then_peer_replace(path, payload, **kwargs):
        original_write(path, payload, **kwargs)
        _replace_restoring_mtime(
            path, path.read_bytes().replace(b"old", b"new"), in_place=False
        )

    monkeypatch.setattr(sync, "_atomic_write_json", publish_then_peer_replace)
    sync._write_artifact_sync_state(
        receipt, source_key="old", tier="module", artifact=artifact
    )
    published = sync._read_artifact_sync_state(receipt)
    assert published is not None and published["source_key"] == "new"


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


def test_receipt_validates_path_bound_identity_against_current_content(
    tmp_path,
):
    artifact = tmp_path / "artifact.rs"
    artifact.write_bytes(b"return 1\n")
    identity = stable_regular_file_identity(artifact, label="test output")
    receipt = tmp_path / "receipt.json"

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
    current = stable_regular_file_identity(artifact, label="current output")
    # The old digest must be rejected even if all detached metadata agrees.
    identity = replace(current, sha256=identity.sha256)
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
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        lambda *args, **kwargs: native_symbol_inspection._NativeGlobalSymbolFacts(
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
def test_backend_output_staged_through_a_symlinked_directory_publishes_its_cache(
    tmp_path, monkeypatch, contract
):
    # macOS hands out temporary directories through /var -> /private/var, so
    # an --out-dir there stages backend output below a symlink. The identity
    # the stage captured must still admit the same file for cache publication.
    _stub_external_inspectors(monkeypatch)
    original, replacement = _artifact_pair(contract)
    real = tmp_path / "real"
    real.mkdir()
    linked = tmp_path / "linked"
    linked.symlink_to(real, target_is_directory=True)
    staged = linked / ("backend" + contract.suffix)
    staged.write_bytes(original)
    identity = cache._validate_backend_cache_artifact(
        staged, artifact_contract=contract
    )
    destination = tmp_path / "cache" / ("key" + contract.suffix)
    warnings: list[str] = []
    published = cache._publish_immutable_backend_cache_artifact(
        staged,
        destination,
        artifact_contract=contract,
        warnings=warnings,
        identity=identity,
    ).identity
    assert destination.read_bytes() == original
    assert published.sha256 == hashlib.sha256(original).hexdigest()
    assert not warnings
    # The captured identity still names one file: another path is refused.
    other = linked / ("other" + contract.suffix)
    other.write_bytes(replacement)
    with pytest.raises(BackendArtifactValidationError):
        cache._validate_backend_cache_artifact(
            other, artifact_contract=contract, identity=identity
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
        ).error
        is None
    )
    assert output.read_bytes() == original
    assert warnings == []


def test_backend_receipt_decision_validates_shape_once_and_readmits_content(
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

    receipt_hashes = []
    capture = sync.stable_regular_file_identity

    def receipt_hash(path, **kwargs):
        receipt_hashes.append(path)
        return capture(path, **kwargs)

    monkeypatch.setattr(cache, "_validate_backend_cache_artifact", validate)
    monkeypatch.setattr(sync, "stable_regular_file_identity", receipt_hash)
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
    assert receipt_hashes == [artifact]


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
    state = _receipt_payload(source_key)

    def fail_inspection(*args, **kwargs):
        raise native_symbol_inspection.NativeSymbolInspectionError(
            artifact, ["reader unavailable"]
        )

    monkeypatch.setattr(cache, "_validate_backend_cache_artifact", fail_inspection)
    with pytest.raises(
        native_symbol_inspection.NativeSymbolInspectionError, match="reader unavailable"
    ):
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
        ).error
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
        native_symbol_inspection,
        "_read_native_global_symbol_facts",
        lambda *a, **kw: native_symbol_inspection._NativeGlobalSymbolFacts(
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
    assert (
        cache._try_cached_backend_candidates(
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
        )
        is None
    )


def _original_emission_payload(
    content: bytes, *, process_id: int, scope: str
) -> dict[str, object]:
    # Literal protocol fields and independent byte hashes, not a publisher oracle.
    lease = b"fixture addressed input"
    return {
        "schema": "molt.backend-emission-origin.v1",
        "producer": {"kind": "one-shot", "process_id": process_id, "scope": scope},
        "input": {
            "schema": "molt.addressed-program-input.v1",
            "lease": {"sha256": hashlib.sha256(lease).hexdigest(), "size": len(lease)},
            "function_digests": "",
        },
        "output": {"sha256": hashlib.sha256(content).hexdigest(), "size": len(content)},
        "codegen": {"kind": "in-process", "process_id": process_id},
    }


@pytest.mark.parametrize("tier", ["module", "function"])
def test_original_emitter_survives_publication_winner_promotion_and_output_reuse(
    tmp_path, tier
):
    content = b"fn original() {}\n"
    expected = _original_emission_payload(content, process_id=101, scope="a" * 64)
    later = _original_emission_payload(content, process_id=202, scope="b" * 64)
    original = BackendEmissionOrigin.from_payload(expected)
    next_origin = BackendEmissionOrigin.from_payload(later)
    source = tmp_path / "emitted.rs"
    source.write_bytes(content)
    contract = BackendArtifactContract(BackendArtifactKind.RUST)
    module = tmp_path / "cache" / "module.rs"
    candidate = module if tier == "module" else module.with_name("function.rs")
    first = cache._publish_immutable_backend_cache_artifact(
        source,
        candidate,
        artifact_contract=contract,
        warnings=[],
        emission_origin=original,
    )
    receipt = backend_cache_sync_receipt_path(candidate)
    sealed_receipt = receipt.read_bytes()
    assert first.emission_origin is not None
    assert first.emission_origin.payload() == expected
    # Same bytes from a later process do not let it claim the winning generation.
    winner = cache._publish_immutable_backend_cache_artifact(
        source,
        candidate,
        artifact_contract=contract,
        warnings=[],
        emission_origin=next_origin,
    )
    assert winner.emission_origin is not None
    assert winner.emission_origin.payload() == expected
    assert receipt.read_bytes() == sealed_receipt
    source.unlink()
    sync._ARTIFACT_SYNC_STATE_CACHE.clear()
    output = tmp_path / "build" / "output.rs"
    hit = cache._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[(tier, candidate)],
        output_artifact=output,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key="function-key",
        cache_path=module,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        warnings=[],
        require_origin=True,
    )
    assert hit is not None and hit.tier == tier
    assert hit.emission_origin is not None and hit.emission_origin.payload() == expected
    assert output.read_bytes() == content
    assert hit.identity.sha256 == hashlib.sha256(content).hexdigest()
    assert (
        json.loads(backend_cache_sync_receipt_path(module).read_bytes())[
            "emission_origin"
        ]
        == expected
    )
    output_receipt = sync._artifact_sync_state_path(tmp_path, output)
    synchronized = output_receipt.read_bytes()
    assert json.loads(synchronized)["emission_origin"] == expected
    assert json.loads(synchronized)["tier"] == "module"
    # Even a new compile finishing after synchronization retains that output's producer.
    source.write_bytes(content)
    staged = cache._stage_backend_output_and_caches(
        tmp_path,
        source,
        output,
        cache_path=module,
        cache_key="module-key",
        stdlib_object_cache_key=None,
        function_cache_path=candidate,
        warnings=[],
        artifact_contract=contract,
        emission_origin=next_origin,
        output_already_synced=True,
    )
    assert staged.error is None
    assert (
        staged.emission_origin is not None
        and staged.emission_origin.payload() == expected
    )
    assert output_receipt.read_bytes() == synchronized
    assert not source.exists()
    # The immutable winner also owns a newly materialized, unsynchronized output.
    source.write_bytes(content)
    fresh_output = tmp_path / "build" / "second.rs"
    staged = cache._stage_backend_output_and_caches(
        tmp_path,
        source,
        fresh_output,
        cache_path=module,
        cache_key="module-key",
        stdlib_object_cache_key=None,
        function_cache_path=candidate,
        warnings=[],
        artifact_contract=contract,
        emission_origin=next_origin,
        output_already_synced=False,
    )
    assert staged.error is None
    assert (
        staged.emission_origin is not None
        and staged.emission_origin.payload() == expected
    )
    assert fresh_output.read_bytes() == content
    module.unlink()
    candidate.unlink(missing_ok=True)
    sync._ARTIFACT_SYNC_STATE_CACHE.clear()
    warm = cache._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=[("module", module)],
        output_artifact=output,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key="function-key",
        cache_path=module,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        warnings=[],
        require_origin=True,
    )
    assert warm is not None and warm.tier == "module"
    assert (
        warm.emission_origin is not None and warm.emission_origin.payload() == expected
    )
    assert output.read_bytes() == content


@pytest.mark.parametrize(
    "receipt_fault",
    ["absent", "wrong-generation", "wrong-output", "missing-field", "legacy-v2"],
)
def test_bytes_only_or_stale_origin_refuses_capture_but_preserves_ordinary_reuse(
    tmp_path, receipt_fault
):
    content = b"fn original() {}\n"
    source = tmp_path / "source.rs"
    source.write_bytes(content)
    candidate = tmp_path / "cache" / "module.rs"
    contract = BackendArtifactContract(BackendArtifactKind.RUST)
    expected = _original_emission_payload(content, process_id=101, scope="a" * 64)
    cache._publish_immutable_backend_cache_artifact(
        source,
        candidate,
        artifact_contract=contract,
        warnings=[],
        emission_origin=BackendEmissionOrigin.from_payload(expected),
    )
    receipt = backend_cache_sync_receipt_path(candidate)
    payload = json.loads(receipt.read_bytes())
    if receipt_fault == "absent":
        receipt.unlink()
    else:
        if receipt_fault == "wrong-generation":
            payload["generation"][1] += 1
        elif receipt_fault == "wrong-output":
            payload["emission_origin"]["output"]["sha256"] = "f" * 64
        elif receipt_fault == "missing-field":
            del payload["emission_origin"]
        else:
            payload["version"] = 2
        receipt.write_text(json.dumps(payload), encoding="utf-8")
    output = tmp_path / "build" / "out.rs"
    arguments = dict(
        project_root=tmp_path,
        cache_candidates=[("module", candidate)],
        output_artifact=output,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=candidate,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        warnings=[],
    )
    with pytest.raises(
        BackendArtifactValidationError, match="requires original emission origin"
    ):
        cache._try_cached_backend_candidates(**arguments, require_origin=True)
    assert not output.exists()
    assert candidate.read_bytes() == content
    ordinary = cache._try_cached_backend_candidates(**arguments)
    assert ordinary is not None and ordinary.emission_origin is None
    assert output.read_bytes() == content
    with pytest.raises(
        BackendArtifactValidationError, match="synchronized output contains bytes only"
    ):
        cache._try_cached_backend_candidates(**arguments, require_origin=True)


@pytest.mark.parametrize("has_new_origin", [False, True])
def test_new_cache_generation_never_inherits_an_orphaned_receipt(
    tmp_path, has_new_origin
):
    content = b"fn original() {}\n"
    source = tmp_path / "source.rs"
    source.write_bytes(content)
    candidate = tmp_path / "cache.rs"
    receipt = backend_cache_sync_receipt_path(candidate)
    old = _original_emission_payload(content, process_id=101, scope="a" * 64)
    new = _original_emission_payload(content, process_id=202, scope="b" * 64)
    sync._write_artifact_sync_state(
        receipt,
        source_key=candidate.name,
        tier="immutable",
        artifact=source,
        emission_origin=BackendEmissionOrigin.from_payload(old),
    )
    published = cache._publish_immutable_backend_cache_artifact(
        source,
        candidate,
        artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
        warnings=[],
        emission_origin=BackendEmissionOrigin.from_payload(new)
        if has_new_origin
        else None,
    )
    assert candidate.read_bytes() == content
    if has_new_origin:
        assert published.emission_origin is not None
        assert published.emission_origin.payload() == new
        assert json.loads(receipt.read_bytes())["emission_origin"] == new
    else:
        assert published.emission_origin is None
        assert not receipt.exists()


def test_interrupted_origin_publication_leaves_only_uncapturable_bytes(
    tmp_path, monkeypatch
):
    content = b"fn original() {}\n"
    source = tmp_path / "source.rs"
    source.write_bytes(content)
    candidate = tmp_path / "cache.rs"
    expected = _original_emission_payload(content, process_id=101, scope="a" * 64)
    contract = BackendArtifactContract(BackendArtifactKind.RUST)

    def interrupt_receipt(*args, **kwargs):
        raise OSError("injected receipt publication failure")

    with monkeypatch.context() as publication:
        publication.setattr(cache, "_write_artifact_sync_state", interrupt_receipt)
        with pytest.raises(OSError, match="injected receipt publication failure"):
            cache._publish_immutable_backend_cache_artifact(
                source,
                candidate,
                artifact_contract=contract,
                warnings=[],
                emission_origin=BackendEmissionOrigin.from_payload(expected),
            )
    assert source.read_bytes() == candidate.read_bytes() == content
    assert not backend_cache_sync_receipt_path(candidate).exists()
    output = tmp_path / "out.rs"
    with pytest.raises(
        BackendArtifactValidationError, match="requires original emission origin"
    ):
        cache._try_cached_backend_candidates(
            project_root=tmp_path,
            cache_candidates=[("module", candidate)],
            output_artifact=output,
            artifact_contract=contract,
            cache_key="key",
            function_cache_key=None,
            cache_path=candidate,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            warnings=[],
            require_origin=True,
        )
    assert not output.exists()
    # A later emitter cannot repair that old generation by attaching its origin.
    later = _original_emission_payload(content, process_id=202, scope="b" * 64)
    existing = cache._publish_immutable_backend_cache_artifact(
        source,
        candidate,
        artifact_contract=contract,
        warnings=[],
        emission_origin=BackendEmissionOrigin.from_payload(later),
    )
    assert existing.emission_origin is None
    assert not backend_cache_sync_receipt_path(candidate).exists()


def test_competing_namespace_winner_keeps_its_origin_and_receipt(tmp_path, monkeypatch):
    content = b"fn original() {}\n"
    expected = _original_emission_payload(content, process_id=101, scope="a" * 64)
    later = _original_emission_payload(content, process_id=202, scope="b" * 64)
    source = tmp_path / "source.rs"
    source.write_bytes(content)
    destination = tmp_path / "cache.rs"
    receipt = backend_cache_sync_receipt_path(destination)
    winning_receipts = []

    def competing_link(_staged, target):
        assert target == destination
        # Order the namespace race exactly after the absence check. The
        # competing generation owns its bytes and its original emission record.
        target.write_bytes(content)
        sync._write_artifact_sync_state(
            receipt,
            source_key=destination.name,
            tier="immutable",
            artifact=target,
            emission_origin=BackendEmissionOrigin.from_payload(expected),
        )
        winning_receipts.append(receipt.read_bytes())
        raise FileExistsError(target)

    install_module_view(monkeypatch, "os", os, cache, link=competing_link)
    winner = cache._publish_immutable_backend_cache_artifact(
        source,
        destination,
        artifact_contract=BackendArtifactContract(BackendArtifactKind.RUST),
        warnings=[],
        emission_origin=BackendEmissionOrigin.from_payload(later),
    )
    assert len(winning_receipts) == 1
    assert winner.emission_origin is not None
    assert winner.emission_origin.payload() == expected
    assert destination.read_bytes() == content
    assert receipt.read_bytes() == winning_receipts[0]
    assert not list(tmp_path.glob(".molt-*.tmp"))


@pytest.mark.parametrize("defect", [None, "output", "receipt", "existing-unknown"])
def test_sealed_cache_transfer_republishes_generation_without_retagging(
    tmp_path, defect
):
    contract = BackendArtifactContract(BackendArtifactKind.RUST)
    content = b"fn original() {}\n"
    name = "a" * 64 + ".artifact-" + contract.cache_identity + ".rs"
    source = tmp_path / "sealed" / name
    source.parent.mkdir()
    source.write_bytes(content)
    origin = _original_emission_payload(content, process_id=101, scope="c" * 64)
    # A transported snapshot cannot retain the source filesystem's generation.
    receipt = {
        "version": 3,
        "source_key": name,
        "tier": "immutable",
        "size": len(content),
        "sha256": hashlib.sha256(content).hexdigest(),
        "generation": [987654, 876543],
        "emission_origin": origin,
    }
    if defect == "receipt":
        receipt["source_key"] = "another-key"
    backend_cache_sync_receipt_path(source).write_text(json.dumps(receipt))
    if defect == "output":
        source.write_bytes(b"fn replaced() {}\n")
    destination = tmp_path / "live" / name
    if defect == "existing-unknown":
        destination.parent.mkdir()
        destination.write_bytes(content)
    if defect:
        with pytest.raises((BackendArtifactValidationError, ValueError)):
            cache.restore_backend_cache_artifact(
                source, destination, artifact_contract=contract
            )
        if defect == "existing-unknown":
            assert not backend_cache_sync_receipt_path(destination).exists()
    else:
        cache.restore_backend_cache_artifact(
            source, destination, artifact_contract=contract
        )
        actual = json.loads(backend_cache_sync_receipt_path(destination).read_text())
        assert actual["emission_origin"] == origin
        assert actual["generation"] == [
            destination.stat().st_dev,
            destination.stat().st_ino,
        ]
        assert actual["generation"] != receipt["generation"]
        assert destination.read_bytes() == content
        assert (
            json.loads(backend_cache_sync_receipt_path(source).read_text()) == receipt
        )


@pytest.mark.parametrize("defect", [None, "escape", "other-origin", "symlink"])
def test_admission_cache_export_keeps_only_consumed_original_scope(
    tmp_path, monkeypatch, defect
):
    root = tmp_path / "cache"
    root.mkdir()
    contract = BackendArtifactContract(BackendArtifactKind.RUST)
    content = b"fn original() {}\n"
    source = tmp_path / "emitted.rs"
    source.write_bytes(content)
    original = BackendEmissionOrigin.from_payload(
        _original_emission_payload(content, process_id=101, scope="a" * 64)
    )
    other = BackendEmissionOrigin.from_payload(
        _original_emission_payload(content, process_id=102, scope="b" * 64)
    )
    paths = []
    for index, origin in enumerate((original, other)):
        destination = root / (
            str(index) * 64 + ".artifact-" + contract.cache_identity + ".rs"
        )
        cache._publish_immutable_backend_cache_artifact(
            source,
            destination,
            artifact_contract=contract,
            warnings=[],
            emission_origin=origin,
        )
        paths.append(destination)
    (root / "unrelated").write_bytes(b"preserve")
    selected_path = paths[0]
    if defect == "escape":
        selected_path = source
    elif defect == "other-origin":
        selected_path = paths[1]
    elif defect == "symlink":
        selected_path = root / (
            "d" * 64 + ".artifact-" + contract.cache_identity + ".rs"
        )
        selected_path.symlink_to(paths[0])
    # The producer already supplied its coordinate; unrelated directory contents
    # must not be enumerated, opened or hashed to rediscover it.
    with monkeypatch.context() as no_scan:
        capture = cache.stable_regular_file_identity

        def reject_unrelated_read(path, **kwargs):
            assert path == selected_path, (
                "export must not read unrelated cache artifacts"
            )
            return capture(path, **kwargs)

        no_scan.setattr(cache, "stable_regular_file_identity", reject_unrelated_read)
        if defect:
            with pytest.raises(ValueError):
                with cache.compilation_cache_export_paths(
                    root,
                    cache={"compiler": str(selected_path), "stdlib": None},
                    compiler_origin=original,
                    stdlib_origin=None,
                    artifact_contract=contract,
                ):
                    raise AssertionError("invalid coordinate exported")
        else:
            with cache.compilation_cache_export_paths(
                root,
                cache={"compiler": str(selected_path), "stdlib": None},
                compiler_origin=original,
                stdlib_origin=None,
                artifact_contract=contract,
            ) as selected:
                assert set(selected) == {
                    paths[0],
                    backend_cache_sync_receipt_path(paths[0]),
                }
    assert paths[1].exists()
    assert (root / "unrelated").read_bytes() == b"preserve"


@pytest.mark.parametrize(
    "defect", [None, "escape", "symlink", "other-key", "other-origin"]
)
def test_admission_cache_export_fences_consumed_stdlib_generation(
    tmp_path, monkeypatch, defect
):
    _stub_external_inspectors(monkeypatch)
    root = (tmp_path / "cache").resolve()
    root.mkdir()
    contract = BackendArtifactContract(
        BackendArtifactKind.NATIVE_ARCHIVE, "aarch64-unknown-linux-gnu"
    )
    obj = native_relocatable_object(
        target_triple="aarch64-unknown-linux-gnu", symbols=("molt_main",)
    )
    raw_archive = static_archive_bytes(obj)
    archive_bytes = raw_archive[:8] + b"molt_00000000.o/".ljust(16) + raw_archive[24:]
    compiler = BackendEmissionOrigin.from_payload(
        _original_emission_payload(archive_bytes, process_id=101, scope="a" * 64)
    )
    worker_payload = {
        "schema": "molt.native-worker-origin.v2",
        "process_id": 202,
        "scope": "b" * 64,
        "input": {"sha256": hashlib.sha256(b"body").hexdigest(), "size": 4},
        "output": {"sha256": hashlib.sha256(obj).hexdigest(), "size": len(obj)},
    }
    stdlib_payload = {
        "schema": "molt.stdlib-archive-origin.v1",
        "output": {
            "sha256": hashlib.sha256(archive_bytes).hexdigest(),
            "size": len(archive_bytes),
        },
        "workers": [worker_payload],
    }
    stdlib = BackendStdlibOrigin.from_payload(stdlib_payload)
    key = "c" * 64
    archive = root / "shared-stdlib" / key / "stdlib.a"
    archive.parent.mkdir(parents=True)
    archive.write_bytes(archive_bytes)
    archive.with_suffix(".origin.json").write_text(json.dumps(stdlib_payload))
    app = root / (
        "d" * 64 + ".artifact-" + contract.cache_identity + ".stdlib-" + key + ".a"
    )
    cache._publish_immutable_backend_cache_artifact(
        archive, app, artifact_contract=contract, warnings=[], emission_origin=compiler
    )
    coordinate = archive
    if defect in ("escape", "other-key", "symlink"):
        coordinate = (
            tmp_path / "outside" / "shared-stdlib" / key / "stdlib.a"
            if defect == "escape"
            else root / "shared-stdlib" / ("e" * 64) / "stdlib.a"
        )
        coordinate.parent.mkdir(parents=True)
        if defect == "symlink":
            coordinate.symlink_to(archive)
        else:
            coordinate.write_bytes(archive_bytes)
            coordinate.with_suffix(".origin.json").write_text(
                json.dumps(stdlib_payload)
            )
    elif defect == "other-origin":
        worker_payload["scope"] = "f" * 64
        archive.with_suffix(".origin.json").write_text(json.dumps(stdlib_payload))
    context = cache.compilation_cache_export_paths(
        root,
        cache={"compiler": str(app), "stdlib": str(coordinate)},
        compiler_origin=compiler,
        stdlib_origin=stdlib,
        artifact_contract=contract,
    )
    if defect:
        with pytest.raises(ValueError):
            with context:
                raise AssertionError("unrelated stdlib generation exported")
    else:
        with context as selected:
            assert set(selected) == {
                app,
                backend_cache_sync_receipt_path(app),
                archive,
                archive.with_suffix(".origin.json"),
            }
        assert stdlib.workers[0].scope == "b" * 64
    assert archive.read_bytes() == archive_bytes
