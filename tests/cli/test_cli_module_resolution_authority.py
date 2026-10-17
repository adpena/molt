from __future__ import annotations

import inspect
import os
import json
import sys
from types import SimpleNamespace
from pathlib import Path

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_resolution
from molt.cli import python_import_topology as topology
from molt.cli.python_import_resolution import (
    LocalPythonModuleResolver,
    python_package_child_names,
)
from tests.process_guard_common import install_module_os_view, run_guarded_test_process

import pytest

_MODULE_RESOLUTION_NAMES = (
    "_ModuleResolutionCache",
    "_case_exact_file",
    "_entry_module_root_for_path",
    "_has_namespace_dir",
    "_is_runtime_owned_module_path",
    "_is_stdlib_module",
    "_is_stdlib_resolved_path",
    "_module_name_from_path",
    "_module_name_from_relative_parts",
    "_module_name_from_resolved_path",
    "_relative_parts_if_within",
    "_resolve_module_path",
    "_resolve_module_path_parts",
    "_roots_for_module",
    "_runtime_owned_module_roots",
    "_stdlib_root_path",
)

_MODULE_RESOLUTION_DEFINITIONS = (
    "class _ModuleResolutionCache",
    "def _case_exact_file(",
    "def _entry_module_root_for_path(",
    "def _has_namespace_dir(",
    "def _is_runtime_owned_module_path(",
    "def _is_stdlib_module(",
    "def _is_stdlib_resolved_path(",
    "def _module_name_from_path(",
    "def _module_name_from_relative_parts(",
    "def _module_name_from_resolved_path(",
    "def _relative_parts_if_within(",
    "def _resolve_module_path(",
    "def _resolve_module_path_parts(",
    "def _roots_for_module(",
    "def _runtime_owned_module_roots(",
    "def _stdlib_root_path(",
)


