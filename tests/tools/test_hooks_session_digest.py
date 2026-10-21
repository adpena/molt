"""Teeth for the SessionStart digest: live facts, fail-open, never blocks."""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks._common as common  # noqa: E402
import tools.hooks.landing_gate as lg  # noqa: E402
import tools.hooks.session_digest as sd  # noqa: E402


def _run_digest(monkeypatch, payload, capsys):
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(payload)))
    with pytest.raises(SystemExit) as ei:
        sd.main()
    out = capsys.readouterr().out
    return ei.value.code, out


def _quiet_queue_probes(monkeypatch, worktrees=3):
    # Avoid a real proof_queue subprocess: force undeterminable quickly.
    monkeypatch.setattr(
        sd._common, "proof_queue_active_count", lambda root, timeout=3.5: None
    )
    monkeypatch.setattr(sd._common, "worktree_count", lambda root: worktrees)


def test_digest_points_at_the_authorities_and_states_the_hooks(
    tmp_path, monkeypatch, capsys
):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch, worktrees=30)
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    assert code == 0
    assert out.startswith("== molt session start ==\n")
    assert "AGENTS.md" in out and "tools/agent_coordination.py context" in out
    assert "docs/agent/V1_HANDOFF_FINDINGS.md" in out
    assert "PROOF QUEUE: status undeterminable." in out
    assert "WORKTREES: 30 (threshold 24); over the threshold" in out
    assert "tools/molt_dev.py land --repo <lane>" in out
    assert "Read-only reviews are complete outcomes" in out
    assert ", ".join(sorted(lg.READ_ONLY_TOOLS)) in out
    # The digest restates no goal or claim board another file owns.
    assert "GOAL" not in out and "witness" not in out and "CLAIMS" not in out
    # tmp_path is no Git checkout: the queue section degrades, the rest stands.
    assert "(landing queue unavailable: " in out


def test_digest_reports_the_landing_queue(tmp_path, monkeypatch, capsys):
    from tools import molt_dev_queue as queue

    assert common._git(tmp_path, "init", "-q").returncode == 0
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch)
    paths = queue.queue_paths(tmp_path)
    failed = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch="lane/one")
    queue.record_result(paths, failed, status=queue.LandingStatus.FAILED, detail="")
    queue.enqueue(paths, head="2" * 40, worktree=tmp_path, branch="lane/two")
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    assert code == 0
    assert "LANDING QUEUE: 1 pending (lane/two).\n" in out
    assert "  failed lane/one at " in out


def test_digest_writes_landing_baseline(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch)
    monkeypatch.setattr(sd._common, "git_head", lambda root: "HEADSHA")
    _run_digest(monkeypatch, {"session_id": "sX", "cwd": str(tmp_path)}, capsys)
    marker = json.loads(
        (tmp_path / ".molt" / "state" / lg.MARKER_NAME).read_text(encoding="utf-8")
    )
    assert marker["session_id"] == "sX" and marker["start_head"] == "HEADSHA"


def test_digest_fail_open_on_broken_section(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))

    def boom(*a, **k):
        raise RuntimeError("worktree list exploded")

    _quiet_queue_probes(monkeypatch)
    monkeypatch.setattr(sd._common, "worktree_count", boom)
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    # A broken section must not sink the digest or the session.
    assert code == 0
    assert "(worktrees unavailable: RuntimeError)" in out
    assert "HOOKS:" in out
    log = tmp_path / ".molt" / "state" / "session_digest_errors.log"
    assert "worktree list exploded" in log.read_text(encoding="utf-8")
