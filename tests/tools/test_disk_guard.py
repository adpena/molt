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
    assert len(results) >= 6
