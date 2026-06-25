from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
from types import ModuleType


REPO_ROOT = Path(__file__).resolve().parents[2]
CAPSULE_PATH = REPO_ROOT / "tools" / "analysis_capsule.py"


def _load_capsule() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_tools_analysis_capsule",
        CAPSULE_PATH,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _build_diagnostics() -> dict[str, object]:
    return {
        "enabled": True,
        "total_sec": 1.25,
        "phase_sec": {"parse": 0.1, "lower": 0.2},
        "module_count": 3,
        "known_module_count": 3,
        "compile_module_count": 2,
        "module_reason_summary": {"entry_root": 1, "entry_closure": 1},
        "frontend_module_timings": [
            {
                "module": "app",
                "path": "app.py",
                "visit_s": 0.01,
                "lower_s": 0.02,
                "total_s": 0.03,
            }
        ],
        "frontend_module_timings_top": [
            {
                "module": "app",
                "path": "app.py",
                "visit_s": 0.01,
                "lower_s": 0.02,
                "total_s": 0.03,
            }
        ],
        "binary_image_closure": {
            "image": {
                "kind": "entry_script",
                "selector_source": "cli:file",
                "entry_module": "app",
                "root_modules": ["app"],
                "closure_mode": "reachable_only",
            },
            "known_modules": ["app", "helper", "base64"],
            "compile_modules": ["app", "helper"],
            "declared_root_modules": ["app"],
            "entry_reachable_modules": ["app", "helper"],
            "runtime_support_modules": [],
            "stdlib_support_modules": ["base64"],
            "package_parent_modules": [],
        },
        "binary_image_analysis": {
            "schema_version": 1,
            "frontend": {
                "schema_version": 1,
                "source_ast": {
                    "known_module_count": 3,
                    "compile_module_count": 2,
                    "source_bytes_known": 120,
                    "source_bytes_compile": 90,
                    "known": {
                        "ast_nodes": 24,
                        "calls": 1,
                        "function_defs": 1,
                    },
                    "compile": {
                        "ast_nodes": 18,
                        "calls": 1,
                        "function_defs": 1,
                    },
                },
                "module_schedule": {
                    "module_order_len": 3,
                    "compile_order_len": 2,
                    "module_order_hash": "module-order",
                    "compile_order_hash": "compile-order",
                },
                "lowering": {
                    "target_python": "3.12",
                    "enable_phi": True,
                    "compile_equals_known": False,
                },
            },
            "backend_ir": {
                "schema_version": 1,
                "backend_ir": {
                    "function_count": 1,
                    "op_count": 4,
                    "call_op_count": 1,
                },
                "source_sites": {
                    "carrier": "backend_ir_op_source_line",
                    "attributed_op_count": 3,
                    "unattributed_op_count": 1,
                    "coverage_ratio": 0.75,
                    "function_count_with_source": 1,
                    "line_count": 2,
                    "explicit_source_line_count": 3,
                    "line_marker_fallback_count": 0,
                    "source_site_digest": "source-sites",
                    "top_source_lines_by_ops": [
                        {"source_file": "app.py", "line": 3, "ops": 2}
                    ],
                },
                "allocation_ownership": {
                    "carrier": "backend_ir_source_sites_and_ownership_kinds",
                    "event_count": 4,
                    "source_attributed_event_count": 3,
                    "unattributed_event_count": 1,
                    "source_coverage_ratio": 0.75,
                    "events_by_category": {
                        "heap_alloc_root": 1,
                        "arena_eligible": 1,
                        "ref_retain": 1,
                        "ref_release": 1,
                    },
                    "top_category_kinds": [
                        {
                            "category_kind": "heap_alloc_root:object_new_bound",
                            "events": 1,
                        }
                    ],
                    "top_source_lines_by_events": [
                        {
                            "source_file": "app.py",
                            "line": 3,
                            "category": "heap_alloc_root",
                            "events": 1,
                        }
                    ],
                    "allocation_ownership_digest": "allocation-ownership",
                },
                "tir_boundary": {
                    "carrier": "backend_ir_json",
                    "semantic_role": "frontend-to-TIR/backend input",
                },
            },
            "artifacts": {
                "schema_version": 1,
                "kind": "native",
                "output_binary": {
                    "path": "app_molt",
                    "exists": True,
                    "size_bytes": 4096,
                },
            },
        },
        "allocations": {
            "current_bytes": 100,
            "peak_bytes": 240,
            "top": [
                {
                    "file": "src/molt/cli/frontend_pipeline.py",
                    "line": 10,
                    "size_bytes": 80,
                    "count": 2,
                }
            ],
        },
        "midend": {
            "requested_profile": "dev",
            "effective_profiles": ["dev"],
            "function_count": 1,
            "degraded_functions": 0,
            "promoted_functions": 0,
            "policy_config": {"tier": "dev"},
            "pass_wall_time_ranked": [{"pass": "type_refine", "ms_total": 2.0}],
            "pass_hotspots_top": [
                {
                    "module": "app",
                    "function": "main",
                    "pass": "type_refine",
                    "ms_total": 2.0,
                }
            ],
        },
    }


