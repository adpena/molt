from pathlib import Path

import pytest

import molt.cli as cli


def test_target_python_defaults_to_lowest_supported_project_floor(
    tmp_path: Path,
) -> None:
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "sample"\nrequires-python = ">=3.12,<3.15"\n'
    )

    target = cli._resolve_target_python_version(
        explicit=None,
        build_config=None,
        project_root=tmp_path,
    )

    assert target.short == "3.12"


def test_target_python_uses_project_requires_python_floor(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "sample"\nrequires-python = ">=3.14,<3.15"\n'
    )

    target = cli._resolve_target_python_version(
        explicit=None,
        build_config=None,
        project_root=tmp_path,
    )

    assert target.short == "3.14"


def test_target_python_cli_overrides_project_requires_python(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "sample"\nrequires-python = ">=3.12,<3.15"\n'
    )

    target = cli._resolve_target_python_version(
        explicit="3.13",
        build_config=None,
        project_root=tmp_path,
    )

    assert target.short == "3.13"


def test_target_python_micro_release_selects_minor_line() -> None:
    target = cli._parse_target_python_version("3.14.4")

    assert target.short == "3.14"


def test_target_python_rejects_unsupported_project_floor(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "sample"\nrequires-python = ">=3.15"\n'
    )

    with pytest.raises(ValueError, match="does not admit"):
        cli._resolve_target_python_version(
            explicit=None,
            build_config=None,
            project_root=tmp_path,
        )


def test_wrapper_build_entry_uses_python_version_build_arg(tmp_path: Path) -> None:
    source_path = tmp_path / "main.py"
    source_path.write_text("print('ok')\n")

    entry, error = cli._resolve_wrapper_build_entry(
        file_path=str(source_path),
        module=None,
        project_root=tmp_path,
        json_output=True,
        command="run",
        build_args=["--python-version", "3.12"],
    )

    assert error is None
    assert entry is not None
    assert entry.target_python.short == "3.12"


def test_wrapper_target_python_reads_build_arg_without_host_parse(
    tmp_path: Path,
) -> None:
    target = cli._wrapper_target_python(
        ["--python-version", "3.14"],
        project_root=tmp_path,
    )

    assert target.short == "3.14"


def test_backend_cache_variant_changes_with_target_python() -> None:
    common = dict(
        profile="dev",
        runtime_cargo="dev-fast",
        backend_cargo="dev-fast",
        emit="bin",
        stdlib_split=False,
        codegen_env="codegen=v1",
        linked=False,
    )

    py312 = cli._build_cache_variant(
        **common,
        target_python=cli._SUPPORTED_TARGET_PYTHON_BY_SHORT["3.12"],
    )
    py314 = cli._build_cache_variant(
        **common,
        target_python=cli._SUPPORTED_TARGET_PYTHON_BY_SHORT["3.14"],
    )

    assert py312 != py314
    assert "target_python=py312" in py312
    assert "target_python=py314" in py314
