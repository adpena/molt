from __future__ import annotations

import ast
from pathlib import Path

import pytest

from molt.cli import build_inputs as cli_build_inputs
from molt.cli import module_graph as cli_module_graph
from molt.cli import module_resolution as cli_module_resolution
from molt.cli import wrapper_build as cli_wrapper_build
from molt.cli.config_resolution import STATIC_IMPORT_MODULES_ENV


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
    _file, _module, _source, selector_error = (
        cli_build_inputs._resolve_build_entry_selector(
            file_path=None,
            module=None,
            project_root=tmp_path,
            build_config={"entry-file": "app.py", "entry-module": "pkg"},
        )
    )

    resolved, error = _resolve_entry(
        tmp_path,
        build_config={"entry-file": "app.py", "entry-module": "pkg"},
    )

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
    with pytest.raises(ValueError, match="outside the closure plan"):
        import_plan.with_compile_modules({"outside"})


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
