"""Teeth for the deterministic, agent-safe disk guard (tools/disk_guard.py).

Proves (M05 -- a gate that cannot fail certifies nothing):

  * ``ensure_free`` reclaims stale dirs in a temp fixture and STOPS once the
    target free-space is met (no over-deletion);
  * an ACTIVE dir -- recently touched OR holding a Cargo build lock -- is NEVER
    reclaimed, by ``ensure_free`` and by the pure planner;
  * the AGENT-SAFETY source scan finds ZERO process-actuation tokens in the
    module's executable code (no kill / Popen / signal / taskkill / subprocess);
  * the guard is IDEMPOTENT (a second call above the threshold is a no-op) and
    FAIL-OPEN (a raising internal -> the build proceeds, logged, no raise);
  * per-lane GC collects a registered dir past its TTL but not one within it;
  * the self-test canaries are all LIVE (gate-liveness).
"""

from __future__ import annotations

import io
import os
import sys
import tokenize
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.disk_guard as dg  # noqa: E402

_GB = 1024**3


# --- helpers ----------------------------------------------------------------


def _make_dir(path: Path, *, size_bytes: int, age_s: float, now: float) -> int:
    """Create a dir with a file of ~size_bytes and set its mtime to now-age_s."""
    path.mkdir(parents=True, exist_ok=True)
    blob = path / "artifact.bin"
    # Small on-disk footprint; the SIMULATED free-space fn uses the declared
    # size, so we do not actually write gigabytes.
    blob.write_bytes(b"\0" * min(size_bytes, 4096))
    stamp = now - age_s
    os.utime(path, (stamp, stamp))
    os.utime(blob, (stamp, stamp))
    return size_bytes


class _SimulatedVolume:
    """Free-space fn that starts in deficit and RISES as candidate dirs vanish.

    ``free = start_free + sum(size for created dirs that no longer exist)`` -- so
    each rmtree by ``ensure_free`` is reflected on the next real free read, which
    is exactly how the orchestrator's self-correcting stop condition is meant to
    work (it trusts REAL free space, not an estimate).
    """

    def __init__(self, start_free: int, sized: dict[Path, int]) -> None:
        self.start_free = start_free
        self.sized = sized

    def __call__(self) -> int:
        reclaimed = sum(sz for p, sz in self.sized.items() if not p.exists())
        return self.start_free + reclaimed


def _cfg(**over) -> dg.GuardConfig:
    base = dict(
        high_water_bytes=25 * _GB,
        target_bytes=40 * _GB,
        min_idle_s=15 * 60.0,
        gc_ttl_s=6 * 3600.0,
        lane_globs=dg.DEFAULT_LANE_GLOBS,
    )
    base.update(over)
    return dg.GuardConfig(**base)


def _fake_registered_worktree_scope(tmp_path: Path) -> tuple[Path, Path, Path]:
    """Build the filesystem metadata written by ``git worktree add``."""
    scope = tmp_path
    main = scope / "molt-src"
    linked = scope / "worktrees" / "lane"
    common = main / ".git"
    admin = common / "worktrees" / "lane"
    main.mkdir(parents=True)
    linked.mkdir(parents=True)
    admin.mkdir(parents=True)
    (linked / ".git").write_text(f"gitdir: {admin}\n", encoding="utf-8")
    (admin / "gitdir").write_text(str(linked / ".git") + "\n", encoding="utf-8")
    return scope, main, linked


# --- 1. ensure_free reclaims stale dirs and STOPS at the target -------------


def test_ensure_free_reclaims_stale_and_stops_at_target(tmp_path):
    now = 2_000_000.0
    sessions = tmp_path / "target" / "sessions"
    # Three stale session targets, 20 GB each, oldest first.
    d_old = sessions / "codex-oldest"
    d_mid = sessions / "codex-middle"
    d_new = sessions / "codex-newest"
    sized = {
        d_old: _make_dir(d_old, size_bytes=20 * _GB, age_s=100_000, now=now),
        d_mid: _make_dir(d_mid, size_bytes=20 * _GB, age_s=50_000, now=now),
        d_new: _make_dir(d_new, size_bytes=20 * _GB, age_s=20_000, now=now),
    }
    # Start 10 GB free (below the 25 GB high-water); target 40 GB. Reclaiming the
    # two oldest (20+20) reaches 50 GB, so the loop must stop BEFORE the newest.
    vol = _SimulatedVolume(start_free=10 * _GB, sized=sized)
    result = dg.ensure_free(
        root=str(tmp_path),
        config=_cfg(),
        free_bytes_fn=vol,
        now_fn=lambda: now,
        env={},
    )
    assert result.triggered is True
    reclaimed_paths = {Path(item["path"]) for item in result.reclaimed}
    # Oldest two gone; newest survives (target met after two).
    assert not d_old.exists()
    assert not d_mid.exists()
    assert d_new.exists()
    assert reclaimed_paths == {d_old.resolve(), d_mid.resolve()}
    assert result.free_after >= 40 * _GB  # target reached (re-read real free)
    assert vol() >= 40 * _GB


