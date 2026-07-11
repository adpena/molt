"""Teeth for the SessionStart digest: fast, fail-open, never blocks."""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks.landing_gate as lg  # noqa: E402
import tools.hooks.session_digest as sd  # noqa: E402


def _run_digest(monkeypatch, payload, capsys):
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(payload)))
    with pytest.raises(SystemExit) as ei:
        sd.main()
    out = capsys.readouterr().out
    return ei.value.code, out


def test_digest_exits_zero_and_prints_sections(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    # Avoid any real proof_queue subprocess: force undeterminable quickly.
    monkeypatch.setattr(
        sd._common, "proof_queue_active_count", lambda root, timeout=3.5: None
    )
    monkeypatch.setattr(sd._common, "worktree_count", lambda root: 3)
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    assert code == 0
    assert "session digest" in out
    assert "GOAL" in out and "STANDING DIRECTIVES" in out and "APPARATUS" in out


def test_digest_writes_landing_baseline(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    monkeypatch.setattr(
        sd._common, "proof_queue_active_count", lambda root, timeout=3.5: None
    )
    monkeypatch.setattr(sd._common, "git_head", lambda root: "HEADSHA")
    _run_digest(monkeypatch, {"session_id": "sX", "cwd": str(tmp_path)}, capsys)
    marker = json.loads((tmp_path / ".molt" / "state" / lg.MARKER_NAME).read_text())
    assert marker["session_id"] == "sX" and marker["start_head"] == "HEADSHA"


def test_digest_fail_open_on_broken_section(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))

    def boom(*a, **k):
        raise RuntimeError("worktree list exploded")

    monkeypatch.setattr(sd._common, "worktree_count", boom)
    monkeypatch.setattr(
        sd._common, "proof_queue_active_count", lambda root, timeout=3.5: None
    )
    code, out = _run_digest(
        monkeypatch, {"session_id": "s1", "cwd": str(tmp_path)}, capsys
    )
    # A broken section must not sink the digest or the session.
    assert code == 0
    assert "session digest" in out
