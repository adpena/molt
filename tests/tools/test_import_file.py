from __future__ import annotations

import importlib
import importlib.machinery
import importlib.resources
import sys
from pathlib import Path
from types import ModuleType

import pytest

from tools.import_file import (
    bind_repository_imports,
    load_module_from_path,
    load_sibling_package_module_from_path,
)

ROOT = Path(__file__).resolve().parents[2]


def _namespace_package(name: str, locations: list[Path]) -> ModuleType:
    package = ModuleType(name)
    package.__package__ = name
    package.__path__ = [str(location) for location in locations]
    spec = importlib.machinery.ModuleSpec(name, loader=None, is_package=True)
    spec.submodule_search_locations = list(package.__path__)
    package.__spec__ = spec
    return package


def test_top_level_import_reuses_loaded_canonical_module(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.syspath_prepend(str(ROOT / "tools"))
    sys.modules.pop("import_file", None)
    try:
        direct = importlib.import_module("import_file")
        canonical = sys.modules["tools.import_file"]
        assert direct is canonical
        assert sys.modules["import_file"] is canonical
        assert getattr(direct, "bind_repository_imports") is bind_repository_imports
    finally:
        sys.modules.pop("import_file", None)


def test_top_level_import_publishes_file_first_module_canonically(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    canonical = sys.modules["tools.import_file"]
    tools_package = sys.modules["tools"]
    monkeypatch.syspath_prepend(str(ROOT / "tools"))
    sys.modules.pop("import_file", None)
    sys.modules.pop("tools.import_file", None)
    try:
        direct = importlib.import_module("import_file")
        assert sys.modules["tools.import_file"] is direct
        assert importlib.import_module("tools.import_file") is direct
        assert tools_package.import_file is direct
    finally:
        sys.modules.pop("import_file", None)
        sys.modules["tools.import_file"] = canonical
        tools_package.import_file = canonical


def test_repository_binding_repositions_selected_roots_ahead_of_foreign_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign = tmp_path / "foreign"
    foreign.mkdir()
    monkeypatch.setattr(
        sys,
        "path",
        [str(foreign), str(ROOT / "src"), "sentinel", str(ROOT)],
    )

    assert bind_repository_imports(ROOT / "tools" / "structural_audit.py") == ROOT

    assert sys.path[:2] == [str(ROOT / "src"), str(ROOT)]
    assert sys.path[2:] == [str(foreign), "sentinel"]


def test_repository_binding_canonicalizes_unexecuted_namespace_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign = tmp_path / "foreign" / "tools"
    foreign.mkdir(parents=True)
    (foreign / "foreign-only.txt").write_text("foreign", encoding="utf-8")
    monkeypatch.syspath_prepend(str(foreign.parent))
    namespace = _namespace_package("tools", [ROOT / "tools", foreign])
    selected = ModuleType("tools.selected")
    selected.__file__ = str(ROOT / "tools" / "selected.py")
    monkeypatch.setitem(sys.modules, "tools", namespace)
    monkeypatch.setitem(sys.modules, "tools.selected", selected)

    bind_repository_imports(ROOT / "tools" / "structural_audit.py")

    expected = [str((ROOT / "tools").resolve())]
    assert sys.modules["tools"] is namespace
    assert list(namespace.__path__) == expected
    assert list(namespace.__spec__.submodule_search_locations) == expected
    importlib.invalidate_caches()
    resources = importlib.resources.files(namespace)
    assert resources.joinpath("import_file.py").is_file()
    assert not resources.joinpath("foreign-only.txt").is_file()


def test_repository_binding_canonicalizes_nested_namespace_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    selected = ROOT / "tools" / "bench"
    foreign = tmp_path / "foreign" / "tools" / "bench"
    foreign.mkdir(parents=True)
    namespace = _namespace_package("tools.bench", [selected, foreign])
    monkeypatch.setitem(sys.modules, "tools.bench", namespace)

    bind_repository_imports(ROOT / "tools" / "structural_audit.py")

    expected = [str(selected.resolve())]
    assert sys.modules["tools.bench"] is namespace
    assert list(namespace.__path__) == expected
    assert list(namespace.__spec__.submodule_search_locations) == expected


def test_repository_binding_rejects_foreign_descendant_without_namespace_mutation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign = tmp_path / "foreign" / "tools"
    foreign.mkdir(parents=True)
    namespace = _namespace_package("tools", [ROOT / "tools", foreign])
    descendant = ModuleType("tools.foreign")
    descendant.__file__ = str(foreign / "foreign.py")
    monkeypatch.setitem(sys.modules, "tools", namespace)
    monkeypatch.setitem(sys.modules, "tools.foreign", descendant)
    original_path = namespace.__path__
    original_spec = namespace.__spec__
    original_spec_locations = namespace.__spec__.submodule_search_locations

    with pytest.raises(RuntimeError, match="repository import custody mismatch"):
        bind_repository_imports(ROOT / "tools" / "structural_audit.py")

    assert namespace.__path__ is original_path
    assert namespace.__spec__ is original_spec
    assert namespace.__spec__.submodule_search_locations is original_spec_locations


def test_repository_binding_publishes_selected_namespace_before_foreign_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign_root = tmp_path / "foreign"
    foreign_tools = foreign_root / "tools"
    foreign_tools.mkdir(parents=True)
    marker = tmp_path / "foreign-executed"
    (foreign_tools / "__init__.py").write_text(
        f"from pathlib import Path\nPath({str(marker)!r}).touch()\n",
        encoding="utf-8",
    )
    monkeypatch.syspath_prepend(str(foreign_root))
    monkeypatch.delitem(sys.modules, "tools")
    canonical_import_file = sys.modules["tools.import_file"]

    bind_repository_imports(ROOT / "tools" / "structural_audit.py")

    selected_tools = sys.modules["tools"]
    assert importlib.import_module("tools") is selected_tools
    assert sys.modules["tools.import_file"] is canonical_import_file
    assert selected_tools.import_file is canonical_import_file
    assert list(selected_tools.__path__) == [str((ROOT / "tools").resolve())]
    assert not marker.exists()


def test_repository_binding_rejects_real_foreign_root_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign = tmp_path / "foreign" / "tools"
    foreign.mkdir(parents=True)
    package = ModuleType("tools")
    package.__file__ = str(foreign / "__init__.py")
    package.__path__ = [str(foreign)]
    monkeypatch.setitem(sys.modules, "tools", package)

    with pytest.raises(RuntimeError, match="repository import custody mismatch"):
        bind_repository_imports(ROOT / "tools" / "structural_audit.py")


def test_repository_binding_rejects_loaded_foreign_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign = ModuleType("molt.foreign")
    foreign.__file__ = str(tmp_path / "molt" / "foreign.py")
    monkeypatch.setitem(sys.modules, "molt.foreign", foreign)

    with pytest.raises(RuntimeError, match="repository import custody mismatch"):
        bind_repository_imports(ROOT / "tools" / "structural_audit.py")


def test_load_module_registers_identity_before_dataclass_execution(tmp_path) -> None:
    source = tmp_path / "loaded.py"
    source.write_text(
        "from dataclasses import dataclass\n"
        "import sys\n"
        "SELF = sys.modules[__name__]\n"
        "@dataclass\n"
        "class Record:\n"
        "    value: int\n",
        encoding="utf-8",
    )
    name = "_molt_test_registered_import"
    try:
        module = load_module_from_path(name, source)
        assert module.SELF is module
        assert module.Record(7).value == 7
        assert sys.modules[name] is module
    finally:
        sys.modules.pop(name, None)


def test_load_module_restores_prior_binding_after_failure(tmp_path) -> None:
    source = tmp_path / "broken.py"
    source.write_text("raise RuntimeError('broken body')\n", encoding="utf-8")
    name = "_molt_test_transactional_import"
    prior = ModuleType(name)
    sys.modules[name] = prior
    try:
        with pytest.raises(RuntimeError, match="broken body"):
            load_module_from_path(name, source)
        assert sys.modules[name] is prior
    finally:
        sys.modules.pop(name, None)


def test_load_module_removes_new_binding_after_failure(tmp_path) -> None:
    source = tmp_path / "broken.py"
    source.write_text("raise RuntimeError('broken body')\n", encoding="utf-8")
    name = "_molt_test_failed_import"
    sys.modules.pop(name, None)

    with pytest.raises(RuntimeError, match="broken body"):
        load_module_from_path(name, source)

    assert name not in sys.modules


def test_sibling_package_loader_resolves_relative_import_without_sys_path(
    tmp_path,
) -> None:
    package_root = tmp_path / "authority"
    package_root.mkdir()
    (package_root / "policy.py").write_text("VALUE = 41\n", encoding="utf-8")
    source = package_root / "consumer.py"
    source.write_text(
        "from .policy import VALUE\nRESULT = VALUE + 1\n",
        encoding="utf-8",
    )
    package_name = "_molt_test_sibling_package"
    module_name = f"{package_name}.consumer"
    assert str(tmp_path) not in sys.path
    try:
        module = load_sibling_package_module_from_path(module_name, source)
        assert module.RESULT == 42
        assert sys.modules[module_name] is module
        assert sys.modules[f"{package_name}.policy"].VALUE == 41
    finally:
        sys.modules.pop(f"{package_name}.policy", None)
        sys.modules.pop(module_name, None)
        sys.modules.pop(package_name, None)


def test_sibling_package_loader_restores_parent_after_failure(tmp_path) -> None:
    package_root = tmp_path / "authority"
    package_root.mkdir()
    (package_root / "policy.py").write_text("VALUE = 41\n", encoding="utf-8")
    source = package_root / "broken.py"
    source.write_text(
        "from .policy import VALUE\nraise RuntimeError('broken sibling')\n",
        encoding="utf-8",
    )
    package_name = "_molt_test_sibling_transaction"
    module_name = f"{package_name}.broken"
    prior = ModuleType(package_name)
    sys.modules[package_name] = prior
    try:
        with pytest.raises(RuntimeError, match="broken sibling"):
            load_sibling_package_module_from_path(module_name, source)
        assert sys.modules[package_name] is prior
        assert module_name not in sys.modules
        assert f"{package_name}.policy" not in sys.modules
    finally:
        sys.modules.pop(module_name, None)
        sys.modules.pop(package_name, None)
