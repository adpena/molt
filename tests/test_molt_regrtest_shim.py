from __future__ import annotations

from pathlib import Path

import tools.molt_regrtest_shim as shim


def test_build_env_enables_hermetic_module_roots(monkeypatch, tmp_path: Path) -> None:
    cpython_dir = tmp_path / "cpython"
    cpython_lib = cpython_dir / "Lib"
    cpython_lib.mkdir(parents=True)

    monkeypatch.setenv("PYTHONPATH", str(tmp_path / "ambient"))

    env = shim.build_env(cpython_dir)

    assert env["MOLT_HERMETIC_MODULE_ROOTS"] == "1"
    assert env["PYTHONNOUSERSITE"] == "1"
    assert env["MOLT_REGRTEST_CPYTHON_DIR"] == str(cpython_dir)
    assert env["MOLT_MODULE_ROOTS"] == str(cpython_lib.resolve())
