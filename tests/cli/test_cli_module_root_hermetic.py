from __future__ import annotations

import os
from pathlib import Path

import molt.cli as cli
from molt.cli import build_inputs as cli_build_inputs


def test_resolve_module_roots_omits_auto_site_packages_when_hermetic(
    monkeypatch, tmp_path: Path
) -> None:
    project_root = tmp_path / "project"
    cwd_root = tmp_path / "cwd"
    explicit_root = tmp_path / "cpython-lib"
    venv_site = project_root / ".venv" / "lib" / "python3.12" / "site-packages"
    molt_venv_site = (
        project_root / cli.MOLT_VENV_DIR / "lib" / "python3.12" / "site-packages"
    )
    vendor_root = project_root / "vendor" / "packages"

    for path in (
        project_root / "src",
        cwd_root / "src",
        explicit_root,
        venv_site,
        molt_venv_site,
        vendor_root,
    ):
        path.mkdir(parents=True, exist_ok=True)

    monkeypatch.setenv("MOLT_HERMETIC_MODULE_ROOTS", "1")
    monkeypatch.setenv("MOLT_MODULE_ROOTS", str(explicit_root))

    roots = cli_build_inputs._resolve_module_roots(
        project_root,
        cwd_root,
        respect_pythonpath=False,
    )

    assert explicit_root.resolve() in roots
    assert vendor_root.resolve() in roots
    assert venv_site.resolve() not in roots
    assert molt_venv_site.resolve() not in roots


def test_missing_module_root_env_entries_fail_closed(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    """An explicit MOLT_MODULE_ROOTS entry that does not exist must fail the
    build with a precise diagnostic, not be silently dropped. Silent drops
    surface later as unattributable custody failures (e.g. POSIX-style paths
    exported to Windows Python resolve to nothing and empty the external
    root set)."""
    project_root = tmp_path / "project"
    (project_root / "src").mkdir(parents=True)
    entry = project_root / "main.py"
    entry.write_text("print('hi')\n", encoding="utf-8")
    real_root = tmp_path / "real-root"
    real_root.mkdir()
    missing_root = tmp_path / "does-not-exist"
    posix_style = "/c/Users/nobody/definitely-missing"

    monkeypatch.setenv(
        "MOLT_MODULE_ROOTS",
        os.pathsep.join([str(real_root), str(missing_root), posix_style]),
    )

    resolution = cli_build_inputs._resolve_module_root_resolution(
        project_root,
        project_root,
        respect_pythonpath=False,
    )
    assert resolution.missing_env_roots == (str(missing_root), posix_style)
    assert real_root.resolve() in resolution.external_roots

    resolved, failure = cli_build_inputs._resolve_build_entry(
        file_path=str(entry),
        module=None,
        project_root=project_root,
        cwd_root=project_root,
        stdlib_root=tmp_path / "stdlib",
        respect_pythonpath=False,
        json_output=False,
    )
    assert resolved is None
    assert failure is not None
    message = capsys.readouterr().err
    assert "MOLT_MODULE_ROOTS" in message
    assert str(missing_root) in message
    assert posix_style in message
