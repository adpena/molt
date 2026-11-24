from __future__ import annotations

import hashlib
import json
import os
from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.product_store import ProductStore, ProductStoreError
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.process_environment import ProcessEnvironment
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmCodegenUse,
    RuntimeWasmReceiptRole,
    RuntimeWasmGeneration,
    RuntimeWasmTrust,
    SourceRuntimeWasmGeneration,
    bind_runtime_wasm_codegen,
    _generation_receipts,
    hydrate_runtime_wasm_product,
    publish_runtime_wasm_product,
    publish_stored_runtime_wasm_generation,
    read_runtime_wasm_generation,
    read_runtime_wasm_product,
    read_trusted_runtime_wasm_generation,
    read_trusted_runtime_wasm_reloc,
    runtime_wasm_generation_identities,
    runtime_wasm_product_identities,
    runtime_wasm_generation_path,
    stored_runtime_wasm_generation,
)
from tests.runtime_build_identity_helper import (
    runtime_build_identity,
    runtime_cargo_environment_inputs,
)
from molt.toolchain_identity import StableRegularFileError, stable_regular_file_identity


def _identity(
    kind: str,
    pair_seed: str = "pair",
) -> RuntimeBuildIdentity:
    return runtime_build_identity(kind, pair_seed)


def _source_pair(root: Path, shared: bytes, reloc: bytes) -> tuple[Path, Path]:
    root.mkdir()
    source_shared = root / "shared.wasm"
    source_reloc = root / "reloc.wasm"
    source_shared.write_bytes(shared)
    source_reloc.write_bytes(reloc)
    return source_shared, source_reloc


def _store(root: Path) -> ProductStore:
    return ProductStore(root / "store" / "wasm-runtime")


def _publish_pair(
    root: Path,
    *,
    pair_seed: str = "pair",
    shared: bytes = b"shared",
    reloc: bytes = b"reloc",
    cargo_environment: CargoEnvironmentInputs | None = None,
):
    source_shared, source_reloc = _source_pair(
        root / f"source-{pair_seed}", shared, reloc
    )
    shared_identity = _identity("shared", pair_seed)
    reloc_identity = _identity("reloc", pair_seed)
    generation = publish_stored_runtime_wasm_generation(
        _store(root),
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
        cargo_environment=cargo_environment or runtime_cargo_environment_inputs(),
    )
    return generation, shared_identity, reloc_identity


@pytest.mark.parametrize(
    "failure", ["shared-collision", "reloc-collision", "shared-source"]
)
def test_runtime_pair_staging_reaps_only_successfully_created_files(
    tmp_path, monkeypatch, failure
):
    from molt.cli import runtime_wasm_generation as module

    source_shared, source_reloc = _source_pair(tmp_path / "source", b"shared", b"reloc")
    shared = tmp_path / "molt_runtime.wasm"
    reloc = tmp_path / "molt_runtime_reloc.wasm"
    shared.write_bytes(b"previous shared")
    reloc.write_bytes(b"previous reloc")
    stages = {
        path: module.staged_file_path(path, purpose="generation")
        for path in (shared, reloc)
    }
    collision = stages[shared if failure == "shared-collision" else reloc]
    collision.write_bytes(b"another owner's stage")
    before = collision.stat()
    monkeypatch.setattr(module, "staged_file_path", lambda path, **kwargs: stages[path])
    if failure == "shared-source":
        source_shared.unlink()

    with pytest.raises(
        StableRegularFileError if failure == "shared-source" else FileExistsError
    ):
        publish_runtime_wasm_product(
            shared,
            reloc,
            shared_identity=_identity("shared"),
            reloc_identity=_identity("reloc"),
            source_shared=source_shared,
            source_reloc=source_reloc,
        )

    assert collision.read_bytes() == b"another owner's stage"
    assert os.path.samestat(before, collision.stat())
    other = stages[reloc if collision == stages[shared] else shared]
    assert not other.exists()
    assert shared.read_bytes() == b"previous shared"
    assert reloc.read_bytes() == b"previous reloc"
    assert source_reloc.read_bytes() == b"reloc"
    assert not runtime_wasm_generation_path(shared).exists()


