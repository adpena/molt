"""Cross-session reuse of the shared, content-addressed runtime wasm cache.

These tests have teeth: they prove that two *different* ``MOLT_SESSION_ID``
values resolve to the *same* shared cache slot for an identical runtime build
identity, that publishing under one session is reused (hydrated) under another
into a cold destination, and -- at the integration layer -- that a second
session with a warm shared cache skips the cargo runtime build entirely instead
of recompiling the runtime crate.

The tests are memory-light: no real wasm build is performed. The heavy cargo
step is replaced by a spy that fails the test if it is ever invoked when a warm
shared-cache entry exists.
"""

from __future__ import annotations

import contextlib
import subprocess
from pathlib import Path

import pytest

import molt.dx as DX
import molt.wasm_artifact as wasm_artifact
from molt.cli import cargo_execution as CARGO_EXEC
from molt.cli import runtime_build as RUNTIME_BUILD
from molt.cli import runtime_wasm_cache as RWC


# A pair of fixed 64-hex content-address digests standing in for the real
# fingerprint hash / meta_digest (which are sha256 hex over the runtime source
# tree and the resolved profile/target/features).
_HASH_A = "a1" * 32
_META_A = "b2" * 32
_HASH_B = "c3" * 32
_META_B = "d4" * 32


def _fingerprint(hash_value: str, meta_digest: str) -> dict[str, object]:
    return {
        "hash": hash_value,
        "meta_digest": meta_digest,
        "rustc": "rustc 1.99.0 (fixture)",
        "inputs_digest": "e5" * 32,
    }


def _valid_wasm_bytes(label: bytes = b"") -> bytes:
    if not label:
        return wasm_artifact._build_wasm_sections([])
    payload = wasm_artifact._write_wasm_string("molt.test") + label
    return wasm_artifact._build_wasm_sections([(0, payload)])