# --- 2a. an ACTIVE (recently-touched) dir is never reclaimed -----------------


def test_active_recent_dir_never_reclaimed(tmp_path):
    now = 2_000_000.0
    sessions = tmp_path / "target" / "sessions"
    stale = sessions / "codex-stale"
    active = sessions / "codex-active"
    sized = {
        stale: _make_dir(stale, size_bytes=5 * _GB, age_s=100_000, now=now),
        # Touched 5 seconds ago -> inside the 15-min min-idle window.
        active: _make_dir(active, size_bytes=100 * _GB, age_s=5, now=now),
    }
    vol = _SimulatedVolume(start_free=1 * _GB, sized=sized)
    result = dg.ensure_free(
        root=str(tmp_path),
        config=_cfg(),
        free_bytes_fn=vol,
        now_fn=lambda: now,
        env={},
    )
    # Even though the active dir is huge and free is critically low, it is never
    # touched; only the stale dir is reclaimed (and the target stays unmet).
    assert active.exists()
    assert not stale.exists()
    reasons = {Path(s["path"]): s["reason"] for s in result.skipped}
    assert active.resolve() in reasons
    assert "min ago" in reasons[active.resolve()]


# --- 2b. current session dir is protected -----------------------------------


def test_current_session_dir_never_reclaimed(tmp_path):
    now = 2_000_000.0
    sessions = tmp_path / "target" / "sessions"
    mine = sessions / "my-session"
    other = sessions / "codex-other"
    sized = {
        # BOTH are old, but "my-session" is the live MOLT_SESSION_ID.
        mine: _make_dir(mine, size_bytes=50 * _GB, age_s=100_000, now=now),
        other: _make_dir(other, size_bytes=5 * _GB, age_s=100_000, now=now),
    }
    vol = _SimulatedVolume(start_free=1 * _GB, sized=sized)
    result = dg.ensure_free(
        root=str(tmp_path),
        config=_cfg(),
        free_bytes_fn=vol,
        now_fn=lambda: now,
        env={"MOLT_SESSION_ID": "my-session"},
    )
    assert mine.exists()  # protected in-flight build of THIS process
    assert not other.exists()
    reasons = {Path(s["path"]): s["reason"] for s in result.skipped}
    assert reasons.get(mine.resolve()) == "protected"


def test_explicit_active_target_nested_in_candidate_protects_parent(tmp_path):
    now = 2_000_000.0
    lane = tmp_path / "target" / "codex-active-parent"
    nested_target = lane / "nested-cargo-target"
    _make_dir(lane, size_bytes=100 * _GB, age_s=100_000, now=now)
    nested_target.mkdir()
    os.utime(nested_target, (now - 100_000, now - 100_000))
    os.utime(lane, (now - 100_000, now - 100_000))
    result = dg.ensure_free(
        root=tmp_path,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, {lane: 100 * _GB}),
        now_fn=lambda: now,
        env={"CARGO_TARGET_DIR": str(nested_target)},
    )

    assert lane.exists()
    reasons = {Path(item["path"]): item["reason"] for item in result.skipped}
    assert reasons[lane.resolve()] == "protected"


def test_shared_target_parent_does_not_protect_independent_session_child(tmp_path):
    now = 2_000_000.0
    session = tmp_path / "target" / "sessions" / "old-independent"
    _make_dir(session, size_bytes=100 * _GB, age_s=100_000, now=now)
    result = dg.ensure_free(
        root=tmp_path,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, {session: 100 * _GB}),
        now_fn=lambda: now,
        env={"CARGO_TARGET_DIR": str(tmp_path / "target")},
    )

    assert not session.exists()
    assert {Path(item["path"]) for item in result.reclaimed} == {session.resolve()}