@pytest.mark.parametrize("change", ["provenance", "shared", "reloc", "roles"])
def test_codegen_identity_tracks_member_bytes_separately_from_provenance(
    tmp_path: Path, change: str
) -> None:
    generation, _, _ = _publish_pair(tmp_path)
    first = bind_runtime_wasm_codegen(generation, {"PyTuple_New"})
    shared = b"new-shared" if change == "shared" else b"shared"
    reloc = b"new-reloc" if change == "reloc" else b"reloc"
    if change == "roles":
        shared, reloc = reloc, shared
    republished, shared_identity, reloc_identity = _publish_pair(
        tmp_path, pair_seed="new-provenance", shared=shared, reloc=reloc
    )
    second = bind_runtime_wasm_codegen(republished, {"PyTuple_New"})
    linked_first = bind_runtime_wasm_codegen(
        generation, {"PyTuple_New"}, use=RuntimeWasmCodegenUse.LINKED
    )
    linked_second = bind_runtime_wasm_codegen(
        republished, {"PyTuple_New"}, use=RuntimeWasmCodegenUse.LINKED
    )
    assert first.generation.manifest != second.generation.manifest
    assert first.generation.manifest == generation.manifest
    assert (first.semantic_digest == second.semantic_digest) == (change == "provenance")
    assert linked_first.semantic_digest != first.semantic_digest
    assert (linked_first.semantic_digest == linked_second.semantic_digest) == (
        change in {"provenance", "shared"}
    )
    # Equal app inputs never authorize an old source/toolchain claim.
    assert (
        read_runtime_wasm_generation(
            first.generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )


def test_generation_receipts_require_exact_string_keyed_pair() -> None:
    shared = {"sha256": "a" * 64, "size": 1}
    reloc = {"sha256": "b" * 64, "size": 2}

    assert _generation_receipts({"shared": shared, "reloc": reloc}) == {
        "shared": shared,
        "reloc": reloc,
    }
    assert _generation_receipts({"shared": shared}) is None
    assert _generation_receipts({"shared": shared, "reloc": reloc, "old": {}}) is None
    assert _generation_receipts({"shared": {1: "alias"}, "reloc": reloc}) is None


def test_generation_requires_exact_pair_and_both_immutable_artifact_hashes(
    tmp_path: Path,
) -> None:
    generation, shared_identity, reloc_identity = _publish_pair(tmp_path)
    variant = _store(tmp_path).variant(
        shared_identity.family_digest, generation.cargo_environment
    )
    assert generation.manifest == runtime_wasm_generation_path(
        variant / "molt_runtime.wasm"
    )
    assert generation.shared.read_bytes() == b"shared"
    assert generation.reloc.read_bytes() == b"reloc"
    assert not (variant / "molt_runtime.wasm").exists()
    assert not (variant / "molt_runtime_reloc.wasm").exists()
    observed = read_runtime_wasm_generation(
        generation.manifest,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    assert observed is not None
    assert observed.receipt_identity == stable_regular_file_identity(
        generation.manifest, label="published generation receipt"
    )
    assert observed == generation

    generation.reloc.write_bytes(b"impersonated")
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )


def test_generation_rejects_self_asserted_identity_and_cross_pair_mix(
    tmp_path: Path,
) -> None:
    generation, shared_identity, reloc_identity = _publish_pair(tmp_path)
    payload = json.loads(generation.manifest.read_text(encoding="utf-8"))
    payload["product"]["receipts"]["shared"]["identity"]["digest"] = "0" * 64
    generation.manifest.write_text(json.dumps(payload), encoding="utf-8")
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=_identity("reloc", "other-pair"),
        )
        is None
    )


def test_receipt_stores_the_shared_family_once_and_rebuilds_both_members(
    tmp_path: Path,
) -> None:
    generation, shared_identity, reloc_identity = _publish_pair(tmp_path)
    payload = json.loads(generation.manifest.read_text(encoding="utf-8"))
    # HF-183: one family payload; each member keeps only its reference.
    assert (
        payload["product"]["family"] == shared_identity.to_dict()["payload"]["family"]
    )
    for kind, identity in (("shared", shared_identity), ("reloc", reloc_identity)):
        assert payload["product"]["receipts"][kind]["identity"] == {
            "digest": identity.digest,
            "compile_digest": identity.compile_digest,
            "member_kind": kind,
        }
    rebuilt = runtime_wasm_generation_identities(payload)
    assert [
        (item.digest, item.compile_digest, item.family_digest) for item in rebuilt
    ] == [
        (item.digest, item.compile_digest, item.family_digest)
        for item in (shared_identity, reloc_identity)
    ]

    # A family that no longer hashes to family_digest is refused by both readers.
    payload["product"]["family"]["schema"] = "tampered"
    generation.manifest.write_text(json.dumps(payload), encoding="utf-8")
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )
    with pytest.raises(ValueError):
        runtime_wasm_generation_identities(payload)


