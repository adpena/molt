"""Unit tests for tools/drift_harvest.py classification (git mocked).

Pin the safety invariants: protected/locked/fresh worktrees are NEVER classified
prunable; a gone directory is STALE; only clean+old worktrees with no unique
commits are SUPERSEDED (prunable); clean+old WITH unique commits is SIGNAL
(bundled, never silently dropped).
"""

from __future__ import annotations

from pathlib import Path

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
    monkeypatch.setattr(dh, "_worktrees", lambda _root: wts)
    monkeypatch.setattr(dh, "_worktree_exists", lambda p: not p.endswith("/gone"))
    monkeypatch.setattr(
        dh, "_dirty_count", lambda p: 4 if p.endswith("fresh-dirty") else 0
    )
    monkeypatch.setattr(
        dh,
        "_last_commit_epoch",
        lambda p: (now - 60) if p.endswith("fresh-recent") else 0,
    )
    uniq = {
        "fd": 1,
        "fr": 1,
        "sup": 0,
        "sig": 5,
        "l": 2,
        "x": 0,
        "cli": 0,
        "main": 0,
        "g": 0,
    }
    monkeypatch.setattr(dh, "_unique_commit_count", lambda ref, _root: uniq.get(ref, 0))

    state = {
        r["path"]: r["state"]
        for r in dh.classify(fresh_hours=24.0, now=now, root=Path("/d/repo"))
    }
    assert state["/Users/x/OneDrive/Documents/molt"] == "PROTECTED"
    assert state["/d/Molt/worktrees/recover-mainclean-20260707"] == "PROTECTED"
    assert state["/d/Molt/worktrees/molt-cli"] == "PROTECTED"
    assert state["/d/wt/locked"] == "LOCKED"
    assert state["/d/wt/fresh-dirty"] == "FRESH"
    assert state["/d/wt/fresh-recent"] == "FRESH"
    assert state["/d/wt/superseded"] == "SUPERSEDED"
    assert state["/d/wt/signal"] == "SIGNAL"
    assert state["/d/wt/gone"] == "STALE"


def test_classify_computes_uniq_lazily(monkeypatch):
    # Perf invariant: the expensive git-cherry (uniq) call is skipped for PROTECTED,
    # LOCKED, and FRESH worktrees (they never consume uniq). This is what keeps the
    # pre-push gate fast on a checkout of mostly-fresh worktrees. Only clean+old
    # worktrees (SUPERSEDED/SIGNAL candidates) pay for the patch-id computation.
    now = 1_000_000.0
    wts = [
        {"path": "/d/wt/fresh-dirty", "branch": "fd"},
        {"path": "/d/wt/fresh-recent", "branch": "fr"},
        {"path": "/Users/x/OneDrive/Documents/molt", "branch": "prot"},
        {"path": "/d/wt/locked", "branch": "lk", "locked": True},
        {"path": "/d/wt/old-clean", "branch": "oc"},
    ]
    monkeypatch.setattr(dh, "_worktrees", lambda _root: wts)
    monkeypatch.setattr(dh, "_worktree_exists", lambda p: True)
    monkeypatch.setattr(
        dh, "_dirty_count", lambda p: 4 if p.endswith("fresh-dirty") else 0
    )
    monkeypatch.setattr(
        dh,
        "_last_commit_epoch",
        lambda p: (now - 60) if p.endswith("fresh-recent") else 0,
    )
    called: list[str] = []

    def spy(ref, _root):
        called.append(ref)
        return 3

    monkeypatch.setattr(dh, "_unique_commit_count", spy)
    dh.classify(fresh_hours=24.0, now=now, root=Path("/d/repo"))
    # uniq computed ONLY for the clean+old worktree; never for fresh/protected/locked.
    assert called == ["oc"]


def test_classify_protects_executing_repository_without_name_conventions(
    monkeypatch, tmp_path
):
    repository = tmp_path / "ordinary-checkout"
    sibling = tmp_path / "ordinary-retired-checkout"
    repository.mkdir()
    sibling.mkdir()
    monkeypatch.setattr(
        dh,
        "_worktrees",
        lambda _root: [{"path": str(path)} for path in (repository, sibling)],
    )
    monkeypatch.setattr(dh, "_dirty_count", lambda _path: 0)
    monkeypatch.setattr(dh, "_last_commit_epoch", lambda _path: 0)
    monkeypatch.setattr(dh, "_unique_commit_count", lambda _ref, _root: 0)

    states = {
        row["path"]: row["state"]
        for row in dh.classify(24.0, 1_000_000.0, root=repository)
    }

    assert states[str(repository)] == "PROTECTED"
    assert states[str(sibling)] == "SUPERSEDED"
    assert dh._protected(str(repository / ".." / repository.name), repository)
    assert dh._protected(str(repository.parent), repository)


