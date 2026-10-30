"""Source-qualified analysis/lowering reuse through one shared record store.

Sessions and projects reuse admitted rows without hydration or duplicate writes;
source, semantic context and typed body ownership remain independently qualified.
"""

from __future__ import annotations

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

import os
import sqlite3
import json
from pathlib import Path

import pytest

from molt.cli import module_cache as MC
from molt.cli.source_record_store import (
    SourceRecord,
    SourceRecordStore,
    shared_source_records,
)
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION
from tests.source_record_test_support import source_records, replace_source_record
from molt.cli.module_source import PythonSourceSnapshot, _ModuleSourceLease


# --------------------------------------------------------------------------
# Fixtures: isolate the shared cache root and the per-session build-state root,
# and clear every lru_cache that keys on the env we mutate.
# --------------------------------------------------------------------------


def _clear_path_caches() -> None:
    from molt import default_paths
    from molt.cli import runtime_paths
    from molt.cli import module_source

    default_paths._default_molt_cache_cached.cache_clear()
    runtime_paths._build_state_root_cached.cache_clear()
    module_source._source_content_sha256_cached.cache_clear()


@pytest.fixture(autouse=True)
def _isolated_caches(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, developer_host_context: None
) -> Path:
    # The developer-host context drops the session's generated MOLT_SESSION_ID,
    # so `_use_session` pins one and scopes the project target by it.
    shared_root = tmp_path / "molt_cache"
    monkeypatch.setenv("MOLT_CACHE", str(shared_root))
    monkeypatch.delenv("XDG_CACHE_HOME", raising=False)
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
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


def _record(kind, key) -> SourceRecord:
    record = shared_source_records().get(
        kind,
        key,
        generation=MC._frontend_semantic_tooling_fingerprint(),
        max_bytes=None,
    )
    assert record is not None
    return record


def _age_record(kind, key) -> SourceRecord:
    with sqlite3.connect(shared_source_records().path) as connection:
        connection.execute(
            "UPDATE recency SET used_ns = 0 WHERE kind = ? AND key = ?", (kind, key)
        )
    return _record(kind, key)


# --------------------------------------------------------------------------
# Shared-slot identity: session-independent, and separating build identities.
# --------------------------------------------------------------------------


def test_analysis_record_key_is_session_independent(tmp_path, monkeypatch):
    path = _write_module(tmp_path, "m", "def f(a, b=1):\n    return a\n")
    options = dict(
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    _use_session(monkeypatch, "alpha")
    alpha = MC._module_analysis_record_key(path, **options)
    _use_session(monkeypatch, "beta")
    assert MC._module_analysis_record_key(path, **options) == alpha


def test_lowering_records_separate_context_digests(tmp_path):
    path = _write_module(tmp_path, "m", "pass\n")
    for digest, value in (("ab" * 32, 41), ("cd" * 32, 42)):
        MC._write_persisted_module_lowering(
            path,
            source_lease=_ModuleSourceLease.path_backed(path),
            context_digest=digest,
            result={
                "body_selection": {"kind": "complete_source", "roots": []},
                "function_metadata": {},
                "functions": [],
                "answer": value,
            },
        )
    for digest, value in (("ab" * 32, 41), ("cd" * 32, 42)):
        loaded = MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest=digest,
        )
        assert loaded is not None and loaded["answer"] == value
    assert set(source_records("module-lowering")) == {"ab" * 32, "cd" * 32}


@pytest.mark.parametrize(
    "stored_selection", [None, {"kind": "native_support", "roots": ["f"]}]
)
def test_lowering_rejects_missing_or_different_selection_before_body_decode(
    tmp_path, monkeypatch, stored_selection
):
    path = _write_module(tmp_path, "m", "def f(): return 1\ndef g(): return 2\n")
    digest = "de" * 32
    lease = _ModuleSourceLease.path_backed(path)
    MC._write_persisted_module_lowering(
        path,
        source_lease=lease,
        context_digest=digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [],
        },
    )
    payload = source_records("module-lowering")[digest]
    if stored_selection is None:
        del payload["result"]["body_selection"]
        payload["result"]["native_support_function_roots"] = []
    else:
        payload["result"]["body_selection"] = stored_selection
    replace_source_record("module-lowering", digest, payload)
    before = _age_record("module-lowering", digest)

    def forbidden_decode(_view):
        raise AssertionError("unadmitted producer body was decoded")

    monkeypatch.setattr(MC, "decode_function_records", forbidden_decode)
    assert (
        MC._read_persisted_module_lowering(
            path,
            source_lease=lease,
            module_name="m",
            context_digest=digest,
            body_selection=ModuleBodySelection("complete_source"),
        )
        is None
    )
    after = _record("module-lowering", digest)
    assert (after.revision, after.used_ns) == (before.revision, before.used_ns)