def test_reader_and_existing_member_race_reject_symlink_indirection(
    tmp_path: Path,
) -> None:
    generation, shared_identity, reloc_identity = _publish_pair(tmp_path)
    external = tmp_path / "outside.wasm"
    external.write_bytes(generation.shared.read_bytes())
    generation.shared.unlink()
    try:
        generation.shared.symlink_to(external)
    except OSError as exc:
        pytest.skip(f"file symlinks unavailable on this host: {exc}")

    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )
    # The variant exists, so publication refuses it rather than writing into it.
    source_shared, source_reloc = _source_pair(
        tmp_path / "second-source", b"shared", b"reloc"
    )
    with pytest.raises(ProductStoreError, match="exists but does not admit"):
        publish_stored_runtime_wasm_generation(
            _store(tmp_path),
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            source_shared=source_shared,
            source_reloc=source_reloc,
            cargo_environment=runtime_cargo_environment_inputs(),
        )


def test_binding_rejects_changed_member_content_with_matching_metadata(
    tmp_path: Path,
) -> None:
    generation, _, _ = _publish_pair(tmp_path)
    path = generation.shared
    before = path.stat()
    path.write_bytes(b"change")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    current = stable_regular_file_identity(path, label="current metadata fixture")
    generation = replace(
        generation,
        shared_member_identity=replace(
            current, sha256=generation.shared_member_identity.sha256
        ),
    )
    # Binding fences metadata only; the final link's verification rehashes.
    binding = bind_runtime_wasm_codegen(generation, None)
    with pytest.raises(ValueError, match="content changed since identity capture"):
        binding.verify()


def test_binding_rejects_changed_receipt_content_with_matching_metadata(
    tmp_path: Path,
) -> None:
    generation, _, _ = _publish_pair(tmp_path)
    binding = bind_runtime_wasm_codegen(generation, None)
    old = binding.generation.receipt_identity
    assert old is not None
    before = old.path.stat()
    data = bytearray(old.path.read_bytes())
    data[0] ^= 1
    old.path.write_bytes(data)
    os.utime(old.path, ns=(before.st_atime_ns, before.st_mtime_ns))
    current = stable_regular_file_identity(old.path, label="current metadata fixture")
    binding = replace(
        binding,
        generation=replace(
            binding.generation, receipt_identity=replace(current, sha256=old.sha256)
        ),
    )
    with pytest.raises(ValueError, match="content changed"):
        binding.verify()


def test_generation_staging_checks_supplied_digest_without_a_receipt(
    tmp_path: Path,
) -> None:
    from molt.cli import runtime_wasm_generation as module

    source = tmp_path / "source.wasm"
    source.write_bytes(b"before")
    observed = stable_regular_file_identity(source, label="stage fixture")
    before = source.stat()
    source.write_bytes(b"after!")
    os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
    current = stable_regular_file_identity(source, label="current metadata fixture")
    observed = replace(current, sha256=observed.sha256)
    with (tmp_path / "stage").open("xb") as staged_handle:
        with pytest.raises(ValueError, match="content changed"):
            module._stage_artifact(
                source,
                staged_handle,
                published_name="molt_runtime.wasm",
                identity=_identity("shared"),
                observed=observed,
            )


class _Clock:
    def __init__(self, now: float) -> None:
        self.now = now

    def time(self) -> float:
        return self.now


@pytest.fixture
def store_clock(monkeypatch: pytest.MonkeyPatch) -> _Clock:
    from molt.cli import generation_store as module

    clock = _Clock(1_000_000.0)
    monkeypatch.setattr(module, "time", clock)
    return clock


_HOUR = 60 * 60