def test_registered_worktree_discovery_is_filesystem_only_and_scope_bounded(tmp_path):
    scope, main, linked = _fake_registered_worktree_scope(tmp_path)
    outside = tmp_path.parent / "outside-worktree"
    outside.mkdir(exist_ok=True)
    outside_admin = main / ".git" / "worktrees" / "outside"
    outside_admin.mkdir()
    (outside_admin / "gitdir").write_text(
        str(outside / ".git") + "\n", encoding="utf-8"
    )

    assert set(dg.registered_worktree_roots(main)) == {
        main.resolve(),
        linked.resolve(),
    }
    assert set(dg.reclaim_roots(main)) == {
        scope.resolve(),
        main.resolve(),
        linked.resolve(),
    }


def test_ensure_free_reclaims_across_registered_worktrees_only(tmp_path):
    scope, main, linked = _fake_registered_worktree_scope(tmp_path)
    now = 2_000_000.0
    main_session = main / "target" / "sessions" / "main-old"
    linked_session = linked / "target" / "sessions" / "linked-old"
    unregistered_session = (
        scope / "worktrees" / "unregistered" / "target" / "sessions" / "keep"
    )
    sized = {
        main_session: _make_dir(
            main_session, size_bytes=20 * _GB, age_s=100_000, now=now
        ),
        linked_session: _make_dir(
            linked_session, size_bytes=20 * _GB, age_s=90_000, now=now
        ),
        unregistered_session: _make_dir(
            unregistered_session, size_bytes=100 * _GB, age_s=200_000, now=now
        ),
    }
    # A sibling checkout cannot self-authorize merely by containing its own
    # repository metadata; only the canonical repo's registration is trusted.
    (scope / "worktrees" / "unregistered" / ".git").mkdir()
    result = dg.ensure_free(
        root=main,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, sized),
        now_fn=lambda: now,
        env={},
    )

    assert not main_session.exists()
    assert not linked_session.exists()
    assert unregistered_session.exists()
    assert linked.resolve() in {Path(path) for path in result.scope_roots}


def test_live_guard_protects_its_worktree_but_terminal_marker_does_not(tmp_path):
    _, main, linked = _fake_registered_worktree_scope(tmp_path)
    now = 2_000_000.0
    main_session = main / "target" / "sessions" / "main-old"
    linked_session = linked / "target" / "sessions" / "linked-active"
    sized = {
        main_session: _make_dir(
            main_session, size_bytes=5 * _GB, age_s=100_000, now=now
        ),
        linked_session: _make_dir(
            linked_session, size_bytes=100 * _GB, age_s=100_000, now=now
        ),
    }
    markers = linked / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    marker = markers / "guard-1-token.json"
    marker.write_text('{"status":"child_running"}\n', encoding="utf-8")

    result = dg.ensure_free(
        root=main,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, sized),
        now_fn=lambda: now,
        env={},
    )
    assert not main_session.exists()
    assert linked_session.exists()
    reasons = {Path(item["path"]): item["reason"] for item in result.skipped}
    assert reasons[linked_session.resolve()] == "active-guard"

    marker.write_text('{"status":"completed"}\n', encoding="utf-8")
    result = dg.ensure_free(
        root=main,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, {linked_session: 100 * _GB}),
        now_fn=lambda: now,
        env={},
    )
    assert not linked_session.exists()
    assert {Path(item["path"]) for item in result.reclaimed} == {
        linked_session.resolve()
    }


def test_terminal_parent_custody_retires_nested_nonterminal_marker(tmp_path):
    markers = tmp_path / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    nested = markers / "guard-7-nested.json"
    nested.write_text('{"pid":7,"status":"child_running"}\n', encoding="utf-8")
    os.utime(nested, (100.0, 100.0))
    parent = markers / "guard-1-parent.json"
    parent.write_text(
        '{"pid":1,"status":"completed","termination_reports":[{"watched_pids":[7]}]}\n',
        encoding="utf-8",
    )
    os.utime(parent, (200.0, 200.0))

    assert dg._has_active_guard(tmp_path) is False