def test_cli_module_resolution_authority_is_single_home() -> None:
    for name in _MODULE_RESOLUTION_NAMES:
        assert hasattr(module_resolution, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_RESOLUTION_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source


def test_stdlib_root_path_is_package_local_not_cwd(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("MOLT_PROJECT_ROOT", raising=False)

    stdlib_root = module_resolution._stdlib_root_path()

    assert stdlib_root.name == "stdlib"
    assert stdlib_root.parent.name == "molt"
    assert (stdlib_root / "importlib" / "__init__.py").exists()
    assert module_resolution._resolve_module_path("importlib", [stdlib_root]) == (
        stdlib_root / "importlib" / "__init__.py"
    )
    assert not stdlib_root.is_relative_to(tmp_path)


def test_fresh_resolution_observes_a_new_file_with_restored_directory_mtime(
    tmp_path: Path,
) -> None:
    assert module_resolution._resolve_module_path("later", [tmp_path]) is None
    before = tmp_path.stat()
    source = tmp_path / "later.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    os.utime(tmp_path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert module_resolution._resolve_module_path("later", [tmp_path]) == source


def _cpython_origin(module: str, roots: list[Path]) -> str | None:
    result = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\n"
            "from importlib.machinery import PathFinder\n"
            "locations = json.loads(sys.argv[2])\n"
            "spec = None\n"
            "parts = sys.argv[1].split('.')\n"
            "for index in range(len(parts)):\n"
            "    if locations is None:\n"
            "        spec = None\n"
            "        break\n"
            "    spec = PathFinder.find_spec('.'.join(parts[:index+1]), locations)\n"
            "    if spec is None:\n"
            "        break\n"
            "    locations = spec.submodule_search_locations\n"
            "print(json.dumps(None if spec is None else spec.origin))\n",
            module,
            json.dumps([str(root) for root in roots]),
        ],
        timeout=10,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def test_negative_resolution_and_earlier_shadow_cannot_cross_topology_generation(
    tmp_path,
):
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()
    old = second / "item.py"
    old.write_text("VALUE = 1\n", encoding="utf-8")
    cache = module_resolution._ModuleResolutionCache()
    assert (
        cache.resolve_module("item", [first, second], tmp_path / "stdlib", set()) == old
    )
    assert (
        cache.resolve_module("missing", [first, second], tmp_path / "stdlib", set())
        is None
    )
    metadata = first.stat()
    shadow = first / "item.py"
    shadow.write_bytes(old.read_bytes())
    (first / "missing.py").write_text("pass\n", encoding="utf-8")
    os.utime(first, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    assert _cpython_origin("item", [first, second]) == str(shadow)
    assert _cpython_origin("missing", [first, second]) == str(first / "missing.py")
    with pytest.raises(topology.PythonImportTopologyChangedError):
        cache.import_topology.validate()
    shadow.unlink()
    (first / "missing.py").unlink()
    # Failure is terminal, even if a caller restores the former directory.
    with pytest.raises(topology.PythonImportTopologyChangedError):
        cache.import_topology.validate()
    with pytest.raises(topology.PythonImportTopologyChangedError):
        cache.import_topology.is_file(old)


@pytest.mark.parametrize("mutation", ["add", "remove", "rename"])
def test_package_child_generation_is_custodied(tmp_path, mutation):
    source = tmp_path / "before.py"
    source.write_text("pass\n", encoding="utf-8")
    owner = topology.PythonImportTopology()
    assert python_package_child_names((tmp_path,), import_topology=owner) == {"before"}
    metadata = tmp_path.stat()
    if mutation == "add":
        (tmp_path / "after.py").write_text("pass\n", encoding="utf-8")
    elif mutation == "remove":
        source.unlink()
    else:
        source.rename(tmp_path / "after.py")
    os.utime(tmp_path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    with pytest.raises(topology.PythonImportTopologyChangedError):
        owner.validate()
    expected = str(source) if mutation == "add" else None
    assert _cpython_origin("before", [tmp_path]) == expected


@pytest.mark.parametrize("policy", ["racy", "windows", "unavailable_change_time"])
def test_coarse_directory_metadata_never_certifies_stale_child_names(
    tmp_path, monkeypatch, policy
):
    if policy == "windows":
        # Change only this module's platform view; pathlib keeps the real host.
        monkeypatch.setattr(
            topology, "os", SimpleNamespace(name="nt", path=os.path, scandir=os.scandir)
        )
        monkeypatch.setattr(topology, "DIGEST_RACY_WINDOW_NS", 0)
    elif policy == "racy":
        monkeypatch.setattr(topology, "DIGEST_RACY_WINDOW_NS", 10**30)
    else:
        monkeypatch.setattr(topology, "content_change_time_ns", lambda *_args: None)
    real_state = topology._directory_state
    frozen = real_state(tmp_path)
    monkeypatch.setattr(
        topology,
        "_directory_state",
        lambda path: frozen if path == tmp_path else real_state(path),
    )
    owner = topology.PythonImportTopology()
    assert python_package_child_names((tmp_path,), import_topology=owner) == set()
    added = tmp_path / "added.py"
    added.write_text("pass\n", encoding="utf-8")
    assert _cpython_origin("added", [tmp_path]) == str(added)
    with pytest.raises(topology.PythonImportTopologyChangedError):
        owner.validate()


def test_unrelated_build_output_does_not_change_a_single_name_lookup(tmp_path):
    source = tmp_path / "item.py"
    source.write_text("pass\n", encoding="utf-8")
    cache = module_resolution._ModuleResolutionCache()
    assert (
        cache.resolve_module("item", [tmp_path], tmp_path / "stdlib", set()) == source
    )
    (tmp_path / "build-output").mkdir()
    (tmp_path / "unrelated.log").write_text("diagnostic\n", encoding="utf-8")
    cache.import_topology.validate()
    assert (
        cache.resolve_module("item", [tmp_path], tmp_path / "stdlib", set()) == source
    )


@pytest.mark.parametrize("alias", [False, True])
def test_root_alias_retarget_is_not_hidden_by_identical_source_bytes(
    tmp_path, monkeypatch, alias
):
    roots = [tmp_path / "first", tmp_path / "second"]
    for root in roots:
        root.mkdir()
        (root / "item.py").write_text("VALUE = 1\n", encoding="utf-8")
    selected = tmp_path / "selected"
    try:
        selected.symlink_to(roots[0], target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"host cannot create directory alias: {exc}")
    if alias:
        monkeypatch.setenv("MOLT_MODULE_ROOTS", f"chosen={selected}")
    cache = module_resolution._ModuleResolutionCache()
    name = "chosen.item" if alias else "item"
    assert (
        cache.resolve_module(name, [selected], tmp_path / "stdlib", set()) is not None
    )
    selected.unlink()
    selected.symlink_to(roots[1], target_is_directory=True)
    assert _cpython_origin("item", [selected]) == str(selected / "item.py")
    with pytest.raises(topology.PythonImportTopologyChangedError):
        cache.import_topology.validate()


def test_local_tooling_resolver_misses_use_same_topology_owner(tmp_path):
    resolver = LocalPythonModuleResolver((tmp_path,))
    assert resolver.source_for_module("later") is None
    source = tmp_path / "later.py"
    source.write_text("pass\n", encoding="utf-8")
    assert _cpython_origin("later", [tmp_path]) == str(source)
    with pytest.raises(topology.PythonImportTopologyChangedError):
        resolver.import_topology.validate()
    assert LocalPythonModuleResolver((tmp_path,)).source_for_module("later") == source


def test_exact_name_queries_share_one_listing_without_walking_subtrees(
    tmp_path, monkeypatch
):
    for index in range(12):
        (tmp_path / f"member{index}.py").write_text("pass\n", encoding="utf-8")
    untouched = tmp_path / "unrelated"
    untouched.mkdir()
    (untouched / "nested.py").write_text("pass\n", encoding="utf-8")
    reads = []
    real_scandir = topology.os.scandir

    def record(path):
        reads.append(Path(path))
        return real_scandir(path)

    # A private os view keeps the probe off other threads' directory scans.
    install_module_os_view(monkeypatch, topology)
    monkeypatch.setattr(topology.os, "scandir", record)
    cache = module_resolution._ModuleResolutionCache()
    for index in range(12):
        assert (
            cache.resolve_module(
                f"member{index}", [tmp_path], tmp_path / "stdlib", set()
            )
            == tmp_path / f"member{index}.py"
        )
    assert reads == [tmp_path]
    cache.import_topology.validate()
    assert untouched not in reads


@pytest.mark.parametrize("parent_kind", ["package", "module"])
def test_parent_shadow_generation_matches_segmented_pathfinder(tmp_path, parent_kind):
    roots = [tmp_path / "first", tmp_path / "second"]
    for root in roots:
        (root / "pkg").mkdir(parents=True)
    child = roots[1] / "pkg" / "child.py"
    child.write_text("pass\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    cache = module_resolution._ModuleResolutionCache()
    assert _cpython_origin("pkg.child", roots) == str(child)
    assert cache.resolve_module("pkg.child", roots, stdlib, set()) == child
    assert cache.package_locations("pkg", roots, stdlib, set()) == tuple(
        root / "pkg" for root in roots
    )
    assert cache.has_namespace_dir("pkg", roots, stdlib, set())
    parent = (
        roots[0] / "pkg" / "__init__.py"
        if parent_kind == "package"
        else roots[0] / "pkg.py"
    )
    metadata = parent.parent.stat()
    parent.write_text("pass\n", encoding="utf-8")
    os.utime(parent.parent, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    assert _cpython_origin("pkg.child", roots) is None
    with pytest.raises(topology.PythonImportTopologyChangedError):
        cache.import_topology.validate()
    fresh = module_resolution._ModuleResolutionCache()
    assert fresh.resolve_module("pkg.child", roots, stdlib, set()) is None
    assert fresh.resolve_module("pkg", roots, stdlib, set()) == parent
    assert fresh.package_locations("pkg", roots, stdlib, set()) == (
        (roots[0] / "pkg",) if parent_kind == "package" else ()
    )
    assert not fresh.has_namespace_dir("pkg", roots, stdlib, set())


def test_regular_package_wins_module_and_later_namespace_portions(tmp_path):
    roots = [tmp_path / "first", tmp_path / "second"]
    for root in roots:
        (root / "pkg").mkdir(parents=True)
    initializer = roots[0] / "pkg" / "__init__.py"
    initializer.write_text("pass\n", encoding="utf-8")
    (roots[0] / "pkg.py").write_text("pass\n", encoding="utf-8")
    child = roots[0] / "pkg" / "child.py"
    child.write_text("pass\n", encoding="utf-8")
    (roots[1] / "pkg" / "other.py").write_text("pass\n", encoding="utf-8")
    cache = module_resolution._ModuleResolutionCache()
    stdlib = tmp_path / "stdlib"
    assert _cpython_origin("pkg", roots) == str(initializer)
    assert _cpython_origin("pkg.child", roots) == str(child)
    assert _cpython_origin("pkg.other", roots) is None
    assert cache.resolve_module("pkg", roots, stdlib, set()) == initializer
    assert cache.resolve_module("pkg.child", roots, stdlib, set()) == child
    assert cache.resolve_module("pkg.other", roots, stdlib, set()) is None
    assert cache.package_locations("pkg", roots, stdlib, set()) == (roots[0] / "pkg",)
    # Three names use one canonical per-root resolver, including its prefixes.
    assert len(cache.python_resolvers) == 1
    cache.import_topology.validate()


def test_logical_alias_prefix_and_namespace_source_fallthrough(tmp_path, monkeypatch):
    alias_root = tmp_path / "vendored"
    alias_root.mkdir()
    child = alias_root / "child.py"
    child.write_text("pass\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_MODULE_ROOTS", f"logical.vendor={alias_root}")
    cache = module_resolution._ModuleResolutionCache()
    stdlib = tmp_path / "stdlib"
    # An explicit alias supplies its prefix; physical logical/vendor parents
    # are intentionally unnecessary. Ordinary PathFinder gets no such policy.
    assert _cpython_origin("logical.vendor.child", [tmp_path]) is None
    assert (
        cache.resolve_module("logical.vendor.child", [tmp_path], stdlib, set()) == child
    )
    assert cache.package_locations("logical.vendor", [tmp_path], stdlib, set()) == (
        alias_root,
    )
    assert not (tmp_path / "logical").exists()

    ordinary = tmp_path / "ordinary"
    ordinary.mkdir()
    package = ordinary / "logical"
    package.mkdir()
    (package / "__init__.py").write_text("pass\n", encoding="utf-8")
    source = package / "vendor.py"
    source.write_text("pass\n", encoding="utf-8")
    fresh = module_resolution._ModuleResolutionCache()
    assert fresh.resolve_module("logical.vendor", [ordinary], stdlib, set()) == source
    assert fresh.package_locations("logical.vendor", [ordinary], stdlib, set()) == ()
    assert not fresh.has_namespace_dir("logical.vendor", [ordinary], stdlib, set())
    fresh.import_topology.validate()


def test_symlink_parent_selector_keeps_filesystem_traversal_order(
    tmp_path: Path,
) -> None:
    base = tmp_path / "base"
    elsewhere = tmp_path / "elsewhere"
    base.mkdir()
    (elsewhere / "deep").mkdir(parents=True)
    link = base / "link"
    try:
        link.symlink_to(elsewhere / "deep", target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"symlinks unavailable: {exc}")
    expected = elsewhere / "owner.py"
    expected.write_text("VALUE = 1\n", encoding="utf-8")
    (base / "owner.py").write_text("VALUE = 2\n", encoding="utf-8")
    root = link / ".."
    cache = module_resolution._ModuleResolutionCache()
    selected = cache.resolve_module("owner", [root], tmp_path / "stdlib", set())
    assert selected == Path(_cpython_origin("owner", [root])).resolve() == expected
    cache.import_topology.validate()
    other = tmp_path / "other"
    (other / "deep").mkdir(parents=True)
    (other / "owner.py").write_text("VALUE = 1\n", encoding="utf-8")
    link.unlink()
    link.symlink_to(other / "deep", target_is_directory=True)
    with pytest.raises(topology.PythonImportTopologyChangedError):
        cache.import_topology.validate()


def test_module_ast_digest_follows_source_content(tmp_path) -> None:
    # Finalization asks for every runtime-import owner's digest once per round,
    # and a resident process asks again each build. The memo must answer from
    # the source content alone: an unchanged source is not parsed again, and
    # any edit yields the digest of the edited source.
    import ast

    from molt.cli import module_resolution
    from molt.compiler_analysis.python_source_keys import python_ast_digest
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION as target

    path = tmp_path / "owner.py"
    path.write_text("value = 1\n", encoding="utf-8")
    first = module_resolution._ModuleResolutionCache().module_ast_digest(
        path, native_roots=(), target_python=target
    )
    assert first == python_ast_digest(ast.parse("value = 1\n"))

    fresh = module_resolution._ModuleResolutionCache()
    assert fresh.module_ast_digest(path, native_roots=(), target_python=target) == first
    assert fresh.source_parse_count == 0

    path.write_text("value = 2\n", encoding="utf-8")
    edited = fresh.module_ast_digest(path, native_roots=(), target_python=target)
    assert edited == python_ast_digest(ast.parse("value = 2\n")) != first
    assert fresh.source_parse_count == 1
