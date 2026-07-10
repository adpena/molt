"""Cross-session reuse of the shared, content-addressed frontend caches.

The per-module *analysis* cache (function defaults / kinds / import scan) and
*lowering* cache (frontend IR result) live under a per-session ``.molt_state``
dir, so a fresh ``MOLT_SESSION_ID`` re-runs the frontend for every module from
cold. These tests prove the shared tier fixes that -- and, critically, that it
is *correctness-safe*: a shared entry is only ever reused when the source content
and full tooling/target/scan identity match, so a shared hit can never be a
miscompile.

The tests have teeth:

* **Reuse** -- session B, cold ``.molt_state``, reuses session A's published
  entry: a spy over the actual persistence-write proves session B never re-writes
  (never re-lowers / re-analyses) the module.
* **Correctness** -- changing the *source content* (not just mtime), flipping the
  tooling fingerprint, or flipping the target Python must all force a cold miss:
  the shared entry is not reused, so the stale result can never be served.
* **exFAT-safe** -- the publish/hydrate path never calls ``os.link`` (byte copy
  only), so it works on the hard-link-less APDataStore build volume.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from molt.cli import module_cache as MC
from molt.cli import module_frontend_cache as MFC


# --------------------------------------------------------------------------
# Fixtures: isolate the shared cache root and the per-session build-state root,
# and clear every lru_cache that keys on the env we mutate.
# --------------------------------------------------------------------------


def _clear_path_caches() -> None:
    from molt.cli import default_paths, module_graph_cache, runtime_paths
    from molt.cli import module_source

    default_paths._default_molt_cache_cached.cache_clear()
    runtime_paths._build_state_root_cached.cache_clear()
    runtime_paths._cargo_target_root_cached.cache_clear()
    module_graph_cache._resolved_module_cache_key.cache_clear()
    module_source._source_content_sha256_cached.cache_clear()


@pytest.fixture(autouse=True)
def _isolated_caches(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    shared_root = tmp_path / "molt_cache"
    monkeypatch.setenv("MOLT_CACHE", str(shared_root))
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("XDG_CACHE_HOME", raising=False)
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    _clear_path_caches()
    yield shared_root
    _clear_path_caches()


def _use_session(monkeypatch: pytest.MonkeyPatch, session_id: str) -> None:
    monkeypatch.setenv("MOLT_SESSION_ID", session_id)
    _clear_path_caches()


def _write_module(project_root: Path, name: str, body: str) -> Path:
    path = project_root / f"{name}.py"
    path.write_text(body, encoding="utf-8")
    return path


# --------------------------------------------------------------------------
# Shared-slot identity: session-independent, and separating build identities.
# --------------------------------------------------------------------------


def test_shared_analysis_slot_is_session_independent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = _write_module(tmp_path, "m", "def f(a, b=1):\n    return a\n")

    _use_session(monkeypatch, "alpha")
    slot_alpha = MC._shared_module_analysis_cache_path_for(
        path, module_name="m", is_package=False, import_scan_mode="module_init"
    )
    _use_session(monkeypatch, "beta")
    slot_beta = MC._shared_module_analysis_cache_path_for(
        path, module_name="m", is_package=False, import_scan_mode="module_init"
    )

    assert slot_alpha is not None and slot_alpha == slot_beta
    assert "sessions" not in slot_alpha.parts
    assert "alpha" not in str(slot_alpha) and "beta" not in str(slot_alpha)
    assert slot_alpha.parent.name == "module_analysis"


def test_shared_lowering_slot_separates_context_digests(
    tmp_path: Path,
) -> None:
    path = _write_module(tmp_path, "m", "def f():\n    return 1\n")
    slot_x = MC._shared_module_lowering_cache_path_for(
        path, module_name="m", is_package=False, context_digest="ab" * 32
    )
    slot_y = MC._shared_module_lowering_cache_path_for(
        path, module_name="m", is_package=False, context_digest="cd" * 32
    )
    assert slot_x is not None and slot_y is not None
    # Distinct lowering identities (different context_digest) MUST occupy distinct
    # shared slots so one can never clobber or be mis-served for the other.
    assert slot_x != slot_y
    assert slot_x.parent.name == "module_lowering"


def test_shared_slot_rejects_bad_key() -> None:
    assert MFC._shared_module_analysis_cache_path("m", "not hex!") is None
    assert (
        MFC._shared_module_lowering_cache_path("m", "ab" * 12, "not-a-digest") is None
    )


# --------------------------------------------------------------------------
# Reuse (teeth): session B hydrates session A's shared entry -> no re-run.
# --------------------------------------------------------------------------


def test_analysis_reuse_across_sessions_hydrates_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f(a, b=2):\n    return a + b\n")

    # Session A: cold write populates the session-local AND shared tiers.
    _use_session(monkeypatch, "A")
    MC._write_persisted_module_analysis(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
        func_defaults={
            "f": {
                "kind": "sync",
                "has_decorators": False,
                "params": 2,
                "defaults": [2],
                "posonly": 0,
                "kwonly": 0,
            }
        },
        func_kinds={"f": "sync"},
        imports=["os"],
    )
    shared_slot = MC._shared_module_analysis_cache_path_for(
        path, module_name="m", is_package=False, import_scan_mode="module_init"
    )
    assert shared_slot is not None and shared_slot.is_file()

    # Session B: cold session-local dir; the ONLY way it can read a hit is by
    # hydrating from the shared tier. Prove the session-local slot did not exist
    # before the read, and that the read returns the correct payload.
    _use_session(monkeypatch, "B")
    session_slot_b = MC._module_analysis_cache_path(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
    )
    assert not session_slot_b.exists()  # cold session-local dir

    result = MC._read_persisted_module_analysis(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
    )
    assert result is not None
    defaults, kinds, imports = result
    assert kinds == {"f": "sync"}
    assert imports == ("os",)
    assert defaults["f"]["params"] == 2
    # Hydration materialized the shared entry into the cold session dir.
    assert session_slot_b.is_file()


def test_lowering_reuse_across_sessions_no_relower(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def g():\n    return 41 + 1\n")
    context_digest = "ab" * 32
    ir_result = {"functions": [{"name": "m__g", "params": []}], "profile": "release"}

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result=ir_result,
    )
    shared_slot = MC._shared_module_lowering_cache_path_for(
        path, module_name="m", is_package=False, context_digest=context_digest
    )
    assert shared_slot is not None and shared_slot.is_file()

    # Teeth: in session B, a re-write must NOT happen -- if the shared tier is
    # working, the read hits and the caller never re-lowers. Spy on the write.
    _use_session(monkeypatch, "B")
    session_slot_b = MC._module_lowering_cache_path(
        project_root, path, module_name="m", is_package=False
    )
    assert not session_slot_b.exists()

    write_calls: list[str] = []
    original_write = MC._write_cached_json_object

    def _spy_write(p, payload, *, default=None):  # type: ignore[no-untyped-def]
        write_calls.append(str(p))
        return original_write(p, payload, default=default)

    monkeypatch.setattr(MC, "_write_cached_json_object", _spy_write)

    reused = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )
    assert reused is not None
    assert reused["functions"][0]["name"] == "m__g"
    assert reused["profile"] == "release"
    # The read hydrated via byte-copy (no cache-write), so no re-lowering write
    # occurred: the frontend never ran in session B.
    assert write_calls == []
    assert session_slot_b.is_file()


def test_lowering_cache_hit_returns_fresh_decoded_payload_each_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def g():\n    return 41 + 1\n")
    context_digest = "ab" * 32
    ir_result = {
        "functions": [
            {
                "name": "m__g",
                "params": [],
                "ops": [{"kind": "const", "value": 42}],
            }
        ],
        "profile": "release",
        "metadata": {"shape": ("module", "lowering")},
    }

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result=ir_result,
    )

    first = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )
    assert first is not None
    first["functions"][0]["name"] = "mutated"
    first["functions"][0]["ops"][0]["value"] = 99
    first["metadata"]["shape"] = ("mutated",)

    second = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )

    assert second is not None
    assert second["functions"][0]["name"] == "m__g"
    assert second["functions"][0]["ops"][0]["value"] == 42
    assert second["metadata"]["shape"] == ("module", "lowering")


# --------------------------------------------------------------------------
# mtime-decoupling (build-dedup doctrine 74, law 1): a re-checkout / copy /
# reinstall that resets file mtime WITHOUT changing bytes must still reuse the
# shared entry. mtime is content-invariant metadata; gating on it made the
# "content-addressed" cache metadata-addressed and cold-started every session.
# --------------------------------------------------------------------------


# Minimal lowering context inputs (mirrors the real call shape) so the tests can
# compute a *real* context_digest and prove it no longer moves with mtime.
_LOWERING_CTX = dict(
    entry_override=None,
    known_classes_snapshot={},
    parse_codec="utf-8",
    type_hint_policy="trust",
    fallback_policy="python",
    type_facts=None,
    enable_phi=True,
    known_modules=("m",),
    stdlib_allowlist=(),
    known_func_defaults={},
    known_func_kinds={},
    module_deps={"m": set()},
    module_is_namespace=False,
    module_chunking=False,
    module_chunk_max_ops=0,
    optimization_profile="default",
    pgo_hot_function_names=(),
    module_dep_closures={"m": frozenset({"m"})},
)


def _bump_mtime(path: Path, delta_ns: int = 5_000_000_000) -> None:
    st = path.stat()
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + delta_ns))
    _clear_path_caches()  # drop any stat-keyed content-sha memoization


def test_lowering_context_digest_is_independent_of_mtime(tmp_path: Path) -> None:
    """The content-addressed lowering key must NOT move when only mtime changes.

    A re-checkout / copy / reinstall of byte-identical source resets mtime; if
    that changed the context_digest, the shared slot name would change and force
    a cold cross-session re-lower of unchanged source (the exact V4 failure).
    """
    path = _write_module(tmp_path, "m", "def g():\n    return 41 + 1\n")
    digest_before = MC._module_lowering_context_digest_for_module(
        "m", path, logical_source_path=str(path), is_package=False, **_LOWERING_CTX
    )
    _bump_mtime(path)
    digest_after = MC._module_lowering_context_digest_for_module(
        "m", path, logical_source_path=str(path), is_package=False, **_LOWERING_CTX
    )
    assert digest_before is not None and digest_before == digest_after


def test_mtime_only_change_still_reuses_lowering_from_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Session B reads a byte-identical module whose mtime was reset -> HIT.

    Teeth: the read must return the reused IR without ever re-writing (the
    frontend never runs), proving the ``_payload_source_matches`` mtime gate no
    longer rejects a content-identical entry.
    """
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def g():\n    return 41 + 1\n")
    context_digest = "ab" * 32
    ir_result = {"functions": [{"name": "m__g", "params": []}], "profile": "release"}

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result=ir_result,
    )

    # Session B: byte-identical source, but mtime reset (git checkout / copy).
    _use_session(monkeypatch, "B")
    _bump_mtime(path)

    write_calls: list[str] = []
    original_write = MC._write_cached_json_object

    def _spy_write(p, payload, *, default=None):  # type: ignore[no-untyped-def]
        write_calls.append(str(p))
        return original_write(p, payload, default=default)

    monkeypatch.setattr(MC, "_write_cached_json_object", _spy_write)

    reused = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )
    assert reused is not None
    assert reused["functions"][0]["name"] == "m__g"
    # Hydrated via byte-copy; no re-lowering write occurred despite the new mtime.
    assert write_calls == []


