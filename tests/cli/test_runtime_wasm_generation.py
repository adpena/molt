from __future__ import annotations

import json
import multiprocessing
from pathlib import Path

import pytest

from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmExpectedPair,
    bind_runtime_wasm_codegen,
    _generation_receipts,
    hydrate_runtime_wasm_generation,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_path,
)
from tests.runtime_build_identity_helper import (
    runtime_build_identity,
)


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
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        == generation
    )

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
    assert (
        read_runtime_wasm_generation(
            dest_generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        == dest_generation
    )


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
    assert (
        read_runtime_wasm_generation(
            loser.manifest,
            expected_shared_identity=winner_shared,
            expected_reloc_identity=winner_reloc,
        )
        == winner
    )


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
    with pytest.raises(ValueError, match="changed after source generation validation"):
        hydrate_runtime_wasm_generation(
            source_manifest=generation.manifest,
            dest_shared=tmp_path / "dest" / "molt_runtime.wasm",
            dest_reloc=tmp_path / "dest" / "molt_runtime_reloc.wasm",
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
