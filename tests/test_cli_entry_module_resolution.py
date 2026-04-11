from __future__ import annotations

from pathlib import Path

from molt.cli import _resolve_build_entry


def test_resolve_build_entry_preserves_package_main_context(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    pkg = tmp_path / "probe_pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    main = pkg / "__main__.py"
    main.write_text("from . import VALUE\nprint(VALUE)\n", encoding="utf-8")

    entry, err = _resolve_build_entry(
        file_path=str(main),
        module=None,
        project_root=root,
        cwd_root=root,
        stdlib_root=root / "src" / "molt" / "stdlib",
        respect_pythonpath=False,
        json_output=False,
        command="build",
        lib_paths=None,
    )

    assert err is None
    assert entry is not None
    assert entry.entry_module == "probe_pkg.__main__"
    assert tmp_path.resolve() in entry.module_roots