def test_lowering_writer_refuses_missing_producer_selection(tmp_path):
    path = _write_module(tmp_path, "m", "pass\n")
    with pytest.raises(ValueError, match="module body selection"):
        MC._write_persisted_module_lowering(
            path,
            source_lease=_ModuleSourceLease.path_backed(path),
            context_digest="ef" * 32,
            result={"functions": [], "function_metadata": {}},
        )
    assert not source_records("module-lowering")


# --------------------------------------------------------------------------
# Reuse (teeth): session B reads session A's admitted row -> no re-run.
# --------------------------------------------------------------------------


def test_analysis_reuse_across_sessions_uses_same_row(tmp_path, monkeypatch):
    path = _write_module(tmp_path, "m", "def f(a, b=2):\n    return a + b\n")
    _use_session(monkeypatch, "A")
    MC._write_persisted_module_analysis(
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
        snapshot=PythonSourceSnapshot.capture(path),
    )
    key = MC._module_analysis_record_key(
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    before = _age_record("module-analysis", key)
    _use_session(monkeypatch, "B")
    monkeypatch.setattr(
        SourceRecordStore,
        "put",
        lambda *_a, **_k: pytest.fail("cache hit republished a record"),
    )
    result = MC._read_persisted_module_analysis(
        path, module_name="m", is_package=False, import_scan_mode="module_init"
    )
    assert result is not None
    defaults, kinds = result
    assert kinds == {"f": "sync"} and defaults["f"]["defaults"] == [2]
    # The owning operation already flushed its partial advisory batch.
    assert shared_source_records().save_renewals() == 0
    after = _record("module-analysis", key)
    assert after.revision == before.revision and after.payload == before.payload
    assert after.used_ns > before.used_ns


def test_lowering_reuse_across_sessions_no_relower(tmp_path, monkeypatch):
    path = _write_module(tmp_path, "m", "def g():\n    return 41 + 1\n")
    digest = "ab" * 32
    function = {"name": "m__g", "params": [], "return_abi": "value"}
    result = {
        "body_selection": {"kind": "complete_source", "roots": []},
        "function_metadata": {},
        "functions": [function],
        "profile": "release",
    }
    _use_session(monkeypatch, "A")
    writer_seeds = MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=digest,
        result=result,
    )
    assert len(writer_seeds) == 1 and writer_seeds[0].function is function
    before = _age_record("module-lowering", digest)
    from molt.cli.function_records import decode_lowering_record

    frame = decode_lowering_record(before.payload)
    metadata_result = json.loads(frame.metadata)["result"]
    assert "functions" not in metadata_result
    assert "function_digest_seeds" not in metadata_result
    _use_session(monkeypatch, "B")
    monkeypatch.setattr(
        SourceRecordStore,
        "put",
        lambda *_a, **_k: pytest.fail("cache hit republished a record"),
    )
    loaded = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
        context_digest=digest,
    )
    assert loaded is not None and loaded["profile"] == "release"
    assert loaded["functions"] == [function]
    (seed,) = loaded["function_digest_seeds"]
    assert seed.function is loaded["functions"][0] and seed.function is not function
    assert seed.digest == writer_seeds[0].digest and len(seed.digest) == 32
    assert shared_source_records().save_renewals() == 1
    after = _record("module-lowering", digest)
    assert after.revision == before.revision and after.payload == before.payload
    assert after.used_ns > before.used_ns


def test_lowering_reuse_across_projects_and_working_directories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A toolchain module is the same file in every project. A build in a
    # second project, run from that project's directory, must find the first
    # project's lowering of it.
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    path = _write_module(stdlib_root, "m", "def g():\n    return 41 + 1\n")
    context_digest = "cd" * 32
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()

    _use_session(monkeypatch, "A")
    monkeypatch.chdir(first)
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=context_digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [{"name": "m__g", "params": [], "return_abi": "value"}],
        },
    )

    _use_session(monkeypatch, "B")
    monkeypatch.chdir(second)
    reused = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
        context_digest=context_digest,
    )
    assert reused is not None
    assert reused["functions"][0]["name"] == "m__g"


