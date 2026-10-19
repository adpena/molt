from __future__ import annotations

import json
import hashlib
import multiprocessing
import os
from dataclasses import replace
from contextlib import contextmanager
from pathlib import Path

import pytest

from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmExpectedPair,
    bind_runtime_wasm_codegen,
    _generation_receipts,
    hydrate_runtime_wasm_generation,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_identities,
    runtime_wasm_generation_path,
)
from tests.runtime_build_identity_helper import (
    runtime_build_identity,
)
from molt.toolchain_identity import StableRegularFileError, stable_regular_file_identity


def _identity(
    kind: str,
    pair_seed: str = "pair",
) -> RuntimeBuildIdentity:
    return runtime_build_identity(kind, pair_seed)


def test_expected_pair_is_one_exact_shared_reloc_pair(tmp_path: Path) -> None:
    shared = _identity("shared")
    reloc = _identity("reloc")
    pair = RuntimeWasmExpectedPair(shared, reloc)
    path = tmp_path / "expected.json"

    pair.write(path)

    assert RuntimeWasmExpectedPair.read(path) == pair
    with pytest.raises(ValueError, match="schema is invalid"):
        RuntimeWasmExpectedPair.from_dict({**pair.to_dict(), "legacy": {}})
    with pytest.raises(ValueError, match="one shared/reloc build pair"):
        RuntimeWasmExpectedPair(shared, _identity("reloc", "other-pair"))
    with pytest.raises(ValueError, match="one shared/reloc build pair"):
        RuntimeWasmExpectedPair(_identity("reloc"), reloc)


@pytest.mark.parametrize(
    "text",
    (
        '{"schema":"molt.runtime-wasm-expected-pair.v2","schema":"duplicate"}',
        '{"schema":"molt.runtime-wasm-expected-pair.v2","shared":NaN,"reloc":{}}',
    ),
)
def test_expected_pair_reader_rejects_inexact_json(tmp_path: Path, text: str) -> None:
    path = tmp_path / "expected.json"
    path.write_text(text, encoding="utf-8")

    with pytest.raises(ValueError, match="expected pair is unreadable"):
        RuntimeWasmExpectedPair.read(path)


def _source_pair(root: Path, shared: bytes, reloc: bytes) -> tuple[Path, Path]:
    root.mkdir()
    source_shared = root / "shared.wasm"
    source_reloc = root / "reloc.wasm"
    source_shared.write_bytes(shared)
    source_reloc.write_bytes(reloc)
    return source_shared, source_reloc


