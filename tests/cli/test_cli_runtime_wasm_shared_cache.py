"""Atomic runtime pair cache and resource-policy contracts."""

from __future__ import annotations

import os
import time
from contextlib import contextmanager

from molt.process_environment import ProcessEnvironment

from tests.runtime_build_identity_helper import runtime_cargo_environment_inputs

import json
from pathlib import Path

import pytest

from molt import host_capacity
from molt.cli import cargo_execution as CARGO_EXEC
from molt.cli import runtime_wasm_cache as cache
from molt.cli import runtime_wasm_cache_diagnostics as diagnostics
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from tests.runtime_build_identity_helper import runtime_build_identity


def _identity(kind: str, pair_seed: str = "pair") -> RuntimeBuildIdentity:
    return runtime_build_identity(kind, pair_seed)


@pytest.fixture(autouse=True)
def _isolated_cache(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        cache,
        "_default_molt_cache",
        lambda: tmp_path / "cache",
    )
    diagnostics._reset_runtime_wasm_cache_diagnostics()


def _retention_pair(tmp_path: Path, pair_seed: str = "pair"):
    shared, reloc = tmp_path / "molt_runtime.wasm", tmp_path / "molt_runtime_reloc.wasm"
    shared.write_bytes(b"retention-shared")
    reloc.write_bytes(b"retention-reloc")
    identities = _identity("shared", pair_seed), _identity("reloc", pair_seed)
    assert (
        cache.publish_runtime_wasm_pair_to_shared_cache(
            shared=shared,
            reloc=reloc,
            shared_identity=identities[0],
            reloc_identity=identities[1],
            cargo_environment=runtime_cargo_environment_inputs(),
        )
        is None
    )
    manifest = cache._cached_pair_paths(identities[0])[2]
    generation = cache.read_runtime_wasm_generation(
        manifest,
        expected_shared_identity=identities[0],
        expected_reloc_identity=identities[1],
    )
    assert generation is not None
    return generation, identities


@pytest.mark.parametrize("dry_run", [False, True])
def test_shared_family_budget_preserves_reader_grace_and_member_identity(
    tmp_path, dry_run
):
    from molt import cache_eviction

    generation, _ = _retention_pair(tmp_path)
    before = [path.stat() for path in (generation.shared, generation.reloc)]
    result = cache_eviction._prune(
        tmp_path / "cache", max_bytes=0, max_age_days=0, dry_run=dry_run
    )
    assert result["entries_busy"] == 1 and result["entries_removed"] == 0
    for path, observed in zip(
        (generation.shared, generation.reloc), before, strict=True
    ):
        assert path.read_bytes().startswith(b"retention-")
        assert path.stat() == observed


def test_shared_family_expires_without_another_publication_and_keeps_lock_inode(
    tmp_path,
):
    from molt import cache_eviction
    from molt.cli import runtime_wasm_generation as generations
    from molt.default_paths import runtime_wasm_cache_entry

    generation, identities = _retention_pair(tmp_path)
    target = generations.publish_runtime_wasm_generation(
        tmp_path / "target" / "molt_runtime.wasm",
        tmp_path / "target" / "molt_runtime_reloc.wasm",
        shared_identity=identities[0],
        reloc_identity=identities[1],
        cargo_environment=runtime_cargo_environment_inputs(),
        source_shared=generation.shared,
        source_reloc=generation.reloc,
    )
    _, lock = runtime_wasm_cache_entry(tmp_path / "cache", identities[0].family_digest)
    before = lock.stat()
    old = time.time() - 2 * 86400
    os.utime(generation.manifest.parent, (old, old))
    result = cache_eviction._prune(
        tmp_path / "cache", max_bytes=0, max_age_days=0, dry_run=False
    )
    assert result["entries_removed"] == 1
    assert not generation.manifest.parent.exists()
    assert os.path.samestat(before, lock.stat())
    assert (
        cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=identities[0],
            reloc_identity=identities[1],
            environment=ProcessEnvironment(os.environ),
        )
        is None
    )
    assert not generation.manifest.parent.exists()
    target.verify_members(hash_content=True)


