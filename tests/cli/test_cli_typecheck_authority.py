from __future__ import annotations

import inspect
import json
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import frontend_pipeline, module_source, typecheck
from molt.type_facts import collect_type_facts_from_paths


def test_cli_typecheck_authority_is_single_home() -> None:
    for name in ("_collect_py_files", "_run_ty_check", "check"):
        assert hasattr(typecheck, name)
        assert not hasattr(cli, name)
        assert f"def {name}(" not in inspect.getsource(cli)


def _prepare(source: Path, *, policy="check", is_wasm=False):
    warnings: list[str] = []
    details: dict = {}
    config, failure = frontend_pipeline._prepare_frontend_lowering_config(
        type_facts_path=None,
        type_hint_policy=policy,
        module_graph={"entry": source},
        source_path=source,
        json_output=True,
        warnings=warnings,
        module_deps={"entry": set()},
        module_dep_closures={"entry": frozenset()},
        has_back_edges=False,
        known_modules={"entry"},
        direct_call_modules={"entry"},
        known_func_defaults={},
        known_func_kinds={},
        native_callable_exports={},
        pgo_hot_function_names=set(),
        generated_module_source_paths={},
        entry_module="entry",
        entry_execution_kind="script",
        namespace_module_names=set(),
        module_source_catalog=module_source._build_module_source_catalog(
            {"entry": source},
            module_sources={"entry": source.read_text(encoding="utf-8")},
        ),
        is_wasm=is_wasm,
        frontend_parallel_details=details,
        frontend_phase_timeout=None,
        is_luau_transpile=False,
    )
    return config, failure, warnings, details


@pytest.mark.parametrize("policy", ["check", "ignore"])
@pytest.mark.parametrize("is_wasm", [False, True])
def test_build_keeps_source_annotations_without_running_a_checker(
    tmp_path, monkeypatch, policy, is_wasm
) -> None:
    source = tmp_path / "entry.py"
    source.write_text(
        "def f(x: int):\n    y = 1\n    y = 'text'\n    return y\n", encoding="utf-8"
    )

    def unexpected_check(*args, **kwargs):
        pytest.fail("ordinary compilation must not consult an ambient checker")

    monkeypatch.setattr(typecheck, "_run_ty_check", unexpected_check)
    config, failure, warnings, _ = _prepare(source, policy=policy, is_wasm=is_wasm)
    assert failure is None
    assert config is not None
    assert config.type_facts is None
    assert warnings == []


def test_trusted_build_revalidates_external_environment_and_reports_failure(
    tmp_path, monkeypatch, capsys
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("value: int = 1\n", encoding="utf-8")
    outcomes = iter([(True, ""), (False, "dependency annotation changed")])
    monkeypatch.setattr(typecheck, "_run_ty_check", lambda _: next(outcomes))
    config, failure, _, details = _prepare(source, policy="trust")
    assert failure is None and config is not None
    assert config.type_facts is None
    assert "validate_type_hints" in details["pipeline_stage_ms"]
    config, failure, _, _ = _prepare(source, policy="trust")
    assert config is None and failure is not None
    assert "dependency annotation changed" in capsys.readouterr().out


def test_type_fact_export_never_promotes_assignments_to_scope_wide_types(tmp_path):
    source = tmp_path / "entry.py"
    source.write_text(
        "value = 1\nvalue = 'changed'\nannotated: int = 3\n"
        "def f(flag, declared: int):\n"
        "    local = []\n"
        "    if flag:\n        local = {'key': 1}\n"
        "    annotated_local: int = 4\n"
        "    return local\n",
        encoding="utf-8",
    )
    for trust in ("guarded", "trusted"):
        module = collect_type_facts_from_paths([source], trust).modules["entry"]
        assert set(module.globals) == {"annotated"}
        assert set(module.functions["f"].params) == {"declared"}
        assert set(module.functions["f"].locals) == {"annotated_local"}


@pytest.mark.parametrize("ty_ok", [False, True])
def test_check_exports_the_same_guarded_annotations_regardless_of_validation(
    tmp_path, monkeypatch, ty_ok
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("annotated: int = 1\nunannotated = 2\n", encoding="utf-8")
    output = tmp_path / "facts.json"
    monkeypatch.setattr(typecheck, "_run_ty_check", lambda _: (ty_ok, "diagnostic"))
    assert (
        typecheck.check(
            str(source),
            str(output),
            strict=False,
            deterministic=False,
            json_output=True,
        )
        == 0
    )
    facts = json.loads(output.read_text(encoding="utf-8"))
    assert facts["modules"]["entry"]["globals"] == {
        "annotated": {"type": "int", "trust": "guarded"}
    }
    assert facts["tool"] == "molt-check"


def test_failed_strict_check_never_publishes_facts_even_without_diagnostics(
    tmp_path, monkeypatch
) -> None:
    source = tmp_path / "entry.py"
    source.write_text("value: int = 1\n", encoding="utf-8")
    output = tmp_path / "facts.json"
    output.write_text("previous artifact\n", encoding="utf-8")
    monkeypatch.setattr(typecheck, "_run_ty_check", lambda _: (False, ""))
    assert (
        typecheck.check(
            str(source), str(output), strict=True, deterministic=False, json_output=True
        )
        != 0
    )
    assert output.read_text(encoding="utf-8") == "previous artifact\n"


def test_explicit_checker_cannot_import_a_project_shadow(tmp_path, monkeypatch):
    source = tmp_path / "entry.py"
    source.write_text("value: int = 1\n", encoding="utf-8")
    marker = tmp_path / "executed"
    (tmp_path / "ty.py").write_text(
        f"from pathlib import Path\nPath({str(marker)!r}).write_text('shadow')\n"
        "raise RuntimeError('project ty module executed')\n",
        encoding="utf-8",
    )
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))
    monkeypatch.setenv("MOLT_TY_TIMEOUT", "30")
    ok, diagnostics = typecheck._run_ty_check(source)
    assert ok, diagnostics
    assert not marker.exists()
