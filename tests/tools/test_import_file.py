from __future__ import annotations

import importlib
import importlib.machinery
import importlib.resources
import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools.import_file import (
    bind_repository_imports,
    load_module_from_path,
    load_sibling_package_module_from_path,
)

ROOT = Path(__file__).resolve().parents[2]
_ADMISSION_MODULE = "_molt_package_import_custody"


@pytest.fixture(params=["tool", "probe"])
def package_admission_loader(request):
    # Both bootstrap edges must obey the same independent loader/rollback
    # oracles. Only their standard importlib mechanics are duplicated.
    if request.param == "tool":
        from tools import import_file as owner
    else:
        from molt import python_environment_identity as owner
    return owner._load_package_import_custody


def test_package_admission_reuses_one_neutral_module(package_admission_loader):
    from tools import import_file

    selected = ROOT / "src/molt/package_import_custody.py"
    loaded = package_admission_loader(selected)
    assert loaded is import_file._package_import_custody
    assert loaded is sys.modules[_ADMISSION_MODULE]
    assert "molt.package_import_custody" not in sys.modules
    assert loaded.__name__ == _ADMISSION_MODULE
    assert loaded.__package__ == ""
    assert loaded.__file__ == str(selected.resolve())


@pytest.mark.parametrize(
    "field",
    ["name", "package", "file", "spec-name", "origin", "loader", "loader-path"],
)
def test_package_admission_refuses_incoherent_cached_identity(
    package_admission_loader, field, monkeypatch, tmp_path
):
    selected = (ROOT / "src/molt/package_import_custody.py").resolve()
    spec = importlib.util.spec_from_file_location(_ADMISSION_MODULE, selected)
    assert spec is not None
    loaded = importlib.util.module_from_spec(spec)
    foreign = str(tmp_path / "foreign.py")
    if field == "name":
        loaded.__name__ = "foreign"
    elif field == "package":
        loaded.__package__ = "foreign"
    elif field == "file":
        loaded.__file__ = foreign
    elif field == "spec-name":
        spec.name = "foreign"
    elif field == "origin":
        spec.origin = foreign
    elif field == "loader":
        loaded.__loader__ = object()
    else:
        spec.loader.path = foreign
    monkeypatch.setitem(sys.modules, _ADMISSION_MODULE, loaded)

    with pytest.raises(ImportError, match="already loaded from another authority"):
        package_admission_loader(selected)

    assert sys.modules[_ADMISSION_MODULE] is loaded


def test_package_admission_preserves_none_binding(
    package_admission_loader, monkeypatch
):
    monkeypatch.setitem(sys.modules, _ADMISSION_MODULE, None)
    with pytest.raises(ImportError, match="already loaded from another authority"):
        package_admission_loader(ROOT / "src/molt/package_import_custody.py")
    assert _ADMISSION_MODULE in sys.modules
    assert sys.modules[_ADMISSION_MODULE] is None


def test_package_admission_rejects_a_second_source_owner(
    package_admission_loader, tmp_path
):
    selected = ROOT / "src/molt/package_import_custody.py"
    foreign = tmp_path / "package_import_custody.py"
    foreign.write_bytes(selected.read_bytes())
    prior = sys.modules[_ADMISSION_MODULE]
    with pytest.raises(ImportError, match="already loaded from another authority"):
        package_admission_loader(foreign)
    assert sys.modules[_ADMISSION_MODULE] is prior


def test_package_admission_rolls_back_failed_initial_load(
    package_admission_loader, monkeypatch, tmp_path
):
    monkeypatch.delitem(sys.modules, _ADMISSION_MODULE)
    broken = tmp_path / "package_import_custody.py"
    broken.write_text("raise RuntimeError('ordinary failed body')\n", encoding="utf-8")
    with pytest.raises(RuntimeError, match="ordinary failed body"):
        package_admission_loader(broken)
    assert _ADMISSION_MODULE not in sys.modules


