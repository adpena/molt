"""Teeth for the dead-code and cfg-corpse registry ratchet."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "tools" / "dead_code_allow_ratchet.py"
REGISTRY = ROOT / "tools" / "dead_code_allow_baseline.json"


def _run() -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GATE)],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )


def test_registry_entries_name_owner_and_waiver() -> None:
    data = json.loads(REGISTRY.read_text(encoding="utf-8"))
    assert data["baseline_total"] == len(data["entries"])
    assert data["entries"]
    assert all(entry["owner"].strip() for entry in data["entries"])
    assert all(entry["waiver"].strip() for entry in data["entries"])


def test_passes_at_registered_baseline() -> None:
    proc = _run()
    assert proc.returncode == 0, proc.stdout
    assert "PASS" in proc.stdout


def test_liveness_canary_rejects_new_dead_code_allow() -> None:
    probe = ROOT / "runtime" / "molt-runtime" / "src" / "_ratchet_teeth_probe.rs"
    probe.write_text("#[allow(dead_code)]\nfn probe() {}\n", encoding="utf-8")
    try:
        proc = _run()
        assert proc.returncode == 2, proc.stdout
        assert "unwaived allow_dead_code" in proc.stdout
    finally:
        probe.unlink(missing_ok=True)
    assert _run().returncode == 0


def test_liveness_canary_rejects_cfg_disabled_corpse() -> None:
    probe = ROOT / "runtime" / "molt-runtime" / "src" / "_ratchet_cfg_probe.rs"
    probe.write_text("#[cfg(any())]\nfn corpse() {}\n", encoding="utf-8")
    try:
        proc = _run()
        assert proc.returncode == 2, proc.stdout
        assert "unwaived cfg_corpse" in proc.stdout
    finally:
        probe.unlink(missing_ok=True)
    assert _run().returncode == 0