def test_incident_capsules_in_active_directory_do_not_claim_guard_custody(tmp_path):
    markers = tmp_path / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    (markers / "manual-death-capsule.json").write_text(
        '{"status":"starting","command":"cargo test"}\n', encoding="utf-8"
    )

    assert dg._has_active_guard(tmp_path) is False


def test_recent_registry_event_protects_old_target_from_pressure_reclaim(tmp_path):
    now = 3_000_000.0
    target = tmp_path / "target" / "codex-live"
    _make_dir(target, size_bytes=100 * _GB, age_s=100_000, now=now)
    dg.register_lane_target(target, root=tmp_path, env={}, now=now - 60)
    result = dg.ensure_free(
        root=tmp_path,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, {target: 100 * _GB}),
        now_fn=lambda: now,
        env={},
    )
    assert target.exists()
    reasons = {Path(item["path"]): item["reason"] for item in result.skipped}
    assert reasons[target.resolve()] == "live-lane"


# --- 2c. a lock-held dir is never reclaimed (real held lock) ----------------


def test_lock_held_dir_never_reclaimed(tmp_path):
    now = 2_000_000.0
    d = tmp_path / "target" / "codex-building"
    d.mkdir(parents=True)
    (d / "artifact.bin").write_bytes(b"\0" * 4096)
    lock_path = d / ".cargo-lock"
    lock_path.write_bytes(b"\0")
    stamp = now - 100_000  # otherwise-stale
    os.utime(d, (stamp, stamp))

    handle = open(lock_path, "a+b")
    got_lock = False
    try:
        try:
            if os.name == "nt":
                import msvcrt

                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl

                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            got_lock = True
        except OSError:
            got_lock = False
        if not got_lock:
            pytest.skip("could not establish a held lock on this platform")

        # The guard's probe must observe the held lock.
        assert dg._cargo_lock_held(d) is True
        vol = _SimulatedVolume(start_free=1 * _GB, sized={d: 100 * _GB})
        result = dg.ensure_free(
            root=str(tmp_path),
            config=_cfg(),
            free_bytes_fn=vol,
            now_fn=lambda: now,
            env={},
        )
        assert d.exists()  # never reclaim an actively-locked build dir
        reasons = {Path(s["path"]): s["reason"] for s in result.skipped}
        assert reasons.get(d.resolve()) == "lock-held"
    finally:
        try:
            if got_lock:
                if os.name == "nt":
                    import msvcrt

                    handle.seek(0)
                    msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    import fcntl

                    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        finally:
            handle.close()


# --- 3. AGENT-SAFETY source scan: zero process-actuation tokens -------------


# Forbidden identifiers in EXECUTABLE code (docstrings/comments/strings are
# excluded via tokenize, so the module may DESCRIBE what it does not do). Mirrors
# pact's test_no_actuation_capability: prove the module cannot touch a process.
_FORBIDDEN_CODE_NAMES = frozenset(
    {
        "Popen",
        "subprocess",
        "kill",
        "killpg",
        "pkill",
        "taskkill",
        "TerminateProcess",
        "terminate",
        "signal",
        "SIGTERM",
        "SIGKILL",
        "SIGINT",
        "psutil",
        "CreateProcess",
        "system",  # os.system
        "spawn",
        "spawnv",
        "execv",
        "execve",
    }
)


def _name_tokens_from_source(source: str) -> set[str]:
    names: set[str] = set()
    reader = io.BytesIO(source.encode("utf-8")).readline
    for tok in tokenize.tokenize(reader):
        if tok.type == tokenize.NAME:
            names.add(tok.string)
    return names


def _code_name_tokens(path: Path) -> set[str]:
    return _name_tokens_from_source(path.read_text(encoding="utf-8"))


def test_agent_safety_source_scan():
    """The module's CODE contains no process-actuation capability at all."""
    module_path = Path(dg.__file__)
    names = _code_name_tokens(module_path)
    offenders = sorted(names & _FORBIDDEN_CODE_NAMES)
    assert not offenders, (
        f"disk_guard.py must be AGENT-SAFE: found process-actuation identifiers "
        f"in executable code: {offenders}. The disk guard must NEVER be able to "
        f"touch a process (it reclaims disk only)."
    )


