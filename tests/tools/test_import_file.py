from __future__ import annotations

import ast
import functools
import importlib
import importlib.machinery
import importlib.resources
import importlib.util
import sys
from collections.abc import Iterable, Iterator
from pathlib import Path
from types import ModuleType

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools.import_file import (
    bind_repository_imports,
    load_module_from_path,
    load_sibling_package_module_from_path,
    molt_module_name,
    tool_module_name,
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


def test_bare_binder_import_is_refused_without_side_effects(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # One import identity (HF-221): with tools/ on sys.path, a bare
    # `import_file` must fail before it can build a second binder, load the
    # custody helper, or touch the canonical module and its parent binding.
    tools_package = sys.modules["tools"]
    canonical = sys.modules["tools.import_file"]
    custody = sys.modules[_ADMISSION_MODULE]
    monkeypatch.syspath_prepend(str(ROOT / "tools"))
    monkeypatch.delitem(sys.modules, "import_file", raising=False)

    with pytest.raises(ImportError, match="as tools.import_file, not 'import_file'"):
        importlib.import_module("import_file")

    assert "import_file" not in sys.modules
    assert sys.modules["tools.import_file"] is canonical
    assert tools_package.import_file is canonical
    assert sys.modules[_ADMISSION_MODULE] is custody


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


class _ObservedSearchPath(list):
    def __init__(self, entries):
        super().__init__(entries)
        self.publications = []

    def __setitem__(self, key, value):
        super().__setitem__(key, value)
        self.publications.append(list(self))

    def insert(self, index, value):
        super().insert(index, value)
        self.publications.append(list(self))


@pytest.mark.parametrize(
    "prefix", [(ROOT, ROOT / "src"), (ROOT / "src", ROOT), (ROOT, ROOT / "src", ROOT)]
)
def test_repository_binding_preserves_an_admitted_path_without_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, prefix: tuple[Path, ...]
) -> None:
    entries = [*map(str, prefix), str(tmp_path / "foreign"), "sentinel"]
    path = _ObservedSearchPath(entries)
    monkeypatch.setattr(sys, "path", path)

    for _ in range(2):
        assert bind_repository_imports(ROOT / "tools/structural_audit.py") == ROOT

    assert sys.path is path
    assert path == entries
    assert path.publications == []


@pytest.mark.parametrize(
    "damage",
    [
        "relative-selected",
        "symlink-selected",
        "bare-tools",
        "nested-tools",
        "foreign-first",
        "missing-src",
    ],
)
def test_repository_binding_repairs_an_unadmitted_path_in_one_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, damage: str
) -> None:
    foreign = str(tmp_path / "foreign")
    entries = [str(ROOT), str(ROOT / "src"), foreign, "sentinel"]
    if damage == "relative-selected":
        monkeypatch.chdir(ROOT)
        entries.append("src")
    elif damage == "symlink-selected":
        alias = tmp_path / "selected-alias"
        try:
            alias.symlink_to(ROOT, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"host cannot create directory symlinks: {exc}")
        entries.append(str(alias))
    elif damage == "bare-tools":
        entries.append(str(ROOT / "tools"))
    elif damage == "nested-tools":
        entries.append(str(ROOT / "tools/compat"))
    elif damage == "foreign-first":
        entries = [foreign, str(ROOT), str(ROOT / "src"), "sentinel"]
    else:
        entries.remove(str(ROOT / "src"))
    path = _ObservedSearchPath(entries)
    monkeypatch.setattr(sys, "path", path)

    bind_repository_imports(ROOT / "tools/structural_audit.py")

    expected = [str(ROOT / "src"), str(ROOT), foreign, "sentinel"]
    assert path == expected
    assert path.publications == [expected]