def test_lowering_cache_hit_returns_fresh_decoded_payload_each_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def g():\n    return 41 + 1\n")
    context_digest = "ab" * 32
    ir_result = {
        "body_selection": {"kind": "complete_source", "roots": []},
        "function_metadata": {},
        "functions": [
            {
                "name": "m__g",
                "params": [],
                "return_abi": "value",
                "ops": [{"kind": "const", "value": 42}],
            }
        ],
        "profile": "release",
        "metadata": {"shape": ("module", "lowering")},
    }

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=context_digest,
        result=ir_result,
    )

    first = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
        context_digest=context_digest,
    )
    assert first is not None
    first["functions"][0]["name"] = "mutated"
    first["functions"][0]["ops"][0]["value"] = 99
    first["metadata"]["shape"] = ("mutated",)

    second = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
        context_digest=context_digest,
    )

    assert second is not None
    assert second["functions"][0]["name"] == "m__g"
    assert second["functions"][0]["ops"][0]["value"] == 42
    assert second["metadata"]["shape"] == ("module", "lowering")


# --------------------------------------------------------------------------
# mtime-decoupling (observed_inputs.md invariant 1): a re-checkout / copy /
# reinstall that resets file mtime WITHOUT changing bytes must still reuse the
# shared entry. mtime is content-invariant metadata; gating on it made the
# "content-addressed" cache metadata-addressed and cold-started every session.
# --------------------------------------------------------------------------


# Minimal lowering context inputs (mirrors the real call shape) so the tests can
# compute a *real* context_digest and prove it no longer moves with mtime.
_LOWERING_CTX = dict(
    module_execution_kind="imported",
    known_classes_snapshot={},
    type_hint_policy="trust",
    type_facts=None,
    enable_phi=True,
    known_modules=("m",),
    stdlib_allowlist=(),
    known_func_defaults={},
    known_func_kinds={},
    module_deps={"m": set()},
    module_chunking=False,
    module_chunk_max_ops=0,
    optimization_profile="dev",
    pgo_hot_function_names=(),
    module_dep_closures={"m": frozenset({"m"})},
)


@pytest.mark.parametrize("retired_policy", ["error", "bridge"])
def test_retired_policy_context_cannot_reuse_a_persisted_lowering(
    tmp_path: Path, retired_policy: str
) -> None:
    path = _write_module(tmp_path, "m", "pass\n")
    lease = _ModuleSourceLease.path_backed(path)
    context = MC._module_lowering_context_payload(
        "m",
        path,
        source_lease=lease,
        logical_source_path=str(path),
        is_package=False,
        **_LOWERING_CTX,
        module_source_role="user",
        namespace_premise=None,
    )
    assert context is not None and "fallback_policy" not in context
    current_digest = MC._module_lowering_context_digest(context)
    retired_digest = MC._module_lowering_context_digest(
        {**context, "fallback_policy": retired_policy}
    )
    assert current_digest is not None and retired_digest is not None
    assert current_digest != retired_digest
    poison = {
        "body_selection": {"kind": "complete_source", "roots": []},
        "functions": [],
        "function_metadata": {},
        "retired_policy_result": retired_policy,
    }
    # Use the CURRENT source-store generation and record schema deliberately;
    # only the removed semantic context field distinguishes this old record.
    MC._write_persisted_module_lowering(
        path, source_lease=lease, context_digest=retired_digest, result=poison
    )
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=lease,
            module_name="m",
            context_digest=current_digest,
        )
        is None
    )
    retained = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=lease,
        module_name="m",
        context_digest=retired_digest,
    )
    assert retained is not None
    assert retained["retired_policy_result"] == retired_policy


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
        "m",
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        logical_source_path=str(path),
        is_package=False,
        **_LOWERING_CTX,
        module_source_role="user",
        namespace_premise=None,
    )
    _bump_mtime(path)
    digest_after = MC._module_lowering_context_digest_for_module(
        "m",
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        logical_source_path=str(path),
        is_package=False,
        **_LOWERING_CTX,
        module_source_role="user",
        namespace_premise=None,
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
    ir_result = {
        "body_selection": {"kind": "complete_source", "roots": []},
        "function_metadata": {},
        "functions": [{"name": "m__g", "params": [], "return_abi": "value"}],
        "profile": "release",
    }

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=context_digest,
        result=ir_result,
    )

    # Session B: byte-identical source, but mtime reset (git checkout / copy).
    _use_session(monkeypatch, "B")
    _bump_mtime(path)

    monkeypatch.setattr(
        SourceRecordStore,
        "put",
        lambda *_a, **_k: pytest.fail("mtime-only hit republished a record"),
    )

    reused = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
        context_digest=context_digest,
    )
    assert reused is not None
    assert reused["functions"][0]["name"] == "m__g"
    # An admitted row hit does not republish despite the new source mtime.