def _publish_retained_product(
    root: Path, seed: str, shared: bytes, reloc: bytes
) -> RuntimeWasmGeneration:
    source_shared, source_reloc = _source_pair(root / f"source-{seed}", shared, reloc)
    return publish_runtime_wasm_product(
        root / "molt_runtime.wasm",
        root / "molt_runtime_reloc.wasm",
        shared_identity=_identity("shared", seed),
        reloc_identity=_identity("reloc", seed),
        source_shared=source_shared,
        source_reloc=source_reloc,
    )


@pytest.mark.parametrize("ledger", [None, "{not json", '{"schema": "other"}'])
def test_unknown_retained_members_start_their_window_at_first_sight(
    tmp_path: Path, store_clock: _Clock, ledger: str | None
) -> None:
    """An installed cell's retained directory evicts members no build selects."""
    retained = tmp_path / "retained"
    retained.mkdir()
    stray = retained / f"molt_runtime.wasm.{'a' * 64}.runtime-wasm-member"
    stray.write_bytes(b"left by an older build")
    if ledger is not None:
        (retained / ".runtime-wasm-store.json").write_text(ledger, encoding="utf-8")

    _publish_retained_product(retained, "first", b"f-shared", b"f-reloc")
    assert stray.is_file()

    store_clock.now += 25 * _HOUR
    later = _publish_retained_product(retained, "later", b"x-shared", b"x-reloc")
    assert not stray.exists()
    assert later.shared.is_file() and later.reloc.is_file()


@pytest.mark.parametrize(
    "invalid", ["prior-epoch", "prior-v5", "missing-inputs", "malformed-inputs"]
)
def test_generation_rejects_receipts_without_complete_environment_observation(
    tmp_path, invalid
):
    generation, shared, reloc = _publish_pair(tmp_path)
    payload = json.loads(generation.manifest.read_text(encoding="utf-8"))
    assert payload["cargo_environment"] == generation.cargo_environment.to_dict()
    if invalid == "prior-epoch":
        payload["schema"] = "molt.runtime-wasm-generation.v4"
    elif invalid == "prior-v5":
        payload["schema"] = "molt.runtime-wasm-generation.v5"
    elif invalid == "missing-inputs":
        del payload["cargo_environment"]
    else:
        payload["cargo_environment"]["local_unit_count"] = 0
    generation.manifest.write_text(json.dumps(payload), encoding="utf-8")
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared,
            expected_reloc_identity=reloc,
        )
        is None
    )
    with pytest.raises(ValueError):
        runtime_wasm_generation_identities(payload)


def test_generation_observation_is_context_free_and_byte_keys_ignore_provenance(
    tmp_path, monkeypatch
):
    first_root = tmp_path / "first"
    second_root = tmp_path / "second"
    first_root.mkdir()
    second_root.mkdir()
    monkeypatch.setenv("MOLT_TEST_RUNTIME_INPUT", "first-build")
    first, shared, reloc = _publish_pair(first_root)
    first_binding = bind_runtime_wasm_codegen(first, None)
    monkeypatch.setenv("MOLT_TEST_RUNTIME_INPUT", "second-build")
    # Source transport parsing validates shape and exact members, independently
    # of a consumer process's unrelated ambient configuration.
    admitted = read_runtime_wasm_generation(
        first.manifest, expected_shared_identity=shared, expected_reloc_identity=reloc
    )
    assert (
        admitted is not None and admitted.cargo_environment == first.cargo_environment
    )
    second, _, _ = _publish_pair(second_root)
    second_binding = bind_runtime_wasm_codegen(second, None)
    assert first.cargo_environment != second.cargo_environment
    assert first_binding.semantic_digest == second_binding.semantic_digest
    assert first_binding.generation.receipt_identity is not None
    assert second_binding.generation.receipt_identity is not None
    assert (
        first_binding.generation.receipt_identity.sha256
        != second_binding.generation.receipt_identity.sha256
    )
    first_binding.verify()
    second_binding.verify()