def test_family_batch_recovers_retired_scope_once(tmp_path, monkeypatch):
    from molt import cache_eviction, file_publication

    for seed in ("first", "second"):
        generation, _ = _retention_pair(tmp_path, seed)
        old = time.time() - 2 * 86400
        os.utime(generation.manifest.parent, (old, old))
    original = file_publication.reclaim_retired_paths
    scans = []

    def observed(parent, *, retirement_scope):
        scans.append((parent, retirement_scope))
        return original(parent, retirement_scope=retirement_scope)

    monkeypatch.setattr(file_publication, "reclaim_retired_paths", observed)
    monkeypatch.setattr(cache_eviction, "reclaim_retired_paths", observed)
    result = cache_eviction._prune(
        tmp_path / "cache", max_bytes=0, max_age_days=0, dry_run=False
    )
    assert result["entries_removed"] == 2
    assert len(scans) == 1


@pytest.mark.parametrize("renewal", ["admission", "binding", "hydration"])
def test_shared_family_renewal_invalidates_stale_eviction_inventory(tmp_path, renewal):
    from molt import cache_eviction
    from molt.cli import runtime_wasm_generation as generations

    generation, identities = _retention_pair(tmp_path)
    binding = generations.bind_runtime_wasm_codegen(generation, None)
    old = time.time() - 2 * 86400
    os.utime(generation.manifest.parent, (old, old))
    (entry,) = cache_eviction._collect_entries(tmp_path / "cache")
    if renewal == "admission":
        assert (
            cache.admit_runtime_wasm_pair_from_shared_cache(
                shared_identity=identities[0],
                reloc_identity=identities[1],
                environment=ProcessEnvironment(os.environ),
            )
            is not None
        )
    elif renewal == "binding":
        binding.verify()
    else:
        generations.hydrate_runtime_wasm_generation(
            source_manifest=generation.manifest,
            source_generation=generation,
            dest_shared=tmp_path / "hydrated" / "molt_runtime.wasm",
            dest_reloc=tmp_path / "hydrated" / "molt_runtime_reloc.wasm",
            expected_shared_identity=identities[0],
            expected_reloc_identity=identities[1],
        )
    assert cache_eviction._remove_entry(entry, False) == "busy"
    binding.verify()


def test_publication_owns_family_before_first_stage(tmp_path, monkeypatch):
    from molt import cache_eviction
    from molt.cli import runtime_wasm_generation as generations

    original = generations.staged_file_path
    outcomes = []

    def interleave(path, **kwargs):
        if not outcomes:
            old = time.time() - 2 * 86400
            os.utime(path.parent, (old, old))
            outcomes.append(
                cache_eviction._prune(
                    tmp_path / "cache", max_bytes=0, max_age_days=0, dry_run=False
                )
            )
        return original(path, **kwargs)

    monkeypatch.setattr(generations, "staged_file_path", interleave)
    generation, _ = _retention_pair(tmp_path)
    assert outcomes[0]["entries_busy"] == 1
    assert outcomes[0]["entries_removed"] == 0
    generation.verify_members(hash_content=True)


def test_binding_cannot_recreate_family_evicted_after_validation(tmp_path, monkeypatch):
    from molt import cache_eviction
    from molt.cli import runtime_wasm_generation as generations

    generation, _ = _retention_pair(tmp_path)
    root = generation.manifest.parent
    old = time.time() - 2 * 86400
    os.utime(root, (old, old))
    original = generations._runtime_wasm_store

    @contextmanager
    def interleave(path):
        cache_eviction._prune(
            tmp_path / "cache", max_bytes=0, max_age_days=0, dry_run=False
        )
        assert not root.exists()
        with original(path) as store:
            yield store

    monkeypatch.setattr(generations, "_runtime_wasm_store", interleave)
    with pytest.raises(OSError):
        generations.bind_runtime_wasm_codegen(generation, None)
    assert not root.exists()


