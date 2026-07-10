"""V3 config-lattice reuse unit tests (doctrine 74 law 3) -- no real cargo build.

Populates the session-independent shared runtime-wasm cache with a fake
higher-opt artifact + compat sidecar and verifies the opt-in lattice hydrate
serves a same-source iteration-profile request, and refuses every unsafe edge.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

import molt.cli.runtime_wasm_cache as rc


def _hex(seed: str) -> str:
    return hashlib.sha256(seed.encode()).hexdigest()


def _fingerprint(profile: str) -> dict[str, object]:
    # hash/meta_digest must be 64-hex for the content-address key; meta_digest
    # varies by profile (as the real fingerprint does).
    return {
        "hash": _hex("src-tree-v1"),
        "meta_digest": _hex(f"meta:{profile}"),
        "inputs_digest": _hex("inputs-v1"),
        "rustc": "rustc-1.96.0",
    }


def _compat(profile: str) -> dict[str, object]:
    return {
        "inputs_digest": _hex("inputs-v1"),
        "compat_digest": rc._runtime_wasm_compat_digest(
            target_triple="wasm32-wasip1", rustflags="-Cx", features=("a", "b")
        ),
        "cargo_profile": profile,
    }


@pytest.fixture(autouse=True)
def _isolated_cache(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path))
    rc._reset_runtime_wasm_cache_diagnostics()
    yield
    rc._reset_runtime_wasm_cache_diagnostics()


def _publish(tmp_path: Path, profile: str, *, reloc: bool = False) -> None:
    src = tmp_path / f"built.{profile}.wasm"
    src.write_bytes(b"\x00asm\x01\x00\x00\x00" + profile.encode())
    err = rc._publish_runtime_wasm_to_shared_cache(
        src=src, fingerprint=_fingerprint(profile), reloc=reloc, compat=_compat(profile)
    )
    assert err is None


def test_profile_rank_ordering() -> None:
    assert rc._profile_reuse_rank("release-output") > rc._profile_reuse_rank("wasm-release")
    assert rc._profile_reuse_rank("wasm-release") > rc._profile_reuse_rank("dev-fast")
    assert rc._profile_reuse_rank("dev-fast") > rc._profile_reuse_rank("dev")
    assert rc._profile_reuse_rank("totally-unknown") == -1


def test_env_gate(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("MOLT_BUILD_REUSE_COMPATIBLE", raising=False)
    assert rc._build_reuse_compatible_enabled() is False
    monkeypatch.setenv("MOLT_BUILD_REUSE_COMPATIBLE", "1")
    assert rc._build_reuse_compatible_enabled() is True


def test_compat_digest_excludes_profile() -> None:
    d1 = rc._runtime_wasm_compat_digest(
        target_triple="wasm32-wasip1", rustflags="-Cx", features=("a", "b")
    )
    d2 = rc._runtime_wasm_compat_digest(
        target_triple="wasm32-wasip1", rustflags="-Cx", features=("b", "a")
    )
    d3 = rc._runtime_wasm_compat_digest(
        target_triple="wasm32-wasip1", rustflags="-Cy", features=("a", "b")
    )
    assert d1 == d2  # feature order-independent
    assert d1 != d3  # rustflags-sensitive


def test_release_output_serves_dev_fast_request(tmp_path: Path) -> None:
    _publish(tmp_path, "release-output")
    dest = tmp_path / "hydrated.wasm"
    ok = rc._hydrate_runtime_wasm_from_compatible_cache(
        dest=dest,
        reloc=False,
        inputs_digest=_hex("inputs-v1"),
        compat_digest=_compat("dev-fast")["compat_digest"],
        request_profile="dev-fast",
        is_valid=lambda p: True,
        exports_ok=lambda p: True,
    )
    assert ok is True
    assert dest.read_bytes().endswith(b"release-output")
    assert rc._RUNTIME_WASM_CACHE_STATS["compat_hydrate_hits"] == 1


def test_lower_opt_does_not_serve_higher_request(tmp_path: Path) -> None:
    _publish(tmp_path, "dev-fast")
    dest = tmp_path / "hydrated.wasm"
    ok = rc._hydrate_runtime_wasm_from_compatible_cache(
        dest=dest,
        reloc=False,
        inputs_digest=_hex("inputs-v1"),
        compat_digest=_compat("release-output")["compat_digest"],
        request_profile="release-output",
        is_valid=lambda p: True,
        exports_ok=lambda p: True,
    )
    assert ok is False
    assert not dest.exists()


def test_mismatched_compat_digest_is_rejected(tmp_path: Path) -> None:
    _publish(tmp_path, "release-output")
    dest = tmp_path / "hydrated.wasm"
    ok = rc._hydrate_runtime_wasm_from_compatible_cache(
        dest=dest,
        reloc=False,
        inputs_digest=_hex("inputs-v1"),
        compat_digest=_hex("DIFFERENT-abi"),
        request_profile="dev-fast",
        is_valid=lambda p: True,
        exports_ok=lambda p: True,
    )
    assert ok is False


def test_export_check_gates_reuse(tmp_path: Path) -> None:
    _publish(tmp_path, "release-output")
    dest = tmp_path / "hydrated.wasm"
    ok = rc._hydrate_runtime_wasm_from_compatible_cache(
        dest=dest,
        reloc=False,
        inputs_digest=_hex("inputs-v1"),
        compat_digest=_compat("dev-fast")["compat_digest"],
        request_profile="dev-fast",
        is_valid=lambda p: True,
        exports_ok=lambda p: False,  # missing a required export -> refuse
    )
    assert ok is False
    assert rc._RUNTIME_WASM_CACHE_STATS["compat_hydrate_misses"] == 1


def test_reloc_and_shared_kinds_do_not_cross(tmp_path: Path) -> None:
    _publish(tmp_path, "release-output", reloc=True)  # only a reloc artifact cached
    dest = tmp_path / "hydrated.wasm"
    ok = rc._hydrate_runtime_wasm_from_compatible_cache(
        dest=dest,
        reloc=False,  # request the shared kind
        inputs_digest=_hex("inputs-v1"),
        compat_digest=_compat("dev-fast")["compat_digest"],
        request_profile="dev-fast",
        is_valid=lambda p: True,
        exports_ok=lambda p: True,
    )
    assert ok is False