def test_concurrent_repository_bindings_preserve_an_admitted_path(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from concurrent.futures import ThreadPoolExecutor

    entries = [str(ROOT), str(ROOT / "src"), *sys.path]
    entries = list(dict.fromkeys(entries))
    path = _ObservedSearchPath(entries)
    monkeypatch.setattr(sys, "path", path)
    with ThreadPoolExecutor(max_workers=2) as executor:
        assert (
            list(
                executor.map(
                    bind_repository_imports, [ROOT / "tools/structural_audit.py"] * 4
                )
            )
            == [ROOT] * 4
        )
    assert path == entries
    assert path.publications == []


@pytest.mark.parametrize("path_admitted", [False, True])
def test_repository_binding_canonicalizes_unexecuted_namespace_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, path_admitted: bool
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
    if path_admitted:
        monkeypatch.setattr(
            sys, "path", [str(ROOT), str(ROOT / "src"), str(foreign.parent)]
        )
    before_path = list(sys.path)

    bind_repository_imports(ROOT / "tools" / "structural_audit.py")

    if path_admitted:
        assert sys.path == before_path
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
    selected = ROOT / "tools" / "compat"
    foreign = tmp_path / "foreign" / "tools" / "compat"
    foreign.mkdir(parents=True)
    namespace = _namespace_package("tools.compat", [selected, foreign])
    monkeypatch.setitem(sys.modules, "tools.compat", namespace)

    bind_repository_imports(ROOT / "tools" / "structural_audit.py")

    expected = [str(selected.resolve())]
    assert sys.modules["tools.compat"] is namespace
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


@pytest.mark.parametrize("path_admitted", [False, True])
def test_repository_binding_rejects_real_foreign_root_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, path_admitted: bool
) -> None:
    foreign = tmp_path / "foreign" / "tools"
    foreign.mkdir(parents=True)
    package = ModuleType("tools")
    package.__file__ = str(foreign / "__init__.py")
    package.__path__ = [str(foreign)]
    monkeypatch.setitem(sys.modules, "tools", package)
    if path_admitted:
        monkeypatch.setattr(sys, "path", [str(ROOT), str(ROOT / "src")])
    before_path = list(sys.path)

    with pytest.raises(RuntimeError, match="repository import custody mismatch"):
        bind_repository_imports(ROOT / "tools" / "structural_audit.py")
    assert sys.path == before_path


@pytest.mark.parametrize("path_admitted", [False, True])
def test_repository_binding_rejects_loaded_foreign_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, path_admitted: bool
) -> None:
    foreign = ModuleType("molt.foreign")
    foreign.__file__ = str(tmp_path / "molt" / "foreign.py")
    monkeypatch.setitem(sys.modules, "molt.foreign", foreign)
    if path_admitted:
        monkeypatch.setattr(sys, "path", [str(ROOT), str(ROOT / "src")])
    before_path = list(sys.path)

    with pytest.raises(RuntimeError, match="repository import custody mismatch"):
        bind_repository_imports(ROOT / "tools" / "structural_audit.py")
    assert sys.path == before_path


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


_STDLIB = frozenset(sys.stdlib_module_names)
# A tools module named like a stdlib module (`profile`) means the stdlib one.
_TOOLS_TOP = (
    frozenset(
        {path.stem for path in (ROOT / "tools").glob("*.py")}
        | {
            path.name
            for path in (ROOT / "tools").iterdir()
            if path.is_dir() and path.name.isidentifier()
        }
    )
    - _STDLIB
)


def _is_entrypoint(tree: ast.Module) -> bool:
    # Generated script text and nested guards do not turn their owning module
    # into a path entrypoint. Inspect the executable module body itself.
    return any(
        isinstance(node, ast.If)
        and isinstance(node.test, ast.Compare)
        and len(node.test.ops) == 1
        and isinstance(node.test.ops[0], ast.Eq)
        and {ast.unparse(node.test.left), ast.unparse(node.test.comparators[0])}
        == {"__name__", "'__main__'"}
        for node in tree.body
    )


def _edits_sys_path(node: ast.AST) -> bool:
    return any(
        isinstance(call, ast.Call)
        and ast.unparse(call.func)
        in {"sys.path.insert", "sys.path.append", "sys.path.extend"}
        for call in ast.walk(node)
    )


# Python a developer's CPython runs from a checkout. demos/, examples/ and
# demo/ hold programs Molt compiles; src/ is the installed package.
_HOST_SCRIPT_ROOTS = (
    "tools",
    "bench",
    "deploy",
    "drivers",
    "collab",
    "packaging",
    "docs",
    ".github",
)


@functools.cache
def _repository_python(*roots: str) -> tuple[Path, ...]:
    """Python files under ``roots`` that git tracks or would track.

    Ignored trees (virtualenvs, node_modules, cloned upstream repos) are
    not repository code.
    """
    listed = run_guarded_test_process(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            *(f"{root}/*.py" for root in roots),
        ],
        cwd=ROOT,
        check=True,
    ).stdout
    return tuple(
        path
        for path in sorted({ROOT / item for item in listed.split("\0") if item})
        if path.is_file()
    )


def _is_root_insertion(node: ast.stmt) -> bool:
    """`if str(_REPO_ROOT) not in sys.path: sys.path.insert(0, str(_REPO_ROOT))`."""
    return (
        isinstance(node, ast.If)
        and not node.orelse
        and ast.unparse(node.test) == "str(_REPO_ROOT) not in sys.path"
        and [ast.unparse(statement) for statement in node.body]
        == ["sys.path.insert(0, str(_REPO_ROOT))"]
    )


