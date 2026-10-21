"""Teeth for the deterministic, agent-safe disk guard (tools/disk_guard.py).

Proves (M05 -- a gate that cannot fail certifies nothing):

  * ``ensure_free`` reclaims stale dirs in a temp fixture and STOPS once the
    target free-space is met (no over-deletion);
  * an ACTIVE dir -- recently touched OR holding a Cargo build lock -- is NEVER
    reclaimed, by ``ensure_free`` and by the pure planner;
  * the AGENT-SAFETY source scan finds ZERO process-actuation tokens in the
    module's executable code (no kill / Popen / signal / taskkill / subprocess);
  * reclamation is IDEMPOTENT (a second call above threshold is a no-op),
    and measurement failures remain visible to callers;
  * the GC retires a pinned session's target once idle past its limit, and
    checkout state by last use and owner, never under a held lock;
  * the self-test canaries are all LIVE (gate-liveness).
"""

from __future__ import annotations

import io
import json
import errno
import os
import sys
import tokenize
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.disk_guard as dg  # noqa: E402
from molt import dx  # noqa: E402

_GB = 1024**3


def _guard_marker(markers: Path, pid: int, status: str) -> Path:
    token = f"{pid:032x}"
    marker = markers / f"guard-{pid}-{token}.json"
    marker.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "pid": pid,
                "token": token,
                "status": status,
                "guard_process": {"pid": pid, "started_at_ns": pid * 100},
                "child_launch_state": "recorded",
                "child_process": {"pid": pid + 1000, "started_at_ns": pid * 100 + 1},
            }
        ),
        encoding="utf-8",
    )
    return marker


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
        lane_globs=dg.DEFAULT_LANE_GLOBS,
    )
    base.update(over)
    return dg.GuardConfig(**base)


def _registered_worktree_scope(
    tmp_path: Path, *, separate_git_dir: Path | None = None
) -> tuple[Path, Path, Path]:
    """Produce registrations through the actual Git worktree boundary."""
    from tests.test_dx_run_context import _git

    scope = tmp_path
    main = scope / "molt-src"
    linked = scope / "worktrees" / "lane"
    main.mkdir(parents=True)
    options = (
        [] if separate_git_dir is None else [f"--separate-git-dir={separate_git_dir}"]
    )
    _git(main, "init", "-q", "-b", "main", *options)
    (main / "tracked").write_text("source\n", encoding="utf-8")
    _git(main, "add", "tracked")
    _git(main, "commit", "-q", "-m", "source")
    _git(main, "worktree", "add", "-q", "--detach", str(linked))
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


@pytest.mark.usefixtures("developer_host_context")
def test_long_current_session_target_is_protected_by_its_real_name(tmp_path):
    now = 2_000_000.0
    session = "agent-unit-agent-440e87baa421421f9c0d6f2e-67518"
    sessions = tmp_path / "target" / "sessions"
    # RunContext names the pinned session's target; the guard must protect
    # that directory, not a path spelled from the raw session ID.
    mine = Path(
        dx.RunContext(tmp_path).canonical_env(
            {"PATH": "/usr/bin", "MOLT_SESSION_ID": session}, create_dirs=False
        )["CARGO_TARGET_DIR"]
    )
    assert mine.parent == sessions.resolve()
    other = sessions / "codex-other"
    sized = {
        mine: _make_dir(mine, size_bytes=50 * _GB, age_s=100_000, now=now),
        other: _make_dir(other, size_bytes=5 * _GB, age_s=100_000, now=now),
    }
    result = dg.ensure_free(
        root=str(tmp_path),
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(start_free=1 * _GB, sized=sized),
        now_fn=lambda: now,
        env={"MOLT_SESSION_ID": session},
    )

    assert mine.exists()
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


def test_registered_worktree_discovery_is_filesystem_only_and_scope_bounded(
    tmp_path, monkeypatch
):
    from tests.test_dx_run_context import _git

    scope, main, linked = _registered_worktree_scope(tmp_path)
    outside = tmp_path.parent / f"{tmp_path.name}-outside-worktree"
    _git(main, "worktree", "add", "-q", "--detach", str(outside))
    monkeypatch.setenv("PATH", str(tmp_path / "no-git-tools"))

    assert set(dg.registered_worktree_roots(main)) == {
        main.resolve(),
        linked.resolve(),
    }
    assert set(dg.reclaim_roots(main)) == {
        scope.resolve(),
        main.resolve(),
        linked.resolve(),
    }


@pytest.mark.parametrize(
    "name",
    [
        "metadata-α-é",
        pytest.param(
            " metadata ",
            marks=pytest.mark.skipif(os.name == "nt", reason="Win32 trims path spaces"),
        ),
        pytest.param(
            "metadata\r\nline",
            marks=pytest.mark.skipif(os.name == "nt", reason="Win32 forbids CR/LF"),
        ),
        pytest.param(
            os.fsdecode(b"metadata-\xff"),
            marks=pytest.mark.skipif(os.name == "nt", reason="POSIX filesystem bytes"),
        ),
    ],
)
def test_separate_git_registration_preserves_path_bytes_without_guessing_main(
    tmp_path, monkeypatch, name
):
    from tests.test_dx_run_context import _git

    original = tmp_path / "initial-metadata"
    scope, main, linked = _registered_worktree_scope(
        tmp_path, separate_git_dir=original
    )
    common = tmp_path / name
    try:
        original.rename(common)
    except OSError as exc:
        if exc.errno == errno.EILSEQ and any(
            0xDC80 <= ord(char) <= 0xDCFF for char in name
        ):
            pytest.skip(f"fixture filesystem rejects non-UTF8 path bytes: {exc}")
        raise
    for checkout in (main, linked):
        pointer = checkout / ".git"
        recorded = pointer.read_bytes()
        assert recorded.count(os.fsencode(original)) == 1
        pointer.write_bytes(
            recorded.replace(os.fsencode(original), os.fsencode(common))
        )
    # Git validates the relocated records from ordinary checkout argv. This
    # also exercises POSIX bytes unsupported by the command-JSON transport.
    assert _git(main, "status", "--porcelain") == ""
    assert _git(linked, "status", "--porcelain") == ""
    monkeypatch.setenv("PATH", str(tmp_path / "no-git-tools"))

    # A separate common directory does not declare its parent as a checkout.
    assert dg.registered_worktree_roots(main) == (linked.resolve(),)
    assert dg.registered_worktree_roots(linked) == (linked.resolve(),)
    assert set(dg.reclaim_roots(main)) == {
        scope.resolve(),
        main.resolve(),
        linked.resolve(),
    }


