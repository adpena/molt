from __future__ import annotations

import ast
from pathlib import Path

import molt.cli as cli
import pytest


ROOT = Path(__file__).resolve().parents[2]


def test_resolve_module_path_prefers_package_over_module(tmp_path: Path) -> None:
    module = tmp_path / "shadowed.py"
    module.write_text("value = 'module'\n")
    package_dir = tmp_path / "shadowed"
    package_dir.mkdir()
    package_init = package_dir / "__init__.py"
    package_init.write_text("value = 'package'\n")
    assert cli._resolve_module_path("shadowed", [tmp_path]) == package_init


def test_stdlib_test_support_layout_resolves_like_cpython() -> None:
    stdlib_root = cli._stdlib_root_path()
    support_pkg = cli._resolve_module_path("test.support", [stdlib_root])
    import_helper = cli._resolve_module_path(
        "test.support.import_helper", [stdlib_root]
    )
    os_helper = cli._resolve_module_path("test.support.os_helper", [stdlib_root])
    warnings_helper = cli._resolve_module_path(
        "test.support.warnings_helper", [stdlib_root]
    )

    assert support_pkg == stdlib_root / "test" / "support" / "__init__.py"
    assert import_helper == stdlib_root / "test" / "support" / "import_helper.py"
    assert os_helper == stdlib_root / "test" / "support" / "os_helper.py"
    assert warnings_helper == stdlib_root / "test" / "support" / "warnings_helper.py"