def test_package_admission_disallows_package_alias_execution():
    prior = sys.modules[_ADMISSION_MODULE]
    with pytest.raises(ImportError, match="requires its selected file loader"):
        load_module_from_path(
            "molt.package_import_custody", ROOT / "src/molt/package_import_custody.py"
        )
    assert "molt.package_import_custody" not in sys.modules
    assert sys.modules[_ADMISSION_MODULE] is prior


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


@pytest.mark.parametrize("canonical_state", ["absent", "none"])
@pytest.mark.parametrize("parent_state", ["absent", "existing"])
@pytest.mark.parametrize("failure", ["missing", "body", "cached-none", "foreign"])
def test_top_level_import_failure_preserves_alias_and_parent_bindings(
    tmp_path, monkeypatch, canonical_state, parent_state, failure
):
    selected = tmp_path / "selected"
    tools_root = selected / "tools"
    tools_root.mkdir(parents=True)
    (tools_root / "import_file.py").write_bytes(
        (ROOT / "tools/import_file.py").read_bytes()
    )
    helper = selected / "src/molt/package_import_custody.py"
    helper.parent.mkdir(parents=True)
    if failure == "body":
        helper.write_text(
            "raise RuntimeError('ordinary helper body failure')\n", encoding="utf-8"
        )
    elif failure != "missing":
        helper.write_bytes((ROOT / "src/molt/package_import_custody.py").read_bytes())

    tools_package = sys.modules["tools"]
    foreign_authority = sys.modules[_ADMISSION_MODULE]
    marker = object()
    expected_error, diagnostic = {
        "missing": (FileNotFoundError, "package_import_custody"),
        "body": (RuntimeError, "ordinary helper body failure"),
        "cached-none": (ImportError, "already loaded from another authority"),
        "foreign": (ImportError, "already loaded from another authority"),
    }[failure]

    # Scope altered imports to this operation so pytest/report hooks observe
    # their original package bindings even when the regression assertion fails.
    with monkeypatch.context() as scoped:
        scoped.syspath_prepend(str(tools_root))
        scoped.delitem(sys.modules, "import_file", raising=False)
        if canonical_state == "none":
            scoped.setitem(sys.modules, "tools.import_file", None)
        else:
            scoped.delitem(sys.modules, "tools.import_file", raising=False)
        if parent_state == "existing":
            scoped.setattr(tools_package, "import_file", marker, raising=False)
        else:
            scoped.delattr(tools_package, "import_file", raising=False)
        if failure == "cached-none":
            scoped.setitem(sys.modules, _ADMISSION_MODULE, None)
        elif failure == "foreign":
            scoped.setitem(sys.modules, _ADMISSION_MODULE, foreign_authority)
        else:
            scoped.delitem(sys.modules, _ADMISSION_MODULE, raising=False)

        before = (
            "tools.import_file" in sys.modules,
            sys.modules.get("tools.import_file"),
            hasattr(tools_package, "import_file"),
            getattr(tools_package, "import_file", None),
            _ADMISSION_MODULE in sys.modules,
            sys.modules.get(_ADMISSION_MODULE),
        )
        with pytest.raises(expected_error, match=diagnostic):
            importlib.import_module("import_file")

        assert "import_file" not in sys.modules
        assert (
            "tools.import_file" in sys.modules,
            sys.modules.get("tools.import_file"),
            hasattr(tools_package, "import_file"),
            getattr(tools_package, "import_file", None),
            _ADMISSION_MODULE in sys.modules,
            sys.modules.get(_ADMISSION_MODULE),
        ) == before


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


def _resolves_beside(script: Path, module: str) -> bool:
    head = module.split(".", 1)[0]
    return (script.parent / f"{head}.py").is_file() or (script.parent / head).is_dir()