def test_pair_cache_admits_interleaved_families_without_copy_or_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import runtime_wasm_generation as generation_module
    from molt.toolchain_identity import stable_regular_file_identity

    families = {}
    for seed in ("A", "B"):
        source = tmp_path / seed
        source.mkdir()
        shared, reloc = source / "molt_runtime.wasm", source / "molt_runtime_reloc.wasm"
        shared.write_bytes((seed + "-shared").encode())
        reloc.write_bytes((seed + "-reloc").encode())
        identities = _identity("shared", seed), _identity("reloc", seed)
        assert (
            cache.publish_runtime_wasm_pair_to_shared_cache(
                shared=shared,
                reloc=reloc,
                shared_identity=identities[0],
                reloc_identity=identities[1],
                cargo_environment=runtime_cargo_environment_inputs(),
            )
            is None
        )
        _, _, manifest = cache._cached_pair_paths(identities[0])
        generation = cache.read_runtime_wasm_generation(
            manifest,
            expected_shared_identity=identities[0],
            expected_reloc_identity=identities[1],
        )
        assert generation is not None
        pinned = generation_module.bind_runtime_wasm_codegen(generation, set())
        members = (generation.shared, generation.reloc, pinned.generation.manifest)
        families[seed] = (
            identities,
            generation,
            pinned,
            tuple(
                stable_regular_file_identity(path, label="test retained generation")
                for path in members
            ),
        )

    # The common host selection belongs to B; A still exists in the canonical
    # family cache. An A/B/A sequence must never replace that pointer or members.
    host = tmp_path / "host"
    b_identities, b_generation, _, _ = families["B"]
    selected = generation_module.publish_runtime_wasm_generation(
        host / "molt_runtime.wasm",
        host / "molt_runtime_reloc.wasm",
        shared_identity=b_identities[0],
        reloc_identity=b_identities[1],
        source_shared=b_generation.shared,
        source_reloc=b_generation.reloc,
        cargo_environment=runtime_cargo_environment_inputs(),
    )
    host_before = stable_regular_file_identity(
        selected.manifest, label="test host selection"
    )

    def forbidden(*_args, **_kwargs):
        pytest.fail("ordinary family admission copied or published runtime bytes")

    monkeypatch.setattr(cache, "publish_runtime_wasm_generation", forbidden)
    monkeypatch.setattr(generation_module, "publish_runtime_wasm_generation", forbidden)
    monkeypatch.setattr(generation_module, "hydrate_runtime_wasm_generation", forbidden)
    monkeypatch.setattr(generation_module, "_stage_artifact", forbidden)
    for seed in ("A", "B", "A"):
        identities, generation, pinned, before = families[seed]
        ledger_path = generation.manifest.parent / ".runtime-wasm-store.json"
        ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
        for path in (generation.shared, generation.reloc):
            ledger["last_used"][path.name] = 0
        ledger_path.write_text(json.dumps(ledger), encoding="utf-8")
        admitted = cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=identities[0],
            reloc_identity=identities[1],
            environment=ProcessEnvironment(os.environ),
        )
        assert admitted is not None
        # Admission itself renews retention before the later codegen binding.
        renewed = json.loads(ledger_path.read_text(encoding="utf-8"))
        assert all(
            renewed["last_used"][path.name] > 0
            for path in (generation.shared, generation.reloc)
        )
        assert (
            admitted.shared == generation.shared and admitted.reloc == generation.reloc
        )
        assert (admitted.shared.read_bytes(), admitted.reloc.read_bytes()) == (
            (seed + "-shared").encode(),
            (seed + "-reloc").encode(),
        )
        bound = generation_module.bind_runtime_wasm_codegen(admitted, set())
        assert bound == pinned
        after = tuple(
            stable_regular_file_identity(
                identity.path, label="test retained generation"
            )
            for identity in before
        )
        assert after == before
        ledger = json.loads(
            (admitted.manifest.parent / ".runtime-wasm-store.json").read_text(
                encoding="utf-8"
            )
        )
        assert all(
            path.name in ledger["last_used"]
            for path in (admitted.shared, admitted.reloc)
        )
    assert (
        stable_regular_file_identity(selected.manifest, label="test host selection")
        == host_before
    )

    # Missing/corrupt members and absent families remain misses without a copy
    # fallback, even though the other family's host selection is still usable.
    for seed, damage in (("A", "missing"), ("B", "corrupt")):
        identities, generation, _, _ = families[seed]
        if damage == "missing":
            generation.shared.unlink()
        else:
            generation.reloc.write_bytes(b"corrupt")
        assert (
            cache.admit_runtime_wasm_pair_from_shared_cache(
                shared_identity=identities[0],
                reloc_identity=identities[1],
                environment=ProcessEnvironment(os.environ),
            )
            is None
        )
    assert (
        cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=_identity("shared", "absent"),
            reloc_identity=_identity("reloc", "absent"),
            environment=ProcessEnvironment(os.environ),
        )
        is None
    )
    snapshot = diagnostics._runtime_wasm_cache_diagnostics_snapshot()
    assert snapshot is not None
    assert (
        snapshot["admit_hits"],
        snapshot["admit_failures"],
        snapshot["admit_misses"],
    ) == (3, 2, 1)