def test_mtime_only_change_still_reuses_analysis_from_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The analysis cache must also reuse a byte-identical, mtime-reset module."""
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f(a, b=2):\n    return a + b\n")

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_analysis(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
        func_defaults={
            "f": {
                "kind": "sync",
                "has_decorators": False,
                "params": 2,
                "defaults": [2],
                "posonly": 0,
                "kwonly": 0,
            }
        },
        func_kinds={"f": "sync"},
        imports=["os"],
    )

    _use_session(monkeypatch, "B")
    _bump_mtime(path)
    result = MC._read_persisted_module_analysis(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
    )
    assert result is not None
    _, kinds, imports = result
    assert kinds == {"f": "sync"}
    assert imports == ("os",)


def test_same_size_different_content_is_not_reused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """sha256 -- not size or mtime -- is the content authority.

    With the mtime gate removed, the coarse-mtime same-size collision defence
    rests entirely on the source-content sha256. A same-length edit (identical
    size, and here identical mtime) must still be rejected as a MISCOMPILE risk.
    """
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f(a):\n    return a + 1\n")
    context_digest = "ab" * 32
    st = path.stat()

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result={"functions": [{"name": "m__f", "params": ["a"]}]},
    )

    _use_session(monkeypatch, "B")
    # Same-length edit ('+ 1' -> '+ 9'): identical size, and we pin mtime back to
    # the original so ONLY the bytes differ -> the sha256 gate must still reject.
    path.write_text("def f(a):\n    return a + 9\n", encoding="utf-8")
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns))
    assert path.stat().st_size == st.st_size
    assert path.stat().st_mtime_ns == st.st_mtime_ns
    _clear_path_caches()

    reused = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )
    assert reused is None


# --------------------------------------------------------------------------
# Correctness (the critical one): a stale shared entry must NEVER be reused.
# --------------------------------------------------------------------------


def test_changed_source_content_is_not_reused_from_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f(a):\n    return a\n")
    context_digest = "ab" * 32

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result={"functions": [{"name": "m__f", "params": ["a"]}]},
    )
    shared_slot = MC._shared_module_lowering_cache_path_for(
        path, module_name="m", is_package=False, context_digest=context_digest
    )
    assert shared_slot is not None and shared_slot.is_file()

    # Session B, DIFFERENT source content at the same path (a real edit).
    _use_session(monkeypatch, "B")
    path.write_text("def f(a):\n    return a + 999\n", encoding="utf-8")
    _clear_path_caches()  # drop any cached content sha for the old bytes

    reused = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )
    # A stale hit here would be a MISCOMPILE. The source-content sha256 in the
    # payload no longer matches the edited file, so the shared entry is rejected.
    assert reused is None


def test_changed_tooling_fingerprint_is_not_reused_from_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f(a, b=1):\n    return a\n")

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_analysis(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
        func_defaults={
            "f": {
                "kind": "sync",
                "has_decorators": False,
                "params": 2,
                "defaults": [1],
                "posonly": 0,
                "kwonly": 0,
            }
        },
        func_kinds={"f": "sync"},
        imports=[],
    )

    # Session B with a DIFFERENT tooling fingerprint (a compiler/frontend change).
    _use_session(monkeypatch, "B")
    monkeypatch.setattr(
        MC, "_frontend_semantic_tooling_fingerprint", lambda: "different-tooling-fp"
    )

    # The shared slot for the NEW fingerprint identity must be a different slot,
    # and no entry exists there -> cold miss (no cross-identity reuse).
    new_slot = MC._shared_module_analysis_cache_path_for(
        path, module_name="m", is_package=False, import_scan_mode="module_init"
    )
    assert new_slot is not None and not new_slot.is_file()

    result = MC._read_persisted_module_analysis(
        project_root,
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
    )
    assert result is None


def test_changed_target_python_is_not_reused_from_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli.target_python import TargetPythonVersion

    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f():\n    return 1\n")
    context_digest = "ab" * 32
    py312 = TargetPythonVersion(3, 12, 0)
    py313 = TargetPythonVersion(3, 13, 0)
    assert py312.tag != py313.tag

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result={"functions": [{"name": "m__f", "params": []}]},
        target_python=py312,
    )

    # Session B targeting a different Python -> different shared slot -> cold miss.
    _use_session(monkeypatch, "B")
    slot_313 = MC._shared_module_lowering_cache_path_for(
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        target_python=py313,
    )
    assert slot_313 is not None and not slot_313.is_file()

    reused = MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        target_python=py313,
    )
    assert reused is None


# --------------------------------------------------------------------------
# exFAT-safe: the publish/hydrate path never uses os.link (byte copy only).
# --------------------------------------------------------------------------


def test_publish_and_hydrate_never_use_os_link(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f():\n    return 1\n")
    context_digest = "ab" * 32

    link_calls: list[tuple[str, str]] = []
    real_link = os.link

    def _tracking_link(src, dst, *a, **k):  # type: ignore[no-untyped-def]
        link_calls.append((str(src), str(dst)))
        return real_link(src, dst, *a, **k)

    monkeypatch.setattr(os, "link", _tracking_link)

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result={"functions": [{"name": "m__f", "params": []}]},
    )
    _use_session(monkeypatch, "B")
    MC._read_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
    )

    assert link_calls == []


# --------------------------------------------------------------------------
# Debug opt-out: MOLT_DISABLE_FRONTEND_LOWERING_CACHE forces a cold re-lower.
# --------------------------------------------------------------------------


def test_env_optout_forces_cold_lowering_miss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """With the opt-out set, a warm entry (session-local AND shared) is not reused.

    This is the debugging lever for a suspected stale-cache miscompile: it must
    strip *all* lowering-result reuse so the build is a true cold re-lower. A miss
    can only ever force a recompute, so the opt-out can never itself miscompile.
    """
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def g():\n    return 41 + 1\n")
    context_digest = "ab" * 32
    ir_result = {"functions": [{"name": "m__g", "params": []}]}

    # Populate BOTH tiers in session A with the cache enabled.
    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result=ir_result,
    )
    session_slot_a = MC._module_lowering_cache_path(
        project_root, path, module_name="m", is_package=False
    )
    shared_slot = MC._shared_module_lowering_cache_path_for(
        path, module_name="m", is_package=False, context_digest=context_digest
    )
    assert session_slot_a.is_file()
    assert shared_slot is not None and shared_slot.is_file()

    # Same session (warm session-local slot present) but opt-out engaged.
    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "1")
    assert (
        MC._read_persisted_module_lowering(
            project_root,
            path,
            module_name="m",
            is_package=False,
            context_digest=context_digest,
        )
        is None
    )

    # Flipping it back off restores reuse (proves the read path is otherwise warm).
    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "0")
    assert (
        MC._read_persisted_module_lowering(
            project_root,
            path,
            module_name="m",
            is_package=False,
            context_digest=context_digest,
        )
        is not None
    )


def test_env_optout_skips_shared_publish(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """With the opt-out set, a cold write must not publish into the shared tier."""
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def g():\n    return 1\n")
    context_digest = "ab" * 32

    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "1")
    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        project_root,
        path,
        module_name="m",
        is_package=False,
        context_digest=context_digest,
        result={"functions": [{"name": "m__g", "params": []}]},
    )
    shared_slot = MC._shared_module_lowering_cache_path_for(
        path, module_name="m", is_package=False, context_digest=context_digest
    )
    # The session-local entry is still written (correctness isolation), but the
    # shared, cross-session tier is NOT populated while the opt-out is engaged.
    assert shared_slot is not None and not shared_slot.is_file()


# --------------------------------------------------------------------------
# Effectiveness gate: the emitted attestation reflects warm-build reuse. A
# regression that silently disables persistence collapses hit_rate toward 0.0.
# --------------------------------------------------------------------------


def test_lowering_cache_summary_reports_hit_rate() -> None:
    from molt.cli.build_diagnostics import _frontend_lowering_cache_summary

    # A warm second build: every observed module is served from the cache.
    warm = _frontend_lowering_cache_summary(
        {
            "worker_timings": [
                {"mode": "parallel_cache_hit", "reused_ms": 5.0},
                {"mode": "serial_cache_hit", "reused_ms": 3.0},
                {"mode": "parallel_cache_hit", "reused_ms": 4.0},
            ]
        }
    )
    assert warm is not None
    assert warm["observed"] == 3
    assert warm["hits"] == 3
    assert warm["misses"] == 0
    # Effectiveness threshold: a warm rebuild reuses (near-)every module.
    assert warm["hit_rate"] == pytest.approx(1.0)
    assert warm["hit_rate"] >= 0.9

    # A cold build (or a silently-disabled persistent cache) collapses hit_rate.
    cold = _frontend_lowering_cache_summary(
        {
            "worker_timings": [
                {"mode": "parallel", "exec_ms": 100.0},
                {"mode": "serial", "exec_ms": 80.0},
            ]
        }
    )
    assert cold is not None
    assert cold["hits"] == 0
    assert cold["hit_rate"] == pytest.approx(0.0)

    # Mixed warm build: 2 of 4 reused -> 0.5.
    mixed = _frontend_lowering_cache_summary(
        {
            "worker_timings": [
                {"mode": "parallel_cache_hit", "reused_ms": 5.0},
                {"mode": "serial_cache_hit", "reused_ms": 3.0},
                {"mode": "parallel", "exec_ms": 50.0},
                {"mode": "serial", "exec_ms": 40.0},
            ]
        }
    )
    assert mixed is not None
    assert mixed["hit_rate"] == pytest.approx(0.5)
