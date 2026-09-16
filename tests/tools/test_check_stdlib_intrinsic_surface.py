"""Teeth for tools/check_stdlib_intrinsic_surface.py: the surface-preservation
gate must FAIL CLOSED when a stdlib-required intrinsic is not registered."""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import importlib.util
import sys
from pathlib import Path

import pytest

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
    res = run_guarded_test_process(
        [sys.executable, str(GATE), "--check"], cwd=REPO_ROOT, capture_output=True, text=True
    )
    assert res.returncode == 0, res.stdout + res.stderr


@pytest.mark.parametrize(
    "request_source",
    [
        '_require_intrinsic("molt_needed_symbol")',
        "_require_intrinsic('molt_needed_symbol')",
        '_require_callable_intrinsic("molt_needed_symbol")',
        '_intrinsics_require("molt_needed_symbol")',
        'require_intrinsic(name="molt_needed_symbol")',
        'intrinsics.load_intrinsic("molt_needed_symbol")',
        '_lazy_intrinsic("molt_needed_symbol")',
    ],
)
def test_detects_required_but_unregistered(tmp_path, monkeypatch, request_source) -> None:
    gate = _load_gate()
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    (stdlib / "mod.py").write_text(f"x = {request_source}\n", encoding="utf-8")
    gen = tmp_path / "generated.rs"
    gen.write_text('IntrinsicSpec { name: "molt_other", ... }\n')  # molt_needed_symbol absent
    monkeypatch.setattr(gate, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(gate, "STDLIB_ROOT", stdlib)
    monkeypatch.setattr(gate, "GENERATED_RS", gen)
    missing, n_required, n_registered = gate.audit()
    assert missing == {"molt_needed_symbol": ["stdlib/mod.py"]}
    assert n_required == n_registered == 1


def test_request_discovery_ignores_non_calls_and_deduplicates(tmp_path, monkeypatch) -> None:
    gate = _load_gate()
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    (stdlib / "mod.py").write_text(
        '# _require_intrinsic("molt_comment")\n'
        "example = '_require_intrinsic(\"molt_string\")'\n"
        'first = _require_intrinsic("molt_needed")\n'
        'second = _lazy_intrinsic("molt_needed")\n',
        encoding="utf-8",
    )
    monkeypatch.setattr(gate, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(gate, "STDLIB_ROOT", stdlib)
    assert gate.required_intrinsics() == {"molt_needed": ["stdlib/mod.py"]}


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