def _discover_with_core_modules(entry: Path) -> dict[str, Path]:
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    cli._collect_package_parents(module_graph, roots, stdlib_root, stdlib_allowlist)
    cli._ensure_core_stdlib_modules(module_graph, stdlib_root)
    core_paths = [
        path
        for name in ("builtins", "sys")
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = cli._discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    return module_graph


def test_collect_imports_can_skip_nested_imports() -> None:
    tree = ast.parse(
        "import os\ndef f() -> None:\n    import warnings\nclass C:\n    import re\n"
    )
    nested = cli._collect_imports(tree)
    top_level_only = cli._collect_imports(tree, include_nested=False)
    assert "warnings" in nested
    assert "re" in nested
    assert "warnings" not in top_level_only
    assert "re" not in top_level_only
    assert "os" in top_level_only


def test_stdlib_graph_ignores_nested_imports_for_core_scan(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print(1)\n")
    graph = _discover_with_core_modules(entry)
    assert "builtins" in graph
    assert "sys" in graph
    assert "warnings" not in graph
    assert "re" not in graph
    assert "dataclasses" not in graph


def test_typing_enables_nested_import_scan_for_collections_abc(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import typing\n")
    graph = _discover_with_core_modules(entry)
    assert "typing" in graph
    assert "_collections_abc" in graph


def test_spawn_entry_override_not_required_for_plain_script(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    cli._collect_package_parents(module_graph, roots, stdlib_root, stdlib_allowlist)
    cli._ensure_core_stdlib_modules(module_graph, stdlib_root)
    core_paths = [
        path
        for name in ("builtins", "sys")
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = cli._discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    assert not cli._requires_spawn_entry_override(module_graph, explicit_imports)


def test_spawn_entry_override_required_for_multiprocessing(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("import multiprocessing\nprint('ok')\n")
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, explicit_imports = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    assert "multiprocessing" in module_graph
    assert cli._requires_spawn_entry_override(module_graph, explicit_imports)


def test_spawn_entry_override_required_for_spawn_import() -> None:
    graph = {"__main__": ROOT / "script.py"}
    explicit_imports = {"multiprocessing.spawn"}
    assert cli._requires_spawn_entry_override(graph, explicit_imports)


def test_merge_module_graph_with_reason_tracks_sources(tmp_path: Path) -> None:
    module_graph = {"__main__": tmp_path / "main.py"}
    reasons: dict[str, set[str]] = {}
    additions = {
        "__main__": tmp_path / "main.py",
        "multiprocessing.spawn": tmp_path / "spawn.py",
    }
    cli._merge_module_graph_with_reason(
        module_graph,
        additions,
        reasons,
        "spawn_closure",
    )
    assert "multiprocessing.spawn" in module_graph
    assert reasons["__main__"] == {"spawn_closure"}
    assert reasons["multiprocessing.spawn"] == {"spawn_closure"}


def test_build_reason_summary_is_stable() -> None:
    reasons = {
        "a": {"entry_closure"},
        "b": {"entry_closure", "core_closure"},
        "c": {"core_closure"},
    }
    summary = cli._build_reason_summary(reasons)
    assert summary == {"core_closure": 2, "entry_closure": 2}


def test_build_diagnostics_enabled_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BUILD_DIAGNOSTICS", "1")
    assert cli._build_diagnostics_enabled()
    monkeypatch.setenv("MOLT_BUILD_DIAGNOSTICS", "0")
    assert not cli._build_diagnostics_enabled()


def test_phase_duration_map_orders_by_start(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli.time, "perf_counter", lambda: 10.0)
    durations = cli._phase_duration_map({"module_graph": 2.0, "resolve_entry": 1.0})
    assert durations["resolve_entry"] == 1.0
    assert durations["module_graph"] == 8.0


def test_resolve_build_diagnostics_path_relative_and_absolute(tmp_path: Path) -> None:
    rel = cli._resolve_build_diagnostics_path("diag.json", tmp_path)
    assert rel == tmp_path / "diag.json"
    abs_path = tmp_path / "absolute_diag.json"
    resolved_abs = cli._resolve_build_diagnostics_path(str(abs_path), tmp_path)
    assert resolved_abs == abs_path


def test_build_midend_diagnostics_payload_summarizes_policy_and_passes() -> None:
    payload = cli._build_midend_diagnostics_payload(
        requested_profile="release",
        policy_outcomes_by_function={
            "pkg.mod::fn_a": {
                "profile": "release",
                "tier": "A",
                "budget_ms": 120.0,
                "spent_ms": 140.0,
                "degraded": True,
                "degrade_events": [
                    {
                        "reason": "budget_exceeded",
                        "stage": "round_2_post_dce",
                        "action": "disable_cse",
                        "spent_ms": 140.0,
                    }
                ],
            }
        },
        pass_stats_by_function={
            "pkg.mod::fn_a": {
                "sccp_edge_thread": {
                    "attempted": 2,
                    "accepted": 1,
                    "rejected": 1,
                    "degraded": 0,
                    "ms_total": 9.5,
                    "ms_max": 6.0,
                    "samples_ms": [3.5, 6.0],
                },
                "cse": {
                    "attempted": 1,
                    "accepted": 0,
                    "rejected": 1,
                    "degraded": 1,
                    "ms_total": 4.25,
                    "ms_max": 4.25,
                    "samples_ms": [4.25],
                },
            }
        },
    )
    assert payload is not None
    assert payload["requested_profile"] == "release"
    assert payload["degraded_functions"] == 1
    assert payload["tier_summary"] == {"A": 1}
    assert payload["degrade_reason_summary"] == {"budget_exceeded": 1}
    assert payload["function_count"] == 1
    hotspots = payload["pass_hotspots_top"]
    assert hotspots
    assert hotspots[0]["module"] == "pkg.mod"
    assert hotspots[0]["function"] == "fn_a"
    assert hotspots[0]["pass"] == "sccp_edge_thread"
    fn_hotspots = payload["function_hotspots_top"]
    assert fn_hotspots
    assert fn_hotspots[0]["module"] == "pkg.mod"
    assert fn_hotspots[0]["function"] == "fn_a"
    degrade_hotspots = payload["degrade_event_hotspots_top"]
    assert degrade_hotspots
    assert degrade_hotspots[0]["reason"] == "budget_exceeded"


def test_resolve_frontend_parallel_module_workers_from_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_FRONTEND_PARALLEL_MODULES", raising=False)
    assert cli._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "0")
    assert cli._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "3")
    assert cli._resolve_frontend_parallel_module_workers() == 3

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "auto")
    assert cli._resolve_frontend_parallel_module_workers() >= 2


def test_module_dependency_layers_preserve_topological_determinism() -> None:
    order = ["a", "b", "c", "d", "e"]
    deps = {
        "a": set(),
        "b": {"a"},
        "c": {"a"},
        "d": {"b", "c"},
        "e": {"b"},
    }
    layers = cli._module_dependency_layers(order, deps)
    assert layers == [["a"], ["b", "c"], ["d", "e"]]


def test_choose_frontend_parallel_layer_workers_applies_policy_gates() -> None:
    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b"],
        module_sources={"a": "x=1\n", "b": "y=2\n"},
        module_deps={"a": set(), "b": set()},
        max_workers=8,
        min_modules=3,
        min_predicted_cost=1.0,
        target_cost_per_worker=10.0,
    )
    assert decision["enabled"] is False
    assert decision["reason"] == "layer_module_count_below_min"

    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b", "c"],
        module_sources={"a": "x=1\n", "b": "y=2\n", "c": "z=3\n"},
        module_deps={"a": set(), "b": set(), "c": set()},
        max_workers=8,
        min_modules=2,
        min_predicted_cost=100_000.0,
        target_cost_per_worker=10.0,
    )
    assert decision["enabled"] is False
    assert decision["reason"] == "layer_predicted_cost_below_min"


def test_choose_frontend_parallel_layer_workers_scales_workers_by_cost() -> None:
    decision = cli._choose_frontend_parallel_layer_workers(
        candidates=["a", "b", "c", "d"],
        module_sources={
            "a": "x" * 40_000,
            "b": "y" * 40_000,
            "c": "z" * 40_000,
            "d": "w" * 40_000,
        },
        module_deps={"a": {"x"}, "b": {"x"}, "c": {"x"}, "d": {"x"}},
        max_workers=6,
        min_modules=2,
        min_predicted_cost=1.0,
        target_cost_per_worker=50_000.0,
    )
    assert decision["enabled"] is True
    assert int(decision["workers"]) == 4


def test_module_order_has_back_edges_detects_cycles() -> None:
    order = ["a", "b"]
    assert cli._module_order_has_back_edges(order, {"a": {"b"}, "b": {"a"}})
    assert not cli._module_order_has_back_edges(order, {"a": set(), "b": {"a"}})


def test_frontend_lower_module_worker_smoke(tmp_path: Path) -> None:
    module_path = tmp_path / "worker_module.py"
    payload = {
        "module_name": "worker_module",
        "module_path": str(module_path),
        "source": "x = 1\ny = x + 2\n",
        "parse_codec": "msgpack",
        "type_hint_policy": "ignore",
        "fallback_policy": "error",
        "module_is_namespace": False,
        "entry_module": None,
        "enable_phi": True,
        "known_modules": ["worker_module"],
        "known_classes": {},
        "stdlib_allowlist": [],
        "known_func_defaults": {},
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
    }
    result = cli._frontend_lower_module_worker(payload)
    assert result["ok"] is True
    assert isinstance(result["functions"], list)
    assert isinstance(result["func_code_ids"], dict)
    assert isinstance(result["timings"]["total_s"], float)
    worker = result["worker"]
    assert isinstance(worker["pid"], int)
    assert worker["started_ns"] > 0
    assert worker["finished_ns"] >= worker["started_ns"]


def test_duration_ms_from_ns_clamps_and_converts() -> None:
    assert cli._duration_ms_from_ns(1_000_000, 2_500_000) == 1.5
    assert cli._duration_ms_from_ns(5, 4) == 0.0
    assert cli._duration_ms_from_ns("bad", 10) == 0.0


def test_emit_build_diagnostics_includes_frontend_parallel_layer_counters(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli._emit_build_diagnostics(
        diagnostics={
            "total_sec": 1.25,
            "frontend_parallel": {
                "enabled": True,
                "workers": 4,
                "mode": "process_pool",
                "reason": "enabled",
                "policy": {
                    "min_modules": 2,
                    "min_predicted_cost": 32768.0,
                    "target_cost_per_worker": 65536.0,
                },
                "layers": [
                    {
                        "index": 0,
                        "mode": "parallel",
                        "policy_reason": "enabled",
                        "module_count": 3,
                        "candidate_count": 3,
                        "workers": 3,
                        "queue_ms_total": 4.5,
                        "wait_ms_total": 2.0,
                        "exec_ms_total": 9.0,
                    }
                ],
                "worker_summary": {
                    "count": 3,
                    "queue_ms_total": 4.5,
                    "queue_ms_max": 2.5,
                    "wait_ms_total": 2.0,
                    "wait_ms_max": 1.0,
                    "exec_ms_total": 9.0,
                    "exec_ms_max": 4.0,
                },
            },
        },
        diagnostics_path=None,
        json_output=False,
    )
    stderr = capsys.readouterr().err
    assert "frontend_parallel.policy: min_modules=2" in stderr
    assert "- frontend_parallel.layers: 1" in stderr
    assert "frontend_parallel.layer.1: mode=parallel" in stderr
    assert "frontend_parallel.worker_ms: count=3" in stderr


def test_module_name_from_path_outside_module_roots_uses_stem(tmp_path: Path) -> None:
    script = tmp_path / "outside_script.py"
    script.write_text("print('ok')\n")
    stdlib_root = cli._stdlib_root_path()
    roots = [ROOT.resolve(), (ROOT / "src").resolve()]
    assert cli._module_name_from_path(script, roots, stdlib_root) == "outside_script"


def test_expand_module_chain_ignores_invalid_module_names() -> None:
    assert cli._expand_module_chain("pkg.sub") == ["pkg", "pkg.sub"]
    assert cli._expand_module_chain("") == []
    assert cli._expand_module_chain("/.Volumes.bad.mod") == []


def test_resolve_backend_profile_defaults_to_selected_build_profile(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "")
    profile, error = cli._resolve_backend_profile("dev")
    assert profile == "dev"
    assert error is None


def test_resolve_backend_profile_env_override_and_validation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "release")
    profile, error = cli._resolve_backend_profile("dev")
    assert profile == "release"
    assert error is None

    monkeypatch.setenv("MOLT_BACKEND_PROFILE", "invalid")
    profile, error = cli._resolve_backend_profile("dev")
    assert profile == "dev"
    assert error == "Invalid MOLT_BACKEND_PROFILE value: invalid"


def test_resolve_cargo_profile_name_defaults_and_validation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_DEV_CARGO_PROFILE", raising=False)
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev-fast"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "my-dev_1")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "my-dev_1"
    assert error is None

    monkeypatch.setenv("MOLT_DEV_CARGO_PROFILE", "bad profile")
    profile, error = cli._resolve_cargo_profile_name("dev")
    assert profile == "dev"
    assert error == "Invalid MOLT_DEV_CARGO_PROFILE value: bad profile"


def test_backend_daemon_retryable_error_classification() -> None:
    assert cli._backend_daemon_retryable_error("backend daemon returned empty response")
    assert cli._backend_daemon_retryable_error("unsupported protocol version 9")
    assert cli._backend_daemon_retryable_error(
        "backend daemon connection failed: timeout"
    )
    assert not cli._backend_daemon_retryable_error(
        "backend daemon failed to compile job"
    )
