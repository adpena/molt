"""Teeth for the dead-code-allow ratchet metabug guard."""
from __future__ import annotations

from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "tools" / "dead_code_allow_ratchet.py"
BASELINE = ROOT / "tools" / "dead_code_allow_baseline.json"


def _run() -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GATE)],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )


def test_baseline_committed() -> None:
    assert BASELINE.is_file(), "the ratchet baseline must be committed"


def test_passes_at_baseline() -> None:
    proc = _run()
    assert proc.returncode == 0, proc.stdout
    assert "PASS" in proc.stdout


def test_fails_when_a_new_allow_is_added(tmp_path: Path) -> None:
    # Inject a masking allow into the scanned tree, assert the gate FAILS, remove it.
    probe = ROOT / "runtime" / "molt-runtime" / "src" / "_ratchet_teeth_probe.rs"
    probe.write_text("#[allow(dead_code)]\nfn _probe() {}\n", encoding="utf-8")
    try:
        proc = _run()
        assert proc.returncode == 2, proc.stdout
        assert "rose" in proc.stdout and "wire" in proc.stdout.lower()
    finally:
        probe.unlink(missing_ok=True)
    # after removal, back to green
    assert _run().returncode == 0
