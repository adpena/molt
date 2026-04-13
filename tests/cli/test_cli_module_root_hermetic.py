from __future__ import annotations

from pathlib import Path

import molt.cli as cli


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

    roots = cli._resolve_module_roots(
        project_root,
        cwd_root,
        respect_pythonpath=False,
    )

    assert explicit_root.resolve() in roots
    assert vendor_root.resolve() in roots
    assert venv_site.resolve() not in roots
    assert molt_venv_site.resolve() not in roots