def _fact_graph() -> dict[str, object]:
    return {
        "schema_version": 3,
        "kind": "molt_tir_fact_graph",
        "function": "app::main",
        "values": [
            {
                "value": 0,
                "producer": {
                    "kind": "op_result",
                    "block": 0,
                    "op_index": 0,
                    "opcode": "Alloc",
                    "result_index": 0,
                    "source_site": {
                        "source_file": "app.py",
                        "line": 3,
                        "col": 4,
                        "end_col": 10,
                    },
                },
                "consumers": [],
                "facts": [
                    {
                        "kind": "repr_floor",
                        "value": "RawI64",
                        "confidence": "proven",
                        "producer": "type_refine",
                        "event_id": None,
                        "source_site": None,
                        "guards": [],
                        "invalidators": [],
                    },
                    {
                        "kind": "allocation.heap_root",
                        "value": "escape_alloc_site",
                        "confidence": "proven",
                        "producer": "op_kinds.escape_alloc_site_opcodes",
                        "event_id": "app::main:bb0:op0:Alloc:result0:allocation.heap_root",
                        "source_site": {
                            "source_file": "app.py",
                            "line": 3,
                            "col": 4,
                            "end_col": 10,
                        },
                        "guards": [],
                        "invalidators": ["op_kinds.toml"],
                    },
                ],
            }
        ],
        "edges": [],
        "summary": {
            "value_count": 1,
            "fact_count": 2,
            "edge_count": 0,
            "call_fact_count": 0,
            "source_site_value_count": 1,
            "allocation_ownership_fact_count": 1,
        },
    }


def test_build_capsule_bridges_frontend_tir_allocation_and_binary() -> None:
    capsule_mod = _load_capsule()

    capsule = capsule_mod.build_capsule(
        build_diagnostics=_build_diagnostics(),
        build_diagnostics_path="build-diagnostics.json",
        binary_size={
            "format": "wasm",
            "path": "probe.wasm",
            "total_bytes": 128,
            "by_type": {"code": 80, "data": 16},
            "sections": [{"id": 10, "name": "code", "size": 80}],
        },
        binary_size_path="binary-size.json",
        tir_fact_graphs=(("fact-graph.json", _fact_graph()),),
        label="unit",
        recorded_at="2026-06-25T00:00:00+00:00",
    )

    assert capsule["kind"] == "molt_analysis_capsule"
    assert capsule["cross_checks"]["passed"] is True
    assert capsule["source_frontend"]["closure"]["known_module_count"] == 3
    assert capsule["source_frontend"]["closure"]["compile_module_count"] == 2
    assert capsule["compiler_binary_image_analysis"]["stages"] == [
        "artifacts",
        "backend_ir",
        "frontend",
    ]
    assert (
        capsule["compiler_binary_image_analysis"]["frontend"]["source_ast"][
            "source_bytes_compile"
        ]
        == 90
    )
    assert (
        capsule["compiler_binary_image_analysis"]["source_sites"]["attributed_op_count"]
        == 3
    )
    assert (
        capsule["compiler_binary_image_analysis"]["source_sites"][
            "top_source_lines_by_ops"
        ][0]["source_file"]
        == "app.py"
    )
    assert (
        capsule["compiler_binary_image_analysis"]["allocation_ownership"]["event_count"]
        == 4
    )
    assert (
        capsule["compiler_binary_image_analysis"]["allocation_ownership"][
            "events_by_category"
        ]["heap_alloc_root"]
        == 1
    )
    assert capsule["ir_tir"]["tir_fact_graphs"][0]["fact_count"] == 2
    assert capsule["ir_tir"]["tir_fact_graphs"][0]["source_site_value_count"] == 1
    assert capsule["ir_tir"]["tir_fact_graphs"][0]["source_site_record_count"] == 2
    assert capsule["ir_tir"]["tir_fact_graphs"][0]["source_files"] == ["app.py"]
    assert (
        capsule["ir_tir"]["tir_fact_graphs"][0]["top_source_lines_by_records"][0][
            "records"
        ]
        == 2
    )
    assert (
        capsule["ir_tir"]["tir_fact_graphs"][0]["allocation_ownership_fact_count"] == 1
    )
    assert (
        capsule["ir_tir"]["tir_fact_graphs"][0]["allocation_ownership_by_kind"][
            "allocation.heap_root"
        ]
        == 1
    )
    assert capsule["allocation"]["peak_bytes"] == 240
    assert capsule["binary"]["size"]["total_bytes"] == 128


