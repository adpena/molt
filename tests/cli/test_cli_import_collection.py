from __future__ import annotations

import ast
import json
import os
from pathlib import Path
import subprocess
import sys

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
        for name in (
            "builtins",
            "sys",
            "types",
            "importlib",
            "importlib.util",
            "importlib.machinery",
        )
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
            nested_stdlib_scan_modules=set(),
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


def test_collect_imports_resolves_module_constant_via_helper_call() -> None:
    tree = ast.parse(
        "import importlib\n"
        "MODULE_NAME = '_socket'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "_probe(MODULE_NAME)\n"
    )
    imports = cli._collect_imports(tree)
    assert "_socket" in imports


def test_collect_imports_resolves_helper_call_nested_in_expression() -> None:
    tree = ast.parse(
        "import importlib\n"
        "MODULE_NAME = '_socket'\n"
        "def _probe(module_name):\n"
        "    return importlib.import_module(module_name)\n"
        "print(_probe(MODULE_NAME))\n"
    )
    imports = cli._collect_imports(tree)
    assert "_socket" in imports


def test_collect_imports_resolves_name_argument_for_import_module() -> None:
    tree = ast.parse(
        "import importlib\nTARGET = 'pathlib'\nimportlib.import_module(TARGET)\n"
    )
    imports = cli._collect_imports(tree)
    assert "pathlib" in imports


def test_collect_imports_resolves_helper_join_dynamic_module_name() -> None:
    tree = ast.parse(
        "import importlib\n"
        "def _module_name(parts):\n"
        "    return ''.join(parts)\n"
        "def _load(parts):\n"
        "    return importlib.import_module(_module_name(parts))\n"
        "_load(('ma', 'th'))\n"
        "_load(('sy', 's'))\n"
    )
    imports = cli._collect_imports(tree)
    assert "math" in imports
    assert "sys" in imports


def test_stdlib_graph_ignores_nested_imports_for_core_scan(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print(1)\n")
    graph = _discover_with_core_modules(entry)
    assert "builtins" in graph
    assert "sys" in graph
    assert "importlib" in graph
    assert "importlib.util" in graph
    assert "importlib.machinery" in graph
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
        for name in (
            "builtins",
            "sys",
            "types",
            "importlib",
            "importlib.util",
            "importlib.machinery",
        )
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
            nested_stdlib_scan_modules=set(),
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


def test_resolve_build_diagnostics_verbosity_aliases() -> None:
    assert cli._resolve_build_diagnostics_verbosity(None) == "default"
    assert cli._resolve_build_diagnostics_verbosity("brief") == "summary"
    assert cli._resolve_build_diagnostics_verbosity("verbose") == "full"
    assert cli._resolve_build_diagnostics_verbosity("unknown") == "default"


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
                "tier_base": "B",
                "tier_effective": "A",
                "tier_source": "default",
                "promoted": True,
                "promotion_source": "pgo_hot_functions",
                "promotion_signal": "pkg.mod::fn_a",
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
    assert payload["tier_base_summary"] == {"B": 1}
    assert payload["promoted_functions"] == 1
    assert payload["promotion_source_summary"] == {"pgo_hot_functions": 1}
    assert payload["degrade_reason_summary"] == {"budget_exceeded": 1}
    assert payload["policy_config"]["hot_tier_promotion_enabled"] is True
    assert payload["policy_config"]["budget_alpha"] == 0.03
    assert payload["policy_config"]["budget_beta"] == 0.75
    assert payload["policy_config"]["budget_scale"] == 1.0
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
    promotion_hotspots = payload["promotion_hotspots_top"]
    assert promotion_hotspots
    assert promotion_hotspots[0]["module"] == "pkg.mod"
    assert promotion_hotspots[0]["function"] == "fn_a"
    assert promotion_hotspots[0]["tier_base"] == "B"
    assert promotion_hotspots[0]["tier_effective"] == "A"


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
        "pgo_hot_functions": ["worker_module::molt_main"],
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
            "midend": {
                "policy_config": {
                    "profile_override": None,
                    "hot_tier_promotion_enabled": True,
                    "budget_override_ms": None,
                    "budget_alpha": 0.03,
                    "budget_beta": 0.75,
                    "budget_scale": 1.0,
                },
                "promoted_functions": 2,
                "promotion_source_summary": {"pgo_hot_functions": 2},
                "promotion_hotspots_top": [
                    {
                        "module": "pkg.mod",
                        "function": "hot_fn",
                        "tier_base": "B",
                        "tier_effective": "A",
                        "source": "pgo_hot_functions",
                        "signal": "pkg.mod::hot_fn",
                        "spent_ms": 12.5,
                    }
                ],
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
    assert "- midend.policy.hot_tier_promotion_enabled: True" in stderr
    assert "- midend.policy.budget_formula: alpha=0.0300 beta=0.7500 scale=1.0000" in stderr
    assert "- midend.promoted_functions: 2" in stderr
    assert "- midend.promotion_source.pgo_hot_functions: 2" in stderr
    assert "midend.promotion_hotspot.1: pkg.mod::hot_fn B->A" in stderr


def test_midend_policy_config_snapshot_honors_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_MIDEND_PROFILE", "release")
    monkeypatch.setenv("MOLT_MIDEND_HOT_TIER_PROMOTION", "0")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_MS", "42")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_ALPHA", "0.5")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_BETA", "2.0")
    monkeypatch.setenv("MOLT_MIDEND_BUDGET_SCALE", "1.5")

    assert cli._midend_policy_config_snapshot() == {
        "profile_override": "release",
        "hot_tier_promotion_enabled": False,
        "budget_override_ms": 42.0,
        "budget_alpha": 0.5,
        "budget_beta": 2.0,
        "budget_scale": 1.5,
    }


def test_emit_build_diagnostics_summary_omits_hotspot_details(
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
            "midend": {
                "promoted_functions": 2,
                "promotion_source_summary": {"pgo_hot_functions": 2},
                "promotion_hotspots_top": [
                    {
                        "module": "pkg.mod",
                        "function": "hot_fn",
                        "tier_base": "B",
                        "tier_effective": "A",
                        "source": "pgo_hot_functions",
                        "signal": "pkg.mod::hot_fn",
                        "spent_ms": 12.5,
                    }
                ],
            },
        },
        diagnostics_path=None,
        json_output=False,
        verbosity="summary",
    )
    stderr = capsys.readouterr().err
    assert "Build diagnostics:" in stderr
    assert "- frontend_parallel: enabled=True workers=4 mode=process_pool" in stderr
    assert "- midend.promoted_functions: 2" in stderr
    assert "frontend_parallel.layer.1:" not in stderr
    assert "frontend_parallel.worker_ms:" not in stderr
    assert "midend.promotion_hotspot.1:" not in stderr


