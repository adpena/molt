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
import tools.hooks._common as common  # noqa: E402
import tools.hooks.bash_guard as bg  # noqa: E402
import tools.hooks.landing_gate as lg  # noqa: E402
import tools.hooks.session_digest as sd  # noqa: E402
import tools.hooks.stop_gates as stop_gates  # noqa: E402
import tools.hooks.waivers as waivers  # noqa: E402


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


def test_digest_points_at_the_authorities_and_states_the_hooks(
    tmp_path, monkeypatch, capsys
):
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
    # The Stop line names exactly the wired legs and each one's live state; an
    # unwired gate is absent.
    legs = ", ".join(
        f"{leg.name} ({stop_gates.enforcement_state(leg)})" for leg in stop_gates.LEGS
    )
    assert f"  Stop legs: {legs};" in out
    assert "landing_gate" not in out and "record-blocker" not in out
    assert f"  Waivers: {waivers.grammar_summary()}.\n" in out
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


@pytest.mark.parametrize(
    ("state", "said", "unsaid"),
    [
        ("warn", "bash_guard records, without blocking,", "MOLT_GUARD_OK"),
        ("strict", "bash_guard refuses", bg.SHADOW_NAME),
    ],
)
def test_digest_states_the_guard_enforcement_it_reads(
    tmp_path, monkeypatch, capsys, state, said, unsaid
):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch)
    monkeypatch.setattr(bg, "enforcement_state", lambda: state)
    _code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    hooks = out[out.index("HOOKS: ") :]
    assert said in hooks and unsaid not in hooks


def test_digest_reports_recent_would_be_blocks_by_rule_and_leg(
    tmp_path, monkeypatch, capsys
):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch)
    for rule in ("https-push", "https-push", "git-add-sweep"):
        common.record_shadow(tmp_path, bg.SHADOW_NAME, {"rule": rule})
    common.record_shadow(tmp_path, stop_gates.SHADOW_NAME, {"leg": "triality_gate"})
    # A record older than the window does not count.
    common.append_jsonl(
        common.state_dir(tmp_path) / bg.SHADOW_NAME,
        {"recorded_at": "2020-01-01T00:00:00Z", "rule": "https-push"},
    )
    _code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    assert (
        "bash_guard 3 (git-add-sweep 1, https-push 2); Stop legs 1 (triality_gate 1)."
        in out
    )


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


def test_digest_writes_landing_baseline(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch)
    monkeypatch.setattr(sd._common, "git_head", lambda root: "HEADSHA")
    _run_digest(monkeypatch, {"session_id": "sX", "cwd": str(tmp_path)}, capsys)
    marker = json.loads(
        (tmp_path / ".molt" / "state" / lg.MARKER_NAME).read_text(encoding="utf-8")
    )
    assert marker["session_id"] == "sX" and marker["start_head"] == "HEADSHA"


@pytest.mark.parametrize(
    ("source", "anchors"),
    [("startup", True), ("clear", True), ("resume", False), ("compact", False)],
)
def test_digest_anchors_the_window_only_when_a_conversation_begins(
    tmp_path, monkeypatch, capsys, source, anchors
):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    _quiet_queue_probes(monkeypatch)
    monkeypatch.setattr(sd._common, "git_head", lambda root: "HEADSHA")
    payload = {"session_id": "sX", "cwd": str(tmp_path), "source": source}
    _run_digest(monkeypatch, payload, capsys)
    marker = tmp_path / ".molt" / "state" / lg.MARKER_NAME
    assert marker.exists() is anchors


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
    assert "HOOKS:" in out
    log = tmp_path / ".molt" / "state" / "session_digest_errors.log"
    assert "worktree list exploded" in log.read_text(encoding="utf-8")
