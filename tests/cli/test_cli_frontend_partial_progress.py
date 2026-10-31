"""Failure keeps progress (observed_inputs.md invariant 4) for frontend lowering.

A killed / failed build must NOT forfeit the modules it already lowered: each
module's lowering is persisted THE MOMENT it succeeds, so a retry pays only for
the failed step forward ("cold compile once, incremental forever -- even through
failures").

The parallel frontend used to defer every per-module cache write to a post-layer
consume loop, so a mid-layer kill (memory_guard SIGTERM / OOM / phase timeout) or
a later sibling module's failure discarded EVERY already-lowered module in the
in-flight layer. These tests drive the REAL parallel batch runner with a
synchronous executor and prove the completed module is persisted even when the
consume loop never runs and a sibling module fails.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable
import os

import pytest

from molt.cli import frontend_execution as FE
from molt.cli import frontend_worker as FW
from molt.cli.models import _ModuleGraphMetadata
from molt.cli.module_source import _build_module_source_catalog
from tests.source_record_test_support import source_records
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION


def _clear_path_caches() -> None:
    from molt import default_paths
    from molt.cli import module_source, runtime_paths

    default_paths._default_user_state_cached.cache_clear()
    runtime_paths._build_state_root_cached.cache_clear()
    module_source._source_content_sha256_cached.cache_clear()


@pytest.fixture(autouse=True)
def _isolated(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "molt_cache"))
    # Source-qualified records must stay private to this test's cache.
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "build_state"))
    monkeypatch.setenv("MOLT_SESSION_ID", "partial-progress")
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", raising=False)
    _clear_path_caches()
    yield
    _clear_path_caches()


class _ImmediateFuture:
    def __init__(self, fn: Callable[..., Any], *args: Any) -> None:
        self._fn = fn
        self._args = args

    def result(self) -> Any:
        return self._fn(*self._args)


class _SyncExecutor:
    """Runs each submitted worker in-process (no ProcessPool spawn / flake)."""

    def submit(self, fn: Callable[..., Any], *args: Any) -> _ImmediateFuture:
        return _ImmediateFuture(fn, *args)


def _run_batch(project_root: Path, module_graph: dict[str, Path]):
    names = sorted(module_graph)
    metadata = _ModuleGraphMetadata(
        logical_source_path_by_module={n: str(module_graph[n]) for n in names},
        namespace_portions_by_module={},
        module_is_package_by_module={n: False for n in names},
        module_execution_kind_by_module={n: "imported" for n in names},
        frontend_module_costs={n: 1.0 for n in names},
        stdlib_like_by_module={n: False for n in names},
        module_source_role_by_module={n: "user" for n in names},
        scan_authority=None,
    )
    return FE._run_frontend_parallel_layer_batches(
        names,
        layer_workers=len(names),
        executor=_SyncExecutor(),
        known_classes_snapshot_source={},
        module_graph=module_graph,
        module_source_catalog=_build_module_source_catalog(module_graph),
        project_root=project_root,
        type_hint_policy="ignore",
        type_facts=None,
        enable_phi=True,
        known_modules=set(names),
        direct_call_modules=set(names),
        stdlib_allowlist=set(),
        known_func_defaults={},
        known_func_kinds={},
        native_callable_exports={},
        native_python_exports=(),
        module_deps={n: set() for n in names},
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        known_modules_sorted=tuple(names),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(),
        module_dep_closures={n: frozenset({n}) for n in names},
        module_graph_metadata=metadata,
        module_chunking=False,
        scoped_lowering_inputs=None,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        target_sys_platform=None,
        frontend_phase_timeout=None,
    )


def test_completed_module_is_persisted_before_consume_loop(tmp_path: Path) -> None:
    """The good module's lowering is on disk right after the batch returns.

    The batch runner is called WITHOUT the post-batch consume loop -- exactly the
    state a mid-layer kill leaves. Pre-fix the good module's slot would be absent
    (persist was deferred to the consume loop); it must now be present.
    """
    project_root = tmp_path / "proj"
    (project_root / "src").mkdir(parents=True)
    good = project_root / "src" / "good.py"
    good.write_text("x = 1\ny = x + 2\n", encoding="utf-8")
    module_graph = {"good": good}

    assert source_records("module-lowering") == {}

    layer_state, batch_error, _detail, _broken = _run_batch(project_root, module_graph)
    assert batch_error is None
    assert layer_state.results["good"]["ok"] is True
    # Persisted THE MOMENT it succeeded -- no consume loop was run.
    records = source_records("module-lowering")
    assert list(records) == [layer_state.context_digests["good"]]
    assert (
        records[layer_state.context_digests["good"]]["result"]["functions"]
        == layer_state.results["good"]["functions"]
    )


def test_sibling_failure_does_not_forfeit_completed_module(tmp_path: Path) -> None:
    """A syntax-error sibling fails, but the good module is still persisted.

    This is the failure-resilience acceptance in miniature: one module of the
    layer fails (ok=False), yet the completed module's lowering survives so a
    retry re-lowers only the failed one.
    """
    project_root = tmp_path / "proj"
    (project_root / "src").mkdir(parents=True)
    good = project_root / "src" / "good.py"
    good.write_text("a = 10\nb = a * 3\n", encoding="utf-8")
    boom = project_root / "src" / "boom.py"
    boom.write_text("def broken(:\n    pass\n", encoding="utf-8")  # SyntaxError
    module_graph = {"good": good, "boom": boom}

    layer_state, batch_error, _detail, _broken = _run_batch(project_root, module_graph)
    assert batch_error is None
    assert layer_state.results["good"]["ok"] is True
    assert layer_state.results["boom"]["ok"] is False  # syntax error

    # Completed module persisted; failed module did not (nothing to reuse).
    records = source_records("module-lowering")
    assert list(records) == [layer_state.context_digests["good"]]
    assert (
        records[layer_state.context_digests["good"]]["result"]["functions"]
        == layer_state.results["good"]["functions"]
    )
    assert (
        "boom" not in layer_state.context_digests
        or layer_state.context_digests["boom"] not in records
    )


def test_persisted_completed_module_reused_on_retry(tmp_path, monkeypatch):
    project_root = tmp_path / "proj"
    (project_root / "src").mkdir(parents=True)
    good = project_root / "src/good.py"
    good.write_text("value = 7\nsquared = value * value\n", encoding="utf-8")
    first, error, detail, broken = _run_batch(project_root, {"good": good})
    assert error is None and detail is None and not broken
    assert first.context_digests["good"] in source_records("module-lowering")
    monkeypatch.setenv("MOLT_SESSION_ID", "partial-progress-retry")
    _clear_path_caches()
    monkeypatch.setattr(
        FW,
        "_frontend_lower_module_worker",
        lambda *_a, **_k: pytest.fail("completed module lowered again on retry"),
    )
    retry, error, detail, broken = _run_batch(project_root, {"good": good})
    assert error is None and detail is None and not broken
    assert retry.results["good"]["functions"] == first.results["good"]["functions"]
    seeds = retry.results["good"]["function_digest_seeds"]
    assert seeds and all(
        seed.function is function
        for seed, function in zip(
            seeds, retry.results["good"]["functions"], strict=True
        )
    )


@pytest.mark.parametrize("defect", ["source", "stamp"])
def test_changed_worker_generation_is_terminal_and_never_persisted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, defect: str
) -> None:
    path = tmp_path / "good.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    original_stat = path.stat()
    assert source_records("module-lowering") == {}
    lower = FW._frontend_lower_module_worker

    def changed_worker(payload: dict[str, Any]) -> dict[str, Any]:
        result = lower(payload)
        assert result["ok"] is True
        if defect == "source":
            path.write_text("VALUE = 2\n", encoding="utf-8")
            os.utime(path, ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns))
        else:
            result["source_sha256"] = "0" * 64
        return result

    monkeypatch.setattr(FW, "_frontend_lower_module_worker", changed_worker)
    state, error, detail, broken = _run_batch(tmp_path, {"good": path})
    assert error is not None
    assert "source" in error.lower()
    assert detail is None
    assert broken is False
    assert "good" not in state.results
    assert source_records("module-lowering") == {}