def test_build_capsule_fails_closed_on_compile_modules_outside_known() -> None:
    capsule_mod = _load_capsule()
    diagnostics = _build_diagnostics()
    closure = diagnostics["binary_image_closure"]
    assert isinstance(closure, dict)
    closure["compile_modules"] = ["app", "missing"]

    capsule = capsule_mod.build_capsule(
        build_diagnostics=diagnostics,
        build_diagnostics_path="build-diagnostics.json",
        recorded_at="2026-06-25T00:00:00+00:00",
    )

    assert capsule["cross_checks"]["passed"] is False
    assert any(
        "compile_modules contains entries outside known_modules" in error
        for error in capsule["cross_checks"]["errors"]
    )


def test_build_capsule_rejects_binary_image_analysis_closure_mismatch() -> None:
    capsule_mod = _load_capsule()
    diagnostics = _build_diagnostics()
    binary_analysis = diagnostics["binary_image_analysis"]
    assert isinstance(binary_analysis, dict)
    frontend = binary_analysis["frontend"]
    assert isinstance(frontend, dict)
    source_ast = frontend["source_ast"]
    assert isinstance(source_ast, dict)
    source_ast["compile_module_count"] = 99

    capsule = capsule_mod.build_capsule(
        build_diagnostics=diagnostics,
        build_diagnostics_path="build-diagnostics.json",
        recorded_at="2026-06-25T00:00:00+00:00",
    )

    assert capsule["cross_checks"]["passed"] is False
    assert any(
        "binary_image_analysis.frontend.source_ast.compile_module_count=99" in error
        for error in capsule["cross_checks"]["errors"]
    )


def test_build_capsule_rejects_backend_source_site_coverage_mismatch() -> None:
    capsule_mod = _load_capsule()
    diagnostics = _build_diagnostics()
    binary_analysis = diagnostics["binary_image_analysis"]
    assert isinstance(binary_analysis, dict)
    backend_ir = binary_analysis["backend_ir"]
    assert isinstance(backend_ir, dict)
    source_sites = backend_ir["source_sites"]
    assert isinstance(source_sites, dict)
    source_sites["unattributed_op_count"] = 9

    capsule = capsule_mod.build_capsule(
        build_diagnostics=diagnostics,
        build_diagnostics_path="build-diagnostics.json",
        recorded_at="2026-06-25T00:00:00+00:00",
    )

    assert capsule["cross_checks"]["passed"] is False
    assert any(
        "binary_image_analysis.backend_ir.source_sites coverage" in error
        for error in capsule["cross_checks"]["errors"]
    )


def test_build_capsule_rejects_backend_allocation_ownership_mismatch() -> None:
    capsule_mod = _load_capsule()
    diagnostics = _build_diagnostics()
    binary_analysis = diagnostics["binary_image_analysis"]
    assert isinstance(binary_analysis, dict)
    backend_ir = binary_analysis["backend_ir"]
    assert isinstance(backend_ir, dict)
    allocation = backend_ir["allocation_ownership"]
    assert isinstance(allocation, dict)
    allocation["source_attributed_event_count"] = 20

    capsule = capsule_mod.build_capsule(
        build_diagnostics=diagnostics,
        build_diagnostics_path="build-diagnostics.json",
        recorded_at="2026-06-25T00:00:00+00:00",
    )

    assert capsule["cross_checks"]["passed"] is False
    assert any(
        "binary_image_analysis.backend_ir.allocation_ownership" in error
        for error in capsule["cross_checks"]["errors"]
    )


def test_analysis_capsule_cli_writes_json(tmp_path: Path) -> None:
    capsule_mod = _load_capsule()
    diagnostics_path = tmp_path / "diag.json"
    binary_size_path = tmp_path / "binary-size.json"
    out_path = tmp_path / "capsule.json"
    diagnostics_path.write_text(json.dumps(_build_diagnostics()), encoding="utf-8")
    binary_size_path.write_text(
        json.dumps(
            {
                "format": "wasm",
                "path": "probe.wasm",
                "total_bytes": 128,
                "by_type": {"code": 80},
            }
        ),
        encoding="utf-8",
    )

    rc = capsule_mod.main(
        [
            "--build-diagnostics",
            str(diagnostics_path),
            "--binary-size-json",
            str(binary_size_path),
            "--label",
            "unit",
            "--out",
            str(out_path),
        ]
    )

    assert rc == 0
    payload = json.loads(out_path.read_text(encoding="utf-8"))
    assert payload["label"] == "unit"
    assert payload["analysis_tools"]["binary_size_analysis"]["present"] is True
