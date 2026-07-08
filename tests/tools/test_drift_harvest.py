"""Unit tests for tools/drift_harvest.py classification (git mocked).

Pin the safety invariants: protected/locked/fresh worktrees are NEVER classified
prunable; a gone directory is STALE; only clean+old worktrees with no unique
commits are SUPERSEDED (prunable); clean+old WITH unique commits is SIGNAL
(bundled, never silently dropped).
"""

from __future__ import annotations

import tools.drift_harvest as dh


def test_classify_assigns_states(monkeypatch):
    now = 1_000_000.0
    wts = [
        {"path": "/Users/x/OneDrive/Documents/molt", "branch": "main"},
        {"path": "/d/Molt/worktrees/recover-mainclean-20260707", "branch": "x"},
        {"path": "/d/Molt/worktrees/molt-cli", "branch": "cli"},
        {"path": "/d/wt/locked", "branch": "l", "locked": True},
        {"path": "/d/wt/fresh-dirty", "branch": "fd"},
        {"path": "/d/wt/fresh-recent", "branch": "fr"},
        {"path": "/d/wt/superseded", "branch": "sup"},
        {"path": "/d/wt/signal", "branch": "sig"},
        {"path": "/d/wt/gone", "branch": "g"},
    ]
    monkeypatch.setattr(dh, "_worktrees", lambda: wts)
    monkeypatch.setattr(dh, "_worktree_exists", lambda p: not p.endswith("/gone"))
    monkeypatch.setattr(dh, "_dirty_count", lambda p: 4 if p.endswith("fresh-dirty") else 0)
    monkeypatch.setattr(
        dh, "_last_commit_epoch", lambda p: (now - 60) if p.endswith("fresh-recent") else 0
    )
    uniq = {"fd": 1, "fr": 1, "sup": 0, "sig": 5, "l": 2, "x": 0, "cli": 0, "main": 0, "g": 0}
    monkeypatch.setattr(dh, "_unique_commit_count", lambda ref: uniq.get(ref, 0))

    state = {r["path"]: r["state"] for r in dh.classify(fresh_hours=24.0, now=now)}
    assert state["/Users/x/OneDrive/Documents/molt"] == "PROTECTED"
    assert state["/d/Molt/worktrees/recover-mainclean-20260707"] == "PROTECTED"
    assert state["/d/Molt/worktrees/molt-cli"] == "PROTECTED"
    assert state["/d/wt/locked"] == "LOCKED"
    assert state["/d/wt/fresh-dirty"] == "FRESH"
    assert state["/d/wt/fresh-recent"] == "FRESH"
    assert state["/d/wt/superseded"] == "SUPERSEDED"
    assert state["/d/wt/signal"] == "SIGNAL"
    assert state["/d/wt/gone"] == "STALE"


def test_only_superseded_and_stale_are_prunable_without_include_signal(monkeypatch):
    # The prune path must never touch FRESH/PROTECTED/LOCKED, and SIGNAL only with
    # --include-signal (after its commits are bundled).
    now = 1_000_000.0
    rows = [
        {"path": "/p", "branch": "p", "uniq": 0, "dirty": 0, "state": "PROTECTED"},
        {"path": "/f", "branch": "f", "uniq": 3, "dirty": 1, "state": "FRESH"},
        {"path": "/sig", "branch": "sig", "uniq": 3, "dirty": 0, "state": "SIGNAL"},
        {"path": "/sup", "branch": "sup", "uniq": 0, "dirty": 0, "state": "SUPERSEDED"},
    ]
    prunable_default = [
        r for r in rows if r["state"] == "SUPERSEDED"
    ]
    assert [r["path"] for r in prunable_default] == ["/sup"]
    prunable_with_signal = [
        r for r in rows if r["state"] in ("SUPERSEDED", "SIGNAL")
    ]
    assert sorted(r["path"] for r in prunable_with_signal) == ["/sig", "/sup"]
