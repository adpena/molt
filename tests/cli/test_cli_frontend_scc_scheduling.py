"""Teeth for the frontend-parallel-disable bug class.

These cover the whole class of "silently fall back to fully serial frontend
lowering" defects that a rigorous audit collapsed into one structural cut:

  A1 an import cycle anywhere disabled ALL frontend parallelism
      (``has_back_edges -> serial``); fixed by Tarjan SCC condensation.
  A2 a configured frontend phase timeout disabled ALL parallelism
      (``frontend_phase_timeout -> serial``); fixed by applying the per-phase
      timeout inside each parallel worker instead.
  A3/A4 one transient worker error latched ``parallel_pool_usable = False`` for
      the entire rest of the build; fixed by isolating the failure to the
      failing module and only surrendering (and recreating) the pool when it is
      genuinely broken.

Each test also documents the mutation that makes it fail, so the teeth are
provably load-bearing.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from molt.cli import frontend_execution as cli_frontend_execution
from molt.cli import frontend_parallel as cli_frontend_parallel
from molt.cli import module_dependencies as cli_module_dependencies
from molt.cli import frontend_worker as cli_frontend_worker
from molt.cli.models import _FrontendLayerPlan, _FrontendParallelLayerState


def _graph(names: list[str]) -> dict[str, Path]:
    return {name: Path(f"/tmp/{name}.py") for name in names}


# ---------------------------------------------------------------------------
# A1: SCC condensation — a cycle must not serialize the whole frontend.
# ---------------------------------------------------------------------------


def test_strongly_connected_components_iterative_tarjan() -> None:
    # a<->b cycle, plus a self-referential-free chain c -> d, and isolated e.
    module_deps = {
        "a": {"b"},
        "b": {"a"},
        "c": set(),
        "d": {"c"},
        "e": set(),
    }
    sccs = cli_module_dependencies._module_strongly_connected_components(
        set(module_deps),
        module_deps,
    )
    # a and b collapse into one component; everything else is a singleton.
    by_size = {frozenset(component) for component in sccs}
    assert frozenset({"a", "b"}) in by_size
    assert frozenset({"c"}) in by_size
    assert frozenset({"d"}) in by_size
    assert frozenset({"e"}) in by_size
    assert len(sccs) == 4
    # Every SCC is emitted in topological order: a dependency's SCC precedes the
    # dependent's SCC. c must appear before d.
    order = [component for component in sccs]
    c_index = next(i for i, comp in enumerate(order) if comp == ("c",))
    d_index = next(i for i, comp in enumerate(order) if comp == ("d",))
    assert c_index < d_index


def test_tarjan_handles_deep_chain_without_recursion_error() -> None:
    # A chain far deeper than the default Python recursion limit proves the
    # iterative form does not blow the stack (numpy/scipy graphs are this deep).
    depth = 5000
    module_deps: dict[str, set[str]] = {}
    for i in range(depth):
        module_deps[f"m{i}"] = {f"m{i + 1}"} if i + 1 < depth else set()
    sccs = cli_module_dependencies._module_strongly_connected_components(
        set(module_deps),
        module_deps,
    )
    assert len(sccs) == depth
    assert all(len(component) == 1 for component in sccs)


def test_a1_cycle_does_not_disable_frontend_parallelism(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "8")
    # a<->b cycle, plus independent c, d, e, and a chain to create layers.
    module_graph = _graph(["a", "b", "c", "d", "e", "f", "g"])
    module_deps = {
        "a": {"b"},
        "b": {"a"},
        "c": set(),
        "d": set(),
        "e": set(),
        "f": {"c"},
        "g": {"f"},
    }
    (
        order,
        _reverse,
        has_back_edges,
        layers,
        _closures,
        scc_serial_modules,
    ) = cli_module_dependencies._analyze_module_schedule(module_graph, module_deps)

    # No module is dropped despite the cycle (the pre-fix Kahn path dropped the
    # cyclic modules from `order`/`layers`).
    flat = [name for layer in layers for name in layer]
    assert sorted(flat) == sorted(module_graph)
    assert len(flat) == len(module_graph)
    assert set(order) == set(module_graph)

    # a<->b is exactly one SCC unit that stays together in one layer.
    assert ["a", "b"] in layers
    assert scc_serial_modules == frozenset({"a", "b"})

    # The independent modules c, d, e land together in a parallel-eligible layer
    # even though the a<->b cycle exists.
    assert ["c", "d", "e"] in layers

    # And the global parallel config is NOT disabled by the cycle. Pre-fix this
    # returned enabled=False, reason="dependency_back_edge".
    config = cli_frontend_parallel._resolve_frontend_parallel_config(
        module_count=len(module_graph),
    )
    assert config.enabled is True
    assert config.reason == "enabled"


def test_a1_pure_dag_layers_are_unchanged() -> None:
    # A pure DAG must produce the same topological layering as before (each
    # module in the earliest layer whose dependencies all precede it).
    module_graph = _graph(["a", "b", "c", "d", "e"])
    module_deps = {
        "a": set(),
        "b": {"a"},
        "c": {"a"},
        "d": {"b", "c"},
        "e": {"b"},
    }
    (
        order,
        _reverse,
        has_back_edges,
        layers,
        _closures,
        scc_serial_modules,
    ) = cli_module_dependencies._analyze_module_schedule(module_graph, module_deps)
    assert has_back_edges is False
    assert scc_serial_modules == frozenset()
    assert layers == [["a"], ["b", "c"], ["d", "e"]]
    assert order == ["a", "b", "c", "d", "e"]


def test_a1_layer_plan_forces_serial_for_scc_unit_but_parallel_for_peers(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "8")
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MIN_PREDICTED_COST", "0")
    from molt.cli.module_source import _ModuleSourceCatalog

    module_graph = _graph(["a", "b", "c", "d", "e"])
    module_deps = {
        "a": {"b"},
        "b": {"a"},
        "c": set(),
        "d": set(),
        "e": set(),
    }
    costs = {name: 100000.0 for name in module_graph}
    stdlib_like = {name: False for name in module_graph}
    catalog = _ModuleSourceCatalog(leases={})
    config = cli_frontend_parallel._resolve_frontend_parallel_config(
        module_count=len(module_graph),
    )
    scc_serial_modules = frozenset({"a", "b"})

    scc_plan = cli_frontend_parallel._frontend_layer_plan(
        ["a", "b"],
        syntax_error_modules={},
        module_source_catalog=catalog,
        module_graph=module_graph,
        module_deps=module_deps,
        frontend_module_costs=costs,
        stdlib_like_by_module=stdlib_like,
        frontend_parallel_config=config,
        parallel_pool_usable=True,
        scc_serial_modules=scc_serial_modules,
    )
    # The cycle must be a single serial scheduling unit — never split across
    # parallel workers (that would lower each member against a frozen snapshot
    # missing its cyclic peer, changing results).
    assert scc_plan.mode == "serial"
    assert scc_plan.workers == 1
    assert scc_plan.policy_reason == "cyclic_scc_serial_unit"

    peer_plan = cli_frontend_parallel._frontend_layer_plan(
        ["c", "d", "e"],
        syntax_error_modules={},
        module_source_catalog=catalog,
        module_graph=module_graph,
        module_deps=module_deps,
        frontend_module_costs=costs,
        stdlib_like_by_module=stdlib_like,
        frontend_parallel_config=config,
        parallel_pool_usable=True,
        scc_serial_modules=scc_serial_modules,
    )
    assert peer_plan.mode == "parallel"
    assert peer_plan.workers >= 2


# ---------------------------------------------------------------------------
# A2: a configured phase timeout must not serialize; the timeout must fire
# per-worker instead.
# ---------------------------------------------------------------------------


def test_a2_phase_timeout_does_not_disable_parallelism(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "8")
    # Pre-fix, a non-None frontend_phase_timeout forced enabled=False with
    # reason="phase_timeout_configured". The config no longer even takes the
    # timeout: it is applied inside each worker.
    config = cli_frontend_parallel._resolve_frontend_parallel_config(
        module_count=4,
    )
    assert config.enabled is True
    assert config.reason == "enabled"


def test_a2_worker_applies_phase_timeout_per_module(tmp_path, monkeypatch) -> None:
    # Prove the per-worker timeout actually fires: force _phase_timeout to raise
    # for the visit phase and confirm the worker returns a timed-out error
    # result rather than a successful lowering. This is the mechanism that
    # replaces the deleted "serialize when a timeout is configured" branch.
    from molt.cli.module_source import _ModuleSourceLease

    module_path = tmp_path / "slow_module.py"
    source = "x = 1\ny = x + 2\n"

    import contextlib

    @contextlib.contextmanager
    def _fake_phase_timeout(timeout_s, *, phase_name):
        # Only trip when a timeout is actually configured (mirrors real usage);
        # this models SIGALRM firing mid-phase.
        if timeout_s is not None and "visit" in phase_name:
            raise TimeoutError(f"{phase_name} timed out after {timeout_s:.1f}s")
        yield

    monkeypatch.setattr(cli_frontend_worker, "_phase_timeout", _fake_phase_timeout)

    def _payload(frontend_phase_timeout):
        return {
            "module_name": "slow_module",
            "module_path": str(module_path),
            "source_lease": _ModuleSourceLease.inline(
                module_path, source
            ).worker_payload(),
            "parse_codec": "msgpack",
            "type_hint_policy": "ignore",
            "fallback_policy": "error",
            "module_is_namespace": False,
            "entry_module": None,
            "enable_phi": True,
            "known_modules": ["slow_module"],
            "direct_call_modules": ["slow_module"],
            "known_classes": {},
            "stdlib_allowlist": [],
            "known_func_defaults": {},
            "known_func_kinds": {},
            "module_chunking": False,
            "module_chunk_max_ops": 0,
            "optimization_profile": "dev",
            "pgo_hot_functions": ["slow_module::molt_main"],
            "frontend_phase_timeout": frontend_phase_timeout,
        }

    result = cli_frontend_worker._frontend_lower_module_worker(_payload(0.25))
    assert result["ok"] is False
    assert result.get("timed_out") is True
    assert "timed out" in result["error"]

    # With NO timeout configured, the same module lowers successfully — the
    # timeout is genuinely per-module and only fires when configured.
    result_ok = cli_frontend_worker._frontend_lower_module_worker(_payload(None))
    assert result_ok["ok"] is True


# ---------------------------------------------------------------------------
# A3/A4: a transient worker error must not latch parallelism off for the rest
# of the build.
# ---------------------------------------------------------------------------


def test_a3_transient_worker_error_keeps_pool_usable() -> None:
    # A per-module exception (pool_broken=False) records an isolated-serial note
    # and must NOT flag the pool broken. The layer plan mode/workers are
    # preserved so the caller re-lowers only the failed module serially while
    # keeping parallelism for later layers.
    details: dict[str, Any] = {}
    warnings: list[str] = []
    cli_frontend_parallel._note_frontend_parallel_layer_failure(
        frontend_parallel_details=details,
        warnings=warnings,
        failure_detail="/tmp/mod.py: boom",
        pool_broken=False,
    )
    assert details["reason"] == "worker_error_isolated_serial"
    assert warnings and "keeping the pool" in warnings[0]


def test_a4_broken_pool_is_reported_for_recreation() -> None:
    details: dict[str, Any] = {}
    warnings: list[str] = []
    cli_frontend_parallel._note_frontend_parallel_layer_failure(
        frontend_parallel_details=details,
        warnings=warnings,
        failure_detail="/tmp/mod.py: BrokenProcessPool",
        pool_broken=True,
    )
    assert details["reason"] == "worker_pool_broken_recreate"
    assert warnings and "recreating pool" in warnings[0]


def _make_execution_context(
    module_graph: dict[str, Path],
    *,
    scc_serial_modules: frozenset[str] = frozenset(),
):
    from molt.cli.models import _FrontendLayerExecutionContext

    names = list(module_graph)
    return _FrontendLayerExecutionContext(
        syntax_error_modules={},
        module_graph=module_graph,
        module_source_catalog=None,
        project_root=None,
        module_resolution_cache=None,
        parse_codec=None,
        type_hint_policy=None,
        fallback_policy=None,
        type_facts=None,
        enable_phi=False,
        known_modules=names,
        direct_call_modules=[],
        stdlib_allowlist=[],
        known_func_defaults={},
        known_func_kinds={},
        native_callable_exports={},
        native_python_exports=[],
        module_deps={name: set() for name in names},
        source_modules=names,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        known_modules_sorted=tuple(names),
        stdlib_allowlist_sorted=(),
        pgo_hot_function_names_sorted=(),
        module_dep_closures={name: frozenset({name}) for name in names},
        module_graph_metadata=None,
        path_stat_by_module=None,
        module_chunking=False,
        scoped_lowering_inputs=None,
        dirty_lowering_modules=set(),
        frontend_module_costs={name: 1.0 for name in names},
        stdlib_like_by_module={name: False for name in names},
        known_classes={},
        target_python=None,
        target_sys_platform=None,
        frontend_phase_timeout=None,
        scc_serial_modules=scc_serial_modules,
    )


def test_a3_run_frontend_layer_isolates_failed_module_and_keeps_parallel(
    monkeypatch,
) -> None:
    # End-to-end within one layer: three modules go parallel, module "b" raises
    # a transient error. The failed module must be re-lowered serially, the two
    # good results kept, and the pool must remain usable (parallel_pool_usable
    # stays True) so subsequent layers still run parallel.
    module_graph = _graph(["a", "b", "c"])
    execution_context = _make_execution_context(module_graph)

    lowered_serially: list[str] = []
    integrated: list[str] = []

    def _fake_layer_plan(layer, **kwargs):
        return _FrontendLayerPlan(
            candidates=tuple(layer),
            predicted_cost_total=0.0,
            effective_min_predicted_cost=0.0,
            stdlib_candidates=0,
            workers=3,
            policy_reason="enabled",
            mode="parallel",
        )

    def _fake_batches(candidates, **kwargs):
        # Simulate: a and c lowered fine in parallel; b raised a transient error
        # (pool still healthy -> pool_broken False). b is absent from results.
        state = _FrontendParallelLayerState()
        for name in ("a", "c"):
            state.results[name] = {"ok": True, "functions": {}, "timings": {}}
        return state, None, "/tmp/b.py: boom", False

    def _fake_serial_layer_modules(module_names, *, layer_state, **kwargs):
        for name in module_names:
            lowered_serially.append(name)
            layer_state.results[name] = {"ok": True, "functions": {}, "timings": {}}
        return None

    def _fake_consume(*, module_name, **kwargs):
        integrated.append(module_name)
        return None

    monkeypatch.setattr(
        cli_frontend_parallel, "_frontend_layer_plan", _fake_layer_plan
    )
    monkeypatch.setattr(
        cli_frontend_execution,
        "_run_frontend_parallel_layer_batches",
        _fake_batches,
    )
    monkeypatch.setattr(
        cli_frontend_execution,
        "_run_frontend_serial_layer_modules",
        _fake_serial_layer_modules,
    )
    monkeypatch.setattr(
        cli_frontend_execution,
        "_consume_frontend_parallel_layer_result",
        _fake_consume,
    )

    warnings: list[str] = []
    details: dict[str, Any] = {}
    runtime_hooks = _make_runtime_hooks(warnings, details)

    class _FakeExecutor:
        pass

    result, error = cli_frontend_execution._run_frontend_layer(
        ["a", "b", "c"],
        layer_index=0,
        executor=_FakeExecutor(),
        execution_context=execution_context,
        runtime_hooks=runtime_hooks,
        frontend_parallel_config=_make_config(),
        parallel_pool_usable=True,
    )
    assert error is None
    assert result is not None
    # Only the failed module "b" is re-lowered serially; a and c are kept.
    assert lowered_serially == ["b"]
    # a and c integrate from parallel results, b integrates from serial re-run.
    assert set(integrated) == {"a", "c"}
    # A transient error keeps the pool usable for later layers.
    assert result.parallel_pool_usable is True
    assert details["reason"] == "worker_error_isolated_serial"


def test_a4_run_frontend_layer_surrenders_pool_only_when_broken(
    monkeypatch,
) -> None:
    # Same shape, but the batch reports pool_broken=True (a worker process
    # crashed). Now the pool must be surrendered (parallel_pool_usable False) so
    # the enclosing loop recreates it once; the failed module is still re-lowered
    # serially and good results are still kept.
    module_graph = _graph(["a", "b", "c"])
    execution_context = _make_execution_context(module_graph)
    lowered_serially: list[str] = []

    def _fake_layer_plan(layer, **kwargs):
        return _FrontendLayerPlan(
            candidates=tuple(layer),
            predicted_cost_total=0.0,
            effective_min_predicted_cost=0.0,
            stdlib_candidates=0,
            workers=3,
            policy_reason="enabled",
            mode="parallel",
        )

    def _fake_batches(candidates, **kwargs):
        state = _FrontendParallelLayerState()
        state.results["a"] = {"ok": True, "functions": {}, "timings": {}}
        return state, None, "/tmp/b.py: BrokenProcessPool", True

    def _fake_serial_layer_modules(module_names, *, layer_state, **kwargs):
        for name in module_names:
            lowered_serially.append(name)
            layer_state.results[name] = {"ok": True, "functions": {}, "timings": {}}
        return None

    def _fake_consume(*, module_name, **kwargs):
        return None

    monkeypatch.setattr(
        cli_frontend_parallel, "_frontend_layer_plan", _fake_layer_plan
    )
    monkeypatch.setattr(
        cli_frontend_execution,
        "_run_frontend_parallel_layer_batches",
        _fake_batches,
    )
    monkeypatch.setattr(
        cli_frontend_execution,
        "_run_frontend_serial_layer_modules",
        _fake_serial_layer_modules,
    )
    monkeypatch.setattr(
        cli_frontend_execution,
        "_consume_frontend_parallel_layer_result",
        _fake_consume,
    )

    warnings: list[str] = []
    details: dict[str, Any] = {}
    runtime_hooks = _make_runtime_hooks(warnings, details)

    result, error = cli_frontend_execution._run_frontend_layer(
        ["a", "b", "c"],
        layer_index=0,
        executor=object(),
        execution_context=execution_context,
        runtime_hooks=runtime_hooks,
        frontend_parallel_config=_make_config(),
        parallel_pool_usable=True,
    )
    assert error is None
    assert result is not None
    # b and c were missing from results -> re-lowered serially; a kept.
    assert lowered_serially == ["b", "c"]
    assert result.parallel_pool_usable is False
    assert details["reason"] == "worker_pool_broken_recreate"


def _make_config():
    return cli_frontend_parallel._resolve_frontend_parallel_config(module_count=8)


def _make_runtime_hooks(warnings: list[str], details: dict[str, Any]):
    from molt.cli.models import _FrontendLayerRuntimeHooks

    def _record_timing(**kwargs):
        return {}

    def _integrate(**kwargs):
        return None

    def _accumulate(**kwargs):
        return None

    def _fail(*args, **kwargs):
        raise AssertionError(f"fail() should not be called: {args} {kwargs}")

    def _run_serial(module_name, module_path):
        return {"ok": True}, None, None

    return _FrontendLayerRuntimeHooks(
        warnings=warnings,
        frontend_parallel_details=details,
        record_frontend_parallel_worker_timing=_record_timing,
        record_frontend_timing=lambda *a, **k: None,
        integrate_module_frontend_result=_integrate,
        accumulate_midend_diagnostics=_accumulate,
        fail=_fail,
        json_output=False,
        run_serial_frontend_lower=_run_serial,
    )
