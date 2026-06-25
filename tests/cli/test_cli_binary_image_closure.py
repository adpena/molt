from __future__ import annotations

import ast
import time
from pathlib import Path

import pytest

from molt.cli import build_inputs as cli_build_inputs
from molt.cli import binary_image_analysis as cli_binary_image_analysis
from molt.cli import frontend_pipeline as cli_frontend_pipeline
from molt.cli import module_graph as cli_module_graph
from molt.cli import module_resolution as cli_module_resolution
from molt.cli import module_dependencies as cli_module_dependencies
from molt.cli import wrapper_build as cli_wrapper_build
from molt.cli.config_resolution import STATIC_IMPORT_MODULES_ENV
from molt.cli.target_python import _DEFAULT_TARGET_PYTHON_VERSION


def _resolve_entry(
    project_root: Path,
    *,
    file_path: str | None = None,
    module: str | None = None,
    build_config: dict[str, object] | None = None,
):
    return cli_build_inputs._resolve_build_entry(
        file_path=file_path,
        module=module,
        project_root=project_root,
        cwd_root=project_root,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        respect_pythonpath=False,
        json_output=False,
        build_config=build_config,
    )


def _materialize_plan(
    project_root: Path,
    entry_path: Path,
    entry_module: str,
    *,
    image_scope=None,
):
    entry_tree = ast.parse(entry_path.read_text(), filename=str(entry_path))
    module_reasons: dict[str, set[str]] = {}
    prepared, error = cli_module_graph._prepare_entry_module_graph(
        source_path=entry_path,
        entry_module=entry_module,
        module_roots=[project_root],
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        project_root=project_root,
        entry_tree=entry_tree,
        diagnostics_enabled=False,
        module_reasons=module_reasons,
        json_output=False,
        target="native",
        image_scope=image_scope,
    )
    assert error is None
    assert prepared is not None
    return cli_module_graph._materialize_import_plan(
        prepared_module_graph=prepared,
        module_reasons=module_reasons,
        stdlib_root=cli_module_resolution._stdlib_root_path(),
        artifacts_root=project_root / "tmp" / "closure-test",
        entry_module=entry_module,
        diagnostics_enabled=False,
    )