def test_source_and_product_roles_have_strict_distinct_wire_contracts(tmp_path):
    source, shared, reloc = _publish_pair(tmp_path)
    assert isinstance(source, SourceRuntimeWasmGeneration)
    envelope = json.loads(source.manifest.read_text(encoding="utf-8"))
    assert set(envelope) == {"schema", "product", "cargo_environment"}
    assert envelope["schema"] == "molt.runtime-wasm-generation.v6"
    assert envelope == source.source_payload()
    product = source.product_payload()
    assert set(product) == {"schema", "family", "family_digest", "receipts"}
    assert product["schema"] == "molt.runtime-wasm-product.v1"
    assert runtime_wasm_product_identities(product) == (shared, reloc)
    with pytest.raises(ValueError):
        runtime_wasm_product_identities(envelope)
    with pytest.raises(ValueError):
        runtime_wasm_generation_identities(product)
    assert (
        read_runtime_wasm_product(
            source.manifest,
            expected_shared_identity=shared,
            expected_reloc_identity=reloc,
        )
        is None
    )

    product_manifest = source.manifest.with_name("installed-product.json")
    product_manifest.write_text(json.dumps(product), encoding="utf-8")
    installed = read_runtime_wasm_product(
        product_manifest,
        expected_shared_identity=shared,
        expected_reloc_identity=reloc,
    )
    assert type(installed) is RuntimeWasmGeneration
    assert not hasattr(installed, "cargo_environment")
    assert (
        read_runtime_wasm_generation(
            product_manifest,
            expected_shared_identity=shared,
            expected_reloc_identity=reloc,
        )
        is None
    )
    # Source evidence cannot be smuggled into the product's strict key set.
    product["cargo_environment"] = source.cargo_environment.to_dict()
    product_manifest.write_text(json.dumps(product), encoding="utf-8")
    assert (
        read_runtime_wasm_product(
            product_manifest,
            expected_shared_identity=shared,
            expected_reloc_identity=reloc,
        )
        is None
    )


def test_installed_product_hydration_and_binding_never_create_source_evidence(tmp_path):
    source, shared, reloc = _publish_pair(tmp_path)
    product = publish_runtime_wasm_product(
        tmp_path / "installed" / "molt_runtime.wasm",
        tmp_path / "installed" / "molt_runtime_reloc.wasm",
        shared_identity=shared,
        reloc_identity=reloc,
        source_shared=source.shared,
        source_reloc=source.reloc,
    )
    assert product.product_payload() == source.product_payload()
    binding = bind_runtime_wasm_codegen(product, None)
    assert type(binding.generation) is RuntimeWasmGeneration
    assert binding.generation.payload is product.payload
    assert (
        json.loads(binding.generation.manifest.read_text(encoding="utf-8"))
        == product.product_payload()
    )
    binding.verify()
    hydrated = hydrate_runtime_wasm_product(
        source_manifest=binding.generation.manifest,
        dest_shared=tmp_path / "hydrated" / "molt_runtime.wasm",
        dest_reloc=tmp_path / "hydrated" / "molt_runtime_reloc.wasm",
        expected_shared_identity=shared,
        expected_reloc_identity=reloc,
        source_generation=binding.generation,
    )
    assert type(hydrated) is RuntimeWasmGeneration
    assert hydrated.product_payload() == product.product_payload()
    assert hydrated.shared.read_bytes() == b"shared"
    assert hydrated.reloc.read_bytes() == b"reloc"
    with pytest.raises(ValueError, match="product hydration requires"):
        hydrate_runtime_wasm_product(
            source_manifest=source.manifest,
            dest_shared=tmp_path / "forbidden" / "molt_runtime.wasm",
            dest_reloc=tmp_path / "forbidden" / "molt_runtime_reloc.wasm",
            expected_shared_identity=shared,
            expected_reloc_identity=reloc,
            source_generation=source,
        )
    assert not (tmp_path / "forbidden").exists()


def test_equal_products_preserve_distinct_raw_flag_observations(tmp_path):
    from molt.process_environment import ProcessEnvironment

    flags = "CARGO_ENCODED_RUSTFLAGS"
    generations = []
    environments = []
    for name in ("first", "second"):
        root = tmp_path / name
        root.mkdir()
        environment = ProcessEnvironment({flags: f"--remap-path-prefix={root}=/src"})
        environments.append(environment)
        observed = CargoEnvironmentInputs.capture(
            environment, (flags,), local_unit_count=1
        )
        generation, _, _ = _publish_pair(
            root, shared=b"same shared", reloc=b"same reloc", cargo_environment=observed
        )
        generations.append(generation)
    first, second = generations
    assert first.product_payload() == second.product_payload()
    assert first.source_payload() != second.source_payload()
    first.cargo_environment.verify(environments[0])
    with pytest.raises(ValueError):
        first.cargo_environment.verify(environments[1])
    binding = bind_runtime_wasm_codegen(first, None)
    assert isinstance(binding.generation, SourceRuntimeWasmGeneration)
    assert binding.generation is first
    assert (
        json.loads(binding.generation.manifest.read_text(encoding="utf-8"))
        == first.source_payload()
    )