def test_admitted_cache_generation_retains_custody_across_selection_and_eviction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from types import SimpleNamespace

    from molt.cli import generation_store
    from molt.cli import runtime_wasm_generation as generation_module
    from tools import wasm_link

    hour = 60 * 60
    now = 1_000_000.0
    monkeypatch.setattr(generation_store, "time", SimpleNamespace(time=lambda: now))
    shared_identity, reloc_identity = _identity("shared"), _identity("reloc")
    shared, reloc = tmp_path / "shared.wasm", tmp_path / "reloc.wasm"

    def publish(value: bytes) -> None:
        shared.write_bytes(value + b"-shared")
        reloc.write_bytes(value + b"-reloc")
        assert (
            cache.publish_runtime_wasm_pair_to_shared_cache(
                shared=shared,
                reloc=reloc,
                shared_identity=shared_identity,
                reloc_identity=reloc_identity,
                cargo_environment=runtime_cargo_environment_inputs(),
            )
            is None
        )

    publish(b"held")
    now += 25 * hour
    admitted = cache.admit_runtime_wasm_pair_from_shared_cache(
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        environment=ProcessEnvironment(os.environ),
    )
    assert admitted is not None
    binding = generation_module.bind_runtime_wasm_codegen(admitted, None)
    pinned = binding.generation
    expected = tmp_path / "expected.json"
    generation_module.RuntimeWasmExpectedPair(
        shared_identity,
        reloc_identity,
        generation_module.RuntimeWasmReceiptRole.SOURCE_BUILD,
    ).write(expected)

    def link_admission():
        return wasm_link._verify_runtime_generation(
            shared=pinned.shared,
            reloc=pinned.reloc,
            generation_manifest=pinned.manifest,
            expected_identity=expected,
        )

    now += hour
    publish(b"replacement")
    now += 22 * hour
    publish(b"later")  # Runs actual store eviction, 23h after admission/binding.
    binding.verify()
    linked = link_admission()
    assert (linked.shared.read_bytes(), linked.reloc.read_bytes()) == (
        b"held-shared",
        b"held-reloc",
    )
    # Retention is bounded, not a permanent exemption. Once both the binding
    # and replaced selection have been unused for a day, the same authority
    # may evict them; binding and final-link admission must then fail closed.
    now += 3 * hour
    publish(b"current")
    # Existing-binding verification at the previous link was real use and
    # renewed its window, even though no new codegen binding was constructed.
    assert all(path.exists() for path in (pinned.shared, pinned.reloc, pinned.manifest))
    now += 22 * hour
    publish(b"expired")
    assert not any(
        path.exists() for path in (pinned.shared, pinned.reloc, pinned.manifest)
    )
    with pytest.raises((OSError, ValueError)):
        binding.verify()
    with pytest.raises(SystemExit, match="does not match the trusted caller identity"):
        link_admission()


