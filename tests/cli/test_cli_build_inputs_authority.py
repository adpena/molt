from __future__ import annotations

from molt.compilation_target import admit_target
from molt.cli.config_resolution import _select_codegen_backend

import inspect
import importlib
import os
import sys
from importlib.machinery import PathFinder
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import build_inputs
from molt.cli.python_import_topology import (
    PythonImportTopology,
    PythonImportTopologyChangedError,
)
from molt.cli import module_resolution
from tests.process_guard_common import run_guarded_test_process

_BUILD_INPUT_NAMES = (
    "_VALID_AUDIT_SINKS",
    "_binary_image_kind",
    "_build_config_entry_value",
    "_collect_env_overrides",
    "_is_stdlib_path",
    "_latest_mtime",
    "_merge_module_graph_with_reason",
    "_parse_audit_log_flag",
    "_parse_io_mode_flag",
    "_prepare_build_config",
    "_prepare_build_inputs",
    "_prepare_build_preamble",
    "_prepare_build_roots",
    "_resolve_build_entry",
    "_resolve_build_entry_selector",
    "_resolve_entry_module",
    "_resolve_module_root_resolution",
    "_resolve_module_roots",
    "_resolve_wrapper_build_entry",
    "_configured_build_entry_selector",
    "_entry_value_looks_like_path",
    "_entry_search_root",
)


def test_cli_build_inputs_authority_is_single_home() -> None:
    for name in _BUILD_INPUT_NAMES:
        assert hasattr(build_inputs, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _BUILD_INPUT_NAMES:
        assert f"def {name}(" not in cli_source
    assert "_VALID_AUDIT_SINKS =" not in cli_source


def _select_entry(
    project: Path, *, module: str | None = None, file: str | None = None, env=None
):
    entry, error = build_inputs._resolve_build_entry(
        file_path=file,
        module=module,
        project_root=project,
        cwd_root=project,
        stdlib_root=project / "stdlib",
        respect_pythonpath=False,
        json_output=False,
        env={"MOLT_HERMETIC_MODULE_ROOTS": "1", **(env or {})},
        source_cwd=project,
    )
    assert error is None and entry is not None
    return entry


def test_entry_module_selection_retains_earlier_shadow_misses(tmp_path: Path) -> None:
    project, earlier, later = (
        tmp_path / "project",
        tmp_path / "earlier",
        tmp_path / "later",
    )
    project.mkdir()
    earlier.mkdir()
    later.mkdir()
    source = later / "selected_entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    entry = _select_entry(
        project,
        module="selected_entry",
        env={"MOLT_MODULE_ROOTS": os.pathsep.join(map(str, (earlier, later)))},
    )
    assert entry.source_path == source
    shadow = earlier / source.name
    shadow.write_bytes(source.read_bytes())
    importlib.invalidate_caches()
    observed = PathFinder.find_spec("selected_entry", [str(earlier), str(later)])
    assert observed is not None and observed.origin == str(shadow)
    with pytest.raises(PythonImportTopologyChangedError):
        entry.import_topology.validate()


@pytest.mark.parametrize(
    "mode",
    [
        "script",
        "package-script",
        "package",
        "submodule",
        "external",
        "missing",
        "symlink",
    ],
)
def test_entry_search_root_is_cpython_sys_path_zero(tmp_path: Path, mode: str) -> None:
    project = tmp_path / "project"
    package = project / "probe_pkg"
    package.mkdir(parents=True)
    (project / "pyproject.toml").write_text("", encoding="utf-8")
    (package / "__init__.py").write_text("", encoding="utf-8")
    code = "import sys\nprint(sys.path[0])\n"
    for path in (project / "entry.py", package / "__main__.py", package / "sub.py"):
        path.write_text(code, encoding="utf-8")
    outside = tmp_path / "outside"
    outside.mkdir()
    cwd = outside if mode in {"external", "missing"} else project
    env = {"MOLT_HERMETIC_MODULE_ROOTS": "1"}
    oracle_env = dict(os.environ)
    oracle_env.pop("PYTHONPATH", None)
    module = None
    file = None
    if mode in {"package", "submodule", "external", "missing"}:
        module = "probe_pkg.sub" if mode == "submodule" else "probe_pkg"
        args = ["-m", module]
        if mode == "external":
            env["MOLT_MODULE_ROOTS"] = str(project)
            oracle_env["PYTHONPATH"] = str(project)
    else:
        source = project / "entry.py" if mode == "script" else package / "sub.py"
        if mode == "symlink":
            alias = outside / "entry.py"
            try:
                alias.symlink_to(source)
            except OSError as exc:
                pytest.skip(f"host cannot create source alias: {exc}")
            source = alias
        file = str(source)
        args = [file]
    oracle = run_guarded_test_process(
        [sys.executable, "-s", "-B", *args], cwd=cwd, env=oracle_env
    )
    entry, error = build_inputs._resolve_build_entry(
        file_path=file,
        module=module,
        project_root=project,
        cwd_root=project,
        stdlib_root=module_resolution._stdlib_root_path(),
        respect_pythonpath=False,
        json_output=False,
        source_cwd=cwd,
        env=env,
    )
    if mode == "missing":
        assert oracle.returncode != 0
        assert "No module named probe_pkg" in oracle.stderr
        assert entry is None and error is not None
        return
    assert oracle.returncode == 0, oracle.stderr
    assert error is None and entry is not None
    search_root = Path(oracle.stdout.strip())
    assert entry.module_roots[0] == search_root
    assert search_root in entry.program_module_roots
    assert entry.image_scope is not None
    assert entry.image_scope.module_roots == tuple(entry.module_roots)
    assert entry.image_scope.diagnostic_payload()["module_roots"] == [
        str(root) for root in entry.module_roots
    ]
    if module is not None:
        assert package in entry.program_module_roots
        assert package not in entry.module_roots
    elif mode != "script":
        assert project not in entry.module_roots
        # Ownership must survive even though this root cannot resolve imports.
        assert project in entry.program_module_roots


