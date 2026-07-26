"""Atomic runtime pair cache and resource-policy contracts."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

import molt.dx as DX
from molt.cli import cargo_execution as CARGO_EXEC
from molt.cli import runtime_wasm_cache as cache
from molt.cli.runtime_build_identity import RuntimeBuildIdentity


def _identity(kind: str, pair_seed: str = "pair") -> RuntimeBuildIdentity:
    pair = {
        "schema": "molt.runtime-build-pair.v2",
        "sources": {"digest": pair_seed},
        "toolchain": {},
        "config": {},
    }
    payload = {
        "pair": pair,
        "resolved_config": {"artifact_kind": kind},
        "publication": {"transform": kind},
    }
    canonical = lambda value: json.dumps(  # noqa: E731
        value, sort_keys=True, separators=(",", ":")
    ).encode()
    return RuntimeBuildIdentity(
        digest=hashlib.sha256(canonical(payload)).hexdigest(),
        pair_digest=hashlib.sha256(canonical(pair)).hexdigest(),
        payload=payload,
    )


@pytest.fixture(autouse=True)
def _isolated_cache(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        cache,
        "_shared_runtime_wasm_cache_root",
        lambda: tmp_path / "cache",
    )
    cache._reset_runtime_wasm_cache_diagnostics()


def test_pair_cache_is_session_independent_and_hydrates_both(tmp_path: Path) -> None:
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
        )
        is None
    )

    dest_shared = tmp_path / "other-session" / "molt_runtime.wasm"
    dest_reloc = tmp_path / "other-session" / "molt_runtime_reloc.wasm"
    hydrated = cache.hydrate_runtime_wasm_pair_from_shared_cache(
        dest_shared=dest_shared,
        dest_reloc=dest_reloc,
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        is_valid_shared=lambda path: path.read_bytes() == b"shared",
        is_valid_reloc=lambda path: path.read_bytes() == b"reloc",
    )
    assert hydrated is not None
    assert hydrated.shared.read_bytes() == b"shared"
    assert hydrated.reloc.read_bytes() == b"reloc"
    assert not dest_shared.exists()
    assert not dest_reloc.exists()


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
        )
        is None
    )
    cached_shared, _cached_reloc, manifest = cache._cached_pair_paths(shared_identity)
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    immutable_reloc = manifest.parent / payload["receipts"]["reloc"]["member"]
    immutable_reloc.write_bytes(b"tampered")

    dest = tmp_path / "dest"
    assert not cache.hydrate_runtime_wasm_pair_from_shared_cache(
        dest_shared=dest / shared.name,
        dest_reloc=dest / reloc.name,
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        is_valid_shared=lambda _path: True,
        is_valid_reloc=lambda _path: True,
    )
    assert not cache.hydrate_runtime_wasm_pair_from_shared_cache(
        dest_shared=dest / shared.name,
        dest_reloc=dest / reloc.name,
        shared_identity=shared_identity,
        reloc_identity=_identity("reloc", "other"),
        is_valid_shared=lambda _path: True,
        is_valid_reloc=lambda _path: True,
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
        )
        is None
    )
    snapshot = cache._runtime_wasm_cache_diagnostics_snapshot()
    assert snapshot is not None
    assert snapshot["publish_attempts"] == 1
    assert snapshot["publish_successes"] == 1


def test_memory_bounded_cargo_jobs_fits_small_box(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(DX, "_system_memory_bytes", lambda: (8 * 1024**3, 8 * 1024**3))
    monkeypatch.setattr(DX.os, "cpu_count", lambda: 16)
    jobs = CARGO_EXEC._memory_bounded_cargo_jobs()
    assert jobs is not None
    assert jobs < 16
    assert jobs == 3


def test_memory_bounded_cargo_jobs_capped_by_cpu(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        DX, "_system_memory_bytes", lambda: (128 * 1024**3, 128 * 1024**3)
    )
    monkeypatch.setattr(DX.os, "cpu_count", lambda: 4)
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