def test_only_superseded_and_stale_are_prunable_without_include_signal(monkeypatch):
    # The prune path must never touch FRESH/PROTECTED/LOCKED, and SIGNAL only with
    # --include-signal (after its commits are bundled).
    rows = [
        {"path": "/p", "branch": "p", "uniq": 0, "dirty": 0, "state": "PROTECTED"},
        {"path": "/f", "branch": "f", "uniq": 3, "dirty": 1, "state": "FRESH"},
        {"path": "/sig", "branch": "sig", "uniq": 3, "dirty": 0, "state": "SIGNAL"},
        {"path": "/sup", "branch": "sup", "uniq": 0, "dirty": 0, "state": "SUPERSEDED"},
    ]
    prunable_default = [r for r in rows if r["state"] == "SUPERSEDED"]
    assert [r["path"] for r in prunable_default] == ["/sup"]
    prunable_with_signal = [r for r in rows if r["state"] in ("SUPERSEDED", "SIGNAL")]
    assert sorted(r["path"] for r in prunable_with_signal) == ["/sig", "/sup"]


def test_prune_leaves_target_cwd_before_removal(monkeypatch, tmp_path):
    repo_root = tmp_path / "molt"
    worktree = repo_root / "wt-old"
    worktree.mkdir(parents=True)
    monkeypatch.chdir(worktree)

    destination = dh._leave_prune_targets([worktree], repo_root)

    assert destination == tmp_path.resolve()
    assert Path.cwd() == tmp_path.resolve()


def test_empty_orphan_sweep_is_bounded_and_preserves_nonempty(monkeypatch, tmp_path):
    repo_root = tmp_path / "molt"
    repo_root.mkdir()
    empty = tmp_path / "wt-empty"
    nonempty = tmp_path / "wt-nonempty"
    unrelated = tmp_path / "empty-unrelated"
    empty.mkdir()
    nonempty.mkdir()
    unrelated.mkdir()
    (nonempty / "signal.txt").write_text("keep", encoding="utf-8")
    monkeypatch.setattr(dh, "canonical_molt_root", lambda _root: tmp_path)
    monkeypatch.setattr(dh, "_worktrees", lambda _root: [])

    removed = dh._sweep_empty_orphan_worktree_dirs(repo_root)

    assert removed == [empty]
    assert not empty.exists()
    assert nonempty.exists()
    assert unrelated.exists()


def test_bundle_signal_captures_detached_head_signal(monkeypatch, tmp_path):
    # Regression for the zero-signal-loss bug: a detached-HEAD SIGNAL worktree
    # (branch=None, uniq>0) MUST be captured — via a synthetic refs/harvest/<sha>
    # ref — so prune-safety (path in captured) matches bundle-safety. Before the
    # fix, bundle_signal skipped branch=None rows but --include-signal still pruned
    # them, losing the commits to GC.
    import tools.drift_harvest as dh

    calls = []

    class _R:
        returncode = 0
        stdout = ""
        stderr = ""

    def fake_git(args, cwd=None):
        calls.append(list(args))
        return _R()

    monkeypatch.setattr(dh, "_git", fake_git)
    rows = [
        {
            "path": "/branch-sig",
            "branch": "b",
            "head": "aaa",
            "uniq": 2,
            "dirty": 0,
            "state": "SIGNAL",
        },
        {
            "path": "/detached-sig",
            "branch": None,
            "head": "deadbeef",
            "uniq": 3,
            "dirty": 0,
            "state": "SIGNAL",
        },
        {
            "path": "/sup",
            "branch": None,
            "head": "ccc",
            "uniq": 0,
            "dirty": 0,
            "state": "SUPERSEDED",
        },
    ]
    captured = dh.bundle_signal(rows, tmp_path / "b.bundle", root=tmp_path)
    # Both SIGNAL worktrees captured; the zero-unique one is not.
    assert captured == {"/branch-sig", "/detached-sig"}
    # Detached head pinned under a synthetic GC-safe ref AND put in the bundle.
    assert ["update-ref", "refs/harvest/deadbeef", "deadbeef"] in calls
    bundle_cmd = next(a for a in calls if a and a[0] == "bundle")
    assert "b" in bundle_cmd and "refs/harvest/deadbeef" in bundle_cmd
    # Prune-gate parity: a SIGNAL worktree is prunable under --include-signal ONLY
    # if it is in `captured`. An un-captured SIGNAL row must never be prunable.
    uncaptured = {"path": "/orphan", "state": "SIGNAL"}
    assert uncaptured["path"] not in captured


