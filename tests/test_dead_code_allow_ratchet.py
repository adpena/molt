"""Teeth for the dead-code and cfg-corpse registry ratchet.

The canaries plant their probes in a fixture checkout, never in this
repository: a file there would dirty the checkout that every parallel proof
command attests.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import json
from pathlib import Path
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "tools" / "dead_code_allow_ratchet.py"
REGISTRY = ROOT / "tools" / "dead_code_allow_baseline.json"


def _run(root: Path = ROOT) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [sys.executable, str(GATE), "--root", str(root)],
        cwd=ROOT,
        capture_output=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
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


def _fixture_checkout(root: Path) -> Path:
    """A checkout with an empty registry and an empty runtime source tree."""
    (root / "tools").mkdir(parents=True)
    (root / "tools" / REGISTRY.name).write_text(
        json.dumps({"baseline_total": 0, "entries": []}), encoding="utf-8"
    )
    source = root / "runtime" / "molt-runtime" / "src"
    source.mkdir(parents=True)
    return source


@pytest.mark.parametrize(
    ("probe_source", "finding"),
    [
        ("#[allow(dead_code)]\nfn probe() {}\n", "unwaived allow_dead_code"),
        ("#[cfg(any())]\nfn corpse() {}\n", "unwaived cfg_corpse"),
    ],
)
def test_liveness_canary_rejects_an_unregistered_site(
    tmp_path: Path, probe_source: str, finding: str
) -> None:
    root = tmp_path / "checkout"
    source = _fixture_checkout(root)
    assert _run(root).returncode == 0

    probe = source / "probe.rs"
    probe.write_text(probe_source, encoding="utf-8")
    proc = _run(root)
    assert proc.returncode == 2, proc.stderr
    assert finding in proc.stderr
    assert "runtime/molt-runtime/src/probe.rs" in proc.stderr

    probe.unlink()
    assert _run(root).returncode == 0