def _publish_pair(
    root: Path,
    *,
    pair_seed: str = "pair",
    shared: bytes = b"shared",
    reloc: bytes = b"reloc",
):
    source_shared, source_reloc = _source_pair(
        root / f"source-{pair_seed}", shared, reloc
    )
    shared_identity = _identity("shared", pair_seed)
    reloc_identity = _identity("reloc", pair_seed)
    generation = publish_runtime_wasm_generation(
        root / "molt_runtime.wasm",
        root / "molt_runtime_reloc.wasm",
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
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
        publish_runtime_wasm_generation(
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


@pytest.mark.parametrize("operation", ["publish", "bind"])
def test_wasm_pair_remains_owned_through_receipt_publication(
    tmp_path, monkeypatch, operation
):
    from molt.cli import runtime_wasm_generation as module

    generation, _, _ = _publish_pair(tmp_path)
    active = {}
    original_open = module.open_stable_regular_file

    @contextmanager
    def own(path, **kwargs):
        with original_open(path, **kwargs) as opened:
            active[opened.path] = opened
            try:
                yield opened
            finally:
                del active[opened.path]

    monkeypatch.setattr(module, "open_stable_regular_file", own)
    hook = "_atomic_write_json" if operation == "publish" else "atomic_write_bytes"
    original_publish = getattr(module, hook)
    checked = []

    def publish(path, *args, **kwargs):
        if path.name == module._STORE_LEDGER_NAME:
            # The store ledger records usage, not the pair; it needs no handles.
            return original_publish(path, *args, **kwargs)
        assert generation.shared in active and generation.reloc in active
        assert all(not handle.stream.closed for handle in active.values())
        checked.append(path)
        return original_publish(path, *args, **kwargs)

    monkeypatch.setattr(module, hook, publish)
    if operation == "publish":
        publish_runtime_wasm_generation(
            tmp_path / "molt_runtime.wasm",
            tmp_path / "molt_runtime_reloc.wasm",
            shared_identity=generation.shared_identity,
            reloc_identity=generation.reloc_identity,
            source_shared=generation.shared,
            source_reloc=generation.reloc,
        )
    else:
        bind_runtime_wasm_codegen(generation, None)
    assert len(checked) == 1 and not active


def test_codegen_binding_survives_cache_selection_replacement(tmp_path: Path) -> None:
    generation, shared_identity, reloc_identity = _publish_pair(tmp_path)
    requirements = {"PyTuple_New"}
    binding = bind_runtime_wasm_codegen(generation, requirements)
    requirements.add("unplanned")
    assert binding.required_exports == frozenset({"PyTuple_New"})
    assert binding.generation.manifest != generation.manifest
    assert binding.generation.shared == generation.shared
    assert binding.generation.reloc == generation.reloc
    _publish_pair(
        tmp_path, pair_seed="other", shared=b"other-shared", reloc=b"other-reloc"
    )
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
            binding.generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is not None
    )
    assert bind_runtime_wasm_codegen(binding.generation, {"PyTuple_New"}) == binding


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
    assert first.generation.manifest != second.generation.manifest
    assert (first.semantic_digest == second.semantic_digest) == (change == "provenance")
    # Equal app inputs never authorize an old source/toolchain claim.
    assert (
        read_runtime_wasm_generation(
            first.generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )


def test_codegen_binding_rejects_corrupt_immutable_receipt(tmp_path: Path) -> None:
    generation, _, _ = _publish_pair(tmp_path)
    binding = bind_runtime_wasm_codegen(generation, None)
    binding.generation.manifest.write_text("{}", encoding="utf-8")
    with pytest.raises(
        ValueError, match="immutable runtime generation receipt is corrupt"
    ):
        bind_runtime_wasm_codegen(generation, None)


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
    assert generation.manifest == runtime_wasm_generation_path(
        tmp_path / "molt_runtime.wasm"
    )
    assert generation.shared.read_bytes() == b"shared"
    assert generation.reloc.read_bytes() == b"reloc"
    assert not (tmp_path / "molt_runtime.wasm").exists()
    assert not (tmp_path / "molt_runtime_reloc.wasm").exists()
    observed = read_runtime_wasm_generation(
        generation.manifest,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    assert observed is not None
    assert observed.receipt_identity == stable_regular_file_identity(
        generation.manifest, label="published generation receipt"
    )
    assert generation.receipt_identity is None
    assert replace(observed, receipt_identity=None) == generation

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
    payload["receipts"]["shared"]["identity"]["digest"] = "0" * 64
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
    assert payload["family"] == shared_identity.to_dict()["payload"]["family"]
    for kind, identity in (("shared", shared_identity), ("reloc", reloc_identity)):
        assert payload["receipts"][kind]["identity"] == {
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
    payload["family"]["schema"] = "tampered"
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
    source_shared, source_reloc = _source_pair(
        tmp_path / "second-source", b"shared", b"reloc"
    )
    with pytest.raises(ValueError, match="not one stable regular file"):
        publish_runtime_wasm_generation(
            tmp_path / "molt_runtime.wasm",
            tmp_path / "molt_runtime_reloc.wasm",
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            source_shared=source_shared,
            source_reloc=source_reloc,
        )


def test_hydrate_validates_source_pair_and_publishes_immutable_destination(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source-root"
    source.mkdir()
    generation, shared_identity, reloc_identity = _publish_pair(
        source, shared=b"shared-new", reloc=b"reloc-new"
    )
    dest = tmp_path / "dest"
    dest_generation = hydrate_runtime_wasm_generation(
        source_manifest=generation.manifest,
        dest_shared=dest / "molt_runtime.wasm",
        dest_reloc=dest / "molt_runtime_reloc.wasm",
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    assert dest_generation.shared.read_bytes() == b"shared-new"
    assert dest_generation.reloc.read_bytes() == b"reloc-new"
    assert not (dest / "molt_runtime.wasm").exists()
    assert not (dest / "molt_runtime_reloc.wasm").exists()
    observed = read_runtime_wasm_generation(
        dest_generation.manifest,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    assert observed is not None
    assert observed.receipt_identity == stable_regular_file_identity(
        dest_generation.manifest, label="hydrated generation receipt"
    )
    assert dest_generation.receipt_identity is None
    assert replace(observed, receipt_identity=None) == dest_generation


def test_publish_self_validation_survives_a_concurrent_manifest_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The losing publisher of a manifest race still published a valid pair.

    Two publishers may replace the shared manifest back to back; the one whose
    replacement is overwritten must validate the members IT committed, not
    whatever the manifest points at afterwards (that is the reader's job).
    """
    from molt.cli import runtime_wasm_generation as module

    winner, winner_shared, winner_reloc = _publish_pair(
        tmp_path, pair_seed="winner", shared=b"w-shared", reloc=b"w-reloc"
    )
    winner_manifest = json.loads(winner.manifest.read_text(encoding="utf-8"))
    original_write = module._atomic_write_json

    def write_then_lose_the_race(path: Path, payload: object, **kwargs) -> None:
        original_write(path, payload, **kwargs)
        if path == winner.manifest:
            # The competing publisher's replacement lands before this
            # publisher can look at the manifest again.
            original_write(path, winner_manifest, **kwargs)

    monkeypatch.setattr(module, "_atomic_write_json", write_then_lose_the_race)

    loser, loser_shared, loser_reloc = _publish_pair(
        tmp_path, pair_seed="loser", shared=b"l-shared", reloc=b"l-reloc"
    )

    # The loser's publication is complete and self-consistent ...
    assert loser.shared_identity == loser_shared
    assert loser.reloc_identity == loser_reloc
    assert loser.shared.read_bytes() == b"l-shared"
    assert loser.reloc.read_bytes() == b"l-reloc"
    assert loser.manifest == winner.manifest
    # ... while the manifest, by the last-writer-wins contract, selects the
    # winner: the reader authority reports exactly that.
    assert (
        read_runtime_wasm_generation(
            loser.manifest,
            expected_shared_identity=loser_shared,
            expected_reloc_identity=loser_reloc,
        )
        is None
    )
    observed = read_runtime_wasm_generation(
        loser.manifest,
        expected_shared_identity=winner_shared,
        expected_reloc_identity=winner_reloc,
    )
    assert observed is not None
    assert observed.receipt_identity == stable_regular_file_identity(
        winner.manifest, label="winning generation receipt"
    )
    assert winner.receipt_identity is None
    assert loser.receipt_identity is None
    assert replace(observed, receipt_identity=None) == winner


def _publish_process(
    root: str,
    source_name: str,
    shared_identity: dict[str, object],
    reloc_identity: dict[str, object],
) -> None:
    target = Path(root)
    source = target / source_name
    publish_runtime_wasm_generation(
        target / "molt_runtime.wasm",
        target / "molt_runtime_reloc.wasm",
        shared_identity=RuntimeBuildIdentity.from_dict(shared_identity),
        reloc_identity=RuntimeBuildIdentity.from_dict(reloc_identity),
        source_shared=source / "shared.wasm",
        source_reloc=source / "reloc.wasm",
    )


def test_multiprocess_publishers_leave_one_complete_immutable_pair(
    tmp_path: Path,
) -> None:
    identities = [
        (_identity("shared", seed), _identity("reloc", seed))
        for seed in ("pair-a", "pair-b")
    ]
    for index, seed in enumerate((b"a", b"b")):
        source = tmp_path / f"source-{index}"
        source.mkdir()
        (source / "shared.wasm").write_bytes(seed + b"-shared")
        (source / "reloc.wasm").write_bytes(seed + b"-reloc")
    spawn = multiprocessing.get_context("spawn")
    processes = [
        spawn.Process(
            target=_publish_process,
            args=(
                str(tmp_path),
                f"source-{index}",
                pair[0].to_dict(),
                pair[1].to_dict(),
            ),
        )
        for index, pair in enumerate(identities)
    ]
    for process in processes:
        process.start()
    for process in processes:
        process.join(30)
        assert process.exitcode == 0

    manifest = runtime_wasm_generation_path(tmp_path / "molt_runtime.wasm")
    accepted = [
        read_runtime_wasm_generation(
            manifest,
            expected_shared_identity=pair[0],
            expected_reloc_identity=pair[1],
        )
        for pair in identities
    ]
    assert sum(generation is not None for generation in accepted) == 1
    generation = next(generation for generation in accepted if generation is not None)
    assert generation.shared.is_file()
    assert generation.reloc.is_file()


def test_reader_follows_atomic_manifest_across_member_publications(
    tmp_path: Path,
) -> None:
    old, old_shared_identity, old_reloc_identity = _publish_pair(
        tmp_path,
        pair_seed="old",
        shared=b"old-shared",
        reloc=b"old-reloc",
    )
    old_pointer = old.manifest.read_bytes()

    _publish_pair(
        tmp_path,
        pair_seed="new",
        shared=b"new-shared",
        reloc=b"new-reloc",
    )
    old.manifest.write_bytes(old_pointer)

    selected = read_runtime_wasm_generation(
        old.manifest,
        expected_shared_identity=old_shared_identity,
        expected_reloc_identity=old_reloc_identity,
    )
    assert selected is not None
    assert selected.shared.read_bytes() == b"old-shared"
    assert selected.reloc.read_bytes() == b"old-reloc"
    assert not (tmp_path / "molt_runtime.wasm").exists()
    assert not (tmp_path / "molt_runtime_reloc.wasm").exists()


def test_hydrate_rechecks_staged_hash_after_validated_pointer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source-root"
    source.mkdir()
    generation, shared_identity, reloc_identity = _publish_pair(source)

    from molt.cli import runtime_wasm_generation as generation_module

    original = generation_module._stage_artifact
    mutated = False

    def mutate_before_stage(*args: object, **kwargs: object) -> dict[str, object]:
        nonlocal mutated
        if not mutated:
            generation.shared.write_bytes(b"changed-after-validation")
            mutated = True
        return original(*args, **kwargs)

    monkeypatch.setattr(generation_module, "_stage_artifact", mutate_before_stage)
    with pytest.raises(ValueError, match="changed since identity capture"):
        hydrate_runtime_wasm_generation(
            source_manifest=generation.manifest,
            dest_shared=tmp_path / "dest" / "molt_runtime.wasm",
            dest_reloc=tmp_path / "dest" / "molt_runtime_reloc.wasm",
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )


def test_binding_readmits_member_content_and_freezes_nested_payload(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from dataclasses import replace
    from molt import toolchain_identity
    from molt.cli import runtime_wasm_generation as module

    owned_root = tmp_path / "owned"
    owned_root.mkdir()
    generation, _, _ = _publish_pair(owned_root)
    foreign = tmp_path / "foreign.bin"
    foreign.write_bytes(b"unrelated descriptor traffic")

    captured = []
    capture = toolchain_identity.stable_regular_file_handle_identity

    def current(opened, **kwargs):
        if opened.path.is_relative_to(owned_root):
            captured.append(opened.path)
        return capture(opened, **kwargs)

    monkeypatch.setattr(
        toolchain_identity, "stable_regular_file_handle_identity", current
    )
    monkeypatch.setattr(module, "stable_regular_file_handle_identity", current)
    with toolchain_identity.open_stable_regular_file(
        foreign, label="foreign"
    ) as opened:
        observed = toolchain_identity.stable_regular_file_handle_identity(
            opened, label="foreign"
        )
        assert opened.stream.read() == b"unrelated descriptor traffic"
    assert (
        observed.sha256 == hashlib.sha256(b"unrelated descriptor traffic").hexdigest()
    )
    binding = bind_runtime_wasm_codegen(generation, None)
    assert captured == [
        generation.shared,
        generation.reloc,
        binding.generation.manifest,
    ]
    captured.clear()
    binding.verify()
    assert captured == [
        generation.shared,
        generation.reloc,
        binding.generation.manifest,
    ]
    assert (
        binding.generation.shared_member_identity is generation.shared_member_identity
    )
    with pytest.raises(TypeError):
        generation.payload["receipts"]["shared"]["name"] = "other"
    corrupt = dict(generation.payload)
    corrupt["family_digest"] = "0" * 64
    with pytest.raises(ValueError, match="payload does not match"):
        bind_runtime_wasm_codegen(replace(generation, payload=corrupt), None)


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
    with pytest.raises(ValueError, match="content changed since identity capture"):
        bind_runtime_wasm_codegen(generation, None)


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


def test_hydration_reuses_source_receipt_and_readmits_destination_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import runtime_wasm_generation as module
    from molt import toolchain_identity

    source = tmp_path / "source"
    source.mkdir()
    generation, shared_identity, reloc_identity = _publish_pair(source)
    destination = tmp_path / "dest"
    foreign = tmp_path / "foreign.bin"
    foreign.write_bytes(b"unrelated descriptor traffic")
    captured = []
    original = toolchain_identity.stable_regular_file_handle_identity

    def capture(opened, **kwargs):
        if any(opened.path.is_relative_to(root) for root in (source, destination)):
            captured.append(opened.path)
        return original(opened, **kwargs)

    def no_reread(*args, **kwargs):
        pytest.fail("hydration reopened an admitted source receipt")

    monkeypatch.setattr(module, "read_runtime_wasm_generation", no_reread)
    monkeypatch.setattr(
        toolchain_identity, "stable_regular_file_handle_identity", capture
    )
    monkeypatch.setattr(module, "stable_regular_file_handle_identity", capture)
    with toolchain_identity.open_stable_regular_file(
        foreign, label="foreign"
    ) as opened:
        observed = toolchain_identity.stable_regular_file_handle_identity(
            opened, label="foreign"
        )
        assert opened.stream.read() == b"unrelated descriptor traffic"
    assert (
        observed.sha256 == hashlib.sha256(b"unrelated descriptor traffic").hexdigest()
    )
    hydrated = hydrate_runtime_wasm_generation(
        source_manifest=generation.manifest,
        source_generation=generation,
        dest_shared=destination / "molt_runtime.wasm",
        dest_reloc=destination / "molt_runtime_reloc.wasm",
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    # Pair admission retains both owned members through manifest publication.
    # Source bytes are hashed in the stage copy; each destination is hashed once.
    assert captured == [hydrated.shared, hydrated.reloc]
    assert hydrated.shared.read_bytes() == generation.shared.read_bytes()
    assert hydrated.reloc.read_bytes() == generation.reloc.read_bytes()


def test_binding_rejects_corrupt_pinned_receipt_without_json_parse(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import runtime_wasm_generation as module

    generation, _, _ = _publish_pair(tmp_path)
    binding = bind_runtime_wasm_codegen(generation, None)
    binding.generation.manifest.write_bytes(b"[" * 100_000 + b"]" * 100_000)
    monkeypatch.setattr(
        module,
        "capture_exact",
        lambda *args, **kwargs: pytest.fail("pinning parsed an untrusted receipt"),
    )
    with pytest.raises(ValueError, match="receipt is corrupt"):
        bind_runtime_wasm_codegen(generation, None)


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


def _generation_files(generation) -> set[str]:
    return {generation.shared.name, generation.reloc.name}


def test_publication_evicts_generations_no_build_used_for_a_day(
    tmp_path: Path, store_clock: _Clock
) -> None:
    old, old_shared, old_reloc = _publish_pair(
        tmp_path, pair_seed="old", shared=b"old-shared", reloc=b"old-reloc"
    )
    pinned = bind_runtime_wasm_codegen(old, None).generation.manifest
    unrelated = [
        tmp_path / "molt_runtime.wasm",
        tmp_path / ".molt-artifact-publication.lock",
        tmp_path / "notes.txt",
    ]
    for path in unrelated:
        path.write_bytes(b"not a generation file")
    store_clock.now += _HOUR
    previous, _, _ = _publish_pair(
        tmp_path, pair_seed="previous", shared=b"prev-shared", reloc=b"prev-reloc"
    )
    # The old pair left the selection an hour in; 25 hours later nothing has
    # bound it since, while the pair just replaced keeps a full window.
    store_clock.now += 25 * _HOUR
    current, current_shared, current_reloc = _publish_pair(
        tmp_path, pair_seed="current", shared=b"cur-shared", reloc=b"cur-reloc"
    )

    names = {path.name for path in tmp_path.iterdir()}
    assert not (_generation_files(old) | {pinned.name}) & names
    assert _generation_files(previous) | _generation_files(current) <= names
    assert all(path.is_file() for path in unrelated)
    assert (
        read_runtime_wasm_generation(
            pinned,
            expected_shared_identity=old_shared,
            expected_reloc_identity=old_reloc,
        )
        is None
    )
    assert (
        read_runtime_wasm_generation(
            current.manifest,
            expected_shared_identity=current_shared,
            expected_reloc_identity=current_reloc,
        )
        is not None
    )
    ledger = json.loads(
        (tmp_path / ".runtime-wasm-store.json").read_text(encoding="utf-8")
    )
    assert ledger["selected"] == [current.shared.name, current.reloc.name]
    assert not set(ledger["last_used"]) & (_generation_files(old) | {pinned.name})


def test_binding_renews_an_unselected_generation(
    tmp_path: Path, store_clock: _Clock
) -> None:
    held, _, _ = _publish_pair(
        tmp_path, pair_seed="held", shared=b"held-shared", reloc=b"held-reloc"
    )
    store_clock.now += _HOUR
    _publish_pair(tmp_path, pair_seed="next", shared=b"n-shared", reloc=b"n-reloc")
    store_clock.now += 24 * _HOUR
    pinned = bind_runtime_wasm_codegen(held, None).generation.manifest
    store_clock.now += 2 * _HOUR
    _publish_pair(tmp_path, pair_seed="last", shared=b"l-shared", reloc=b"l-reloc")

    names = {path.name for path in tmp_path.iterdir()}
    assert _generation_files(held) | {pinned.name} <= names


@pytest.mark.parametrize("ledger", [None, "{not json", '{"schema": "other"}'])
def test_unknown_generation_files_start_their_window_at_first_sight(
    tmp_path: Path, store_clock: _Clock, ledger: str | None
) -> None:
    stray = tmp_path / f"molt_runtime.wasm.{'a' * 64}.runtime-wasm-member"
    stray_receipt = tmp_path / f"molt_runtime.{'b' * 64}.generation.json"
    for path in (stray, stray_receipt):
        path.write_bytes(b"left by an older build")
    if ledger is not None:
        (tmp_path / ".runtime-wasm-store.json").write_text(ledger, encoding="utf-8")

    _publish_pair(tmp_path, pair_seed="first", shared=b"f-shared", reloc=b"f-reloc")
    assert stray.is_file() and stray_receipt.is_file()

    store_clock.now += 25 * _HOUR
    _publish_pair(tmp_path, pair_seed="later", shared=b"x-shared", reloc=b"x-reloc")
    assert not stray.exists() and not stray_receipt.exists()