def _module_heads(node: ast.stmt) -> list[str | None]:
    if isinstance(node, ast.Import):
        return [alias.name.split(".")[0] for alias in node.names]
    if isinstance(node, ast.ImportFrom):
        return [None if node.level else (node.module or "").split(".")[0]]
    return []


# Top-level import names that resolve inside this repository.
_REPOSITORY_PACKAGES = frozenset(
    {"tests", *(root for root in _HOST_SCRIPT_ROOTS if root.isidentifier())}
    | {
        path.stem if path.suffix == ".py" else path.name
        for path in (ROOT / "src").iterdir()
        if (path.suffix == ".py" or (path / "__init__.py").is_file())
        and path.stem != "sitecustomize"
    }
)


def _by_path_nodes(node: ast.AST) -> Iterator[ast.AST]:
    """The nodes a by-path launch can execute.

    An ``if __package__:`` body runs only when the file is imported as a
    package module, never when it is launched by path.
    """
    yield node
    if (
        isinstance(node, ast.If)
        and isinstance(node.test, ast.Name)
        and node.test.id == "__package__"
    ):
        children: Iterable[ast.AST] = node.orelse
    else:
        children = ast.iter_child_nodes(node)
    for child in children:
        yield from _by_path_nodes(child)


def _imports_repository_code(tree: ast.Module) -> bool:
    for node in _by_path_nodes(tree):
        if isinstance(node, ast.ImportFrom) and node.module == "tools.import_file":
            continue
        heads = _module_heads(node) if isinstance(node, ast.stmt) else []
        if any(head is None or head in _REPOSITORY_PACKAGES for head in heads):
            return True
    return False


def _bootstrap_failure(script: Path, *, root: Path = ROOT) -> str | None:
    """Why ``script``, launched by path, could import repository code unbound.

    Every tool entrypoint binds the repository before its first repository
    import; the canonical root insertion that reaches the binder is its only
    module-level sys.path edit.
    """
    tree = ast.parse(script.read_text(encoding="utf-8"))
    if not _is_entrypoint(tree) or not _imports_repository_code(tree):
        return None
    root_value = (
        f"Path(__file__).resolve().parents[{len(script.relative_to(root).parts) - 1}]"
    )
    for node in tree.body:
        if isinstance(node, (ast.Expr, ast.Assign)) and ast.unparse(node.value) == (
            "bind_repository_imports(__file__)"
        ):
            return None
        if _is_root_insertion(node):
            continue
        if isinstance(node, ast.Assign) and [
            ast.unparse(target) for target in node.targets
        ] == ["_REPO_ROOT"]:
            if ast.unparse(node.value) != root_value:
                return f"line {node.lineno}: _REPO_ROOT must be {root_value}"
            continue
        if isinstance(node, ast.ImportFrom) and node.module == "tools.import_file":
            continue
        for head in _module_heads(node):
            if head is None or head not in _STDLIB:
                return f"line {node.lineno}: import before the repository binding"
        if _edits_sys_path(node):
            return f"line {node.lineno}: sys.path edit outside the bootstrap"
    return "no repository binding"