def _environment(value: str) -> tuple[ProcessEnvironment, CargoEnvironmentInputs]:
    environment = ProcessEnvironment({"MOLT_TEST_BUILD_INPUT": value})
    return environment, CargoEnvironmentInputs.capture(
        environment, ("MOLT_TEST_BUILD_INPUT",), local_unit_count=1
    )


def test_store_admits_only_its_own_environment_variant(tmp_path: Path) -> None:
    """One entry per family; one immutable variant per observed environment."""
    first_env, first_observed = _environment("first")
    second_env, second_observed = _environment("second")
    first, shared, reloc = _publish_pair(
        tmp_path, pair_seed="pair", cargo_environment=first_observed
    )
    second_source = _source_pair(tmp_path / "second-source", b"shared", b"reloc")
    second = publish_stored_runtime_wasm_generation(
        _store(tmp_path),
        shared_identity=shared,
        reloc_identity=reloc,
        cargo_environment=second_observed,
        source_shared=second_source[0],
        source_reloc=second_source[1],
    )
    store = _store(tmp_path)
    assert store.variants(shared.family_digest) == tuple(
        sorted(
            (
                store.variant(shared.family_digest, first_observed),
                store.variant(shared.family_digest, second_observed),
            )
        )
    )

    def admit(environment: ProcessEnvironment) -> SourceRuntimeWasmGeneration | None:
        return stored_runtime_wasm_generation(
            store,
            shared_identity=shared,
            reloc_identity=reloc,
            environment=environment,
        )

    assert admit(first_env) == first
    assert admit(second_env) == second
    other_env, _ = _environment("never built")
    assert admit(other_env) is None
    # Another environment's damage never fails this reader ...
    second.reloc.write_bytes(b"damaged")
    assert admit(first_env) == first
    # ... while damage to its own published variant is corruption, not a miss.
    with pytest.raises(ProductStoreError, match="molt cache evict wasm-runtime"):
        admit(second_env)


def test_trusted_link_admission_binds_the_exact_receipt_family_and_role(
    tmp_path: Path,
) -> None:
    generation, shared, _reloc = _publish_pair(tmp_path)
    trust = RuntimeWasmTrust.of(generation)
    receipt_sha256 = hashlib.sha256(generation.manifest.read_bytes()).hexdigest()
    assert trust == RuntimeWasmTrust(
        shared.family_digest, RuntimeWasmReceiptRole.SOURCE_BUILD, receipt_sha256
    )
    assert trust.arguments(generation.manifest) == [
        "--runtime-generation",
        str(generation.manifest),
        "--runtime-family-digest",
        shared.family_digest,
        "--runtime-receipt-role",
        "source-build",
        "--runtime-receipt-sha256",
        receipt_sha256,
    ]
    assert read_trusted_runtime_wasm_generation(generation.manifest, trust) == (
        generation
    )
    other, _, _ = _publish_pair(tmp_path, pair_seed="other")
    # Another family's receipt never satisfies this trust, nor a role mismatch.
    assert read_trusted_runtime_wasm_generation(other.manifest, trust) is None
    assert (
        read_trusted_runtime_wasm_generation(
            generation.manifest,
            replace(trust, receipt_role=RuntimeWasmReceiptRole.INSTALLED_PRODUCT),
        )
        is None
    )