@pytest.mark.parametrize("broken", ["forward", "reverse", "common"])
def test_broken_worktree_registration_never_authorizes_its_target_deletion(
    tmp_path, broken
):
    _, main, linked = _registered_worktree_scope(tmp_path)
    admin = dg.git_directories(linked)[0]
    if broken == "forward":
        (linked / ".git").write_bytes(b"GITDIR: " + os.fsencode(admin) + b"\n")
    elif broken == "reverse":
        (admin / "gitdir").write_bytes(os.fsencode(linked / "wrong-leaf") + b"\n")
    else:
        other = tmp_path / "unrelated-common"
        other.mkdir()
        (admin / "commondir").write_bytes(os.fsencode(other) + b"\n")
    now = 2_000_000.0
    target = linked / "target" / "sessions" / "keep"
    _make_dir(target, size_bytes=100 * _GB, age_s=100_000, now=now)

    assert linked.resolve() not in dg.registered_worktree_roots(main)
    result = dg.ensure_free(
        root=main,
        config=_cfg(),
        free_bytes_fn=_SimulatedVolume(1 * _GB, {target: 100 * _GB}),
        now_fn=lambda: now,
        env={},
    )
    assert result.reclaimed == []
    assert (target / "artifact.bin").read_bytes() == b"\0" * 4096
    assert (linked / "tracked").read_text(encoding="utf-8") == "source\n"


def test_malformed_root_git_metadata_refuses_before_deleting_any_target(tmp_path):
    _, main, _ = _registered_worktree_scope(tmp_path)
    (main / ".git" / "HEAD").unlink()
    now = 2_000_000.0
    target = main / "target" / "sessions" / "keep"
    _make_dir(target, size_bytes=100 * _GB, age_s=100_000, now=now)

    with pytest.raises(ValueError, match="stale Git checkout metadata"):
        dg.ensure_free(
            root=main,
            config=_cfg(),
            free_bytes_fn=_SimulatedVolume(1 * _GB, {target: 100 * _GB}),
            now_fn=lambda: now,
            env={},
        )
    assert (target / "artifact.bin").read_bytes() == b"\0" * 4096


def test_ensure_free_reclaims_across_registered_worktrees_only(tmp_path):
    scope, main, linked = _registered_worktree_scope(tmp_path)
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
    _, main, linked = _registered_worktree_scope(tmp_path)
    now = 2_000_000.0
    main_session = main / "target" / "sessions" / "main-old"
    # A guard keeps its worktree's hand-named lane target; a session target
    # is a last-use root, which guard markers do not keep.
    linked_session = linked / "target" / "codex-linked-active"
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
    _guard_marker(markers, 1, "child_running")

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
    # The result names the blocking record and the command that inspects it.
    (custody,) = result.custody
    assert custody["active_dir"] == str(markers)
    assert custody["blocking_records"] == 1
    assert str(markers / f"guard-1-{1:032x}.json") in custody["examples"][0]
    assert "tools/memory_guard_custody.py --active-dir" in custody["next_step"]
    assert result.to_dict()["custody"] == result.custody

    _guard_marker(markers, 1, "completed")
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
    assert result.custody == []


def test_terminal_parent_watched_pid_cannot_retire_nested_marker(tmp_path):
    markers = tmp_path / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    nested = _guard_marker(markers, 7, "child_running")
    os.utime(nested, (100.0, 100.0))
    parent = _guard_marker(markers, 1, "completed")
    payload = json.loads(parent.read_text(encoding="utf-8"))
    payload["termination_reports"] = [{"watched_pids": [7]}]
    parent.write_text(json.dumps(payload), encoding="utf-8")
    os.utime(parent, (200.0, 200.0))

    assert dg._has_active_guard(tmp_path) is True


def test_incident_capsules_in_active_directory_do_not_claim_guard_custody(tmp_path):
    markers = tmp_path / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    (markers / "manual-death-capsule.json").write_text(
        '{"status":"starting","command":"cargo test"}\n', encoding="utf-8"
    )

    assert dg._has_active_guard(tmp_path) is False


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
        assert dg._lock_held(d) is True
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


# --- 4. idempotence and visible measurement failures -----------------------


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


# --- 5. hard safety rail: refuse to delete shared/out-of-set dirs -----------


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


# --- 6. gate-liveness canary -------------------------------------------------


def test_selftest_canaries_all_live():
    results = dg.selftest()
    dead = [name for name, ok in results if not ok]
    assert not dead, f"dead disk_guard canaries: {dead}"
    assert len(results) >= 8


def test_owner_retirement_skips_the_idle_window_never_a_lock_or_protection():
    target = Path("/x/target/checkouts/alpha-0123456789ab")
    used_now = dg.Candidate(target, "checkout-state", mtime=1_000_000.0)
    assert dg.decide_owner_reclaim(used_now) == (True, "owner-retired")
    locked = dg.Candidate(target, "checkout-state", mtime=0.0, lock_held=True)
    assert dg.decide_owner_reclaim(locked) == (False, "lock-held")
    # A protected session inside the target keeps the target.
    running = target / "sessions" / "shard-7"
    assert dg.decide_owner_reclaim(used_now, protected=[running]) == (
        False,
        "protected",
    )


@pytest.mark.parametrize("name", ["ordinary", "OneDrive - selected artifacts"])
def test_root_selection_requires_explicit_custody(tmp_path, name):
    selected = tmp_path / name
    selected.mkdir()
    unrelated = tmp_path / "unrelated"
    unrelated.mkdir()
    assert (
        dg.resolve_root(selected, env={"MOLT_EXT_ROOT": str(unrelated)})
        == selected.resolve()
    )
    assert dg.resolve_root(env={"MOLT_EXT_ROOT": str(selected)}) == selected.resolve()
    with pytest.raises(SystemExit, match="could not resolve the artifact root"):
        dg.resolve_root(env={})
    with pytest.raises(SystemExit):
        dg.resolve_root(Path(tmp_path.anchor), env={})