def test_emit_build_diagnostics_full_prints_extended_hotspots(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cli._emit_build_diagnostics(
        diagnostics={
            "frontend_module_timings_top": [
                {
                    "module": f"pkg.mod_{idx}",
                    "total_s": float(idx),
                    "visit_s": 0.1,
                    "lower_s": 0.2,
                }
                for idx in range(12)
            ]
        },
        diagnostics_path=None,
        json_output=False,
        verbosity="full",
    )
    stderr = capsys.readouterr().err
    assert "frontend.hotspot.10: pkg.mod_9" in stderr
    assert "frontend.hotspot.12: pkg.mod_11" in stderr


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


def test_extract_runtime_feedback_hot_functions_sorts_and_dedupes() -> None:
    warnings: list[str] = []
    payload = {
        "hot_functions": [
            {"symbol": "pkg.mod::warm_fn", "count": 3},
            {"symbol": "pkg.mod::hot_fn", "count": 9},
            "pkg.mod::hot_fn",
            ["pkg.mod::cold_fn", 1],
        ]
    }

    assert cli._extract_runtime_feedback_hot_functions(payload, warnings) == [
        "pkg.mod::hot_fn",
        "pkg.mod::warm_fn",
        "pkg.mod::cold_fn",
    ]
    assert warnings == []


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


def test_backend_daemon_request_payload_bytes_enforces_limit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_MAX_REQUEST_BYTES", "64")
    payload = {"version": 1, "jobs": [{"id": "x", "ir": "x" * 4096}]}
    data, err = cli._backend_daemon_request_payload_bytes(payload)
    assert data is None
    assert isinstance(err, str)
    assert "too large" in err


def test_backend_codegen_env_digest_tracks_codegen_knobs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    baseline_native = cli._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_BACKEND_REGALLOC_ALGORITHM", "single_pass")
    native_changed = cli._backend_codegen_env_digest(is_wasm=False)
    assert native_changed != baseline_native

    baseline_wasm = cli._backend_codegen_env_digest(is_wasm=True)
    monkeypatch.setenv("MOLT_WASM_TABLE_BASE", "2048")
    wasm_changed = cli._backend_codegen_env_digest(is_wasm=True)
    assert wasm_changed != baseline_wasm


def test_backend_daemon_config_digest_and_socket_path_include_config(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SOCKET", raising=False)
    digest_a = cli._backend_daemon_config_digest(tmp_path, "dev-fast")
    monkeypatch.setenv("MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2", "2")
    digest_b = cli._backend_daemon_config_digest(tmp_path, "dev-fast")
    assert digest_a != digest_b

    socket_a = cli._backend_daemon_socket_path(
        tmp_path, "dev-fast", config_digest=digest_a
    )
    socket_b = cli._backend_daemon_socket_path(
        tmp_path, "dev-fast", config_digest=digest_b
    )
    assert socket_a != socket_b


def test_function_cache_key_tracks_top_level_ir_extras() -> None:
    ir_base = {"functions": [{"name": "f", "ops": []}], "profile": None}
    ir_extra_a = {
        "profile": None,
        "functions": [{"name": "f", "ops": []}],
        "meta": {"x": 1},
    }
    ir_extra_b = {
        "functions": [{"name": "f", "ops": []}],
        "meta": {"x": 1},
        "profile": None,
    }
    key_base = cli._function_cache_key(ir_base, "native", None, "variant")
    key_extra_a = cli._function_cache_key(ir_extra_a, "native", None, "variant")
    key_extra_b = cli._function_cache_key(ir_extra_b, "native", None, "variant")
    assert key_extra_a != key_base
    assert key_extra_a == key_extra_b


def test_compile_with_backend_daemon_surfaces_cache_telemetry(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    backend_output = tmp_path / "output.o"
    captured_payload: dict[str, object] = {}

    def _fake_request(
        socket_path: Path,
        payload: dict[str, object],
        *,
        timeout: float | None,
    ) -> tuple[dict[str, object], None]:
        del socket_path, timeout
        captured_payload.update(payload)
        backend_output.write_bytes(b"\x7fELF")
        return (
            {
                "ok": True,
                "jobs": [
                    {
                        "id": "job0",
                        "ok": True,
                        "cached": True,
                        "cache_tier": "function",
                    }
                ],
                "health": {"pid": 42, "cache_hits": 1, "cache_misses": 0},
            },
            None,
        )

    monkeypatch.setattr(cli, "_backend_daemon_request", _fake_request)
    result = cli._compile_with_backend_daemon(
        Path("/tmp/fake.sock"),
        ir={"functions": []},
        backend_output=backend_output,
        is_wasm=False,
        target_triple=None,
        cache_key="module-cache",
        function_cache_key="function-cache",
        config_digest="digest123",
        timeout=0.1,
    )
    assert result.ok is True
    assert result.cached is True
    assert result.cache_tier == "function"
    assert captured_payload.get("config_digest") == "digest123"


def test_cached_backend_artifact_validity_guard(tmp_path: Path) -> None:
    wasm_bad = tmp_path / "bad.wasm"
    wasm_bad.write_bytes(b"not-wasm")
    assert not cli._is_valid_cached_backend_artifact(wasm_bad, is_wasm=True)

    wasm_good = tmp_path / "good.wasm"
    wasm_good.write_bytes(b"\x00asm\x01\x00\x00\x00")
    assert cli._is_valid_cached_backend_artifact(wasm_good, is_wasm=True)

    native_empty = tmp_path / "empty.o"
    native_empty.write_bytes(b"")
    assert not cli._is_valid_cached_backend_artifact(native_empty, is_wasm=False)

    native_nonempty = tmp_path / "nonempty.o"
    native_nonempty.write_bytes(b"\x01")
    assert cli._is_valid_cached_backend_artifact(native_nonempty, is_wasm=False)


def test_backend_daemon_health_from_response_parses_int_fields() -> None:
    response = {
        "ok": True,
        "pong": True,
        "health": {
            "pid": 123,
            "uptime_ms": 456,
            "cache_entries": 2,
            "cache_bytes": 100,
            "cache_max_bytes": 200,
            "request_limit_bytes": 1024,
            "max_jobs": 8,
            "requests_total": 7,
            "jobs_total": 9,
            "cache_hits": 4,
            "cache_misses": 5,
        },
    }
    health = cli._backend_daemon_health_from_response(response)
    assert isinstance(health, dict)
    assert health["pid"] == 123
    assert health["max_jobs"] == 8


def test_backend_daemon_ping_health_backcompat_without_health(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        cli,
        "_backend_daemon_request",
        lambda socket_path, payload, timeout: ({"ok": True, "pong": True}, None),
    )
    ready, health = cli._backend_daemon_ping_health(Path("/tmp/fake.sock"), timeout=0.1)
    assert ready is True
    assert health is None


def test_internal_batch_build_server_ping_shutdown_roundtrip() -> None:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    proc = subprocess.Popen(
        [sys.executable, "-m", "molt.cli", "internal-batch-build-server"],
        cwd=str(ROOT),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert proc.stdin is not None
    assert proc.stdout is not None
    proc.stdin.write(json.dumps({"id": 1, "op": "ping"}) + "\n")
    proc.stdin.flush()
    ping_response = json.loads(proc.stdout.readline())
    assert ping_response["ok"] is True
    assert ping_response["pong"] is True
    proc.stdin.write(json.dumps({"id": 2, "op": "shutdown"}) + "\n")
    proc.stdin.flush()
    shutdown_response = json.loads(proc.stdout.readline())
    assert shutdown_response["ok"] is True
    assert shutdown_response["shutdown"] is True
    proc.wait(timeout=5)
    assert proc.returncode == 0
