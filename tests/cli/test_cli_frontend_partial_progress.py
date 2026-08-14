"""Failure-resilience (build-dedup doctrine 74, law 2) for frontend lowering.

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

import pytest

from molt.cli import frontend_execution as FE
from molt.cli import module_cache as MC
from molt.cli.models import _ModuleGraphMetadata
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import _build_module_source_catalog
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION


def _clear_path_caches() -> None:
    from molt.cli import default_paths, module_graph_cache, module_source, runtime_paths

    default_paths._default_molt_cache_cached.cache_clear()
    runtime_paths._build_state_root_cached.cache_clear()
    module_graph_cache._resolved_module_cache_key.cache_clear()
    module_source._source_content_sha256_cached.cache_clear()


@pytest.fixture(autouse=True)
def _isolated(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "molt_cache"))
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
        entry_override_by_module={n: None for n in names},
        module_is_namespace_by_module={n: False for n in names},
        module_is_package_by_module={n: False for n in names},
        frontend_module_costs={n: 1.0 for n in names},
        stdlib_like_by_module={n: False for n in names},
    )
    return FE._run_frontend_parallel_layer_batches(
        names,
        layer_workers=len(names),
        executor=_SyncExecutor(),
        known_classes_snapshot_source={},
        module_graph=module_graph,
        module_source_catalog=_build_module_source_catalog(module_graph),
        project_root=project_root,
        module_resolution_cache=_ModuleResolutionCache(),
        parse_codec="msgpack",
        type_hint_policy="ignore",
        fallback_policy="error",
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
        path_stat_by_module=None,
        module_chunking=False,
        scoped_lowering_inputs=None,
        dirty_lowering_modules=set(),
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        target_sys_platform=None,
        frontend_phase_timeout=None,
    )


def _lowering_slot(project_root: Path, path: Path, name: str) -> Path:
    return MC._module_lowering_cache_path(
        project_root, path, module_name=name, is_package=False
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

    good_slot = _lowering_slot(project_root, good, "good")
    assert not good_slot.exists()

    layer_state, batch_error, _detail, _broken = _run_batch(project_root, module_graph)
    assert batch_error is None
    assert layer_state.results["good"]["ok"] is True
    # Persisted THE MOMENT it succeeded -- no consume loop was run.
    assert good_slot.is_file()


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

    good_slot = _lowering_slot(project_root, good, "good")
    boom_slot = _lowering_slot(project_root, boom, "boom")

    layer_state, batch_error, _detail, _broken = _run_batch(project_root, module_graph)
    assert batch_error is None
    assert layer_state.results["good"]["ok"] is True
    assert layer_state.results["boom"]["ok"] is False  # syntax error

    # Completed module persisted; failed module did not (nothing to reuse).
    assert good_slot.is_file()
    assert not boom_slot.exists()


def test_persisted_completed_module_reused_on_retry(tmp_path: Path) -> None:
    """After a partial batch, a fresh session hydrates the completed module.

    Proves the eager write reaches the SHARED tier too, so a retry in a new
    session pays zero re-lower for the already-completed module.
    """
    project_root = tmp_path / "proj"
    (project_root / "src").mkdir(parents=True)
    good = project_root / "src" / "good.py"
    good.write_text("value = 7\nsquared = value * value\n", encoding="utf-8")

    _run_batch(project_root, {"good": good})

    shared = MC._shared_module_lowering_cache_path_for(
        good,
        module_name="good",
        is_package=False,
        context_digest=_context_digest(project_root, good, "good"),
    )
    assert shared is not None and shared.is_file()


def _context_digest(project_root: Path, path: Path, name: str) -> str:
    digest = MC._module_lowering_context_digest_for_module(
        name,
        path,
        logical_source_path=str(path),
        entry_override=None,
        is_package=False,
        known_classes_snapshot={},
        parse_codec="msgpack",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules={name},
        stdlib_allowlist=(),
        known_func_defaults={},
        known_func_kinds={},
        module_deps={name: set()},
        module_is_namespace=False,
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=(),
        module_dep_closures={name: frozenset({name})},
    )
    assert digest is not None
    return digest