def test_out_of_tree_markers_protect_a_checkout_that_is_its_own_root(
    tmp_path, monkeypatch
):
    import tempfile

    from molt import custody_layout

    host_tmp = tmp_path / "host-tmp"
    host_tmp.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(host_tmp))
    clone = tmp_path / "clone"
    clone.mkdir()
    # A plain clone that is its own artifact root keeps guard state out of
    # its tree; the reclaimer must still see that guard.
    markers = custody_layout.out_of_tree_scratch_root(clone) / "memory_guard"
    markers = markers / "active"
    markers.mkdir(parents=True)
    _guard_marker(markers, 1, "child_running")

    assert not (clone / "tmp").exists()
    assert dg._has_active_guard(clone) is True


# --- HF-313: checkout state retires by last use and by its removed owner ------

HOUR, DAY = 3600.0, 86400.0


def _checkout_state(ext: Path, checkout: Path) -> dict[str, Path]:
    """Materialize each state root through the producer that places it."""
    from molt import checkout_state

    env = {"MOLT_EXT_ROOT": str(ext)}
    roots = {
        "cargo-target": dx.cargo_target_dir_for_artifact_root(
            ext, None, source_root=checkout
        ),
        "uv-environment": dx.stable_uv_project_env_dir(
            ext, purpose="dx", python="3.12", source_root=checkout
        ),
        "pytest-cache": checkout_state.checkout_scratch_state_dir(
            "pytest-cache", checkout, env
        ),
        "hypothesis-home": checkout_state.checkout_scratch_state_dir(
            "hypothesis-home", checkout, env
        ),
    }
    from molt.build_state_layout import build_state_root

    roots["build-control"] = build_state_root(
        project_root=checkout,
        cargo_target=roots["cargo-target"],
        environment=env,
    )
    for root in roots.values():
        root.mkdir(parents=True)
        (root / "payload.bin").write_bytes(b"\0" * 64)
    (roots["uv-environment"] / "pyvenv.cfg").write_text("home = /x\n", "utf-8")
    return roots


def _checkout_family(tmp_path: Path) -> tuple[Path, Path, dict[str, str]]:
    ext = tmp_path / "artifacts"
    projects = tmp_path / "projects"
    ext.mkdir()
    projects.mkdir()
    return ext, projects, {"MOLT_EXT_ROOT": str(ext)}


def _last_used(roots, seconds_ago: float, now: float) -> None:
    """Set every root's stamp and activity to ``seconds_ago`` before ``now``."""
    stamp = now - seconds_ago
    for root in roots:
        for path in (*root.iterdir(), root):
            os.utime(path, (stamp, stamp), follow_symlinks=False)


def _gc(ext: Path, env: dict[str, str], now: float):
    return dg.gc(root=ext, config=_cfg(), env=env, now_fn=lambda: now, log=False)


def _reclaimed(result) -> set[str]:
    return {row["path"] for row in result.reclaimed}


def _resolved(roots) -> set[str]:
    return {str(path.resolve()) for path in roots}


def test_producers_place_checkout_state_where_the_guard_looks(tmp_path):
    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    checkout = projects / "alpha"
    checkout.mkdir()
    roots = _checkout_state(ext, checkout)
    found = {
        state.kind: state.path
        for state in checkout_state.checkout_state_roots(checkout, env)
    }
    assert found == {kind: path.resolve() for kind, path in roots.items()}
    assert {p.resolve() for p in dg._checkout_state_dirs(ext)} == set(found.values())
    for kind, path in found.items():
        assert checkout_state.checkout_state_kind_of(path, ext.resolve()) == kind


