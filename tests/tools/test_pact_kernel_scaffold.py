"""Tests for `collab/pact/parity/make_kernel_scaffold.py`.

Deliberately numpy-free (like the scaffolder itself): these run in the plain
host interpreter, no `uv --with numpy` provisioning needed. The engine-side
proof that a scaffold manifest structurally refuses to evaluate (never a fake
pass) lives in `tests/tools/test_pact_parity_engine.py`, which does need
numpy to import `collab.pact.parity.check_parity`.
"""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

import pytest

import collab.pact.parity.make_kernel_scaffold as scaffold


def test_scaffold_status_matches_engine_constant() -> None:
    """The duplicated literal in make_kernel_scaffold.py must not drift from
    the engine's real SCAFFOLD_STATUS — this import is the only place the two
    modules touch, keeping the scaffolder itself numpy-free. The engine module
    imports numpy at load time (it operates on ndarrays), so this one
    cross-check test needs numpy in-process; skip (not fake-pass) if absent —
    the same identity is proved unconditionally, without numpy, via the raw
    string constant in test_pact_parity_engine.py."""
    pytest.importorskip("numpy")
    import collab.pact.parity.check_parity as engine

    assert scaffold.SCAFFOLD_STATUS == engine.SCAFFOLD_STATUS


def test_make_kernel_scaffold_writes_three_files(tmp_path: Path) -> None:
    written = scaffold.make_kernel_scaffold("kernel_c", tmp_path)

    assert written == {
        "kernel": tmp_path / "kernel_c.py",
        "fixture": tmp_path / "make_kernel_c_fixture.py",
        "gates": tmp_path / "kernel_c_gates.json",
    }
    for path in written.values():
        assert path.is_file()


def test_scaffold_kernel_entry_point_raises_not_implemented(tmp_path: Path) -> None:
    written = scaffold.make_kernel_scaffold("kernel_c", tmp_path)
    text = written["kernel"].read_text(encoding="utf-8")
    assert "NOT IMPLEMENTED" in text
    assert "awaiting pact kernel source" in text

    # Prove it at runtime, not just by grepping the template text: importing
    # the module must succeed (it's a normal, analyzable Python file) but
    # CALLING the entry point must raise -- never return a fake result.
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import importlib.util, sys\n"
                f"spec = importlib.util.spec_from_file_location('kernel_c', r'{written['kernel']}')\n"
                "mod = importlib.util.module_from_spec(spec)\n"
                "spec.loader.exec_module(mod)\n"
                "try:\n"
                "    mod.kernel_c(1, 2, x=3)\n"
                "except NotImplementedError as exc:\n"
                "    print('RAISED:', exc)\n"
                "    sys.exit(0)\n"
                "print('DID NOT RAISE -- FAKE PASS')\n"
                "sys.exit(1)\n"
            ),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "RAISED: NOT IMPLEMENTED" in result.stdout


def test_scaffold_fixture_generator_main_raises_not_implemented(tmp_path: Path) -> None:
    written = scaffold.make_kernel_scaffold("kernel_c", tmp_path)

    result = subprocess.run(
        [sys.executable, str(written["fixture"])],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0, "fixture scaffold must never exit 0 (never a fake fixture)"
    assert "NotImplementedError" in result.stderr
    assert "NOT IMPLEMENTED" in result.stderr


def test_scaffold_gates_json_carries_scaffold_status(tmp_path: Path) -> None:
    written = scaffold.make_kernel_scaffold("kernel_c", tmp_path)
    manifest = json.loads(written["gates"].read_text(encoding="utf-8"))

    assert manifest["status"] == scaffold.SCAFFOLD_STATUS
    assert manifest["kernel"] == "kernel_c"
    assert manifest["_scaffold_marker"] == scaffold.SCAFFOLD_MARKER
    # An empty outputs dict is fine here: the status check refuses first, so
    # 'outputs' content can never matter for a scaffold manifest (proved on
    # the engine side in test_pact_parity_engine.py).
    assert manifest["outputs"] == {}


@pytest.mark.parametrize("bad_name", ["Kernel B", "kernel-c", "../evil", "7up", "", "k b"])
def test_make_kernel_scaffold_rejects_unsafe_kernel_names(
    tmp_path: Path, bad_name: str
) -> None:
    with pytest.raises(scaffold.ScaffoldNameError):
        scaffold.make_kernel_scaffold(bad_name, tmp_path)
    # Nothing should have been written for a rejected name.
    assert list(tmp_path.iterdir()) == []


def test_make_kernel_scaffold_refuses_to_clobber_real_kernel_file(tmp_path: Path) -> None:
    real_source = tmp_path / "kernel_c.py"
    real_source.write_text("def kernel_c():\n    return {'real': True}\n", encoding="utf-8")

    with pytest.raises(FileExistsError):
        scaffold.make_kernel_scaffold("kernel_c", tmp_path)

    # The real file must be untouched, and no sibling scaffold files created
    # as a side effect of the refused call.
    assert real_source.read_text(encoding="utf-8") == (
        "def kernel_c():\n    return {'real': True}\n"
    )
    assert not (tmp_path / "make_kernel_c_fixture.py").exists()
    assert not (tmp_path / "kernel_c_gates.json").exists()


def test_make_kernel_scaffold_force_overwrites_real_kernel_file(tmp_path: Path) -> None:
    real_source = tmp_path / "kernel_c.py"
    real_source.write_text("def kernel_c():\n    return {'real': True}\n", encoding="utf-8")

    scaffold.make_kernel_scaffold("kernel_c", tmp_path, force=True)

    assert "NOT IMPLEMENTED" in real_source.read_text(encoding="utf-8")


def test_make_kernel_scaffold_regenerating_a_scaffold_needs_no_force(tmp_path: Path) -> None:
    scaffold.make_kernel_scaffold("kernel_c", tmp_path)
    # Re-running without force must succeed because the existing files carry
    # the scaffold marker (they are not "real" delivered kernel files).
    scaffold.make_kernel_scaffold("kernel_c", tmp_path)


def test_cli_writes_scaffold_and_reports_paths(tmp_path: Path) -> None:
    module_path = Path(scaffold.__file__)
    result = subprocess.run(
        [sys.executable, str(module_path), "kernel_d", "--out-dir", str(tmp_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert (tmp_path / "kernel_d.py").is_file()
    assert (tmp_path / "make_kernel_d_fixture.py").is_file()
    assert (tmp_path / "kernel_d_gates.json").is_file()


def test_cli_refuses_unsafe_name_with_nonzero_exit(tmp_path: Path) -> None:
    module_path = Path(scaffold.__file__)
    result = subprocess.run(
        [sys.executable, str(module_path), "Not Safe", "--out-dir", str(tmp_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0
    assert "refused" in result.stderr
    assert list(tmp_path.iterdir()) == []
