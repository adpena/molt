"""Teeth for tools/check_stdlib_intrinsic_surface.py: the surface-preservation
gate must FAIL CLOSED when a stdlib-required intrinsic is not registered."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
GATE = REPO_ROOT / "tools" / "check_stdlib_intrinsic_surface.py"


def _load_gate():
    spec = importlib.util.spec_from_file_location("check_stdlib_intrinsic_surface", GATE)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_green_on_real_tree_with_check_exits_0() -> None:
    # The gate must be GREEN on the shipped tree (decompositions preserve
    # registration) — else it can't be a tier-1 gate.
    res = subprocess.run(
        [sys.executable, str(GATE), "--check"], cwd=REPO_ROOT, capture_output=True, text=True
    )
    assert res.returncode == 0, res.stdout + res.stderr


def test_detects_required_but_unregistered(tmp_path, monkeypatch) -> None:
    gate = _load_gate()
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    (stdlib / "mod.py").write_text('x = _require_intrinsic("molt_needed_symbol")\n')
    gen = tmp_path / "generated.rs"
    gen.write_text('IntrinsicSpec { name: "molt_other", ... }\n')  # molt_needed_symbol absent
    monkeypatch.setattr(gate, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(gate, "STDLIB_ROOT", stdlib)
    monkeypatch.setattr(gate, "GENERATED_RS", gen)
    missing, n_required, n_registered = gate.audit()
    assert "molt_needed_symbol" in missing
    assert "stdlib/mod.py" in missing["molt_needed_symbol"][0].replace("\\", "/")


def test_passes_when_registered(tmp_path, monkeypatch) -> None:
    gate = _load_gate()
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    (stdlib / "mod.py").write_text('x = _require_intrinsic("molt_present")\n')
    gen = tmp_path / "generated.rs"
    gen.write_text('IntrinsicSpec { name: "molt_present", ... }\n')
    monkeypatch.setattr(gate, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(gate, "STDLIB_ROOT", stdlib)
    monkeypatch.setattr(gate, "GENERATED_RS", gen)
    missing, _, _ = gate.audit()
    assert missing == {}