@pytest.mark.parametrize("source_build", [False, True])
def test_linked_runtime_admits_exact_reloc_without_shared(
    tmp_path: Path, source_build: bool
) -> None:
    if source_build:
        generation, _, _ = _publish_pair(tmp_path)
    else:
        generation = _publish_retained_product(tmp_path, "pair", b"shared", b"reloc")
    trust = RuntimeWasmTrust.of(generation)
    generation.shared.unlink()

    binding = bind_runtime_wasm_codegen(
        generation, None, use=RuntimeWasmCodegenUse.LINKED
    )
    binding.verify()
    with pytest.raises(ValueError):
        bind_runtime_wasm_codegen(generation, None)
    admitted = read_trusted_runtime_wasm_reloc(generation.manifest, trust)
    assert admitted is not None
    assert admitted.reloc_member_identity == generation.reloc_member_identity
    assert admitted.receipt_identity == generation.receipt_identity
    assert read_trusted_runtime_wasm_generation(generation.manifest, trust) is None
    assert (
        read_trusted_runtime_wasm_reloc(
            generation.manifest, replace(trust, family_digest="0" * 64)
        )
        is None
    )

    receipt_bytes = generation.manifest.read_bytes()
    generation.manifest.write_bytes(receipt_bytes + b"\n")
    with pytest.raises(ValueError):
        binding.verify()
    assert read_trusted_runtime_wasm_reloc(generation.manifest, trust) is None
    generation.manifest.write_bytes(receipt_bytes)
    generation.reloc.write_bytes(b"other")
    assert read_trusted_runtime_wasm_reloc(generation.manifest, trust) is None
    rebound = replace(
        generation,
        receipt_identity=stable_regular_file_identity(
            generation.manifest, label="test receipt"
        ),
    )
    with pytest.raises(ValueError):
        bind_runtime_wasm_codegen(rebound, None, use=RuntimeWasmCodegenUse.LINKED)


def test_codegen_binding_pins_the_immutable_receipt_in_place(tmp_path: Path) -> None:
    generation, _, _ = _publish_pair(tmp_path)
    variant = generation.manifest.parent
    before = sorted(path.name for path in variant.iterdir())
    requirements = {"PyTuple_New"}
    binding = bind_runtime_wasm_codegen(generation, requirements)
    requirements.add("unplanned")
    assert binding.required_exports == frozenset({"PyTuple_New"})
    assert binding.generation is generation
    # The variant is immutable: binding writes nothing into it.
    assert sorted(path.name for path in variant.iterdir()) == before
    binding.verify()
    generation.manifest.write_text("{}", encoding="utf-8")
    with pytest.raises(ValueError):
        binding.verify()
    with pytest.raises(ValueError):
        bind_runtime_wasm_codegen(generation, None)


def _ensure_process(root: str, seed: str, builds: str) -> None:
    store = _store(Path(root))
    shared, reloc = _identity("shared", seed), _identity("reloc", seed)
    environment, observed = _environment("concurrent")

    def read() -> SourceRuntimeWasmGeneration | None:
        return stored_runtime_wasm_generation(
            store, shared_identity=shared, reloc_identity=reloc, environment=environment
        )

    def build() -> SourceRuntimeWasmGeneration:
        with open(builds, "a", encoding="utf-8") as log:
            log.write(f"{os.getpid()}\n")
        source = Path(root) / f"source-{os.getpid()}"
        source_shared, source_reloc = _source_pair(source, b"shared", b"reloc")
        return publish_stored_runtime_wasm_generation(
            store,
            shared_identity=shared,
            reloc_identity=reloc,
            cargo_environment=observed,
            source_shared=source_shared,
            source_reloc=source_reloc,
        )

    store.ensure(shared.family_digest, read, build)


def test_concurrent_builders_with_identical_inputs_build_once(tmp_path: Path) -> None:
    """Single flight across processes, as two checkouts at one commit run it."""
    import multiprocessing

    builds = tmp_path / "builds.log"
    spawn = multiprocessing.get_context("spawn")
    processes = [
        spawn.Process(
            target=_ensure_process, args=(str(tmp_path), "concurrent", str(builds))
        )
        for _ in range(3)
    ]
    for process in processes:
        process.start()
    for process in processes:
        process.join(60)
        assert process.exitcode == 0
    assert len(builds.read_text(encoding="utf-8").splitlines()) == 1
    environment, _ = _environment("concurrent")
    shared = _identity("shared", "concurrent")
    assert (
        stored_runtime_wasm_generation(
            _store(tmp_path),
            shared_identity=shared,
            reloc_identity=_identity("reloc", "concurrent"),
            environment=environment,
        )
        is not None
    )
