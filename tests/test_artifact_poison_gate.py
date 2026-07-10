"""Teeth for the artifact-effect poison gate.

The gate must FAIL when a built wasm contains a known stub byte-marker and PASS
when it does not — the mechanical guard against marking a "resolve != effective"
capability done on a proxy signal.
"""
from __future__ import annotations

from pathlib import Path
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "tools" / "artifact_poison_gate.py"
REGISTRY = ROOT / "tools" / "artifact_poison_registry.toml"
# The exact abort string wasi-libc emits when the long-double stub links.
LONG_DOUBLE_MARKER = b"Support for formatting long double values is currently disabled"


def _run_gate(*wasm: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GATE), *[str(p) for p in wasm]],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )


def _fake_wasm(tmp_path: Path, name: str, payload: bytes) -> Path:
    # Minimal wasm header so the file is plausible; the gate scans raw bytes.
    p = tmp_path / name
    p.write_bytes(b"\x00asm\x01\x00\x00\x00" + payload)
    return p


def test_registry_loads_and_has_long_double_marker() -> None:
    proc = _run_gate()  # no args -> argparse usage error (exit 2 from argparse)
    # A real load-failure would be exit 3; here we just assert the registry parses
    # by invoking with a clean artifact below. Sanity: the marker string is in the
    # registry file verbatim.
    assert LONG_DOUBLE_MARKER.decode() in REGISTRY.read_text(encoding="utf-8")


def test_gate_fails_on_poisoned_runtime(tmp_path: Path) -> None:
    poisoned = _fake_wasm(
        tmp_path,
        "molt_runtime.wasm",
        b"...garbage..." + LONG_DOUBLE_MARKER + b"...more...",
    )
    proc = _run_gate(poisoned)
    assert proc.returncode == 2, proc.stdout
    assert "long_double_not_supported" in proc.stdout
    assert "configured != effective" in proc.stdout or "M34" in proc.stdout


def test_gate_passes_on_clean_runtime(tmp_path: Path) -> None:
    clean = _fake_wasm(tmp_path, "molt_runtime.wasm", b"real __multf3 __addtf3 code")
    proc = _run_gate(clean)
    assert proc.returncode == 0, proc.stdout
    assert "PASS" in proc.stdout


def test_gate_scans_multiple_artifacts_and_reports_the_poisoned_one(tmp_path: Path) -> None:
    clean = _fake_wasm(tmp_path, "app.wasm", b"clean app")
    poisoned = _fake_wasm(tmp_path, "molt_runtime.wasm", LONG_DOUBLE_MARKER)
    proc = _run_gate(clean, poisoned)
    assert proc.returncode == 2, proc.stdout
    assert "molt_runtime.wasm" in proc.stdout


def test_gate_errors_on_missing_artifact(tmp_path: Path) -> None:
    proc = _run_gate(tmp_path / "does_not_exist.wasm")
    assert proc.returncode == 3, proc.stdout