def test_agent_safety_no_process_imports():
    """Belt-and-suspenders: the imported module exposes no process primitives."""
    for attr in ("subprocess", "signal", "psutil"):
        assert not hasattr(dg, attr), f"disk_guard must not import {attr}"


def test_agent_safety_scanner_has_teeth():
    """The scanner MUST detect a synthetic process-kill -- a gate that cannot
    fail certifies nothing (M05). If disk_guard.py ever grew a real kill, this
    same scan would catch it."""
    malicious = (
        "import subprocess, signal, os\n"
        "def reap(pid):\n"
        "    os.kill(pid, signal.SIGTERM)\n"
        "    subprocess.Popen(['taskkill', '/F', '/PID', str(pid)])\n"
    )
    names = _name_tokens_from_source(malicious)
    caught = names & _FORBIDDEN_CODE_NAMES
    assert {"subprocess", "signal", "kill", "Popen", "SIGTERM"} <= caught


# --- 4. idempotent + fail-open ----------------------------------------------


def test_idempotent_above_high_water_is_noop(tmp_path):
    now = 2_000_000.0
    d = tmp_path / "target" / "sessions" / "codex-stale"
    _make_dir(d, size_bytes=20 * _GB, age_s=100_000, now=now)
    # Already above the high-water: must be a no-op, dir untouched, twice.
    for _ in range(2):
        result = dg.ensure_free(
            root=str(tmp_path),
            config=_cfg(),
            free_bytes_fn=lambda: 30 * _GB,
            now_fn=lambda: now,
            env={},
        )
        assert result.triggered is False
        assert result.reclaimed == []
        assert d.exists()


def test_fail_open_on_raising_internal(capsys):
    """A raising internal -> ensure_free_fail_open returns None, never raises."""

    def boom() -> int:
        raise RuntimeError("simulated disk-usage failure")

    # Must NOT raise; the caller (a build) proceeds.
    out = dg.ensure_free_fail_open(
        root="C:/Molt" if os.name == "nt" else "/",
        free_bytes_fn=boom,
        env={},
    )
    assert out is None
    err = capsys.readouterr().err
    assert "disk_guard" in err and "fail-open" in err


def test_fail_open_on_unresolvable_root(capsys):
    out = dg.ensure_free_fail_open(root=None, env={})  # no root resolvable
    # Either it resolved a real root (returned a result) or failed open (None);
    # in NEITHER case may it raise.
    assert out is None or isinstance(out, dg.ReclaimResult)


# --- 5. per-lane GC (registry-driven TTL collection) ------------------------


def test_gc_collects_registered_dir_past_ttl(tmp_path):
    now = 3_000_000.0
    done = tmp_path / "target" / "codex-done"
    live = tmp_path / "target" / "codex-live"
    _make_dir(done, size_bytes=1 * _GB, age_s=100_000, now=now)
    _make_dir(live, size_bytes=1 * _GB, age_s=100_000, now=now)

    # Register both, but with different registration times.
    dg.register_lane_target(done, root=str(tmp_path), env={}, now=now - 100_000)
    dg.register_lane_target(live, root=str(tmp_path), env={}, now=now - 100)

    result = dg.gc(
        root=str(tmp_path),
        config=_cfg(),
        now_fn=lambda: now,
        env={},
    )
    assert {Path(item["path"]) for item in result.reclaimed} == {done.resolve()}
    assert not done.exists()  # past the 6h TTL -> collected
    assert live.exists()  # registered 100s ago -> kept
    # The collected entry is dropped from the registry (compaction).
    registry = dg.read_registry(tmp_path.resolve())
    remaining = {Path(rec["path"]).name for rec in registry.values()}
    assert "codex-done" not in remaining
    assert "codex-live" in remaining


def test_gc_compacts_each_registered_worktree_registry(tmp_path):
    _, main, linked = _fake_registered_worktree_scope(tmp_path)
    now = 3_000_000.0
    main_done = main / "target" / "codex-main-done"
    linked_done = linked / "target" / "codex-linked-done"
    _make_dir(main_done, size_bytes=1, age_s=100_000, now=now)
    _make_dir(linked_done, size_bytes=1, age_s=100_000, now=now)
    dg.register_lane_target(main_done, root=main, env={}, now=now - 100_000)
    dg.register_lane_target(linked_done, root=linked, env={}, now=now - 100_000)

    result = dg.gc(root=main, config=_cfg(), now_fn=lambda: now, env={})

    assert {Path(item["path"]) for item in result.reclaimed} == {
        main_done.resolve(),
        linked_done.resolve(),
    }
    assert dg.read_registry(main.resolve()) == {}
    assert dg.read_registry(linked.resolve()) == {}