def _prepare_analysis_for_plan(project_root: Path, import_plan):
    analysis, error = cli_frontend_pipeline._prepare_frontend_analysis(
        module_graph=import_plan.module_graph,
        module_graph_metadata=import_plan.module_graph_metadata,
        module_resolution_cache=import_plan.module_resolution_cache,
        roots=list(import_plan.roots),
        stdlib_root=import_plan.stdlib_root,
        stdlib_allowlist=set(import_plan.stdlib_allowlist),
        project_root=project_root,
        entry_module=import_plan.image_scope.entry_module,
        json_output=False,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert error is None
    assert analysis is not None
    return analysis


def test_project_config_entry_file_defines_binary_image_scope(tmp_path: Path) -> None:
    entry = tmp_path / "app.py"
    entry.write_text("value = 1\n")

    resolved, error = _resolve_entry(
        tmp_path,
        build_config={"entry-file": "app.py"},
    )

    assert error is None
    assert resolved is not None
    assert resolved.entry_module == "app"
    assert resolved.image_scope is not None
    assert resolved.image_scope.kind == "project_entry_script"
    assert resolved.image_scope.selector_source == "config:entry-file"
    assert resolved.image_scope.diagnostic_payload()["root_modules"] == ["app"]


def test_project_config_entry_module_package_defines_binary_image_scope(
    tmp_path: Path,
) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    (package / "__init__.py").write_text("value = 1\n")
    (package / "__main__.py").write_text("from . import value\n")

    resolved, error = _resolve_entry(
        tmp_path,
        build_config={"entry-module": "pkg"},
    )

    assert error is None
    assert resolved is not None
    assert resolved.entry_module == "pkg.__main__"
    assert resolved.image_scope is not None
    assert resolved.image_scope.kind == "project_entry_package"
    assert resolved.image_scope.selector_source == "config:entry-module"


def test_cli_entry_overrides_project_config_entry(tmp_path: Path) -> None:
    config_entry = tmp_path / "configured.py"
    cli_entry = tmp_path / "chosen.py"
    config_entry.write_text("value = 'config'\n")
    cli_entry.write_text("value = 'cli'\n")

    resolved, error = _resolve_entry(
        tmp_path,
        file_path=str(cli_entry),
        build_config={"entry-file": "configured.py"},
    )

    assert error is None
    assert resolved is not None
    assert resolved.entry_module == "chosen"
    assert resolved.image_scope is not None
    assert resolved.image_scope.kind == "entry_script"
    assert resolved.image_scope.selector_source == "cli:file"


def test_project_config_rejects_ambiguous_entry_selectors(tmp_path: Path) -> None:
    (tmp_path / "app.py").write_text("value = 1\n")
    selector, selector_error = cli_build_inputs._resolve_build_entry_selector(
        file_path=None,
        module=None,
        project_root=tmp_path,
        build_config={"entry-file": "app.py", "entry-module": "pkg"},
    )

    resolved, error = _resolve_entry(
        tmp_path,
        build_config={"entry-file": "app.py", "entry-module": "pkg"},
    )

    assert selector is None
    assert selector_error is not None
    assert "multiple entry selectors" in selector_error
    assert resolved is None
    assert error is not None


def test_import_plan_classifies_binary_image_closure(tmp_path: Path) -> None:
    entry = tmp_path / "app.py"
    helper = tmp_path / "helper.py"
    entry.write_text("import helper\nvalue = helper.VALUE\n")
    helper.write_text("VALUE = 7\n")

    import_plan = _materialize_plan(tmp_path, entry, "app")
    payload = import_plan.closure_payload()

    assert "app" in import_plan.declared_root_modules
    assert "helper" not in import_plan.declared_root_modules
    assert {"app", "helper"}.issubset(import_plan.entry_reachable_modules)
    assert import_plan.compile_modules == import_plan.known_modules
    assert payload["image"]["entry_module"] == "app"
    assert {"app", "helper"}.issubset(payload["known_modules"])
    assert {"app", "helper"}.issubset(payload["compile_modules"])
    assert "helper" not in payload["declared_root_modules"]
    narrowed_plan = import_plan.with_compile_modules({"app"})
    assert narrowed_plan.compile_modules == frozenset({"app"})
    assert narrowed_plan.known_modules == import_plan.known_modules
    assert narrowed_plan.module_graph == import_plan.module_graph
    with pytest.raises(ValueError, match="outside the closure plan"):
        import_plan.with_compile_modules({"outside"})


def test_project_config_static_import_dme_keeps_compile_scope_in_image_closure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "app.py"
    helper = tmp_path / "helper.py"
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    entry.write_text("import helper\nprint('APP', helper.VALUE)\n")
    helper.write_text("VALUE = 42\n")
    (package / "__init__.py").write_text("value = 'pkg'\n")
    (runtime / "__init__.py").write_text("value = 'runtime'\n")
    (runtime / "ops_cpu.py").write_text("import base64\nENCODE = base64.b64encode\n")
    (tmp_path / "unreferenced.py").write_text("VALUE = 'dead'\n")
    monkeypatch.setenv(STATIC_IMPORT_MODULES_ENV, "pkg.runtime.ops_cpu")

    resolved, error = _resolve_entry(
        tmp_path,
        build_config={"entry-file": "app.py"},
    )
    assert error is None
    assert resolved is not None
    assert resolved.image_scope is not None

    import_plan = _materialize_plan(
        tmp_path,
        entry,
        resolved.entry_module,
        image_scope=resolved.image_scope,
    )
    analysis = _prepare_analysis_for_plan(tmp_path, import_plan)
    dme_roots = (
        import_plan.runtime_import_dispatch_roots
        | import_plan.declared_root_modules
        | import_plan.runtime_support_modules
        | import_plan.stdlib_support_modules
        | import_plan.package_parent_modules
        | import_plan.namespace_module_names
    )
    compile_order, _compile_layers, _eliminated = (
        cli_module_dependencies._apply_dead_module_elimination(
            list(analysis.module_order),
            [list(layer) for layer in analysis.module_layers],
            entry_module=resolved.entry_module,
            module_deps=analysis.module_deps,
            module_names=set(import_plan.module_graph),
            extra_roots=dme_roots,
        )
    )
    narrowed_plan = import_plan.with_compile_modules(compile_order)

    assert narrowed_plan.image_scope.kind == "project_entry_script"
    assert narrowed_plan.image_scope.selector_source == "config:entry-file"
    assert narrowed_plan.image_scope.diagnostic_payload()["root_modules"] == [
        "app",
        "pkg.runtime.ops_cpu",
    ]
    assert {"app", "helper", "pkg.runtime.ops_cpu", "base64"}.issubset(
        narrowed_plan.compile_modules
    )
    assert "pkg.runtime.ops_cpu" in narrowed_plan.declared_root_modules
    assert "unreferenced" not in narrowed_plan.known_modules
    assert narrowed_plan.compile_modules <= narrowed_plan.known_modules


def test_wrapper_build_cache_input_uses_static_import_closure_plan(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "app.py"
    package = tmp_path / "pkg"
    runtime = package / "runtime"
    runtime.mkdir(parents=True)
    entry.write_text("value = 'entry'\n")
    (package / "__init__.py").write_text("value = 'pkg'\n")
    (runtime / "__init__.py").write_text("value = 'runtime'\n")
    (runtime / "ops_cpu.py").write_text("import base64\nVALUE = base64.b64encode\n")
    resolved, error = _resolve_entry(tmp_path, file_path=str(entry))
    assert error is None
    assert resolved is not None

    cache_input = cli_wrapper_build._wrapper_build_cache_input(
        resolved_build_entry=resolved,
        build_args=["--target", "native"],
        env={STATIC_IMPORT_MODULES_ENV: "pkg.runtime.ops_cpu"},
        project_root=tmp_path,
    )

    assert cache_input is not None
    payload, _cache_key = cache_input
    modules = {
        item["module"]
        for item in payload["module_sources"]
        if item["kind"] == "python_source"
    }
    assert {"app", "pkg.runtime.ops_cpu", "base64"}.issubset(modules)
    assert payload["binary_image"]["entry_module"] == "app"
    closure = payload["binary_image_closure"]
    assert closure["image"] == payload["binary_image"]
    assert closure["image"]["root_modules"] == ["app", "pkg.runtime.ops_cpu"]
    assert {"app", "pkg.runtime.ops_cpu", "base64"}.issubset(closure["known_modules"])
    assert closure["compile_modules"] == closure["known_modules"]
    assert "pkg.runtime.ops_cpu" in closure["declared_root_modules"]


def test_wrapper_build_cache_identity_tracks_dead_module_elimination(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "app.py"
    entry.write_text("value = 'entry'\n")
    resolved, error = _resolve_entry(tmp_path, file_path=str(entry))
    assert error is None
    assert resolved is not None

    base_payload, base_key = cli_wrapper_build._wrapper_build_cache_input(
        resolved_build_entry=resolved,
        build_args=["--target", "native"],
        env={},
        project_root=tmp_path,
    )
    dme_payload, dme_key = cli_wrapper_build._wrapper_build_cache_input(
        resolved_build_entry=resolved,
        build_args=["--target", "native"],
        env={"MOLT_DEAD_MODULE_ELIMINATION": "1"},
        project_root=tmp_path,
    )

    assert base_payload is not None
    assert dme_payload is not None
    assert base_key != dme_key
    assert "MOLT_DEAD_MODULE_ELIMINATION" not in base_payload["semantic_env"]
    assert dme_payload["semantic_env"]["MOLT_DEAD_MODULE_ELIMINATION"] == "1"
    assert (
        dme_payload["binary_image_closure"]["known_modules"]
        == base_payload["binary_image_closure"]["known_modules"]
    )


def test_build_diagnostics_emits_final_binary_image_closure(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "app.py"
    helper = tmp_path / "helper.py"
    entry.write_text("import helper\nvalue = helper.VALUE\n")
    helper.write_text("VALUE = 7\n")
    import_plan = _materialize_plan(tmp_path, entry, "app")
    narrowed_plan = import_plan.with_compile_modules({"app"})
    callbacks = cli_frontend_pipeline._prepare_build_callbacks(
        frontend_module_timings=[],
        frontend_timing_enabled=False,
        frontend_timing_raw="",
        frontend_timing_threshold=0.0,
        json_output=False,
        diagnostics_enabled=True,
        diagnostics_start=time.perf_counter(),
        phase_starts={},
        module_graph=import_plan.module_graph,
        module_reasons={"app": {"entry_root"}, "helper": {"entry_closure"}},
        allocation_diagnostics_enabled=False,
        frontend_parallel_details={},
        profile="dev",
        midend_policy_outcomes_by_function={},
        midend_pass_stats_by_function={},
        backend_daemon_health=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        diagnostics_path_spec="",
        artifacts_root=tmp_path / "tmp" / "diagnostics",
        image_scope=import_plan.image_scope,
    )
    callbacks.set_binary_image_closure_payload(narrowed_plan.closure_payload())
    callbacks.record_binary_image_analysis(
        "frontend",
        {"source_ast": {"compile_module_count": 1}},
    )

    payload, diagnostics_path = callbacks.build_diagnostics_payload()

    assert diagnostics_path is None
    assert payload is not None
    assert payload["binary_image"] == payload["binary_image_closure"]["image"]
    assert payload["binary_image"]["root_modules"] == ["app"]
    assert payload["known_module_count"] == len(import_plan.known_modules)
    assert payload["compile_module_count"] == 1
    assert payload["binary_image_closure"]["known_modules"] == sorted(
        import_plan.known_modules
    )
    assert payload["binary_image_closure"]["compile_modules"] == ["app"]
    assert (
        payload["binary_image_analysis"]["frontend"]["source_ast"][
            "compile_module_count"
        ]
        == 1
    )


def test_frontend_binary_image_analysis_bridges_ast_schedule_and_lowering(
    tmp_path: Path,
) -> None:
    entry = tmp_path / "app.py"
    helper = tmp_path / "helper.py"
    entry.write_text("import helper\n\ndef run():\n    return helper.VALUE\n")
    helper.write_text("VALUE = 7\n")
    import_plan = _materialize_plan(tmp_path, entry, "app")
    analysis = _prepare_analysis_for_plan(tmp_path, import_plan)
    narrowed_plan = import_plan.with_compile_modules({"app"})

    payload = cli_binary_image_analysis._frontend_binary_image_analysis_payload(
        import_plan=narrowed_plan,
        frontend_analysis=analysis,
        frontend_module_costs={"app": 3.0, "helper": 1.0},
        known_classes={},
        enable_phi=True,
        module_chunking=False,
        module_chunk_max_ops=100,
        type_facts_present=False,
        compile_module_order=["app"],
        compile_module_layers=[["app"]],
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert payload["source_ast"]["known_module_count"] == len(import_plan.known_modules)
    assert payload["source_ast"]["compile_module_count"] == 1
    assert payload["source_ast"]["compile"]["function_defs"] == 1
    assert payload["source_ast"]["compile"]["import_statements"] == 1
    assert payload["source_identity"]["semantic_identity_digest"]
    assert payload["source_identity"]["module_count"] == len(import_plan.known_modules)
    assert payload["source_identity"]["compile_module_count"] == 1
    app_identity = next(
        module
        for module in payload["source_identity"]["modules"]
        if module["module"] == "app"
    )
    assert {"compile", "declared_root", "known"}.issubset(app_identity["roles"])
    assert app_identity["site_count"] >= 3
    assert app_identity["source_sha256"]
    assert app_identity["site_digest"]
    assert payload["module_schedule"]["compile_order_len"] == 1
    assert payload["module_schedule"]["dependency_edge_count"] >= 1
    assert payload["lowering"]["target_python"] == _DEFAULT_TARGET_PYTHON_VERSION.tag
    assert payload["lowering"]["compile_equals_known"] is False


def test_backend_ir_and_artifact_analysis_attach_to_same_contract(
    tmp_path: Path,
) -> None:
    ir_payload = cli_binary_image_analysis._backend_ir_binary_image_analysis_payload(
        {
            "functions": [
                {
                    "name": "app__run",
                    "source_file": "app.py",
                    "ops": [
                        {
                            "kind": "const",
                            "source_line": 3,
                            "col_offset": 11,
                            "end_col_offset": 12,
                        },
                        {
                            "kind": "call",
                            "s_value": "helper__value",
                            "source_line": 4,
                        },
                        {
                            "kind": "object_new_bound",
                            "source_line": 5,
                            "arena_eligible": True,
                            "defines_del": True,
                        },
                        {"kind": "stack_alloc", "source_line": 5},
                        {"kind": "borrow", "source_line": 6},
                        {"kind": "release", "source_line": 7},
                    ],
                }
            ]
        }
    )
    binary = tmp_path / "app_molt"
    obj = tmp_path / "app.o"
    runtime = tmp_path / "libmolt_runtime.a"
    binary.write_bytes(b"binary")
    obj.write_bytes(b"object")
    runtime.write_bytes(b"runtime")
    artifact_payload = (
        cli_binary_image_analysis._native_artifact_binary_image_analysis_payload(
            output_binary=binary,
            output_obj=obj,
            runtime_lib=runtime,
            stdlib_obj_path=None,
            link_skipped=False,
            link_fingerprint={"hash": "abc"},
            link_fingerprint_path=tmp_path / "link.json",
            external_native_artifact_count=0,
        )
    )
    diagnostics: dict[str, object] = {}
    cli_binary_image_analysis._merge_binary_image_analysis_stage(
        diagnostics, "backend_ir", ir_payload
    )
    cli_binary_image_analysis._merge_binary_image_analysis_stage(
        diagnostics, "artifacts", artifact_payload
    )

    analysis_payload = diagnostics["binary_image_analysis"]
    assert analysis_payload["backend_ir"]["backend_ir"]["function_count"] == 1
    assert analysis_payload["backend_ir"]["backend_ir"]["op_count"] == 6
    assert analysis_payload["backend_ir"]["source_sites"]["attributed_op_count"] == 6
    assert analysis_payload["backend_ir"]["source_sites"]["coverage_ratio"] == 1.0
    assert analysis_payload["backend_ir"]["source_sites"]["source_site_digest"]
    assert (
        analysis_payload["backend_ir"]["source_sites"]["top_source_lines_by_ops"][0][
            "source_file"
        ]
        == "app.py"
    )
    allocation = analysis_payload["backend_ir"]["allocation_ownership"]
    assert allocation["source_coverage_ratio"] == 1.0
    assert allocation["events_by_category"]["heap_alloc_root"] == 1
    assert allocation["events_by_category"]["stack_alloc_root"] == 1
    assert allocation["events_by_category"]["ref_retain"] == 1
    assert allocation["events_by_category"]["ref_release"] == 1
    assert allocation["events_by_category"]["heap_exposure"] == 1
    assert allocation["events_by_category"]["arena_eligible"] == 1
    assert allocation["events_by_category"]["finalizer_sensitive"] == 1
    assert allocation["allocation_ownership_digest"]
    assert allocation["top_source_lines_by_events"][0]["source_file"] == "app.py"
    assert analysis_payload["backend_ir"]["tir_boundary"]["carrier"] == (
        "backend_ir_json"
    )
    assert analysis_payload["artifacts"]["output_binary"]["size_bytes"] == 6
    assert analysis_payload["artifacts"]["link"]["fingerprint_hash"] == "abc"