def test_pair_cache_rejects_cross_identity_and_single_artifact_tamper(
    tmp_path: Path,
) -> None:
    shared = tmp_path / "built" / "molt_runtime.wasm"
    reloc = tmp_path / "built" / "molt_runtime_reloc.wasm"
    shared.parent.mkdir()
    shared.write_bytes(b"shared")
    reloc.write_bytes(b"reloc")
    shared_identity = _identity("shared")
    reloc_identity = _identity("reloc")
    assert (
        cache.publish_runtime_wasm_pair_to_shared_cache(
            shared=shared,
            reloc=reloc,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            cargo_environment=runtime_cargo_environment_inputs(),
        )
        is None
    )
    cached_shared, _cached_reloc, manifest = cache._cached_pair_paths(shared_identity)
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    immutable_reloc = (
        manifest.parent / payload["product"]["receipts"]["reloc"]["member"]
    )
    assert (
        cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            environment=ProcessEnvironment(os.environ),
        )
        is not None
    )
    # Check identity rejection before damaging the otherwise valid member bytes.
    assert not cache.admit_runtime_wasm_pair_from_shared_cache(
        shared_identity=shared_identity,
        reloc_identity=_identity("reloc", "other"),
        environment=ProcessEnvironment(os.environ),
    )
    immutable_reloc.write_bytes(b"tampered")

    dest = tmp_path / "dest"
    assert not cache.admit_runtime_wasm_pair_from_shared_cache(
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        environment=ProcessEnvironment(os.environ),
    )
    assert not cached_shared.exists()
    assert not dest.exists()


def test_pair_cache_diagnostics_attest_generation_activity(tmp_path: Path) -> None:
    shared = tmp_path / "molt_runtime.wasm"
    reloc = tmp_path / "molt_runtime_reloc.wasm"
    shared.write_bytes(b"shared")
    reloc.write_bytes(b"reloc")
    shared_identity = _identity("shared")
    reloc_identity = _identity("reloc")
    assert (
        cache.publish_runtime_wasm_pair_to_shared_cache(
            shared=shared,
            reloc=reloc,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            cargo_environment=runtime_cargo_environment_inputs(),
        )
        is None
    )
    snapshot = diagnostics._runtime_wasm_cache_diagnostics_snapshot()
    assert snapshot is not None
    assert snapshot["publish_attempts"] == 1
    assert snapshot["publish_successes"] == 1


def test_pair_cache_rejects_excessive_receipt_nesting(tmp_path: Path) -> None:
    shared_identity, reloc_identity = _identity("shared"), _identity("reloc")
    _, _, manifest = cache._cached_pair_paths(shared_identity)
    manifest.parent.mkdir(parents=True)
    manifest.write_text("[" * 100_000 + "]" * 100_000, encoding="utf-8")
    dest = tmp_path / "dest"
    assert (
        cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            environment=ProcessEnvironment(os.environ),
        )
        is None
    )
    snapshot = diagnostics._runtime_wasm_cache_diagnostics_snapshot()
    assert snapshot is not None
    assert snapshot["admit_attempts"] == 1
    assert snapshot["admit_failures"] == 1
    assert snapshot["admit_hits"] == 0
    assert not dest.exists()


@pytest.mark.usefixtures("no_ambient_guard_caps")
def test_memory_bounded_cargo_jobs_fits_small_box(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (8 * 1024**3, 8 * 1024**3)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 16)
    jobs = CARGO_EXEC._memory_bounded_cargo_jobs()
    assert jobs is not None
    assert jobs < 16
    assert jobs == 3


@pytest.mark.usefixtures("no_ambient_guard_caps")
def test_memory_bounded_cargo_jobs_capped_by_cpu(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (128 * 1024**3, 128 * 1024**3)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 4)
    assert CARGO_EXEC._memory_bounded_cargo_jobs() == 4


