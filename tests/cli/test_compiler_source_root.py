from __future__ import annotations

import importlib
import json
from pathlib import Path

import pytest

from molt import source_root
from molt.cli import compiler_metadata, env_paths, project_roots


def _source_tree(root: Path) -> Path:
    for relative in (
        "Cargo.toml",
        "runtime/molt-runtime/Cargo.toml",
        "runtime/molt-backend/Cargo.toml",
        "src/molt/cli/__init__.py",
    ):
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    return root.resolve()


@pytest.fixture(autouse=True)
def _clear_root_caches():
    project_roots._find_molt_root_cached.cache_clear()
    project_roots._find_project_root_cached.cache_clear()
    yield
    project_roots._find_molt_root_cached.cache_clear()
    project_roots._find_project_root_cached.cache_clear()


def test_project_and_source_authorities_are_independent(tmp_path, monkeypatch):
    project = tmp_path / "guest"
    project.mkdir()
    source = _source_tree(tmp_path / "compiler")
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(project))
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(source))
    monkeypatch.chdir(project)

    assert project_roots._find_project_root(project / "main.py") == project
    assert project_roots._find_molt_root(project) == source
    assert compiler_metadata._compiler_root() == source
    assert project_roots._require_molt_root(source, True, "build") is None


def test_invalid_explicit_source_never_falls_back(tmp_path, monkeypatch, capsys):
    checkout = _source_tree(tmp_path / "checkout")
    missing = tmp_path / "missing"
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(checkout))
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(missing))

    assert source_root.compiler_source_root() == missing
    assert project_roots._find_molt_root(checkout) == missing
    assert project_roots._require_molt_root(missing, True, "build") == 2
    failure = json.loads(capsys.readouterr().out)
    assert str(missing) in failure["errors"][0]
    assert "MOLT_SOURCE_ROOT" in failure["errors"][0]
    assert not missing.exists()


def test_relative_source_selection_is_resolved_per_call(tmp_path, monkeypatch):
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()
    monkeypatch.setenv("MOLT_SOURCE_ROOT", "missing-source")
    for cwd in (first, second):
        monkeypatch.chdir(cwd)
        expected = cwd / "missing-source"
        assert source_root.compiler_source_root() == expected
        assert project_roots._find_molt_root(tmp_path) == expected


@pytest.mark.parametrize("error", [OSError("unreadable"), ValueError("drift")])
def test_installed_source_admission_fails_closed_without_repair(
    tmp_path, monkeypatch, capsys, error
):
    source = _source_tree(tmp_path / "source")
    before = {path.relative_to(source) for path in source.rglob("*")}

    class Installed:
        def verify_sources(self):
            raise error

    monkeypatch.setattr(project_roots, "installed_compiler", lambda root: Installed())
    assert project_roots._require_molt_root(source, True, "build") == 2
    failure = json.loads(capsys.readouterr().out)
    assert str(error) in failure["errors"][0]
    assert {path.relative_to(source) for path in source.rglob("*")} == before


def test_installed_source_admission_checks_the_selected_payload(tmp_path, monkeypatch):
    source = _source_tree(tmp_path / "source")
    calls = []

    class Installed:
        def verify_sources(self):
            calls.append("verified")

    def installed(root):
        calls.append(root)
        return Installed()

    monkeypatch.setattr(project_roots, "installed_compiler", installed)
    assert project_roots._require_molt_root(source, True, "build") is None
    assert calls == [source, "verified"]


def test_source_markers_must_be_files(tmp_path):
    source = _source_tree(tmp_path / "source")
    marker = source / "runtime/molt-backend/Cargo.toml"
    marker.unlink()
    marker.mkdir()
    assert not project_roots._has_molt_repo_markers(source)


def test_no_source_override_keeps_package_default(tmp_path, monkeypatch):
    monkeypatch.delenv("MOLT_SOURCE_ROOT", raising=False)
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(tmp_path))
    monkeypatch.chdir(tmp_path)
    assert source_root.compiler_source_root_override() is None
    assert (
        source_root.compiler_source_root()
        == Path(source_root.__file__).resolve().parents[2]
    )


def test_source_override_does_not_relocate_loaded_python_package(tmp_path, monkeypatch):
    installed = tmp_path / "installed" / "site-packages"
    sources = tmp_path / "compiler-source"
    monkeypatch.setattr(compiler_metadata, "_SRC_ROOT", installed)
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(sources))
    assert compiler_metadata._compiler_root() == sources
    assert compiler_metadata._compiler_python_source_root(installed.parent) == installed
    assert compiler_metadata._compiler_python_source_root(sources) == sources / "src"


def test_child_environment_does_not_replace_guest_with_compiler(tmp_path, monkeypatch):
    project, source = tmp_path / "guest", tmp_path / "compiler"
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(project))
    monkeypatch.delenv("MOLT_SOURCE_ROOT", raising=False)
    env = env_paths._base_env(project, molt_root=source)
    assert env["MOLT_PROJECT_ROOT"] == str(project)
    assert env["MOLT_SOURCE_ROOT"] == str(source)


