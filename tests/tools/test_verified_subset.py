from __future__ import annotations

import subprocess

from tools import verified_subset


def test_run_differential_suites_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout=None, stderr=None)

    monkeypatch.setattr(
        verified_subset.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    verified_subset.run_differential_suites(
        {"differential_suites": ["tests/differential/basic"]}
    )

    assert captured["cmd"] == [
        verified_subset.sys.executable,
        str(verified_subset.ROOT / "tests/molt_diff.py"),
        "tests/differential/basic",
    ]
    assert captured["kwargs"]["prefix"] == "MOLT_VERIFIED_SUBSET"
    assert captured["kwargs"]["cwd"] == verified_subset.ROOT
    assert captured["kwargs"]["capture_output"] is False