def _script_launch_failure(script: Path) -> str | None:
    """The first import a by-path launch of ``script`` cannot resolve.

    A launch puts only the script's directory first on ``sys.path``: ``tools``
    resolves after ``bind_repository_imports``, a fallback must name a file
    beside the script, and a relative import never resolves.
    """
    import ast

    tree = ast.parse(script.read_text(encoding="utf-8"))
    # Generated script text and nested guards do not turn their owning module
    # into a path entrypoint. Inspect the executable module body itself.
    if not any(
        isinstance(node, ast.If)
        and isinstance(node.test, ast.Compare)
        and len(node.test.ops) == 1
        and isinstance(node.test.ops[0], ast.Eq)
        and {ast.unparse(node.test.left), ast.unparse(node.test.comparators[0])}
        == {"__name__", "'__main__'"}
        for node in tree.body
    ):
        return None
    bound = False
    for node in tree.body:
        calls = {
            ast.unparse(call.func)
            for call in ast.walk(node)
            if isinstance(call, ast.Call)
        }
        # Binding the repository, or putting a root on sys.path, makes
        # later tools imports resolve.
        if calls & {"bind_repository_imports", "sys.path.insert", "sys.path.append"}:
            bound = True
            if not isinstance(node, (ast.Import, ast.ImportFrom, ast.Try, ast.If)):
                continue
        if isinstance(node, ast.ImportFrom) and node.level:
            return f"line {node.lineno}: relative import"
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            names = (
                [alias.name for alias in node.names]
                if isinstance(node, ast.Import)
                else [node.module or ""]
            )
            if not bound and any(name.split(".")[0] == "tools" for name in names):
                return f"line {node.lineno}: tools import before binding"
        guarded = isinstance(node, ast.Try) or (
            isinstance(node, ast.If) and "__package__" in ast.unparse(node.test)
        )
        if guarded and not bound:
            fallbacks = (
                [stmt for handler in node.handlers for stmt in handler.body]
                if isinstance(node, ast.Try)
                else node.body
            )
            for stmt in fallbacks:
                if isinstance(stmt, ast.ImportFrom) and stmt.level == 0:
                    module = stmt.module or ""
                    if module.split(".")[0] != "tools" and not _resolves_beside(
                        script, module
                    ):
                        if (ROOT / "tools" / f"{module.split('.')[0]}.py").is_file():
                            return f"line {stmt.lineno}: fallback {module} is not beside it"
    return None


def test_tool_scripts_import_when_launched_by_path() -> None:
    failures = {}
    for script in sorted((ROOT / "tools").rglob("*.py")):
        source = script.read_text(encoding="utf-8")
        if "__main__" not in source:
            continue
        failure = _script_launch_failure(script)
        if failure is not None:
            failures[script.relative_to(ROOT).as_posix()] = failure
    assert not failures, failures


@pytest.mark.parametrize(
    "relative",
    [
        "tools/release/build_bundle.py",
        "tools/release/release_authority.py",
        "tools/release/verify_consumer.py",
        "tools/release/provision_execution_archives.py",
        "tools/hooks/landing_gate.py",
        "tools/runtime_wasm_final_preflight.py",
    ],
)
def test_repository_entrypoint_launches_outside_checkout(
    tmp_path: Path, relative: str
) -> None:
    result = run_guarded_test_process(
        [sys.executable, "-I", str(ROOT / relative), "--help"],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "usage:" in result.stdout


@pytest.mark.parametrize(
    ("body", "is_entrypoint"),
    [
        ("generated = 'if __name__ == \"__main__\": run()'\n", False),
        ('def wrapper():\n    if __name__ == "__main__": run()\n', False),
        ('if __name__ == "__main__": run()\n', True),
        ('if "__main__" == __name__: run()\n', True),
    ],
)
def test_script_launch_scan_uses_executable_module_guard(
    tmp_path: Path, body: str, is_entrypoint: bool
) -> None:
    script = tmp_path / "command.py"
    script.write_text("from . import sibling\n" + body, encoding="utf-8")
    failure = _script_launch_failure(script)
    assert (failure is not None) is is_entrypoint
    if is_entrypoint:
        assert failure == "line 1: relative import"