def test_gate_fails_on_sprawl_and_aged_signal_but_not_fresh():
    # The enforcement gate makes drift BLOCKING: worktree sprawl and aged
    # unlanded SIGNAL both fail; fresh SIGNAL and STALE registrations do not.
    now = 1_000_000.0
    hour = 3600.0
    # Clean: 2 live worktrees, one FRESH-recent SIGNAL (2h old) — under thresholds.
    clean = [
        {"path": "/a", "branch": "a", "state": "FRESH", "uniq": 0, "last": now - hour},
        {
            "path": "/sig",
            "branch": "s",
            "state": "SIGNAL",
            "uniq": 3,
            "last": now - 2 * hour,
        },
    ]
    assert dh.gate(clean, now, max_worktrees=24, max_signal_age_hours=72.0) == []

    # Sprawl: 3 live worktrees over a max of 2 → one SPRAWL violation.
    sprawl = [
        {"path": f"/w{i}", "branch": f"b{i}", "state": "FRESH", "uniq": 0, "last": now}
        for i in range(3)
    ]
    v = dh.gate(sprawl, now, max_worktrees=2, max_signal_age_hours=72.0)
    assert len(v) == 1 and v[0].startswith("SPRAWL")

    # Aged SIGNAL (100h old) trips STALE-SIGNAL; a STALE registration never counts
    # as a live worktree and never trips the signal check.
    aged = [
        {
            "path": "/old",
            "branch": "old",
            "state": "SIGNAL",
            "uniq": 5,
            "last": now - 100 * hour,
        },
        {"path": "/gone", "branch": "g", "state": "STALE", "uniq": 0, "last": 0},
    ]
    v2 = dh.gate(aged, now, max_worktrees=24, max_signal_age_hours=72.0)
    assert len(v2) == 1 and v2[0].startswith("STALE-SIGNAL")


def test_no_fetch_flag_skips_origin_refresh(monkeypatch, capsys):
    calls = []

    class _R:
        returncode = 0
        stdout = ""
        stderr = ""

    def fake_git(args, cwd=None):
        calls.append(list(args))
        return _R()

    monkeypatch.setattr(dh, "_git", fake_git)
    monkeypatch.setattr(dh, "_worktrees", lambda _root: [])
    monkeypatch.setattr(
        "sys.argv", ["drift_harvest.py", "--gate", "--no-fetch", "--now", "1"]
    )

    assert dh.main() == 0
    assert ["fetch", "origin", "--quiet"] not in calls
    assert "DRIFT GATE PASSED" in capsys.readouterr().out


def test_free_lane_slots_are_neither_live_nor_prunable(monkeypatch, tmp_path):
    """A released lane slot is detached at a slot path: a warm cache, not work
    in progress (HF-491). The SPRAWL count and --prune skip it; a slot that
    holds a lane, or one with changes, stays an ordinary worktree."""
    main = tmp_path / "molt-src"
    slots = tmp_path / "worktrees"
    for path in (main, slots / "slot-01", slots / "slot-02", slots / "slot-03"):
        path.mkdir(parents=True)
    wts = [
        {"path": str(main), "branch": "main"},
        {"path": str(slots / "slot-01"), "head": "a" * 40},
        {"path": str(slots / "slot-02"), "branch": "lane/busy"},
        {"path": str(slots / "slot-03"), "head": "b" * 40},
    ]
    monkeypatch.setattr(dh, "_worktrees", lambda _root: wts)
    monkeypatch.setattr(dh, "_worktree_exists", lambda _path: True)
    monkeypatch.setattr(
        dh, "_dirty_count", lambda path: 1 if path.endswith("slot-03") else 0
    )
    monkeypatch.setattr(dh, "_last_commit_epoch", lambda _path: 0)
    monkeypatch.setattr(dh, "_unique_commit_count", lambda _ref, _root: 0)

    assert dh.live_worktree_count(main) == 2
    states = {
        Path(row["path"]).name: row["state"]
        for row in dh.classify(fresh_hours=24.0, now=1_000_000.0, root=main)
    }
    assert states["slot-01"] == "SLOT"
    assert states["slot-02"] == "SUPERSEDED"
    assert states["slot-03"] == "FRESH"
    rows = dh.classify(fresh_hours=24.0, now=1_000_000.0, root=main)
    assert not dh.gate(rows, 1_000_000.0, max_worktrees=3, max_signal_age_hours=1.0)
