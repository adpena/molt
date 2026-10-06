from __future__ import annotations

import sys
import tomllib
from pathlib import Path

import pytest

import molt
import molt.cli as cli
import molt._version as version_module


ROOT = Path(__file__).resolve().parents[2]


def _project_version() -> str:
    with (ROOT / "pyproject.toml").open("rb") as handle:
        data = tomllib.load(handle)
    version = data["project"]["version"]
    assert isinstance(version, str)
    return version


def test_package_version_comes_from_project_metadata() -> None:
    assert molt.__version__ == _project_version()


def test_cli_version_comes_from_project_metadata(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(sys, "argv", ["molt", "--version"])

    with pytest.raises(SystemExit) as exc_info:
        cli.main()

    assert exc_info.value.code == 0
    assert capsys.readouterr().out == f"molt {_project_version()}\n"


def test_version_falls_back_to_installed_metadata_without_source_tree(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    version_module.version.cache_clear()
    monkeypatch.setattr(
        version_module,
        "_source_tree_pyproject",
        lambda: tmp_path / "missing-pyproject.toml",
    )
    monkeypatch.setattr(
        version_module.metadata,
        "version",
        lambda project_name: "9.8.7" if project_name == "molt" else "unexpected",
    )

    try:
        assert version_module.version() == "9.8.7"
    finally:
        version_module.version.cache_clear()


def test_package_init_keeps_host_version_lookup_out_of_compiled_guests() -> None:
    """Compiled guests import ``molt.gpu`` (and tinygrad) through ``molt``.

    The package's module-init imports form part of every such guest's module
    closure, and its module body runs at guest import time. The version lookup
    reads package metadata and probes the file system, which a capability-free
    guest denies ("missing fs.read capability") and which pulls
    ``importlib.metadata`` and ``contextlib``'s coroutines into Pure guests.
    It must stay behind the module ``__getattr__``.
    """
    import ast

    from molt.cli.module_import_scanner import _collect_imports

    path = ROOT / "src" / "molt" / "__init__.py"
    tree = ast.parse(path.read_text(encoding="utf-8"))
    imports = _collect_imports(tree, "molt", True, import_scan_mode="module_init")
    assert set(imports) <= {"__future__", "__future__.annotations"}
    assert "__version__" in dir(molt)