def test_gc_preserves_registered_target_under_live_guard(tmp_path):
    now = 3_000_000.0
    target = tmp_path / "target" / "codex-guarded"
    _make_dir(target, size_bytes=1, age_s=100_000, now=now)
    dg.register_lane_target(target, root=tmp_path, env={}, now=now - 100_000)
    markers = tmp_path / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    (markers / "guard-42-live.json").write_text(
        '{"pid":42,"status":"child_running"}\n', encoding="utf-8"
    )

    result = dg.gc(root=tmp_path, config=_cfg(), now_fn=lambda: now, env={})

    assert result.reclaimed == []
    assert target.exists()
    assert dg._norm(target) in dg.read_registry(tmp_path.resolve())


def test_register_lane_target_is_latest_wins(tmp_path):
    d = tmp_path / "target" / "codex-x"
    d.mkdir(parents=True)
    dg.register_lane_target(d, root=str(tmp_path), env={}, now=100.0)
    dg.register_lane_target(d, root=str(tmp_path), env={}, now=200.0)
    registry = dg.read_registry(tmp_path.resolve())
    rec = registry[dg._norm(d)]
    assert rec["registered_at"] == 200.0


# --- 6. hard safety rail: refuse to delete shared/out-of-set dirs -----------


def test_assert_safe_refuses_shared_target_dirs(tmp_path):
    root = tmp_path
    (root / "target" / "debug").mkdir(parents=True)
    (root / "target" / "x86_64-pc-windows-msvc").mkdir(parents=True)
    (root / "wt-somelane").mkdir(parents=True)  # a worktree checkout
    for shared in (
        root / "target" / "debug",
        root / "target" / "x86_64-pc-windows-msvc",
        root / "wt-somelane",
        root,  # the root itself
    ):
        with pytest.raises(ValueError):
            dg.assert_safe_to_delete(shared, root, dg.DEFAULT_LANE_GLOBS)


def test_assert_safe_allows_reclaimable(tmp_path):
    root = tmp_path
    lane = root / "target" / "codex-lane"
    sess = root / "target" / "sessions" / "s1"
    lane.mkdir(parents=True)
    sess.mkdir(parents=True)
    # Must not raise.
    dg.assert_safe_to_delete(lane, root, dg.DEFAULT_LANE_GLOBS)
    dg.assert_safe_to_delete(sess, root, dg.DEFAULT_LANE_GLOBS)


# --- 7. gate-liveness canary -------------------------------------------------


def test_selftest_canaries_all_live():
    results = dg.selftest()
    dead = [name for name, ok in results if not ok]
    assert not dead, f"dead disk_guard canaries: {dead}"
    assert len(results) >= 8


def test_completed_lane_event_reclaims_registered_target(tmp_path):
    target = tmp_path / "target" / "codex-finished"
    target.mkdir(parents=True)
    (target / "artifact.bin").write_bytes(b"x")
    dg.register_lane_target(target, root=tmp_path, now=10.0)
    result = dg.reclaim_completed_lane(target, root=tmp_path, env={})
    assert result.reclaimed == [
        {"path": str(target.resolve()), "kind": "completed-lane"}
    ]
    assert not target.exists()


def test_completed_lane_event_never_reclaims_active_lane():
    candidate = dg.Candidate(Path("/x/target/live"), "registered", 0.0)
    assert dg.decide_completed_lane_reclaim(candidate, completed=True, active=True) == (
        False,
        "lane-active",
    )


def test_completed_lane_event_respects_live_guard_marker(tmp_path):
    target = tmp_path / "target" / "codex-active"
    target.mkdir(parents=True)
    markers = tmp_path / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    (markers / "guard-42-live.json").write_text(
        '{"pid":42,"status":"child_running"}\n', encoding="utf-8"
    )

    result = dg.reclaim_completed_lane(target, root=tmp_path, env={})

    assert result.reclaimed == []
    assert result.skipped == [{"path": str(target.resolve()), "reason": "lane-active"}]
    assert target.exists()
