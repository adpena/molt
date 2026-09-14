from __future__ import annotations

import inspect
from pathlib import Path

from molt.cli.models import _DiscoveredModuleGraph

import pytest

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_discovery
from molt.cli import module_graph_cache
from molt.cli import module_import_scanner
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

_MODULE_GRAPH_DISCOVERY_NAMES = (
    "PLATFORM_EXCLUDED_SUBMODULES",
    "_discover_module_graph",
    "_discover_module_graph_from_paths",
    "_extend_module_graph_with_closure",
    "_extend_module_graph_with_static_import_modules",
    "_load_module_imports",
    "_load_module_import_scan",
    "_LoadedModuleImportScan",
    "_parse_static_import_modules",
    "_parse_static_import_modules_from_env",
    "_record_module_reason",
    "_record_new_module_reasons",
    "_resolve_static_import_module_paths",
)

_MODULE_GRAPH_DISCOVERY_DEFINITIONS = (
    "PLATFORM_EXCLUDED_SUBMODULES =",
    "def _discover_module_graph(",
    "def _discover_module_graph_from_paths(",
    "def _extend_module_graph_with_closure(",
    "def _extend_module_graph_with_static_import_modules(",
    "def _load_module_imports(",
    "def _load_module_import_scan(",
    "class _LoadedModuleImportScan",
    "def _parse_static_import_modules(",
    "def _parse_static_import_modules_from_env(",
    "def _record_module_reason(",
    "def _record_new_module_reasons(",
    "def _resolve_static_import_module_paths(",
)


def test_cli_module_graph_discovery_authority_is_single_home() -> None:
    for name in _MODULE_GRAPH_DISCOVERY_NAMES:
        assert hasattr(module_graph_discovery, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_GRAPH_DISCOVERY_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source


@pytest.mark.parametrize("full_scan", [False, True])
@pytest.mark.parametrize("module_name", ["plain", "collections", "email.message"])
def test_scan_mode_projects_explicit_depth_and_canonical_helpers(
    full_scan: bool, module_name: str
) -> None:
    expected = (
        "full"
        if full_scan
        else "module_init"
        if module_name == "plain"
        else "module_init_static_helpers"
    )
    assert (
        module_import_scanner._module_import_scan_mode(module_name, full_scan=full_scan)
        == expected
    )
    assert module_import_scanner._module_import_scan_mode(
        module_name, full_scan=full_scan, static_import_helper_modules=()
    ) == ("full" if full_scan else "module_init")


@pytest.mark.parametrize("first_full", [False, True])
def test_graph_cache_keeps_initialization_and_full_roots_disjoint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, first_full: bool
) -> None:
    entry = tmp_path / "entry.py"
    entry.write_text("def deferred():\n    import lazy\n", encoding="utf-8")
    lazy = tmp_path / "lazy.py"
    lazy.write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "test"
    )
    for full in (first_full, not first_full, first_full):
        discovery_result = module_graph_discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            stdlib,
            tmp_path,
            set(),
            full_scan_roots=full,
        )
        graph = discovery_result.graph
        imports = discovery_result.explicit_imports
        assert set(graph) == ({"entry", "lazy"} if full else {"entry"})
        assert imports == ({"lazy"} if full else set())


@pytest.mark.parametrize("deferred", [False, True])
def test_initialization_seed_does_not_grant_dynamic_import_custody(
    tmp_path: Path, deferred: bool
) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    entry = package / "__init__.py"
    entry.write_text(
        "__package__ = choose_package()\n"
        + (
            "def deferred():\n    from . import lazy\n"
            if deferred
            else "from . import lazy\n"
        ),
        encoding="utf-8",
    )

    def discover(full: bool) -> _DiscoveredModuleGraph:
        return module_graph_discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            None,
            set(),
            full_scan_roots=full,
        )

    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        discover(True)
    if not deferred:
        with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
            discover(False)
    else:
        assert discover(False).graph == {"pkg": entry}


def test_core_initialization_keeps_named_helper_imports(tmp_path: Path) -> None:
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    entry = stdlib / "collections.py"
    entry.write_text(
        "class UserDict:\n    def copy(self):\n        import copy\n",
        encoding="utf-8",
    )
    copy = stdlib / "copy.py"
    copy.write_text("VALUE = 1\n", encoding="utf-8")
    discovery_result = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [stdlib],
        [stdlib],
        stdlib,
        None,
        {"collections", "copy"},
        full_scan_roots=False,
    )
    graph = discovery_result.graph
    imports = discovery_result.explicit_imports
    assert set(graph) == {"collections", "copy"}
    assert imports == {"copy"}