def test_stdlib_inputs_ignore_guest_and_cwd(tmp_path, monkeypatch):
    from molt.cli import module_resolution, module_stdlib_policy

    guest, source = tmp_path / "guest", tmp_path / "compiler"
    relative = "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
    for root, name in ((guest, "guest_only"), (source, "compiler_only")):
        spec = root / relative
        spec.parent.mkdir(parents=True)
        spec.write_text(f"| Module |\n| --- |\n| {name} |\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_PROJECT_ROOT", str(guest))
    monkeypatch.chdir(guest)
    module_stdlib_policy._stdlib_allowlist_cached.cache_clear()
    try:
        monkeypatch.setenv("MOLT_SOURCE_ROOT", str(source))
        assert module_resolution._stdlib_root_path() == source / "src/molt/stdlib"
        assert "compiler_only" in module_stdlib_policy._stdlib_allowlist()
        assert "guest_only" not in module_stdlib_policy._stdlib_allowlist()
        missing = tmp_path / "missing"
        monkeypatch.setenv("MOLT_SOURCE_ROOT", str(missing))
        assert module_resolution._stdlib_root_path() == missing / "src/molt/stdlib"
        assert module_stdlib_policy._stdlib_allowlist() == set()
    finally:
        module_stdlib_policy._stdlib_allowlist_cached.cache_clear()


@pytest.mark.parametrize(
    ("module", "selector", "relative"),
    [
        ("molt.tool_releases", "tool_releases_path", "config/tool_releases.toml"),
        ("molt.llvm_toolchain", "_repo_root", "."),
        (
            "molt.cli.wasm_link_inputs",
            "wasm_builtins_vendor_dir",
            "vendor/wasm-builtins",
        ),
        ("molt.cli.external_native", "_molt_root_for_external_native_scan", "."),
        (
            "molt.scientific_stack_versions",
            "_config_path",
            "config/scientific_stack_versions.toml",
        ),
        (
            "molt.cli.source_extension_set_registry",
            "_config_path",
            "config/source_extension_package_sets.toml",
        ),
    ],
)
def test_sibling_source_selectors_follow_selection_after_import(
    tmp_path, monkeypatch, module, selector, relative
):
    owner = importlib.import_module(module)
    if hasattr(owner, "CONFIG_ENV"):
        monkeypatch.delenv(owner.CONFIG_ENV, raising=False)
    select = getattr(owner, selector)
    for name in ("first", "second"):
        root = tmp_path / name
        monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))
        actual = select(None) if selector == "_config_path" else select()
        assert actual == root / relative
        assert not root.exists()


def test_browser_assets_follow_source_not_staging(tmp_path, monkeypatch):
    from molt import browser_asset_closure

    source = tmp_path / "source"
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(source))
    seen = []

    def closure(root, entries):
        seen.append(root)
        return ("node_runner.mjs",)

    monkeypatch.setattr(browser_asset_closure, "wasm_loader_asset_closure", closure)
    assert browser_asset_closure.wasm_loader_asset_scope_paths() == (
        "wasm/node_runner.mjs",
    )
    assert seen == [source / "wasm"]


def test_cargo_policy_default_tracks_source_selection(tmp_path, monkeypatch):
    from molt import cargo_execution_policy

    for name in ("first", "second"):
        source = tmp_path / name
        monkeypatch.setenv("MOLT_SOURCE_ROOT", str(source))
        with pytest.raises(ValueError) as failure:
            cargo_execution_policy.load_ci_cargo_policy()
        assert str(source / "config/ci_resource_policy.toml") in str(failure.value)
        assert not source.exists()


def test_verified_subset_default_tracks_source_selection(tmp_path, monkeypatch):
    from molt import verified_subset

    seen = []

    def capture(path, **kwargs):
        seen.append(path)
        raise OSError("read refused")

    monkeypatch.setattr(verified_subset, "capture_stable_regular_file", capture)
    for name in ("first", "second"):
        source = tmp_path / name
        monkeypatch.setenv("MOLT_SOURCE_ROOT", str(source))
        with pytest.raises(OSError, match="read refused"):
            verified_subset.capture_verified_subset_policy()
        assert seen[-1] == source / "config/verified_subset.toml"


def test_guard_source_admission_does_not_fallback_to_loaded_checkout(
    tmp_path, monkeypatch
):
    from molt import process_guard

    missing = tmp_path / "missing"
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(missing))
    with pytest.raises(RuntimeError, match="guard tools are unavailable"):
        process_guard._molt_repo_root()
    assert not missing.exists()


def test_source_selection_does_not_redirect_translation_validation_outputs(
    tmp_path, monkeypatch
):
    from molt.frontend import tv_hooks

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(tmp_path / "read-only-source"))
    output = tmp_path / "artifacts"
    assert tv_hooks._temp_root({"MOLT_EXT_ROOT": str(output)}) == output / "tmp"
    assert tv_hooks._temp_root({"MOLT_DIFF_TMPDIR": str(output)}) == output


def test_dynamic_wasm_source_cache_is_root_bound(tmp_path, monkeypatch):
    from molt import _wasm_runtime_exports as exports

    for name in ("first", "second"):
        source = tmp_path / name
        module = source / "src/molt/gpu/test.py"
        module.parent.mkdir(parents=True)
        module.write_text("", encoding="utf-8")
    monkeypatch.setattr(
        exports,
        "_runtime_intrinsic_names_from_source",
        lambda path: (path.parents[3].name,),
    )
    exports._all_dynamic_runtime_owned_intrinsic_exports_for_root.cache_clear()
    try:
        for name in ("first", "second"):
            monkeypatch.setenv("MOLT_SOURCE_ROOT", str(tmp_path / name))
            assert exports._all_dynamic_runtime_owned_intrinsic_exports() == (name,)
    finally:
        exports._all_dynamic_runtime_owned_intrinsic_exports_for_root.cache_clear()