@pytest.fixture(autouse=True)
def _shared_cache_root(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Route the shared cache to an isolated tmp dir via MOLT_CACHE.

    ``_default_molt_cache`` is lru_cached on its env inputs, so we also clear the
    cache to make MOLT_CACHE authoritative within the test process.
    """
    cache_root = tmp_path / "molt_cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("XDG_CACHE_HOME", raising=False)
    from molt.cli import default_paths

    default_paths._default_molt_cache_cached.cache_clear()
    return cache_root


# --------------------------------------------------------------------------
# Cache-key stability: same identity -> same slot, regardless of session id.
# --------------------------------------------------------------------------


def test_shared_cache_path_is_session_independent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fp = _fingerprint(_HASH_A, _META_A)

    monkeypatch.setenv("MOLT_SESSION_ID", "session-alpha")
    path_alpha = RWC._shared_runtime_wasm_cache_path(fp, reloc=False)

    monkeypatch.setenv("MOLT_SESSION_ID", "session-beta")
    path_beta = RWC._shared_runtime_wasm_cache_path(fp, reloc=False)

    assert path_alpha is not None
    assert path_alpha == path_beta
    # The slot must not carry any session component: cross-session reuse depends
    # on the path being purely content-addressed.
    assert "session-alpha" not in str(path_alpha)
    assert "session-beta" not in str(path_alpha)
    assert "sessions" not in path_alpha.parts


def test_shared_cache_key_separates_build_identities() -> None:
    shared_a = RWC._shared_runtime_wasm_cache_path(
        _fingerprint(_HASH_A, _META_A), reloc=False
    )
    # Different meta_digest (e.g. different profile/features) -> different slot.
    shared_b = RWC._shared_runtime_wasm_cache_path(
        _fingerprint(_HASH_A, _META_B), reloc=False
    )
    # Different source hash -> different slot.
    shared_c = RWC._shared_runtime_wasm_cache_path(
        _fingerprint(_HASH_B, _META_A), reloc=False
    )
    # reloc vs shared kind -> different slot for the same identity.
    reloc_a = RWC._shared_runtime_wasm_cache_path(
        _fingerprint(_HASH_A, _META_A), reloc=True
    )
    paths = {shared_a, shared_b, shared_c, reloc_a}
    assert len(paths) == 4


def test_shared_cache_path_rejects_incomplete_fingerprint() -> None:
    assert (
        RWC._shared_runtime_wasm_cache_path({"hash": None}, reloc=False) is None
    )
    assert (
        RWC._shared_runtime_wasm_cache_path(
            {"hash": _HASH_A, "meta_digest": "not-hex"}, reloc=False
        )
        is None
    )


# --------------------------------------------------------------------------
# Publish under session A -> hydrate under session B into a cold destination.
# --------------------------------------------------------------------------


def test_publish_then_hydrate_across_sessions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fp = _fingerprint(_HASH_A, _META_A)
    payload = _valid_wasm_bytes(b"runtime-body")

    # Session A builds the runtime wasm into its own session-scoped dest and
    # publishes it to the shared cache.
    monkeypatch.setenv("MOLT_SESSION_ID", "session-A")
    dest_a = tmp_path / "sessions" / "A" / "wasm" / "molt_runtime.wasm"
    dest_a.parent.mkdir(parents=True, exist_ok=True)
    dest_a.write_bytes(payload)
    RWC._publish_runtime_wasm_to_shared_cache(src=dest_a, fingerprint=fp, reloc=False)

    cache_path = RWC._shared_runtime_wasm_cache_path(fp, reloc=False)
    assert cache_path is not None and cache_path.is_file()
    assert cache_path.read_bytes() == payload

    # Session B starts cold: a fresh dest that does NOT exist yet.
    monkeypatch.setenv("MOLT_SESSION_ID", "session-B")
    dest_b = tmp_path / "sessions" / "B" / "wasm" / "molt_runtime.wasm"
    assert not dest_b.exists()

    hydrated = RWC._hydrate_runtime_wasm_from_shared_cache(
        dest=dest_b,
        fingerprint=fp,
        reloc=False,
        is_valid=lambda p: Path(p).read_bytes() == payload,
    )
    assert hydrated is True
    assert dest_b.is_file()
    assert dest_b.read_bytes() == payload


def test_hydrate_validates_source_once_before_atomic_copy(
    tmp_path: Path,
) -> None:
    fp = _fingerprint(_HASH_A, _META_A)
    payload = _valid_wasm_bytes(b"runtime-body")
    src = tmp_path / "built" / "molt_runtime.wasm"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_bytes(payload)
    RWC._publish_runtime_wasm_to_shared_cache(src=src, fingerprint=fp, reloc=False)

    validated: list[Path] = []
    dest = tmp_path / "hydrated" / "molt_runtime.wasm"
    assert RWC._hydrate_runtime_wasm_from_shared_cache(
        dest=dest,
        fingerprint=fp,
        reloc=False,
        is_valid=lambda path: validated.append(path) is None
        and path.read_bytes() == payload,
    )

    cache_path = RWC._shared_runtime_wasm_cache_path(fp, reloc=False)
    assert cache_path is not None
    assert validated == [cache_path]
    assert dest.read_bytes() == payload


def test_hydrate_misses_for_different_identity(
    tmp_path: Path,
) -> None:
    fp_built = _fingerprint(_HASH_A, _META_A)
    payload = _valid_wasm_bytes(b"body")
    src = tmp_path / "built" / "molt_runtime.wasm"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_bytes(payload)
    RWC._publish_runtime_wasm_to_shared_cache(
        src=src, fingerprint=fp_built, reloc=False
    )

    # A different build identity must miss (no stale cross-identity reuse).
    dest = tmp_path / "other" / "molt_runtime.wasm"
    hydrated = RWC._hydrate_runtime_wasm_from_shared_cache(
        dest=dest,
        fingerprint=_fingerprint(_HASH_A, _META_B),
        reloc=False,
        is_valid=lambda p: True,
    )
    assert hydrated is False
    assert not dest.exists()


def test_hydrate_rejects_corrupt_cache_entry(tmp_path: Path) -> None:
    fp = _fingerprint(_HASH_A, _META_A)
    cache_path = RWC._shared_runtime_wasm_cache_path(fp, reloc=False)
    assert cache_path is not None
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    cache_path.write_bytes(b"corrupt-not-wasm")

    dest = tmp_path / "wasm" / "molt_runtime.wasm"
    hydrated = RWC._hydrate_runtime_wasm_from_shared_cache(
        dest=dest,
        fingerprint=fp,
        reloc=False,
        is_valid=lambda p: False,  # validator rejects the corrupt entry
    )
    assert hydrated is False
    assert not dest.exists()


def test_hydrate_copy_failure_remains_a_cache_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fp = _fingerprint(_HASH_A, _META_A)
    cache_path = RWC._shared_runtime_wasm_cache_path(fp, reloc=False)
    assert cache_path is not None
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    cache_path.write_bytes(_valid_wasm_bytes(b"runtime-body"))
    monkeypatch.setattr(
        RWC,
        "_atomic_copy_file",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("copy failed")),
    )

    dest = tmp_path / "wasm" / "molt_runtime.wasm"
    assert not RWC._hydrate_runtime_wasm_from_shared_cache(
        dest=dest,
        fingerprint=fp,
        reloc=False,
        is_valid=lambda _path: True,
    )
    assert not dest.exists()


# --------------------------------------------------------------------------
# Integration: a warm shared cache makes _ensure_runtime_wasm reuse instead of
# recompiling the runtime crate, across two different session ids.
# --------------------------------------------------------------------------


def test_ensure_runtime_wasm_reuses_shared_cache_across_sessions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fp = _fingerprint(_HASH_A, _META_A)
    payload = _valid_wasm_bytes(b"shared-runtime-body")

    # Seed the shared cache as if session A had already built + published it.
    RWC._publish_runtime_wasm_to_shared_cache(
        src=_write_tmp(tmp_path / "seed.wasm", payload),
        fingerprint=fp,
        reloc=False,
    )

    # Deterministic, content-addressed fingerprint independent of the real
    # runtime source tree / rustc so both sessions resolve the same key.
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *a, **k: dict(fp),
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_read_runtime_fingerprint", lambda p: None)
    # Structural / export validators pass for our fixture bytes.
    monkeypatch.setattr(
        RUNTIME_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda p: True
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_is_valid_runtime_wasm_artifact", lambda p: True)
    monkeypatch.setattr(RUNTIME_BUILD, "_inspect_wasm_binary", lambda p: "valid")
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_exports_satisfy_for_mode", lambda *a, **k: True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_write_runtime_wasm_integrity_sidecar", lambda *a, **k: None
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_runtime_wasm_integrity_key", lambda fp: _HASH_A
    )

    # Teeth: the cargo runtime build must NOT run when a warm shared cache exists.
    def _forbidden_build(*args: object, **kwargs: object):
        raise AssertionError(
            "cargo runtime wasm build was invoked despite a warm shared cache"
        )

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_runtime_wasm_cargo_build", _forbidden_build
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_build_lock", lambda root, name: contextlib.nullcontext()
    )

    def _run_for_session(session_id: str) -> Path:
        monkeypatch.setenv("MOLT_SESSION_ID", session_id)
        dest = tmp_path / "sessions" / session_id / "wasm" / "molt_runtime.wasm"
        assert not dest.exists()  # each session starts cold
        ok = RUNTIME_BUILD._ensure_runtime_wasm(
            dest,
            reloc=False,
            json_output=True,
            cargo_profile="release",
            cargo_timeout=1.0,
            project_root=tmp_path,
            required_exports=None,
        )
        assert ok is True
        assert dest.is_file()
        assert dest.read_bytes() == payload
        return dest

    dest_first = _run_for_session("session-one")
    dest_second = _run_for_session("session-two")
    # Different session-scoped destinations, both served from the one shared slot.
    assert dest_first != dest_second


def _write_tmp(path: Path, data: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


# --------------------------------------------------------------------------
# Parallelism is bounded to fit memory (8GB-capable).
# --------------------------------------------------------------------------


def test_memory_bounded_cargo_jobs_fits_small_box(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Simulate an 8GB, 16-core box: jobs must be bounded well below 16 so the
    # wasm build fits memory instead of thrashing. Job-sizing authority lives in
    # molt.dx (re-exported through cargo_execution); patch it where it resolves.
    monkeypatch.setattr(DX, "_total_system_memory_bytes", lambda: 8 * 1024**3)
    monkeypatch.setattr(DX.os, "cpu_count", lambda: 16)
    jobs = CARGO_EXEC._memory_bounded_cargo_jobs()
    assert jobs is not None
    assert jobs < 16
    # (8GB - 2GB headroom) / 2GB per job = 3 jobs.
    assert jobs == 3


def test_memory_bounded_cargo_jobs_capped_by_cpu(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Lots of RAM but few cores: never exceed the CPU count. The job-sizing
    # authority lives in molt.dx (re-exported through cargo_execution), so patch
    # the probes where the function resolves them.
    monkeypatch.setattr(DX, "_total_system_memory_bytes", lambda: 128 * 1024**3)
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

    # An explicit operator override always wins.
    monkeypatch.setenv("CARGO_BUILD_JOBS", "7")
    env2 = CARGO_EXEC._cargo_build_env()
    assert env2["CARGO_BUILD_JOBS"] == "7"