def test_entry_search_root_precedes_explicit_roots(tmp_path: Path) -> None:
    cwd = tmp_path / "cwd"
    external = tmp_path / "external"
    for directory in (cwd, external):
        directory.mkdir()
        (directory / "entry.py").write_text("pass\n", encoding="utf-8")
    entry = _select_entry(cwd, module="entry", env={"MOLT_MODULE_ROOTS": str(external)})
    assert entry.source_path == cwd / "entry.py"


def test_entry_package_selection_retains_missing_main(tmp_path: Path) -> None:
    package = tmp_path / "selected_entry"
    package.mkdir()
    initializer = package / "__init__.py"
    initializer.write_text("VALUE = 1\n", encoding="utf-8")
    assert PathFinder.find_spec("selected_entry.__main__", [str(package)]) is None
    entry = _select_entry(tmp_path, module="selected_entry")
    assert entry.source_path == initializer
    main = package / "__main__.py"
    main.write_bytes(initializer.read_bytes())
    importlib.invalidate_caches()
    observed = PathFinder.find_spec("selected_entry.__main__", [str(package)])
    assert observed is not None and observed.origin == str(main)
    with pytest.raises(PythonImportTopologyChangedError):
        entry.import_topology.validate()


def test_file_entry_keeps_lexical_alias_selection(tmp_path: Path) -> None:
    first, second = tmp_path / "first.py", tmp_path / "second.py"
    first.write_text("VALUE = 1\n", encoding="utf-8")
    second.write_bytes(first.read_bytes())
    selected = tmp_path / "selected.py"
    try:
        selected.symlink_to(first)
    except OSError as exc:
        pytest.skip(f"host cannot create source alias: {exc}")
    entry = _select_entry(tmp_path, file=str(selected))
    assert entry.source_path == first
    selected.unlink()
    selected.symlink_to(second)
    assert selected.read_bytes() == entry.entry_snapshot.content
    with pytest.raises(PythonImportTopologyChangedError):
        entry.import_topology.validate()


def test_entry_selection_ignores_unrelated_build_output(tmp_path: Path) -> None:
    source = tmp_path / "selected_entry.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    entry = _select_entry(tmp_path, module="selected_entry")
    (tmp_path / "build-output").mkdir()
    (tmp_path / "diagnostics.log").write_text("done\n", encoding="utf-8")
    entry.import_topology.validate()


@pytest.mark.parametrize(
    "relative",
    [
        "src",
        "vendor/packages",
        "vendor/local",
        ".venv/lib/python3.12/site-packages",
        ".molt-venv/lib/python3.12/site-packages",
        ".venv/Lib/site-packages",
    ],
)
def test_root_selection_retains_missing_root_candidates(
    tmp_path: Path, relative: str
) -> None:
    owner = PythonImportTopology()
    before = build_inputs._resolve_module_root_resolution(
        tmp_path,
        tmp_path,
        entry_search_root=tmp_path,
        respect_pythonpath=False,
        env={},
        import_topology=owner,
    )
    added = tmp_path / relative
    assert added not in before.roots
    added.mkdir(parents=True)
    after = build_inputs._resolve_module_root_resolution(
        tmp_path, tmp_path, entry_search_root=tmp_path, respect_pythonpath=False, env={}
    )
    assert added in after.roots
    with pytest.raises(PythonImportTopologyChangedError):
        owner.validate()


