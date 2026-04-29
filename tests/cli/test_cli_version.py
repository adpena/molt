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