def test_binding_a_by_path_tool_drops_tools_directories_from_sys_path(
    tmp_path: Path,
) -> None:
    # A by-path launch puts tools/ first, where tools/profile.py shadows the
    # stdlib module; binding must leave only package-qualified tool imports.
    script = ROOT / "tools" / "frontend_hot_pass_profile.py"
    code = (
        "import sys\n"
        "from pathlib import Path\n"
        "script = Path(sys.argv[1])\n"
        "sys.path[0] = str(script.parent)\n"
        "sys.path.insert(0, str(script.parents[1]))\n"
        "from tools.import_file import bind_repository_imports\n"
        "bind_repository_imports(script)\n"
        "import profile\n"
        "print(profile.__file__)\n"
        "print(any(Path(p or '.').resolve().is_relative_to(script.parent)"
        " for p in sys.path))\n"
    )
    result = run_guarded_test_process(
        [sys.executable, "-I", "-c", code, str(script)],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    profile_file, tools_on_path = result.stdout.split()
    assert Path(profile_file).parent != ROOT / "tools"
    assert tools_on_path == "False"


def test_every_host_entrypoint_binds_the_repository_first() -> None:
    failures = {
        script.relative_to(ROOT).as_posix(): failure
        for script in _repository_python(*_HOST_SCRIPT_ROOTS)
        if (failure := _bootstrap_failure(script)) is not None
    }
    assert not failures, failures


def _bare_tool_imports(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    return [
        f"line {node.lineno}: {ast.unparse(node)}"
        for node in ast.walk(tree)
        if isinstance(node, (ast.Import, ast.ImportFrom))
        and any(head in _TOOLS_TOP for head in _module_heads(node))
    ]


def test_tools_have_one_import_identity() -> None:
    """Repository tools import each other only as ``tools.<module>``.

    A bare sibling import resolves only while ``tools/`` is on sys.path, so a
    module would load under two names depending on how it was launched.
    """
    failures = {
        path.relative_to(ROOT).as_posix(): bare
        for path in _repository_python(
            *_HOST_SCRIPT_ROOTS, "tests", "src", "demos", "demo", "examples"
        )
        if not path.is_relative_to(ROOT / "tests" / "differential")
        and not path.is_relative_to(ROOT / "src" / "molt" / "stdlib")
        and (bare := _bare_tool_imports(path))
    }
    assert not failures, failures


def test_host_libraries_never_edit_sys_path_at_import() -> None:
    failures = []
    for path in _repository_python(*_HOST_SCRIPT_ROOTS):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        if _is_entrypoint(tree):
            continue
        failures.extend(
            f"{path.relative_to(ROOT).as_posix()}:{node.lineno}"
            for node in tree.body
            if not isinstance(
                node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
            )
            and _edits_sys_path(node)
        )
    assert failures == []


@pytest.mark.parametrize(
    "relative",
    [
        "tools/release/build_bundle.py",
        "tools/release/release_authority.py",
        "tools/release/verify_consumer.py",
        "tools/release/provision_execution_archives.py",
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
    failure = _bootstrap_failure(script, root=tmp_path.parent)
    assert (failure is not None) is is_entrypoint
    if is_entrypoint:
        assert failure == "line 1: import before the repository binding"


def test_tool_module_name_is_the_dotted_tools_path() -> None:
    tools_root = ROOT / "tools"
    assert tool_module_name(tools_root / "import_file.py") == "tools.import_file"
    assert tool_module_name(tools_root / "compat" / "diff_results.py") == (
        "tools.compat.diff_results"
    )
    assert tool_module_name(tools_root / "proof_queue_pkg" / "__init__.py") == (
        "tools.proof_queue_pkg"
    )
    assert tool_module_name(ROOT / "src" / "molt" / "dx.py") is None
    assert tool_module_name(ROOT / "tests" / "tools" / "test_import_file.py") is None
    assert tool_module_name(tools_root / "proof_plan.toml") is None


def test_test_session_refuses_a_second_tool_identity() -> None:
    source = ROOT / "tools" / "import_file.py"
    with pytest.raises(ImportError, match="import tools.import_file instead"):
        importlib.util.spec_from_file_location("private_binder", source)
    assert importlib.util.spec_from_file_location("tools.import_file", source)
    with pytest.raises(ImportError, match="import tools.import_file instead"):
        load_module_from_path("private_binder", source)
    assert "private_binder" not in sys.modules


def test_molt_module_name_is_the_dotted_host_package_path() -> None:
    molt_root = ROOT / "src" / "molt"
    assert molt_module_name(molt_root / "dx.py") == "molt.dx"
    assert molt_module_name(molt_root / "cli" / "__init__.py") == "molt.cli"
    assert molt_module_name(
        molt_root / "frontend" / "lowering" / "op_kinds_generated.py"
    ) == ("molt.frontend.lowering.op_kinds_generated")
    # Stdlib sources are modules of compiled programs, not of the host package.
    assert molt_module_name(molt_root / "stdlib" / "inspect.py") is None
    assert molt_module_name(ROOT / "tools" / "import_file.py") is None


def test_test_session_refuses_a_second_molt_identity() -> None:
    source = ROOT / "src" / "molt" / "target_python.py"
    with pytest.raises(ImportError, match="import molt.target_python instead"):
        importlib.util.spec_from_file_location("private_target_python", source)
    assert importlib.util.spec_from_file_location("molt.target_python", source)
    with pytest.raises(ImportError, match="import molt.target_python instead"):
        load_module_from_path("private_target_python", source)
    assert "private_target_python" not in sys.modules
    # A stdlib source keeps whatever name its test gives it.
    stdlib = ROOT / "src" / "molt" / "stdlib" / "inspect.py"
    assert importlib.util.spec_from_file_location("molt_test_inspect", stdlib)


def test_neutral_authorities_declare_themselves() -> None:
    from tools.import_file import _declares_neutral_authority

    molt_root = ROOT / "src" / "molt"
    declared = {
        path.relative_to(molt_root).as_posix()
        for path in molt_root.rglob("*.py")
        if not path.is_relative_to(molt_root / "stdlib")
        and "__molt_neutral_authority__" in path.read_text(encoding="utf-8")
        and _declares_neutral_authority(path)
    }
    # Each is selected by exact file before any ``molt`` import: package
    # admission by name, and the process guard and the Cargo policy, each with
    # its sibling, per checkout.
    assert declared == {
        "package_import_custody.py",
        "process_guard.py",
        "cargo_execution_policy.py",
        "process_environment.py",
    }
    assert importlib.util.spec_from_file_location(
        "_molt_package_import_custody", molt_root / "package_import_custody.py"
    )


@pytest.mark.parametrize(
    ("declaration", "neutral"),
    [
        ("__molt_neutral_authority__ = True\n", True),
        ("__molt_neutral_authority__ = 1\n", False),
        ("__molt_neutral_authority__ = flag\n", False),
        ("def f():\n    __molt_neutral_authority__ = True\n", False),
        ("x = 1\n", False),
    ],
)
def test_neutral_authority_is_a_module_level_true_literal(
    tmp_path: Path, declaration: str, neutral: bool
) -> None:
    from tools.import_file import _declares_neutral_authority

    source = tmp_path / "candidate.py"
    source.write_text(declaration, encoding="utf-8")
    assert _declares_neutral_authority(source) is neutral


def test_package_only_imports_do_not_need_a_by_path_binding() -> None:
    # The proof custody bootstrap runs inside payload children and must not
    # bind the repository; its relative import runs only under a package import.
    package_only = ast.parse(
        "if __package__:\n    from .authority import check\nelse:\n    check = None\n"
    )
    by_path = ast.parse(
        "if __package__:\n    pass\nelse:\n    from tools import authority\n"
    )
    assert not _imports_repository_code(package_only)
    assert _imports_repository_code(by_path)


def test_bootstrap_scan_accepts_only_the_canonical_binding(tmp_path: Path) -> None:
    canonical = (
        "import sys\n"
        "from pathlib import Path\n"
        "_REPO_ROOT = Path(__file__).resolve().parents[1]\n"
        "if str(_REPO_ROOT) not in sys.path:\n"
        "    sys.path.insert(0, str(_REPO_ROOT))\n"
        "from tools.import_file import bind_repository_imports\n"
        "bind_repository_imports(__file__)\n"
        "from tools import bench\n"
        'if __name__ == "__main__":\n'
        "    bench.main()\n"
    )
    script = tmp_path / "command.py"
    script.write_text(canonical, encoding="utf-8")
    assert _bootstrap_failure(script, root=tmp_path.parent) is None
    script.write_text(canonical.replace("parents[1]", "parents[2]"), encoding="utf-8")
    assert _bootstrap_failure(script, root=tmp_path.parent) == (
        "line 3: _REPO_ROOT must be Path(__file__).resolve().parents[1]"
    )
    hand_rolled = canonical.replace(
        "if str(_REPO_ROOT) not in sys.path:\n    sys.path.insert(0, str(_REPO_ROOT))",
        "sys.path.insert(0, str(Path(__file__).parent))",
    )
    script.write_text(hand_rolled, encoding="utf-8")
    assert (
        _bootstrap_failure(script, root=tmp_path.parent)
        == "line 4: sys.path edit outside the bootstrap"
    )
    script.write_text(
        'from tools import bench\nif __name__ == "__main__":\n    bench.main()\n',
        encoding="utf-8",
    )
    assert (
        _bootstrap_failure(script, root=tmp_path.parent)
        == "line 1: import before the repository binding"
    )


def test_loading_the_harness_guard_keeps_tools_off_sys_path(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import process_guard

    tools_root = (ROOT / "tools").resolve()

    def inside_tools(item: str) -> bool:
        return Path(item or ".").resolve().is_relative_to(tools_root)

    monkeypatch.setattr(
        sys, "path", [item for item in sys.path if not inside_tools(item)]
    )
    process_guard.load_harness_memory_guard(None)

    # tools/profile.py on sys.path would shadow the stdlib `profile` (cProfile).
    assert [item for item in sys.path if inside_tools(item)] == []


def test_the_cleanup_tool_loads_under_its_one_identity() -> None:
    from molt.cli import maintenance
    from tools import artifact_cleanup

    assert maintenance._load_artifact_cleanup_module(ROOT) is artifact_cleanup