def test_relative_root_alias_uses_request_cwd_and_retains_selector(
    tmp_path: Path, monkeypatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    vendor = project / "vendor_package"
    vendor.mkdir()
    (vendor / "__init__.py").write_text("pass\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    owner = PythonImportTopology()
    selected = build_inputs._resolve_module_root_resolution(
        project,
        project,
        entry_search_root=project,
        respect_pythonpath=False,
        env={
            "MOLT_MODULE_ROOTS": "logical.vendor=vendor_package",
            "MOLT_HERMETIC_MODULE_ROOTS": "1",
        },
        source_cwd=project,
        import_topology=owner,
    )
    assert selected.missing_env_roots == ()
    assert vendor in selected.roots
    assert selected.module_root_aliases == (("logical.vendor", vendor),)
    owner.validate()


def test_entry_alias_uses_explicit_environment_and_request_cwd(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    selected = project / "selected"
    ambient = tmp_path / "ambient"
    for folder, value in ((selected, 1), (ambient, 2)):
        folder.mkdir()
        (folder / "__init__.py").write_text(f"VALUE = {value}\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("MOLT_MODULE_ROOTS", f"logical.vendor={ambient}")
    entry = _select_entry(
        project,
        module="logical.vendor",
        env={"MOLT_MODULE_ROOTS": "logical.vendor=selected"},
    )
    assert entry.source_path == selected / "__init__.py"
    assert entry.entry_source == "VALUE = 1\n"
    assert entry.module_root_aliases == (("logical.vendor", selected),)
    assert entry.entry_module == "logical.vendor"
    assert str(ambient) not in repr(entry.module_roots)
    entry.import_topology.validate()


def test_file_entry_name_uses_explicit_alias_descriptors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    selected = tmp_path / "selected"
    selected.mkdir()
    source = selected / "__init__.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_MODULE_ROOTS", f"ambient.name={selected}")
    entry = _select_entry(
        tmp_path,
        file=str(source),
        env={"MOLT_MODULE_ROOTS": f"selected.name={selected}"},
    )
    assert entry.entry_module == "selected.name"
    assert entry.module_root_aliases == (("selected.name", selected),)


@pytest.mark.parametrize("existing_version", [False, True])
def test_venv_root_selection_custodies_version_and_site_misses(
    tmp_path: Path, existing_version: bool
) -> None:
    library = tmp_path / ".venv" / "lib"
    library.mkdir(parents=True)
    version = library / "python3.13"
    if existing_version:
        version.mkdir()
    owner = PythonImportTopology()
    before = build_inputs._resolve_module_root_resolution(
        tmp_path,
        tmp_path,
        entry_search_root=tmp_path,
        respect_pythonpath=False,
        env={},
        import_topology=owner,
    )
    site = version / "site-packages"
    assert site not in before.roots
    site.mkdir(parents=True)
    after = build_inputs._resolve_module_root_resolution(
        tmp_path, tmp_path, entry_search_root=tmp_path, respect_pythonpath=False, env={}
    )
    assert site in after.roots
    with pytest.raises(PythonImportTopologyChangedError):
        owner.validate()


@pytest.mark.parametrize(
    "target, expected",
    [
        ("wasm", "wasi"),
        ("wasm-freestanding", "wasi"),
        ("wasm32-unknown-unknown", "wasi"),
        ("wasm32-wasip1", "wasi"),
        ("rust", None),
        ("luau", None),
        ("mlir", None),
        ("aarch64-apple-darwin", "darwin"),
        ("x86_64-unknown-linux-gnu", "linux"),
        ("aarch64-unknown-linux-musl", "linux"),
        ("x86_64-pc-windows-msvc", "win32"),
        ("aarch64-pc-windows-msvc", "win32"),
        ("x86_64-pc-windows-gnu", "win32"),
    ],
)
def test_frontend_platform_fact_uses_selected_runtime_target(target, expected):
    assert (
        build_inputs._target_sys_platform(_select_codegen_backend(target, "auto")[0])
        == expected
    )


@pytest.mark.parametrize("target", ["native", "llvm"])
def test_frontend_native_platform_fact_uses_the_actual_host(target):
    assert (
        build_inputs._target_sys_platform(_select_codegen_backend(target, "auto")[0])
        == sys.platform
    )


@pytest.mark.parametrize("target", ["native", "llvm"])
def test_native_build_configuration_preserves_host_platform(tmp_path, target):
    from molt.backend_environment import CodegenSelection

    config, failure = build_inputs._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=True,
        target=_select_codegen_backend(target, "auto")[0],
        profile="dev",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
        codegen=CodegenSelection("llvm" if target == "llvm" else "cranelift"),
    )
    assert failure is None
    assert config is not None
    assert config.target_sys_platform == sys.platform
    assert config.codegen.backend == ("llvm" if target == "llvm" else "cranelift")


def test_frontend_platform_fact_refuses_an_unadmitted_target():
    with pytest.raises(ValueError, match="no object-format policy"):
        build_inputs._target_sys_platform(admit_target("x86_64-notlinux-unknown"))
