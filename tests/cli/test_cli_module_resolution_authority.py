from __future__ import annotations

import inspect
import os
from pathlib import Path

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_resolution

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


def test_case_exact_file_recovers_from_stale_dir_entry_cache(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """A freshly written file must never be reported absent by a stale listing.

    ``_case_exact_dir_entries`` is memoised on the directory ``(mtime_ns, size)``
    stat key, which does not advance on filesystems where directory metadata is
    unchanged when an entry is added within the timestamp resolution
    (Windows/NTFS reports directory size 0 and coarse mtime). A stale empty
    listing would otherwise short-circuit resolution to ``False`` and silently
    drop a just-staged module/manifest. ``_case_exact_file_under`` must re-verify
    a miss against the live directory. Simulate the stale cache directly so the
    guarantee holds on every platform, not only where the mtime is coarse.
    """
    package_dir = tmp_path / "pkg"
    package_dir.mkdir()
    module_path = package_dir / "mod.py"
    module_path.write_text("VALUE = 1\n", encoding="utf-8")
    package_dir_text = os.fspath(package_dir)

    real_entries = module_resolution._case_exact_dir_entries

    def _stale_entries(dir_text: str) -> frozenset[str]:
        if dir_text == package_dir_text:
            return frozenset()
        return real_entries(dir_text)

    monkeypatch.setattr(
        module_resolution, "_case_exact_dir_entries", _stale_entries
    )

    assert module_resolution._case_exact_file(module_path)