def test_cargo_build_env_defaults_jobs_but_respects_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(CARGO_EXEC, "_memory_bounded_cargo_jobs", lambda: 3)
    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    env = CARGO_EXEC._cargo_build_env()
    assert env["CARGO_BUILD_JOBS"] == "3"

    monkeypatch.setenv("CARGO_BUILD_JOBS", "7")
    env2 = CARGO_EXEC._cargo_build_env()
    assert env2["CARGO_BUILD_JOBS"] == "7"


@pytest.mark.parametrize("before,after", [("first", "second"), (None, ""), ("", None)])
def test_shared_cache_checks_exact_observation_before_admission(
    tmp_path, monkeypatch, before, after
):
    name = "MOLT_TEST_RUNTIME_INPUT"

    def select(value):
        if value is None:
            monkeypatch.delenv(name, raising=False)
        else:
            monkeypatch.setenv(name, value)

    select(before)
    shared = tmp_path / "built" / "molt_runtime.wasm"
    reloc = shared.with_name("molt_runtime_reloc.wasm")
    shared.parent.mkdir()
    shared.write_bytes(b"shared")
    reloc.write_bytes(b"reloc")
    inputs = runtime_cargo_environment_inputs()
    identities = (_identity("shared"), _identity("reloc"))
    assert (
        cache.publish_runtime_wasm_pair_to_shared_cache(
            shared=shared,
            reloc=reloc,
            shared_identity=identities[0],
            reloc_identity=identities[1],
            cargo_environment=inputs,
        )
        is None
    )
    from molt.cli import runtime_wasm_generation as generation_module

    monkeypatch.setattr(
        generation_module,
        "_stage_artifact",
        lambda *_a, **_k: pytest.fail("environment admission copied runtime bytes"),
    )
    monkeypatch.setattr(
        generation_module,
        "publish_runtime_wasm_generation",
        lambda *_a, **_k: pytest.fail("environment admission published runtime bytes"),
    )
    select(after)
    destination = tmp_path / "refused"
    assert (
        cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=identities[0],
            reloc_identity=identities[1],
            environment=ProcessEnvironment(os.environ),
        )
        is None
    )
    assert not destination.exists()
    select(before)
    admitted = cache.admit_runtime_wasm_pair_from_shared_cache(
        shared_identity=identities[0],
        reloc_identity=identities[1],
        environment=ProcessEnvironment(os.environ),
    )
    assert admitted is not None
    assert admitted.cargo_environment == inputs
    assert (
        admitted.shared.read_bytes() == b"shared"
        and admitted.reloc.read_bytes() == b"reloc"
    )


def test_shared_source_cache_refuses_bare_installed_product(tmp_path):
    from molt.cli.runtime_wasm_generation import read_runtime_wasm_product

    shared = tmp_path / "molt_runtime.wasm"
    reloc = tmp_path / "molt_runtime_reloc.wasm"
    shared.write_bytes(b"shared")
    reloc.write_bytes(b"reloc")
    identities = _identity("shared"), _identity("reloc")
    assert (
        cache.publish_runtime_wasm_pair_to_shared_cache(
            shared=shared,
            reloc=reloc,
            shared_identity=identities[0],
            reloc_identity=identities[1],
            cargo_environment=runtime_cargo_environment_inputs(),
        )
        is None
    )
    _, _, manifest = cache._cached_pair_paths(identities[0])
    envelope = json.loads(manifest.read_text(encoding="utf-8"))
    manifest.write_text(json.dumps(envelope["product"]), encoding="utf-8")
    # Independent product reader confirms intact family and final member bytes.
    assert (
        read_runtime_wasm_product(
            manifest,
            expected_shared_identity=identities[0],
            expected_reloc_identity=identities[1],
        )
        is not None
    )
    assert (
        cache.admit_runtime_wasm_pair_from_shared_cache(
            shared_identity=identities[0],
            reloc_identity=identities[1],
            environment=ProcessEnvironment(os.environ),
        )
        is None
    )
