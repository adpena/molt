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
        "schema_version": 1,
        "kind": "molt_tir_fact_graph",
        "function": "app::main",
        "values": [
            {
                "value": 0,
                "producer": {"kind": "param"},
                "consumers": [],
                "facts": [
                    {
                        "kind": "repr_floor",
                        "value": "RawI64",
                        "confidence": "proven",
                        "producer": "type_refine",
                        "guards": [],
                        "invalidators": [],
                    }
                ],
            }
        ],
        "edges": [],
        "summary": {
            "value_count": 1,
            "fact_count": 1,
            "edge_count": 0,
            "call_fact_count": 0,
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
    assert capsule["ir_tir"]["tir_fact_graphs"][0]["fact_count"] == 1
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
