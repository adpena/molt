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

import tools.drift_harvest as dh  # noqa: E402
import tools.hooks.session_digest as sd  # noqa: E402


def _run_digest(monkeypatch, payload, capsys):
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(payload)))
    with pytest.raises(SystemExit) as ei:
        sd.main()
    out = capsys.readouterr().out
    return ei.value.code, out


def _quiet_queue_probes(monkeypatch, worktrees=3):
    # The queue state is unreadable: the section must say so.
    monkeypatch.setattr(sd._common, "proof_queue_active_count", lambda root: None)
    monkeypatch.setattr(dh, "live_worktree_count", lambda root: worktrees)


def test_digest_points_at_the_authorities(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    # tmp_path must read as no checkout even when pytest's basetemp sits inside
    # one: Git may not search above it.
    monkeypatch.setenv("GIT_CEILING_DIRECTORIES", str(tmp_path.parent))
    _quiet_queue_probes(monkeypatch, worktrees=dh.MAX_LIVE_WORKTREES + 1)
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    assert code == 0
    assert out.startswith("== molt session start ==\n")
    assert "AGENTS.md" in out and "tools/agent_coordination.py context" in out
    assert "docs/agent/V1_HANDOFF_FINDINGS.md" in out
    assert "PROOF QUEUE: status undeterminable." in out
    assert (
        f"WORKTREES: {dh.MAX_LIVE_WORKTREES + 1} live "
        f"(SPRAWL threshold {dh.MAX_LIVE_WORKTREES}); over the threshold" in out
    )
    # The digest states no hook machinery: the hooks are the launcher and this.
    assert "HOOKS" not in out and "Stop" not in out and "bash_guard" not in out
    # The digest restates no goal or claim board another file owns.
    assert "GOAL" not in out and "witness" not in out and "CLAIMS" not in out
    # tmp_path is no Git checkout: the queue section degrades, the rest stands.
    assert "(landing queue unavailable: " in out


def test_digest_reports_the_landing_queue(tmp_path, monkeypatch, capsys):
    from tools import molt_dev_queue as queue

    assert dh._COMMANDS.run(["git", "init", "-q"], cwd=str(tmp_path)).returncode == 0
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


def test_live_worktree_count_counts_what_the_sprawl_gate_counts(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
    for argv in (
        ["init", "-q"],
        ["commit", "-q", "--allow-empty", "-m", "base"],
        ["worktree", "add", "-q", str(tmp_path / "kept")],
        ["worktree", "add", "-q", str(tmp_path / "gone")],
    ):
        assert dh._COMMANDS.run([*git, *argv], cwd=str(repo)).returncode == 0
    (tmp_path / "gone").rename(tmp_path / "moved-away")
    # main + kept are live; the registration whose directory left is STALE.
    assert dh.live_worktree_count(repo) == 2
    # A failed listing is an error, never an empty (passing) count.
    broken = tmp_path / "broken"
    broken.mkdir()
    (broken / ".git").write_text("gitdir: ./no-such-gitdir\n", encoding="utf-8")
    with pytest.raises(RuntimeError, match="git worktree list failed"):
        dh.live_worktree_count(broken)


def test_digest_fail_open_on_broken_section(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))

    def boom(*a, **k):
        raise RuntimeError("worktree list exploded")

    _quiet_queue_probes(monkeypatch)
    monkeypatch.setattr(dh, "live_worktree_count", boom)
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    # A broken section must not sink the digest or the session.
    assert code == 0
    assert "(worktrees unavailable: RuntimeError)" in out
    assert "PROOF QUEUE: status undeterminable." in out  # the others still render
    log = tmp_path / ".molt" / "state" / "session_digest_errors.log"
    assert "worktree list exploded" in log.read_text(encoding="utf-8")
