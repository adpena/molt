"""Teeth for the A9 uniform waiver grammar."""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks.waivers as w  # noqa: E402


def test_placeholder_rationales_rejected():
    for bad in ("todo", "TODO", "fixme", "n/a", "tbd", "xxx", "wip", "temp", "..."):
        assert not w.is_valid_rationale(bad), bad


def test_too_short_and_all_same_rejected():
    assert not w.is_valid_rationale("ok")
    assert not w.is_valid_rationale("aaaa")
    assert not w.is_valid_rationale("----")


def test_real_rationale_accepted():
    assert w.is_valid_rationale("origin remote intentionally https for the CI mirror")
    assert w.is_valid_rationale("reset ok: throwaway scratch worktree, verified path")


def test_parse_inline_waiver_matches_gate_and_validates():
    text = "some code  # BASH_GUARD_OK: verified safe on scratch worktree\n"
    assert (
        w.parse_inline_waiver(text, "bash_guard") == "verified safe on scratch worktree"
    )
    # Placeholder rationale does NOT count as a waiver.
    assert w.parse_inline_waiver("x  # BASH_GUARD_OK: todo\n", "bash_guard") is None
    # Wrong gate id -> no match.
    assert w.parse_inline_waiver(text, "landing_gate") is None


def test_find_all_waivers_reports_validity():
    text = "# LANDING_GATE_OK: real reason recorded\n# BASH_GUARD_OK: todo\n"
    found = dict((g, ok) for g, r, ok in w.find_all_waivers(text))
    assert found["LANDING_GATE"] is True
    assert found["BASH_GUARD"] is False


def test_skip_tokens_parsed():
    assert w.parse_skip_tokens("fix things [skip-landing-gate] [skip-triality]") == {
        "landing-gate",
        "triality",
    }
    assert w.has_skip_token("wip [skip-bash-guard]", "bash_guard")
    assert not w.has_skip_token("wip", "bash_guard")


def test_record_waiver_appends_ledger(tmp_path):
    w.record_waiver(
        "bash_guard",
        "override verified",
        source="override-token",
        context="git reset --hard",
        root=tmp_path,
    )
    ledger = tmp_path / ".molt" / "state" / "waivers.jsonl"
    assert ledger.exists()
    rows = [
        json.loads(ln)
        for ln in ledger.read_text(encoding="utf-8").splitlines()
        if ln.strip()
    ]
    assert rows[-1]["gate"] == "BASH_GUARD" and rows[-1]["source"] == "override-token"