def test_a_removed_checkouts_state_retires_after_the_protection_window(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone, recent, live = projects / "gone", projects / "recent", projects / "live"
    for checkout in (gone, recent, live):
        checkout.mkdir()
    gone_roots = _checkout_state(ext, gone)
    recent_roots = _checkout_state(ext, recent)
    live_roots = _checkout_state(ext, live)
    # A root without an owner record has no proven owner, even if it is gone.
    unrecorded = _checkout_state(ext, projects / "never-stamped")
    for checkout in (gone, recent, live):
        stamped = checkout_state.stamp_checkout_state(checkout, env)
        assert len(stamped) == 5
    gone.rmdir()
    recent.rmdir()
    now = time.time()
    _last_used([*gone_roots.values(), *live_roots.values()], 3 * HOUR, now)
    _last_used(unrecorded.values(), 3 * HOUR, now)
    _last_used(recent_roots.values(), 1 * HOUR, now)

    result = _gc(ext, env, now)

    assert _reclaimed(result) == _resolved(gone_roots.values())
    assert {row["kind"] for row in result.reclaimed} == {"checkout-state"}
    kept = (*recent_roots.values(), *live_roots.values(), *unrecorded.values())
    assert all(path.is_dir() for path in kept)
    assert _gc(ext, env, now).reclaimed == []


def test_any_checkout_state_retires_once_unused_past_the_max_age(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    old, young = projects / "old", projects / "young"
    old.mkdir()
    young.mkdir()
    old_roots = _checkout_state(ext, old)
    young_roots = _checkout_state(ext, young)
    checkout_state.stamp_checkout_state(old, env)
    checkout_state.stamp_checkout_state(young, env)
    # A root from before stamps existed ages by its own activity.
    legacy = _checkout_state(ext, projects / "legacy")
    now = time.time()
    _last_used([*old_roots.values(), *legacy.values()], 31 * DAY, now)
    _last_used(young_roots.values(), 29 * DAY, now)

    result = _gc(ext, env, now)

    # The owner of `old` still exists: an unused cache retires anyway.
    assert _reclaimed(result) == _resolved([*old_roots.values(), *legacy.values()])
    assert all(path.is_dir() for path in young_roots.values())
    shorter = {**env, checkout_state.MAX_AGE_ENV: "28"}
    assert _reclaimed(_gc(ext, shorter, now)) == _resolved(young_roots.values())


@pytest.mark.parametrize(
    "name", ["MOLT_CHECKOUT_STATE_MAX_AGE_DAYS", "MOLT_SESSION_TARGET_IDLE_HOURS"]
)
# 0.05 days and 0.05 hours both undercut the two-hour protection window.
@pytest.mark.parametrize("raw", ["0", "-3", "soon", "nan", "inf", "0.05"])
def test_an_invalid_idle_limit_fails_the_collection(tmp_path, name, raw):
    ext, _projects, env = _checkout_family(tmp_path)
    with pytest.raises(ValueError, match=name):
        _gc(ext, {**env, name: raw}, 0.0)


def test_an_idle_limit_may_equal_the_protection_window():
    from molt import checkout_state

    limits = checkout_state.retention_limits(
        {checkout_state.SESSION_IDLE_ENV: "2", checkout_state.MAX_AGE_ENV: "1"}
    )
    assert limits == checkout_state.RetentionLimits(
        max_age_s=DAY, session_idle_s=checkout_state.PROTECTION_WINDOW_S
    )
    assert checkout_state.retention_limits({}) == checkout_state.RetentionLimits(
        max_age_s=30 * DAY, session_idle_s=6 * HOUR
    )


def test_last_use_counts_a_write_that_left_the_stamp_old(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    checkout = projects / "alpha"
    checkout.mkdir()
    roots = _checkout_state(ext, checkout)
    checkout_state.stamp_checkout_state(checkout, env)
    target = roots["cargo-target"]
    now = time.time()
    _last_used([target], 40 * DAY, now)
    # A build wrote the target without stamping it.
    fresh = target / "debug"
    fresh.mkdir()
    os.utime(target, (now - 40 * DAY, now - 40 * DAY))
    os.utime(fresh, (now - HOUR, now - HOUR))
    assert checkout_state.checkout_state_last_use(target) == pytest.approx(now - HOUR)
    assert str(target.resolve()) not in _reclaimed(_gc(ext, env, now))


def test_a_use_rewrites_the_stamp_at_most_once_per_resolution(tmp_path):
    from molt import checkout_state

    root = tmp_path / "artifacts" / "target" / "checkouts" / "alpha-0123456789ab"
    checkout_state.record_checkout_state_use(root, now=1_000.0)
    assert not root.exists(), "recording a use never creates a root"
    root.mkdir(parents=True)
    stamp = root / checkout_state.LAST_USE_STAMP
    checkout_state.record_checkout_state_use(root, now=1_000_000.0)
    assert stamp.stat().st_mtime == 1_000_000.0
    checkout_state.record_checkout_state_use(root, now=1_000_000.0 + 0.5 * HOUR)
    assert stamp.stat().st_mtime == 1_000_000.0
    checkout_state.record_checkout_state_use(root, now=1_000_000.0 + 1.5 * HOUR)
    assert stamp.stat().st_mtime == 1_000_000.0 + 1.5 * HOUR
    session = root / "sessions" / "shard-7"
    session.mkdir(parents=True)
    stamp.unlink()
    checkout_state.record_checkout_target_use(session, checkout=tmp_path / "x")
    assert stamp.is_file(), "a pinned session's use marks its checkout target"
    assert (session / checkout_state.LAST_USE_STAMP).is_file(), "and the session"
    # The root's name is not checkout x's component: no record binds it to x.
    assert not (root / checkout_state.OWNER_RECORD).exists()


def _forge(record: Path, **changes) -> None:
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload.update(changes)
    record.write_text(json.dumps(payload), encoding="utf-8")


@pytest.mark.parametrize(
    "case",
    [
        "component",
        "kind",
        "relative",
        "extra-key",
        "owner-present",
        "owner-volume-missing",
        "foreign-record",
    ],
)
def test_a_root_without_exact_owner_proof_waits_for_the_max_age(tmp_path, case):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(gone, env)
    # A pytest cache and a Cargo target are both named by the component, so
    # only the kind binding refuses a record of the other kind.
    target = roots["pytest-cache" if case == "kind" else "cargo-target"]
    record = target / checkout_state.OWNER_RECORD
    if case == "component":
        # A real, absent checkout whose component is not this root's name.
        _forge(record, owner=str(projects / "elsewhere"))
    elif case == "kind":
        _forge(record, kind="cargo-target")
    elif case == "relative":
        _forge(record, owner="projects/gone")
    elif case == "extra-key":
        _forge(record, note="x")
    elif case == "foreign-record":
        # A valid record copied from another checkout's target.
        other = projects / "other"
        other.mkdir()
        other_roots = _checkout_state(ext, other)
        checkout_state.stamp_checkout_state(other, env)
        record.write_bytes(
            (other_roots["cargo-target"] / checkout_state.OWNER_RECORD).read_bytes()
        )
        other.rmdir()
    if case != "owner-present":
        gone.rmdir()
    if case == "owner-volume-missing":
        # The owner's parent is gone too, as for an unmounted volume.
        unmounted = tmp_path / "volume" / "gone"
        _forge(record, owner=str(unmounted))
        moved = target.parent / dx.checkout_component(unmounted)
        target.rename(moved)
        target = moved
    now = time.time()
    _last_used([target], 3 * HOUR, now)
    assert str(target.resolve()) not in _reclaimed(_gc(ext, env, now))
    assert target.is_dir()
    _last_used([target], 31 * DAY, now)
    assert str(target.resolve()) in _reclaimed(_gc(ext, env, now))


def test_foreign_and_symlinked_entries_are_never_checkout_state(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    elsewhere = tmp_path / "elsewhere"
    roots = _checkout_state(elsewhere, gone)
    checkout_state.stamp_checkout_state(gone, {"MOLT_EXT_ROOT": str(elsewhere)})
    gone.rmdir()
    containers = ext / "target" / "checkouts"
    containers.mkdir(parents=True)
    link = containers / dx.checkout_component(gone)
    link.symlink_to(roots["cargo-target"], target_is_directory=True)
    foreign = [
        containers / "notes",
        ext / "uv-project-envs" / "my-env",
        containers / "x",
    ]
    for path in foreign:
        path.mkdir(parents=True)
    now = time.time()
    _last_used([roots["cargo-target"], *foreign], 60 * DAY, now)
    for path in foreign:
        assert dg._is_reclaimable(path, ext, dg.DEFAULT_LANE_GLOBS) is None
    assert _gc(ext, env, now).reclaimed == []
    assert (roots["cargo-target"] / "payload.bin").is_file()
    assert all(path.is_dir() for path in foreign)


def test_live_use_keeps_checkout_state_from_every_reclaim(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    target = roots["cargo-target"]
    now = time.time()
    _last_used(roots.values(), 60 * DAY, now)
    protected = {**env, "CARGO_TARGET_DIR": str(target)}
    assert str(target.resolve()) not in _reclaimed(_gc(ext, protected, now))
    retired = dg.reclaim_checkout_state(gone, root=ext, env=protected)
    assert {row["path"]: row["reason"] for row in retired.skipped} == {
        str(target.resolve()): "protected"
    }
    if os.name == "nt":
        return
    import fcntl

    other = projects / "other"
    other.mkdir()
    other_roots = _checkout_state(ext, other)
    checkout_state.stamp_checkout_state(other, env)
    other.rmdir()
    locked = other_roots["cargo-target"]
    _last_used(other_roots.values(), 60 * DAY, now)
    with open(locked / ".cargo-lock", "a+b") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        os.utime(locked / ".cargo-lock", (now - 60 * DAY, now - 60 * DAY))
        assert str(locked.resolve()) not in _reclaimed(_gc(ext, env, now))
        retired = dg.reclaim_checkout_state(other, root=ext, env=env)
        assert {row["path"]: row["reason"] for row in retired.skipped} == {
            str(locked.resolve()): "lock-held"
        }


def test_pressure_evicts_checkout_state_by_last_use_outside_the_window(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    ages = {"oldest": 20 * DAY, "older": 10 * DAY, "recent": 1 * HOUR}
    roots = {}
    for name, age in ages.items():
        checkout = projects / name
        checkout.mkdir()
        roots[name] = _checkout_state(ext, checkout)["cargo-target"]
        checkout_state.stamp_checkout_state(checkout, env)
    now = time.time()
    for name, age in ages.items():
        _last_used([roots[name]], age, now)
    # Freeing the oldest root alone meets the target.
    volume = _SimulatedVolume(
        start_free=1 * _GB, sized={roots["oldest"]: 50 * _GB, roots["older"]: 50 * _GB}
    )
    result = dg.ensure_free(
        root=ext, config=_cfg(), env=env, free_bytes_fn=volume, now_fn=lambda: now
    )
    assert _reclaimed(result) == {str(roots["oldest"].resolve())}
    assert roots["older"].is_dir() and roots["recent"].is_dir()
    volume.start_free = -100 * _GB
    again = dg.ensure_free(
        root=ext, config=_cfg(), env=env, free_bytes_fn=volume, now_fn=lambda: now
    )
    assert _reclaimed(again) == {str(roots["older"].resolve())}
    assert roots["recent"].is_dir(), "a root used within the window survives pressure"


def test_retirement_reclaims_only_its_checkouts_state_at_once(tmp_path):
    from molt import checkout_state
    from molt.memory_guard_paths import active_guard_marker_dirs_of

    ext, projects, env = _checkout_family(tmp_path)
    retired, earlier = projects / "retired", projects / "earlier"
    retired.mkdir()
    earlier.mkdir()
    retired_roots = _checkout_state(ext, retired)
    earlier_roots = _checkout_state(ext, earlier)
    checkout_state.stamp_checkout_state(retired, env)
    checkout_state.stamp_checkout_state(earlier, env)
    retired.rmdir()
    earlier.rmdir()
    # A guard elsewhere in the artifact root says nothing about these roots.
    markers = active_guard_marker_dirs_of(ext)[0]
    markers.mkdir(parents=True)
    _guard_marker(markers, 4242, "child_running")

    dry = dg.reclaim_checkout_state(retired, root=ext, env=env, apply=False)
    assert len(dry.reclaimed) == 5 and all(p.is_dir() for p in retired_roots.values())
    result = dg.reclaim_checkout_state(retired, root=ext, env=env)
    assert _reclaimed(result) == _resolved(retired_roots.values())
    assert not any(path.exists() for path in retired_roots.values())
    # Fresh state of another removed checkout waits for the protection window.
    assert all(path.is_dir() for path in earlier_roots.values())


def test_the_running_interpreters_environment_is_never_reclaimed(tmp_path):
    import venv

    from molt import checkout_state
    from tests.process_guard_common import run_guarded_test_process

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    # The retiring process may run from the uv environment of its checkout.
    venv.create(roots["uv-environment"], with_pip=False, symlinks=os.name != "nt")
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    python = roots["uv-environment"] / (
        "Scripts/python.exe" if os.name == "nt" else "bin/python"
    )
    code = (
        "import json, sys; from pathlib import Path\n"
        f"sys.path.insert(0, {str(ROOT)!r})\n"
        "import tools.disk_guard as dg\n"
        "r = dg.reclaim_checkout_state(Path(sys.argv[1]), root=sys.argv[2], env={})\n"
        "print(json.dumps([sys.prefix, r.skipped, r.reclaimed]))\n"
    )
    completed = run_guarded_test_process(
        [str(python), "-c", code, str(gone), str(ext)], cwd=tmp_path, timeout=120
    )
    assert completed.returncode == 0, completed.stderr
    prefix, skipped, reclaimed = json.loads(completed.stdout.splitlines()[-1])
    assert Path(prefix).resolve() == roots["uv-environment"].resolve()
    assert {row["path"]: row["reason"] for row in skipped} == {
        str(roots["uv-environment"].resolve()): "protected"
    }
    assert roots["uv-environment"].is_dir()
    assert len(reclaimed) == 4 and not roots["cargo-target"].exists()


def test_dx_collects_idle_checkout_state_at_most_once_a_day(tmp_path):
    import time

    from molt import checkout_state
    from tests.process_guard_common import run_guarded_test_process

    ext, projects, env = _checkout_family(tmp_path)
    gone, user = projects / "gone", projects / "user"
    gone.mkdir()
    user.mkdir()
    gone_roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    now = time.time()
    _last_used(gone_roots.values(), 3 * HOUR, now)
    code = (
        "import sys; from pathlib import Path\n"
        f"sys.path.insert(0, {str(ROOT / 'src')!r})\n"
        "from molt.dx import RunContext\n"
        "RunContext(Path(sys.argv[1]), session_prefix='t').canonical_env(\n"
        "    {'MOLT_EXT_ROOT': sys.argv[2], 'PATH': '/usr/bin:/bin'}, create_dirs=True)\n"
    )
    # canonical_env collects only outside pytest, as for lane registration.
    clean = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("PYTEST_", "MOLT_PYTEST_"))
    }

    def run() -> None:
        completed = run_guarded_test_process(
            [sys.executable, "-c", code, str(user), str(ext)],
            cwd=tmp_path,
            env=clean,
            timeout=120,
        )
        assert completed.returncode == 0, completed.stderr

    run()
    assert not any(path.exists() for path in gone_roots.values())
    assert ext.joinpath(*checkout_state.GC_STAMP).is_file()
    # The next idle orphan waits for tomorrow's collection.
    later = projects / "later"
    later.mkdir()
    later_roots = _checkout_state(ext, later)
    checkout_state.stamp_checkout_state(later, env)
    later.rmdir()
    _last_used(later_roots.values(), 3 * HOUR, now)
    run()
    assert all(path.is_dir() for path in later_roots.values())
    assert not checkout_state.checkout_state_gc_due(ext)
    assert checkout_state.checkout_state_gc_due(ext, now=now + 25 * HOUR)


def test_the_daily_collection_also_relieves_disk_pressure(tmp_path):
    import time

    from molt import checkout_state
    from molt.disk_capacity import DISK_GUARD_HIGH_WATER_ENV

    ext, projects, env = _checkout_family(tmp_path)
    idle, fresh = projects / "idle", projects / "fresh"
    idle.mkdir()
    fresh.mkdir()
    idle_roots = _checkout_state(ext, idle)
    fresh_roots = _checkout_state(ext, fresh)
    checkout_state.stamp_checkout_state(idle, env)
    checkout_state.stamp_checkout_state(fresh, env)
    now = time.time()
    _last_used(idle_roots.values(), 3 * HOUR, now)
    calm = dg.collect(root=ext, env=env)
    # Both owners exist and neither root is past the max age: the GC keeps
    # them, and the pressure pass does nothing above the high-water mark.
    assert [result.reclaimed for result in calm] == [[], []]
    # A high-water mark above any real volume puts the root under pressure.
    pressed = {
        **env,
        DISK_GUARD_HIGH_WATER_ENV: "1000000000",
        dg.ENV_TARGET: "1000000001",
    }
    gc_result, pressure = dg.collect(root=ext, env=pressed)
    assert gc_result.reclaimed == []
    assert _reclaimed(pressure) == _resolved(idle_roots.values())
    assert all(path.is_dir() for path in fresh_roots.values())


@pytest.mark.skipif(os.name == "nt", reason="POSIX flock")
@pytest.mark.parametrize(
    "kind,lock",
    [
        ("cargo-target", ".cargo-lock"),
        ("cargo-target", "debug/.cargo-lock"),
        ("cargo-target", "wasm32-wasip1/dev-fast/.cargo-lock"),
        ("cargo-target", "sessions/shard-7/debug/.cargo-lock"),
        ("cargo-target", "sessions/shard-7/wasm32-wasip1/release/.cargo-lock"),
        ("build-control", "build_locks/runtime.dev-fast.native.lock"),
        ("build-control", "diff_run.lock"),
        ("build-control", "backend_daemon/suite-leases/0123abcd/owner.lock"),
    ],
)
def test_a_held_lock_below_a_root_keeps_it(tmp_path, kind, lock):
    import fcntl
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    root = roots[kind]
    held = root / lock
    held.parent.mkdir(parents=True, exist_ok=True)
    held.write_bytes(b"")
    now = time.time()
    _last_used(roots.values(), 60 * DAY, now)
    with open(held, "a+b") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        assert dg._lock_held(root)
        assert str(root.resolve()) not in _reclaimed(_gc(ext, env, now))
        assert root.is_dir()
    assert not dg._lock_held(root)
    assert str(root.resolve()) in _reclaimed(_gc(ext, env, now))


def test_build_control_binds_only_to_the_target_that_names_it(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone, other = projects / "gone", projects / "other"
    gone.mkdir()
    other.mkdir()
    roots = _checkout_state(ext, gone)
    other_roots = _checkout_state(ext, other)
    checkout_state.stamp_checkout_state(gone, env)
    checkout_state.stamp_checkout_state(other, env)
    control = roots["build-control"]
    owner = checkout_state.read_checkout_owner(control)
    assert owner == checkout_state.CheckoutOwner(
        roots["cargo-target"].resolve(), "build-control"
    )
    # A valid record from another target's build control does not bind here.
    (control / checkout_state.OWNER_RECORD).write_bytes(
        (other_roots["build-control"] / checkout_state.OWNER_RECORD).read_bytes()
    )
    gone.rmdir()
    other.rmdir()
    now = time.time()
    _last_used([*roots.values(), *other_roots.values()], 3 * HOUR, now)
    reclaimed = _reclaimed(_gc(ext, env, now))
    assert str(control.resolve()) not in reclaimed
    assert str(other_roots["build-control"].resolve()) in reclaimed


# --- HF-313: a pinned session's target retires by its own last use ----------

LONG_SESSION = "agent-unit-agent-440e87baa421421f9c0d6f2e-67518"


def _session(ext: Path, checkout: Path, session_id: str) -> tuple[Path, Path]:
    """Materialize a pinned session's target and build control by their producers."""
    from molt.build_state_layout import build_state_root

    target = dx.cargo_target_dir_for_artifact_root(
        ext, session_id, source_root=checkout
    )
    control = build_state_root(
        project_root=checkout,
        cargo_target=target,
        environment={"MOLT_EXT_ROOT": str(ext)},
    )
    for root in (target, control):
        root.mkdir(parents=True)
        (root / "payload.bin").write_bytes(b"\0" * 64)
    return target, control


def _pressed(env: dict[str, str]) -> dict[str, str]:
    """``env`` with a high-water mark above any real volume."""
    from molt.disk_capacity import DISK_GUARD_HIGH_WATER_ENV

    return {**env, DISK_GUARD_HIGH_WATER_ENV: "1000000000", dg.ENV_TARGET: "1000000001"}


def test_producers_place_session_targets_where_the_guard_looks(tmp_path):
    from molt import checkout_state

    ext, projects, _env = _checkout_family(tmp_path)
    checkout = projects / "alpha"
    checkout.mkdir()
    shared = [
        dx.cargo_target_dir_for_artifact_root(ext, session, source_root=checkout)
        for session in ("shard-7", LONG_SESSION)
    ]
    # A plain clone keeps its target, and its sessions, inside the checkout.
    clone = dx.cargo_target_dir_for_artifact_root(
        checkout, "shard-7", source_root=checkout
    )
    legacy = ext / "target" / "sessions" / "diff-59785"
    for path in (*shared, clone, legacy):
        path.mkdir(parents=True)
    found = {path.resolve() for path in checkout_state.session_target_dirs(ext)}
    assert found == {path.resolve() for path in (*shared, legacy)}
    assert checkout_state.session_target_dirs(checkout) == [clone]
    for path, root in [*((path, ext) for path in shared), (clone, checkout)]:
        assert checkout_state.is_session_target(path, root)
        assert dg._is_reclaimable(path, root, dg.DEFAULT_LANE_GLOBS) == (
            "session-target"
        )
    holder = shared[0].parent.parent
    foreign = [
        holder / "sessions" / "a.b",
        holder / "sessions" / ("x" * 33),
        ext / "target" / "checkouts" / "notes" / "sessions" / "shard-7",
        ext / "elsewhere" / "sessions" / "shard-7",
    ]
    for path in foreign:
        path.mkdir(parents=True)
    (holder / "sessions" / "linked").symlink_to(shared[1], target_is_directory=True)
    for path in (*foreign, holder, holder / "sessions"):
        assert not checkout_state.is_session_target(path, ext)
        assert dg._is_reclaimable(path, ext, dg.DEFAULT_LANE_GLOBS) != (
            "session-target"
        )
    after = {path.resolve() for path in checkout_state.session_target_dirs(ext)}
    assert after == found


def test_an_idle_session_target_retires_and_a_recent_one_stays(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    live = projects / "live"
    live.mkdir()
    roots = _checkout_state(ext, live)
    idle, idle_control = _session(ext, live, "idle")
    recent, recent_control = _session(ext, live, "recent")
    legacy = ext / "target" / "sessions" / "legacy"
    legacy.mkdir(parents=True)
    checkout_state.stamp_checkout_state(live, env)
    now = time.time()
    _last_used(roots.values(), 1 * HOUR, now)
    _last_used([idle, idle_control, legacy], 7 * HOUR, now)
    _last_used([recent, recent_control], 5 * HOUR, now)

    result = _gc(ext, env, now)

    # The session goes, then its build control, whose owner it was.
    assert _reclaimed(result) == _resolved([idle, idle_control, legacy])
    assert {row["path"]: row["kind"] for row in result.reclaimed}[
        str(idle.resolve())
    ] == "session-target"
    assert result.errors == []
    assert all(path.is_dir() for path in (recent, recent_control, *roots.values()))
    shorter = {**env, checkout_state.SESSION_IDLE_ENV: "4"}
    assert _reclaimed(_gc(ext, shorter, now)) == _resolved([recent, recent_control])
    assert all(path.is_dir() for path in roots.values())


@pytest.mark.usefixtures("developer_host_context")
def test_a_session_use_keeps_its_target_from_retiring(tmp_path, monkeypatch):
    import time

    from molt import checkout_state
    from molt.cli import runtime_paths

    ext, projects, env = _checkout_family(tmp_path)
    checkout = projects / "alpha"
    checkout.mkdir()
    roots = _checkout_state(ext, checkout)
    session, _control = _session(ext, checkout, "shard-7")
    checkout_state.stamp_checkout_state(checkout, env)
    stamp = session / checkout_state.LAST_USE_STAMP

    def idle_for(hours: float) -> float:
        now = time.time()
        _last_used([*roots.values(), session], hours * HOUR, now)
        assert now - checkout_state.checkout_state_last_use(session) >= hours * HOUR
        return now

    # The run context selects the session's target and records the use.
    idle_for(7)
    selected = dx.RunContext(checkout, session_prefix="t").canonical_env(
        {
            "PATH": "/usr/bin:/bin",
            "MOLT_EXT_ROOT": str(ext),
            "MOLT_SESSION_ID": "shard-7",
        }
    )
    assert Path(selected["CARGO_TARGET_DIR"]) == session
    assert time.time() - stamp.stat().st_mtime < 60
    assert str(session.resolve()) not in _reclaimed(_gc(ext, env, time.time()))

    # A CLI build that resolves the session's target records the use too.
    idle_for(7)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(session))
    monkeypatch.setattr(runtime_paths, "_CARGO_TARGET_ROOTS", {})
    assert runtime_paths._cargo_target_root(checkout) == session
    assert time.time() - stamp.stat().st_mtime < 60
    assert str(session.resolve()) not in _reclaimed(_gc(ext, env, time.time()))

    # Without a use it retires.
    now = idle_for(7)
    assert str(session.resolve()) in _reclaimed(_gc(ext, env, now))


def test_pressure_evicts_session_targets_by_last_use_outside_the_window(tmp_path):
    import time

    from molt import checkout_state
    from molt.memory_guard_paths import active_guard_marker_dirs_of

    ext, projects, env = _checkout_family(tmp_path)
    live = projects / "live"
    live.mkdir()
    roots = _checkout_state(ext, live)
    ages = {"older": 5 * HOUR, "old": 3 * HOUR, "recent": 1 * HOUR}
    sessions = {name: _session(ext, live, name) for name in ages}
    checkout_state.stamp_checkout_state(live, env)
    # A guard elsewhere in the artifact root says nothing about these roots.
    markers = active_guard_marker_dirs_of(ext)[0]
    markers.mkdir(parents=True)
    _guard_marker(markers, 4242, "child_running")
    now = time.time()
    targets = {name: pair[0] for name, pair in sessions.items()}
    # Only the session targets are outside the protection window.
    _last_used([*roots.values(), *(pair[1] for pair in sessions.values())], HOUR, now)
    for name, age in ages.items():
        _last_used([targets[name]], age, now)
    # Every session is inside its idle limit, so the GC keeps them all.
    assert _gc(ext, env, now).reclaimed == []
    volume = _SimulatedVolume(
        start_free=1 * _GB, sized={targets["older"]: 50 * _GB, targets["old"]: 50 * _GB}
    )

    def evict():
        return dg.ensure_free(
            root=ext, config=_cfg(), env=env, free_bytes_fn=volume, now_fn=lambda: now
        )

    assert _reclaimed(evict()) == {str(targets["older"].resolve())}
    assert targets["old"].is_dir()
    volume.start_free = -100 * _GB
    pressed = evict()
    assert str(targets["old"].resolve()) in _reclaimed(pressed)
    assert targets["recent"].is_dir(), "a session used within the window survives"
    reasons = {row["path"]: row["reason"] for row in pressed.skipped}
    assert reasons[str(targets["recent"].resolve())] == (
        "used within the protection window"
    )


@pytest.mark.skipif(os.name == "nt", reason="POSIX flock")
@pytest.mark.parametrize(
    "where,lock",
    [
        ("target", ".cargo-lock"),
        ("target", "debug/.cargo-lock"),
        ("target", "wasm32-wasip1/release/.cargo-lock"),
        ("control", "build_locks/runtime.dev-fast.native.lock"),
        ("control", "diff_run.lock"),
        ("control", "backend_daemon/suite-leases/0123abcd/owner.lock"),
    ],
)
def test_a_held_lock_keeps_a_session_target(tmp_path, where, lock):
    import fcntl
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    session, control = _session(ext, gone, "shard-7")
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    held = (session if where == "target" else control) / lock
    held.parent.mkdir(parents=True, exist_ok=True)
    held.write_bytes(b"")
    now = time.time()
    _last_used([*roots.values(), session, control], 60 * DAY, now)
    target = roots["cargo-target"]
    with open(held, "a+b") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        for result in (
            _gc(ext, env, now),
            dg.ensure_free(root=ext, config=_cfg(), env=_pressed(env), log=False),
        ):
            kept = {str(session.resolve()), str(target.resolve())}
            assert not kept & _reclaimed(result)
        # The checkout's retirement keeps the target that holds the session.
        retired = dg.reclaim_checkout_state(gone, root=ext, env=env)
        assert {row["path"]: row["reason"] for row in retired.skipped}[
            str(target.resolve())
        ] == "lock-held"
        assert session.is_dir()
        # Build control is a root of its own: only its own lock keeps it.
        assert control.is_dir() is (where == "control")
    released = _gc(ext, env, now)
    assert str(target.resolve()) in _reclaimed(released)
    # The session went with its target; the GC does not try it again.
    assert released.errors == []
    assert not session.exists() and not control.exists()


def test_a_checkout_targets_retirement_takes_its_sessions_along(tmp_path):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    retired, orphan = projects / "retired", projects / "orphan"
    retired.mkdir()
    orphan.mkdir()
    retired_roots = _checkout_state(ext, retired)
    retired_session, retired_control = _session(ext, retired, "shard-7")
    orphan_roots = _checkout_state(ext, orphan)
    orphan_session, orphan_control = _session(ext, orphan, "shard-7")
    checkout_state.stamp_checkout_state(retired, env)
    checkout_state.stamp_checkout_state(orphan, env)
    retired.rmdir()
    orphan.rmdir()

    # The owner's retirement takes a session used a moment ago.
    result = dg.reclaim_checkout_state(retired, root=ext, env=env)
    assert _reclaimed(result) == _resolved([*retired_roots.values(), retired_control])
    assert not retired_session.exists() and not retired_control.exists()

    # The GC retires an orphaned target with its session, inside the session's
    # idle limit, and the session's build control after it.
    now = time.time()
    _last_used([*orphan_roots.values(), orphan_session, orphan_control], 3 * HOUR, now)
    collected = _gc(ext, env, now)
    assert _reclaimed(collected) == _resolved([*orphan_roots.values(), orphan_control])
    assert collected.errors == []
    assert not orphan_session.exists()


@pytest.mark.parametrize("operation", ["gc", "pressure", "retire"])
@pytest.mark.parametrize("marker", ["MOLT_SESSION_ID", "CARGO_TARGET_DIR"])
def test_a_running_builds_session_target_is_never_reclaimed(
    tmp_path, operation, marker
):
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    session, control = _session(ext, gone, LONG_SESSION)
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    now = time.time()
    _last_used([*roots.values(), session, control], 60 * DAY, now)
    running = {
        **env,
        marker: LONG_SESSION if marker == "MOLT_SESSION_ID" else str(session),
    }
    if operation == "gc":
        result = _gc(ext, running, now)
    elif operation == "pressure":
        result = dg.ensure_free(
            root=ext,
            config=_cfg(),
            env=running,
            free_bytes_fn=lambda: 1,
            now_fn=lambda: now,
            log=False,
        )
    else:
        result = dg.reclaim_checkout_state(gone, root=ext, env=running)
    assert session.is_dir()
    assert not {str(session.resolve()), str(roots["cargo-target"].resolve())} & (
        _reclaimed(result)
    )
    if operation != "gc":
        reasons = {row["path"]: row["reason"] for row in result.skipped}
        assert reasons[str(roots["cargo-target"].resolve())] == "protected"
