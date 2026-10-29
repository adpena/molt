from __future__ import annotations

import sys
import subprocess
import tomllib
from pathlib import Path

import pytest

import molt
import molt.cli as cli


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


def test_version_works_without_source_tree_or_distribution_metadata(
    tmp_path: Path,
) -> None:
    # Exercise the shipped module in an isolated interpreter, with no package
    # metadata, repository pyproject, site packages or PYTHONPATH to consult.
    source = (ROOT / "src/molt/_version.py").read_text(encoding="utf-8")
    module = tmp_path / "shipped_version.py"
    module.write_text(source, encoding="utf-8")
    run = subprocess.run(
        [
            sys.executable,
            "-B",
            "-I",
            "-S",
            "-c",
            "import runpy, sys; print(runpy.run_path(sys.argv[1])['version']())",
            str(module),
        ],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=True,
    )
    assert run.stdout.strip() == _project_version()


def test_generated_version_is_in_sync() -> None:
    from tools.gen_project_version import generated_outputs

    for path, text in generated_outputs().items():
        assert path.read_text(encoding="utf-8") == text


def test_package_init_keeps_host_version_lookup_out_of_compiled_guests() -> None:
    """Compiled guests import ``molt.gpu`` (and tinygrad) through ``molt``.

    The static closure includes imports inside lazy functions as well as module
    initialization. Neither may pull host package metadata into pure guests.
    """
    import ast

    from molt.compiler_analysis.python_import_scan import _collect_imports

    path = ROOT / "src" / "molt" / "__init__.py"
    tree = ast.parse(path.read_text(encoding="utf-8"))
    imports = _collect_imports(tree, "molt", True, import_scan_mode="module_init")
    assert set(imports) <= {"__future__", "__future__.annotations"}
    version_tree = ast.parse((ROOT / "src/molt/_version.py").read_text())
    assert not _collect_imports(version_tree, "molt._version", False)
    assert "__version__" in dir(molt)