def test_mtime_only_change_still_reuses_analysis_from_shared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The analysis cache must also reuse a byte-identical, mtime-reset module."""
    project_root = tmp_path / "proj"
    project_root.mkdir()
    path = _write_module(project_root, "m", "def f(a, b=2):\n    return a + b\n")

    _use_session(monkeypatch, "A")
    MC._write_persisted_module_analysis(
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
        snapshot=PythonSourceSnapshot.capture(path),
    )

    _use_session(monkeypatch, "B")
    _bump_mtime(path)
    result = MC._read_persisted_module_analysis(
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
    )
    assert result is not None
    _, kinds = result
    assert kinds == {"f": "sync"}


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
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=context_digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [
                {
                    "name": "m__f",
                    "params": ["a"],
                    "param_types": ["i64"],
                    "return_abi": "value",
                }
            ],
        },
    )

    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest=context_digest,
        )
        is not None
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
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
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
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=context_digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [
                {
                    "name": "m__f",
                    "params": ["a"],
                    "param_types": ["i64"],
                    "return_abi": "value",
                }
            ],
        },
    )
    assert _record("module-lowering", context_digest) is not None

    # Session B, DIFFERENT source content at the same path (a real edit).
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest=context_digest,
        )
        is not None
    )

    _use_session(monkeypatch, "B")
    path.write_text("def f(a):\n    return a + 999\n", encoding="utf-8")
    _clear_path_caches()  # drop any cached content sha for the old bytes

    reused = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
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
        snapshot=PythonSourceSnapshot.capture(path),
    )

    # Session B with a DIFFERENT tooling fingerprint (a compiler/frontend change).
    _use_session(monkeypatch, "B")
    monkeypatch.setattr(
        MC, "_frontend_semantic_tooling_fingerprint", lambda: "different-tooling-fp"
    )

    # The shared slot for the NEW fingerprint identity must be a different slot,
    # and no entry exists there -> cold miss (no cross-identity reuse).
    result = MC._read_persisted_module_analysis(
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="module_init",
    )
    assert result is None


def test_changed_target_python_is_not_reused_from_shared(tmp_path, monkeypatch):
    from molt.target_python import TargetPythonVersion

    path = _write_module(tmp_path, "m", "def f():\n    return 1\n")

    def context(target):
        return MC._module_lowering_context_digest_for_module(
            "m",
            path,
            source_lease=_ModuleSourceLease.path_backed(path),
            logical_source_path=str(path),
            is_package=False,
            target_python=target,
            **_LOWERING_CTX,
            module_source_role="user",
            namespace_premise=None,
        )

    py312, py313 = TargetPythonVersion(3, 12, 0), TargetPythonVersion(3, 13, 0)
    first, second = context(py312), context(py313)
    assert first is not None and second is not None and first != second
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=first,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [{"name": "m__f", "params": [], "return_abi": "value"}],
        },
    )
    _use_session(monkeypatch, "B")
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest=second,
        )
        is None
    )


# --------------------------------------------------------------------------
# One store publication; reads never duplicate or hydrate records.
# --------------------------------------------------------------------------


def test_module_cache_publishes_once_without_file_hydration(tmp_path, monkeypatch):
    path = _write_module(tmp_path, "m", "pass\n")
    calls = []
    put = SourceRecordStore.put

    def publish(store, kind, key, payload, *, generation):
        calls.append((kind, key))
        return put(store, kind, key, payload, generation=generation)

    monkeypatch.setattr(SourceRecordStore, "put", publish)
    MC._write_persisted_module_analysis(
        path,
        module_name="m",
        is_package=False,
        import_scan_mode="full",
        func_defaults={},
        func_kinds={},
        snapshot=PythonSourceSnapshot.capture(path),
    )
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest="ab" * 32,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [],
        },
    )
    assert [kind for kind, _ in calls] == ["module-analysis", "module-lowering"]
    before = list(calls)
    assert MC._read_persisted_module_analysis(
        path, module_name="m", is_package=False, import_scan_mode="full"
    ) == ({}, {})
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest="ab" * 32,
        )
        is not None
    )
    assert calls == before
    assert not list(tmp_path.rglob("module_analysis"))
    assert not list(tmp_path.rglob("module_lowering"))


# --------------------------------------------------------------------------
# Debug opt-out: MOLT_DISABLE_FRONTEND_LOWERING_CACHE forces a cold re-lower.
# --------------------------------------------------------------------------


def test_env_optout_forces_cold_lowering_miss(tmp_path, monkeypatch):
    path = _write_module(tmp_path, "m", "pass\n")
    digest = "ab" * 32
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [],
        },
    )
    before = _record("module-lowering", digest)
    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "1")
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest=digest,
        )
        is None
    )
    assert _record("module-lowering", digest).revision == before.revision
    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "0")
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(path),
            module_name="m",
            context_digest=digest,
        )
        is not None
    )


def test_env_optout_skips_shared_publish(tmp_path, monkeypatch):
    path = _write_module(tmp_path, "m", "pass\n")
    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "1")
    assert (
        MC._write_persisted_module_lowering(
            path,
            source_lease=_ModuleSourceLease.path_backed(path),
            context_digest="ab" * 32,
            result={
                "body_selection": {"kind": "complete_source", "roots": []},
                "function_metadata": {},
                "functions": [],
            },
        )
        == ()
    )
    assert source_records("module-lowering") == {}


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


def test_refused_shared_lowering_does_not_refresh_recency(tmp_path, monkeypatch):
    source = _write_module(tmp_path, "m", "def f():\n    return 1\n")
    digest = "ab" * 32
    MC._write_persisted_module_lowering(
        source,
        source_lease=_ModuleSourceLease.path_backed(source),
        context_digest=digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [{"name": "m__f", "params": [], "return_abi": "value"}],
        },
    )
    payload = source_records("module-lowering")[digest]
    payload["context_digest"] = "cd" * 32
    replace_source_record("module-lowering", digest, payload)
    before = _age_record("module-lowering", digest)
    _use_session(monkeypatch, "reader")
    assert (
        MC._read_persisted_module_lowering(
            source,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=_ModuleSourceLease.path_backed(source),
            module_name="m",
            context_digest=digest,
        )
        is None
    )
    after = _record("module-lowering", digest)
    assert after.used_ns == before.used_ns and after.revision == before.revision


@pytest.mark.parametrize(
    "defect",
    [None, "missing", "orphan", "duplicate", "bad-boundary", "embedded", "schema8"],
)
def test_actual_frontend_sidecar_survives_cache_and_refuses_invalid_ownership(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, defect: str | None
) -> None:
    import ast
    from copy import deepcopy
    from molt.frontend import SimpleTIRGenerator

    path = _write_module(tmp_path, "m", "def answer(value):\n    return value\n")
    gen = SimpleTIRGenerator(module_name="m", module_source_role="user")
    gen.visit(ast.parse(path.read_text(encoding="utf-8")))
    document = gen.to_json(scope="module")
    result = {
        "body_selection": {"kind": "complete_source", "roots": []},
        "functions": document["functions"],
        "function_metadata": document["function_metadata"],
    }
    expected = deepcopy(result)
    _use_session(monkeypatch, "A")
    context_digest = "ad" * 32
    lease = _ModuleSourceLease.path_backed(path)
    MC._write_persisted_module_lowering(
        path,
        source_lease=lease,
        context_digest=context_digest,
        result=result,
    )
    _use_session(monkeypatch, "B")
    loaded = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=lease,
        module_name="m",
        context_digest=context_digest,
    )
    assert loaded is not None
    seeds = loaded.pop("function_digest_seeds")
    assert loaded == expected
    assert all(
        seed.function is function
        for seed, function in zip(seeds, loaded["functions"], strict=True)
    )
    assert loaded["functions"] is not result["functions"]
    assert (
        loaded["function_metadata"]["molt_init_m"]
        is not result["function_metadata"]["molt_init_m"]
    )
    assert all(
        "source_module_publication" not in function for function in loaded["functions"]
    )
    if defect is None:
        return
    broken = deepcopy(result)
    if defect == "missing":
        del broken["function_metadata"]
    elif defect == "orphan":
        broken["function_metadata"]["absent"] = broken["function_metadata"].pop(
            "molt_init_m"
        )
    elif defect == "duplicate":
        broken["functions"].append(deepcopy(broken["functions"][0]))
    elif defect == "bad-boundary":
        broken["function_metadata"]["molt_init_m"]["source_module_publication"][
            "boundary_index"
        ] = -1
    elif defect == "embedded":
        broken["functions"][0]["gpu_body_origin"] = {}
    # Exercise the actual read admission; corrupt bytes use the existing codec.
    payload = {
        "version": 8
        if defect == "schema8"
        else MC._MODULE_LOWERING_CACHE_SCHEMA_VERSION,
        "context_digest": context_digest,
        "source_sha256": lease.source_sha256,
        "size": lease.source_size,
        "source_kind": lease.source_kind,
        "result": json.loads(json.dumps(broken)),
    }
    replace_source_record("module-lowering", context_digest, payload)
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=lease,
            module_name="m",
            context_digest=context_digest,
        )
        is None
    )


def test_valid_lowering_record_exceeds_analysis_allocation_limit(tmp_path):
    from molt.cli.json_cache import SOURCE_ANALYSIS_CACHE_MAX_BYTES

    path = _write_module(tmp_path, "m", "pass\n")
    text = "a" * (SOURCE_ANALYSIS_CACHE_MAX_BYTES + 1)
    function = {
        "name": "m__f",
        "params": [],
        "return_abi": "value",
        "ops": [
            {"kind": "const_str", "s_value": text, "out": "value"},
            {"kind": "ret", "args": ["value"]},
        ],
    }
    digest = "ef" * 32
    MC._write_persisted_module_lowering(
        path,
        source_lease=_ModuleSourceLease.path_backed(path),
        context_digest=digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [function],
        },
    )
    loaded = MC._read_persisted_module_lowering(
        path,
        body_selection=ModuleBodySelection("complete_source"),
        source_lease=_ModuleSourceLease.path_backed(path),
        module_name="m",
        context_digest=digest,
    )
    assert loaded is not None
    assert loaded["functions"][0]["ops"][0]["s_value"] == text
    assert loaded["function_digest_seeds"][0].function is loaded["functions"][0]


@pytest.mark.parametrize(
    "metadata_result",
    [{"__tuple__": []}, {"__set__": []}, {"__bytes__": ""}, {"__ellipsis__": True}],
)
def test_nonobject_rich_metadata_refuses_without_refresh(tmp_path, metadata_result):
    import msgspec

    path = _write_module(tmp_path, "m", "pass\n")
    lease = _ModuleSourceLease.path_backed(path)
    digest = "bc" * 32
    MC._write_persisted_module_lowering(
        path,
        source_lease=lease,
        context_digest=digest,
        result={
            "body_selection": {"kind": "complete_source", "roots": []},
            "function_metadata": {},
            "functions": [],
        },
    )
    original = _record("module-lowering", digest)
    # Keep the real frame, source and body identity intact. Only the rich
    # metadata result decodes to a non-object; it must be an optional miss.
    wire = msgspec.msgpack.decode(original.payload)
    metadata = json.loads(wire[1])
    metadata["result"] = metadata_result
    wire[1] = json.dumps(metadata).encode()
    assert shared_source_records().put(
        "module-lowering",
        digest,
        msgspec.msgpack.encode(wire),
        generation=original.generation,
    )
    before = _age_record("module-lowering", digest)
    assert (
        MC._read_persisted_module_lowering(
            path,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=lease,
            module_name="m",
            context_digest=digest,
        )
        is None
    )
    after = _record("module-lowering", digest)
    assert (after.revision, after.used_ns) == (before.revision, before.used_ns)
