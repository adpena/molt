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
            b"metadata-\xff",
            marks=pytest.mark.skipif(os.name == "nt", reason="POSIX filesystem bytes"),
        ),
    ],
)
def test_separate_git_registration_preserves_path_bytes_without_guessing_main(
    tmp_path, monkeypatch, name
):
    from tests.test_dx_run_context import _git

    # Decode opaque POSIX bytes only after this case's platform admission.
    # Windows still exercises the Unicode registration row above.
    if isinstance(name, bytes):
        name = os.fsdecode(name)
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
        # Git's pointer format uses forward slashes on Windows too. Keep
        # POSIX byte names through the filesystem codec rather than UTF-8.
        original_git = os.fsencode(original.as_posix())
        common_git = os.fsencode(common.as_posix())
        assert recorded.count(original_git) == 1
        # Git marks this file hidden on Windows. Update the existing file,
        # preserving that attribute, instead of CREATE_ALWAYS through wb.
        with pointer.open("r+b") as stream:
            stream.write(recorded.replace(original_git, common_git))
            stream.truncate()
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


@pytest.mark.parametrize("operation", ["pressure", "gc"])
@pytest.mark.parametrize("marker_starts", ["before-selection", "after-selection"])
def test_live_guard_protects_its_worktree_but_terminal_marker_does_not(
    tmp_path, monkeypatch, operation, marker_starts
):
    _, main, linked = _registered_worktree_scope(tmp_path)
    now = 20_000_000.0
    main_session = main / "target" / "sessions" / "main-old"
    # Existing worktree marker protection stays on legacy lane targets. Managed
    # sessions instead have their precise shared-use and last-use policy.
    linked_session = linked / "target" / "codex-linked-active"
    sized = {
        main_session: _make_dir(
            main_session, size_bytes=5 * _GB, age_s=40 * DAY, now=now
        ),
        linked_session: _make_dir(
            linked_session, size_bytes=100 * _GB, age_s=40 * DAY, now=now
        ),
    }
    markers = linked / "tmp" / "memory_guard" / "active"
    markers.mkdir(parents=True)
    if marker_starts == "before-selection":
        _guard_marker(markers, 1, "child_running")
    else:
        original = dg._delete_candidate
        started = False

        def start_marker(candidate, *args, **kwargs):
            nonlocal started
            if candidate.path == linked_session and not started:
                started = True
                assert not candidate.guard_active
                _guard_marker(markers, 1, "child_running")
            return original(candidate, *args, **kwargs)

        monkeypatch.setattr(dg, "_delete_candidate", start_marker)

    def collect():
        if operation == "gc":
            return dg.gc(
                root=main, config=_cfg(), now_fn=lambda: now, env={}, log=False
            )
        return dg.ensure_free(
            root=main,
            config=_cfg(),
            free_bytes_fn=_SimulatedVolume(1 * _GB, sized),
            now_fn=lambda: now,
            env={},
            log=False,
        )

    result = collect()
    assert not main_session.exists()
    assert linked_session.exists()
    if operation == "pressure" and marker_starts == "before-selection":
        reasons = {Path(item["path"]): item["reason"] for item in result.skipped}
        assert reasons[linked_session.resolve()] == "active-guard"
        (custody,) = result.custody
        assert custody["active_dir"] == str(markers)
        assert custody["blocking_records"] == 1
        assert str(markers / f"guard-1-{1:032x}.json") in custody["examples"][0]
        assert "tools/memory_guard_custody.py --active-dir" in custody["next_step"]
        assert result.to_dict()["custody"] == result.custody
    _guard_marker(markers, 1, "completed")
    result = collect()
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
        # Retired mutable layout remains covered by the retirement authority.
        "uv-environment": ext
        / "uv-project-envs"
        / (f"dx__py3.12__src-{dx.checkout_component(checkout)}"),
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
    """Age ordinary fixture-owned roots and their direct activity entries."""
    from molt import checkout_state

    stamp = now - seconds_ago
    for root in roots:
        metadata = checkout_state._state_metadata_root(root)
        paths = [*root.iterdir(), root]
        if metadata != root and metadata.is_dir():
            paths.extend((*metadata.iterdir(), metadata))
        for path in paths:
            os.utime(path, (stamp, stamp))


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


def test_retirement_reclaims_only_its_checkouts_state_at_once(tmp_path, monkeypatch):
    from molt import build_state_layout, checkout_state
    from molt.memory_guard_paths import active_guard_marker_dirs_of

    ext, projects, env = _checkout_family(tmp_path)
    retired, earlier = projects / "retired", projects / "earlier"
    retired.mkdir()
    earlier.mkdir()
    retired_roots = _checkout_state(ext, retired)
    earlier_roots = _checkout_state(ext, earlier)
    checkout_state.stamp_checkout_state(retired, env)
    checkout_state.stamp_checkout_state(earlier, env)
    # A matching owner is only a selection hint: a copied record under the
    # wrong target hash must not grant deletion authority.
    control = retired_roots["build-control"]
    wrong_name = ("0" if control.name[0] != "0" else "1") + control.name[1:]
    forged = control.with_name(wrong_name)
    forged.mkdir()
    (forged / checkout_state.OWNER_RECORD).write_bytes(
        (control / checkout_state.OWNER_RECORD).read_bytes()
    )
    resolve_owned_path = build_state_layout.resolve_owned_path
    resolved_owners = []

    def resolve_selected_owner(path):
        assert path != earlier_roots["cargo-target"], (
            "retirement resolved an unrelated target before selecting its owner"
        )
        resolved_owners.append(path)
        return resolve_owned_path(path)

    monkeypatch.setattr(
        build_state_layout, "resolve_owned_path", resolve_selected_owner
    )
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
    assert forged.is_dir(), "selection must not replace target-address binding"
    assert retired_roots["cargo-target"] in resolved_owners


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


def test_dx_completion_collects_idle_state_without_collecting_its_environment(tmp_path):
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
        "from molt.dx import RunContext, collect_checkout_state_after_result\n"
        "env = RunContext(Path(sys.argv[1]), session_prefix='t').canonical_env(\n"
        "    {'MOLT_EXT_ROOT': sys.argv[2], 'PATH': '/usr/bin:/bin'}, create_dirs=True)\n"
        "assert Path(sys.argv[3]).is_dir(), 'environment construction collected global state'\n"
        "collect_checkout_state_after_result(env, cwd=Path(sys.argv[1]))\n"
    )
    # A real source process, outside pytest's automatic-maintenance opt-out.
    clean = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("PYTEST_", "MOLT_PYTEST_"))
    }

    def run() -> None:
        completed = run_guarded_test_process(
            [
                sys.executable,
                "-c",
                code,
                str(user),
                str(ext),
                str(next(iter(gone_roots.values()))),
            ],
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
    # The environment-only assertion still names a real idle root on run two.
    gone_roots = later_roots
    run()
    assert all(path.is_dir() for path in later_roots.values())
    assert not checkout_state.checkout_state_gc_due(ext)
    assert checkout_state.checkout_state_gc_due(ext, now=now + 25 * HOUR)


@pytest.mark.parametrize(
    "extra",
    [
        {"MOLT_DISABLE_DISK_GUARD": "1"},
        {"PYTEST_CURRENT_TEST": "fixture"},
        {"MOLT_MEMORY_GUARD_ACTIVE": "1"},
        {"MOLT_MEMORY_GUARD_STARTUP_JSON": "pending-startup.json"},
    ],
)
def test_automatic_collection_requires_an_eligible_outer_completion(
    tmp_path, monkeypatch, extra
):
    from molt import checkout_state

    ext, _projects, env = _checkout_family(tmp_path)

    def forbidden():
        pytest.fail("ineligible completion loaded the collector")

    monkeypatch.setattr(dx, "_source_disk_guard", forbidden)
    dx.collect_checkout_state_after_result({**env, **extra}, cwd=tmp_path)
    assert not ext.joinpath(*checkout_state.GC_STAMP).exists()


def test_installed_completion_does_not_claim_source_collection(tmp_path, monkeypatch):
    from molt import checkout_state

    ext, _projects, env = _checkout_family(tmp_path)
    monkeypatch.setattr(dx, "_source_disk_guard", lambda: None)
    dx.collect_checkout_state_after_result(env, cwd=tmp_path)
    assert not ext.joinpath(*checkout_state.GC_STAMP).exists()


def test_installed_maintenance_selection_imports_no_source_tools(tmp_path, monkeypatch):
    installed = tmp_path / "installed"
    installed.mkdir()
    monkeypatch.setattr(dx, "compiler_source_root", lambda: installed)

    def forbidden(*_args, **_kwargs):
        pytest.fail("installed maintenance tried to resolve source artifact policy")

    monkeypatch.setattr(dx, "artifact_root", forbidden)
    assert dx.checkout_state_maintenance_environment(installed, {}) is None
    assert list(installed.iterdir()) == []


def test_maintenance_selection_failure_is_reported_without_replacing_result(
    tmp_path, monkeypatch, capsys
):
    def unavailable():
        raise RuntimeError("source unavailable")

    monkeypatch.setattr(dx, "compiler_source_root", unavailable)
    assert dx.checkout_state_maintenance_environment(tmp_path, {}) is None
    assert "checkout state GC failed: source unavailable" in capsys.readouterr().err


def test_automatic_collection_reports_real_reclaim_errors_and_throttles_failure(
    tmp_path, monkeypatch, capsys
):
    import time
    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    _last_used(roots.values(), 3 * HOUR, time.time())
    monkeypatch.setattr(dg, "delete_path", lambda _path: (False, "fixture-denied"))
    dx.collect_checkout_state_after_result(env, cwd=tmp_path)
    diagnostics = capsys.readouterr().err
    assert diagnostics.count("fixture-denied") == 3
    assert "1 more error(s)" in diagnostics
    assert all(path.is_dir() for path in roots.values())
    assert not checkout_state.checkout_state_gc_due(ext)

    def forbidden(**_kwargs):
        pytest.fail("failed attempt was repeated before the next day")

    monkeypatch.setattr(dg, "collect", forbidden)
    dx.collect_checkout_state_after_result(env, cwd=tmp_path)


def test_daily_collection_admission_is_nonblocking_across_processes(tmp_path):
    import subprocess
    from molt import checkout_state
    from tests.process_guard_common import start_owned_test_process

    ext = tmp_path / "artifacts"
    ext.mkdir()
    code = (
        "import sys; from pathlib import Path\n"
        f"sys.path[:0] = [{str(ROOT)!r}, {str(ROOT / 'src')!r}]\n"
        "from molt.checkout_state import checkout_state_gc\n"
        "with checkout_state_gc(Path(sys.argv[1])) as admitted:\n"
        " print(admitted, flush=True)\n"
        " sys.stdin.read(1)\n"
    )
    with start_owned_test_process(
        [sys.executable, "-c", code, str(ext)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    ) as child:
        assert child.stdout is not None and child.stdin is not None
        try:
            assert child.stdout.readline().strip() == "True"
            # Force the cadence due while another process still holds admission.
            # A stamp-only implementation would admit this second collector.
            stamp = ext.joinpath(*checkout_state.GC_STAMP)
            os.utime(stamp, (1, 1))
            with checkout_state.checkout_state_gc(ext) as admitted:
                assert admitted is False
            assert child.poll() is None
        finally:
            child.communicate("x", timeout=10)
        assert child.returncode == 0
    with pytest.raises(RuntimeError, match="interrupted fixture"):
        with checkout_state.checkout_state_gc(ext) as admitted:
            assert admitted is True
            raise RuntimeError("interrupted fixture")
    assert not checkout_state.checkout_state_gc_due(ext)
    os.utime(ext.joinpath(*checkout_state.GC_STAMP), (1, 1))
    with checkout_state.checkout_state_gc(ext) as admitted:
        assert admitted is True


@pytest.mark.parametrize("owner", ["memory_guard", "dx_run", "nested_guarded_exec"])
def test_outer_completion_publishes_payload_before_real_collection(
    tmp_path, monkeypatch, owner
):
    import argparse
    import time
    from molt import checkout_state
    from tools import memory_guard
    from molt.cli import dx_cli
    from tests.process_guard_common import ModuleView

    ext, projects, env = _checkout_family(tmp_path)
    gone = projects / "gone"
    gone.mkdir()
    roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(gone, env)
    gone.rmdir()
    _last_used(roots.values(), 3 * HOUR, time.time())
    marker = tmp_path / "payload-ran"
    summary = tmp_path / "guard.json"
    selected = {
        **env,
        "PATH": os.environ.get("PATH", ""),
        "MOLT_DISK_GUARD_HIGH_WATER_GB": "0.001",
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    command = [
        sys.executable,
        "-c",
        f"from pathlib import Path; Path({str(marker)!r}).write_text('closed payload'); raise SystemExit(7)",
    ]
    if owner == "nested_guarded_exec":
        # The real outer guard supplies ACTIVE to the standalone nested
        # facade. Its payload environment drops internal keys, but its owner
        # eligibility must still defer to the outer guard's later terminal.
        command = [
            sys.executable,
            "-c",
            (
                "import sys; from pathlib import Path\n"
                f"sys.path[:0] = [{str(ROOT)!r}, {str(ROOT / 'src')!r}]\n"
                "from tools import guarded_exec\n"
                f"rc = guarded_exec.main(['--', *{command!r}])\n"
                f"assert all(Path(p).is_dir() for p in {[str(p) for p in roots.values()]!r}), 'nested facade collected inside outer guard'\n"
                "raise SystemExit(rc)\n"
            ),
        ]
    original = dg.collect
    observations = []

    def collect(**kwargs):
        assert marker.read_text() == "closed payload"
        if owner != "dx_run":
            payload = json.loads(summary.read_text())
            assert payload["returncode"] == 7
            assert payload["descendants_closed"] is True
        observations.append(owner)
        return original(**kwargs)

    monkeypatch.setattr(dg, "collect", collect)
    if owner != "dx_run":
        result = memory_guard.main(
            [
                "--max-rss-gb",
                "1",
                "--max-total-rss-gb",
                "1",
                "--poll-interval",
                "0.01",
                "--summary-json",
                str(summary),
                "--",
                *command,
            ],
            environ=selected,
        )
    else:
        monkeypatch.setattr(dx_cli, "os", ModuleView(os, environ=selected))
        result = dx_cli._handle_run(argparse.Namespace(child_command=command))
    assert result == 7
    assert observations == [owner]
    assert not any(path.exists() for path in roots.values())


@pytest.mark.parametrize(
    "owner",
    [
        "build",
        "driver-fix",
        "cli-run",
        "cli-compare",
        "cli-test",
        "cli-diff",
        "cli-bench",
        "cli-profile",
        "cli-lint",
        "cli-harness",
        "cli-deploy",
    ],
)
def test_published_nonzero_result_collects_selected_root_alongside_live_use(
    tmp_path, monkeypatch, capsys, owner
):
    import time
    from molt import checkout_state
    from molt.cli import build_command, build_output_layout
    from molt.cli.build_request import BuildRequest
    from molt.process_environment import ProcessEnvironment
    from tests.process_guard_common import ModuleView, run_guarded_test_process
    from tools import molt_dev
    from tools.molt_dev_policy import GateEnvironment

    ext, projects, selected = _checkout_family(tmp_path)
    live, gone = projects / "live", projects / "gone"
    live.mkdir()
    gone.mkdir()
    live_roots = _checkout_state(ext, live)
    gone_roots = _checkout_state(ext, gone)
    checkout_state.stamp_checkout_state(live, selected)
    checkout_state.stamp_checkout_state(gone, selected)
    selected = ProcessEnvironment(
        {
            **selected,
            "PATH": os.defpath,
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "guards"),
            "PYTHONDONTWRITEBYTECODE": "1",
        }
    )
    ambient = tmp_path / "ambient-artifacts"
    ambient.mkdir()
    valuable = ambient / "keep.bin"
    valuable.write_bytes(b"ambient is not the selected root")
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ambient))
    marker = tmp_path / "payload-result"
    original = dg.collect
    observations = []

    def collect(**kwargs):
        assert marker.read_text() == "7"
        assert "published result 7" in capsys.readouterr().out
        assert kwargs["root"] == ext
        assert kwargs["env"]["MOLT_EXT_ROOT"] == str(ext)
        observations.append("publication precedes maintenance")
        return original(**kwargs)

    monkeypatch.setattr(dg, "collect", collect)

    def payload():
        proc = run_guarded_test_process(
            [
                sys.executable,
                "-c",
                (
                    f"from pathlib import Path; Path({str(marker)!r}).write_text('7'); "
                    "raise SystemExit(7)"
                ),
            ],
            env=selected,
            check=False,
        )
        assert proc.returncode == 7
        print("published result 7")
        return 7

    if owner == "build":
        monkeypatch.setattr(build_output_layout, "os", ModuleView(os, environ=selected))
        command = build_command._in_build_scope(lambda _request: payload())

        def run():
            return command(BuildRequest(file_path="app.py", cache=False))

        expected = 7
    elif owner == "driver-fix":
        context = GateEnvironment(selected, None, False)
        monkeypatch.setattr(
            molt_dev, "prepare_gate_environment", lambda *_a, **_k: context
        )
        monkeypatch.setattr(molt_dev, "run_gate", lambda *_a, **_k: payload())
        args = molt_dev.build_parser().parse_args(["fix", "--repo", str(projects)])

        def run():
            return molt_dev.cmd_fix(args)

        expected = 1
    else:
        from types import SimpleNamespace
        from molt.cli import entrypoint, entrypoint_dispatch

        args = SimpleNamespace(command=owner.removeprefix("cli-"), json=True)
        monkeypatch.setattr(entrypoint, "os", ModuleView(os, environ=selected))
        monkeypatch.setattr(
            entrypoint,
            "_build_entrypoint_parser",
            lambda: SimpleNamespace(parse_args=lambda: args),
        )
        monkeypatch.setattr(
            entrypoint_dispatch,
            "_dispatch_entrypoint_command",
            lambda *_a, **_kw: payload(),
        )
        run = entrypoint.main
        expected = 7
    chosen = live_roots["cargo-target"]
    bound = checkout_state.bound_checkout_owner(chosen, ext)
    with checkout_state.checkout_state_use(((chosen, bound),)):
        live.rmdir()
        gone.rmdir()
        _last_used([*live_roots.values(), *gone_roots.values()], 40 * DAY, time.time())
        before = (chosen / "payload.bin").read_bytes()
        assert run() == expected
        assert chosen.is_dir()
        assert (chosen / "payload.bin").read_bytes() == before
        assert not any(path.exists() for path in gone_roots.values())
    assert observations == ["publication precedes maintenance"]
    assert valuable.read_bytes() == b"ambient is not the selected root"


@pytest.mark.parametrize(
    "owner", ["build-json", "build-human", "difftest", "queue", "cli-harness"]
)
def test_terminal_output_reaches_real_pipe_before_blocked_maintenance(tmp_path, owner):
    import queue
    import subprocess
    import threading
    import time
    from tests.process_guard_common import start_owned_test_process

    entered = tmp_path / "maintenance-entered"
    program = tmp_path / "app.py"
    program.write_text("print('ok')\n")
    source = f"""
import json, sys
from pathlib import Path
from types import SimpleNamespace
sys.path[:0] = [{str(ROOT)!r}, {str(ROOT / "src")!r}]
from molt import dx
assert not sys.stdout.line_buffering, "the oracle needs a buffered real pipe"
def block(env, *, cwd):
    Path({str(entered)!r}).write_text("entered")
    assert sys.stdin.buffer.read(1) == b"r"
dx.collect_checkout_state_after_result = block
if {owner!r}.startswith("build-"):
    from molt.cli import build_command, output
    from molt.cli.build_request import BuildRequest
    def body(request):
        if {owner!r} == "build-json":
            output.emit_json({{"returncode": 7}}, True)
        else:
            output.success("human result 7")
        return 7
    rc = build_command._in_build_scope(body)(BuildRequest(file_path={str(program)!r}, cache=False))
elif {owner!r} == "cli-harness":
    from molt.cli import entrypoint, entrypoint_dispatch
    args = SimpleNamespace(command="harness", json=True)
    entrypoint._build_entrypoint_parser = lambda: SimpleNamespace(parse_args=lambda: args)
    def dispatch(*args, **kwargs):
        print(json.dumps({{"returncode": 7}}))
        return 7
    entrypoint_dispatch._dispatch_entrypoint_command = dispatch
    rc = entrypoint.main()
elif {owner!r} == "queue":
    from tools.proof_queue_pkg import runner
    runner._publish_terminal_result("failed", "fixture", 7, 1.0, Path("fixture.log"))
    block({{}}, cwd=Path.cwd())
    rc = 7
else:
    from tools import molt_dev
    molt_dev.python_interpreter.probe_python_command = lambda command, **kw: molt_dev.python_interpreter.PythonInterpreter(tuple(command), command[0], "3.12.0", "CPython")
    molt_dev._run_driver_command = lambda *a, **kw: SimpleNamespace(returncode=0)
    def capture(command, **kwargs):
        if command[1:3] == ["-m", "molt"]:
            Path(command[command.index("--output") + 1]).write_bytes(b"fixture")
        return 0, b"ok\\n", b""
    molt_dev._difftest_capture = capture
    args = molt_dev.build_parser().parse_args(["difftest", {str(program)!r}, "--root", {str(ROOT)!r}, "--target", "native", "--python-version", "3.12", "--out-dir", {str(tmp_path)!r}, "--json"])
    rc = molt_dev.cmd_difftest(args)
raise SystemExit(rc)
"""
    env = {
        key: value
        for key, value in os.environ.items()
        if key.upper()
        not in {
            "PYTHONUNBUFFERED",
            "PYTEST_CURRENT_TEST",
            "PYTEST_VERSION",
            "MOLT_MEMORY_GUARD_ACTIVE",
            "MOLT_MEMORY_GUARD_STARTUP_JSON",
            "MOLT_PROOF_QUEUE",
            "MOLT_PYTEST_OUTER_GUARD_REEXEC",
        }
    }
    env.update(
        {
            "MOLT_EXT_ROOT": str(tmp_path / "artifacts"),
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "guards"),
            "PYTHONDONTWRITEBYTECODE": "1",
        }
    )
    proc = start_owned_test_process(
        [sys.executable, "-c", source],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    observed = queue.Queue()
    reader = threading.Thread(
        target=lambda: observed.put(proc.stdout.readline()), daemon=True
    )
    reader.start()
    try:
        deadline = time.monotonic() + 5
        while (
            not entered.exists() and proc.poll() is None and time.monotonic() < deadline
        ):
            time.sleep(0.01)
        assert entered.exists(), "the command did not reach maintenance"
        try:
            line = observed.get(timeout=3)
        except queue.Empty:
            pytest.fail("terminal output stayed buffered while maintenance blocked")
        if owner == "build-human":
            assert line == b"human result 7\n"
        elif owner == "queue":
            assert line == b"failed fixture rc=7 elapsed=1.0s\n"
        else:
            payload = json.loads(line)
            assert (
                payload == {"returncode": 7}
                if owner != "difftest"
                else payload["passed"] is True
            )
        assert proc.poll() is None, (
            "publication must be visible before maintenance returns"
        )
    finally:
        try:
            proc.stdin.write(b"r")
            proc.stdin.flush()
        except BrokenPipeError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        reader.join(timeout=2)
        proc.stdin.close()
        proc.stdout.close()
        diagnostic = proc.stderr.read().decode("utf-8", "replace")
        proc.stderr.close()
    assert proc.returncode == (0 if owner == "difftest" else 7), diagnostic


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
    if kind == "cargo-target" and lock.startswith("sessions/"):
        # Keep the lock as the only live signal: creating its session above
        # otherwise supplies genuine recent activity after the root was aged.
        _last_used([root / "sessions" / "shard-7"], 60 * DAY, now)
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
    from molt import checkout_state

    checkout_state.record_checkout_target_use(target, checkout=checkout)
    checkout_state.record_build_control_use(control, cargo_target=target)
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

    # Main's local target/sessions layout uses the same lifetime protocol.
    local = ext / "target" / "sessions" / "local-reader"
    local.mkdir(parents=True)
    checkout_state.record_checkout_target_use(local, checkout=ext)
    assert (local / checkout_state.LAST_USE_STAMP).is_file()
    with checkout_state.checkout_state_use(((local, None),)):
        _last_used([local], 40 * DAY, now)
        assert str(local.resolve()) not in _reclaimed(_gc(ext, env, now))
        assert local.is_dir()
    _last_used([local], 40 * DAY, now)
    assert str(local.resolve()) in _reclaimed(_gc(ext, env, now))


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


def test_checkout_session_use_does_not_refresh_idle_siblings_or_controls(tmp_path):
    import time
    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    source = projects / "source"
    source.mkdir()
    parent = _checkout_state(ext, source)["cargo-target"]
    chosen, chosen_control = _session(ext, source, "chosen")
    idle, idle_control = _session(ext, source, "idle")
    now = time.time()
    _last_used([idle, idle_control, chosen, chosen_control, parent], 10 * DAY, now)
    old = {
        root: checkout_state.checkout_state_last_use(root)
        for root in (idle, idle_control)
    }
    checkout_state.stamp_checkout_state(
        source, {**env, "CARGO_TARGET_DIR": str(chosen)}
    )
    assert all(
        checkout_state.checkout_state_last_use(root) == stamp
        for root, stamp in old.items()
    )
    assert all(
        checkout_state.checkout_state_last_use(root) > now - HOUR
        for root in (parent, chosen, chosen_control)
    )
    assert checkout_state.bound_checkout_owner(chosen, ext).path == source.resolve()


def test_nested_checkout_candidates_count_bytes_once_and_continue_pressure_plan(
    tmp_path,
):
    parent = dg.Candidate(tmp_path / "parent", "sessions", 900, size=11)
    child = dg.Candidate(parent.path / "child", "sessions", 800, size=10)
    other = dg.Candidate(tmp_path / "other", "sessions", 950, size=10)
    plan = dg.plan_reclaim(
        [child, parent, other],
        free_bytes=0,
        high_water_bytes=20,
        target_bytes=20,
        now=1000,
        min_idle_s=0,
    )
    assert [item.path for item in plan.selected] == [parent.path, other.path]
    assert dg.gc_plan([child, parent], now=1000, min_idle_s=0) == [parent]


def test_checkout_session_foreign_record_and_name_do_not_grant_orphan_custody(tmp_path):
    import time
    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    gone, other = projects / "gone", projects / "other"
    gone.mkdir()
    other.mkdir()
    child, control = _session(ext, gone, "valid")
    foreign, _ = _session(ext, other, "foreign")
    (child / checkout_state.OWNER_RECORD).write_bytes(
        (foreign / checkout_state.OWNER_RECORD).read_bytes()
    )
    invalid = child.parent / ("a" * 33)
    invalid.mkdir()
    gone.rmdir()
    other.rmdir()
    now = time.time()
    _last_used([child, control, invalid], 3 * HOUR, now)
    assert checkout_state.bound_checkout_owner(child, ext) is None
    assert checkout_state.checkout_state_kind_of(invalid, ext) is None
    _gc(ext, env, now)
    assert child.is_dir() and invalid.is_dir()


@pytest.mark.parametrize("operation", ["gc", "pressure", "retirement"])
@pytest.mark.parametrize(
    "selected", ["session", "control", "uv-environment", "python-environment"]
)
def test_managed_shared_use_retains_idle_root_through_actual_operation(
    tmp_path, operation, selected
):
    import time
    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    source = projects / "gone"
    source.mkdir()
    roots = _checkout_state(ext, source)
    checkout_state.stamp_checkout_state(source, env)
    child, control = _session(ext, source, "long-reader")
    if selected == "python-environment":
        image = (
            ext
            / "build-environments"
            / "development"
            / dx.checkout_component(source)
            / ("a" * 64)
        )
        image.mkdir(parents=True)
        (image / "pyvenv.cfg").write_text("home = /x\n", encoding="utf-8")
        (image / "payload.bin").write_bytes(b"immutable generation")
        checkout_state.record_checkout_owner(
            image, owner=source, kind="python-environment"
        )
        roots[selected] = image
    chosen = {
        "session": child,
        "control": control,
        "uv-environment": roots["uv-environment"],
        **(
            {"python-environment": roots[selected]}
            if selected == "python-environment"
            else {}
        ),
    }[selected]
    image_bytes = {
        path.relative_to(chosen): path.read_bytes()
        for path in chosen.rglob("*")
        if path.is_file()
    }
    owner = checkout_state.bound_checkout_owner(chosen, ext)
    now = time.time()
    with checkout_state.checkout_state_use(((chosen, owner),)):
        source.rmdir()
        # No activity heartbeat or two-hour grace can explain the retention.
        _last_used([*roots.values(), child, control], 40 * DAY, now)
        if operation == "gc":
            result = _gc(ext, env, now)
        elif operation == "pressure":
            result = dg.ensure_free(
                root=ext,
                env=env,
                config=_cfg(),
                now_fn=lambda: now,
                free_bytes_fn=lambda: 1,
                log=False,
            )
        else:
            result = dg.reclaim_checkout_state(source, root=ext, env=env)
        assert chosen.is_dir()
        assert str(chosen.resolve()) not in _reclaimed(result)
        if selected in ("session", "control"):
            assert roots["cargo-target"].is_dir()
        if selected == "python-environment":
            assert checkout_state.read_checkout_owner(chosen) == owner
            assert {
                path.relative_to(chosen): path.read_bytes()
                for path in chosen.rglob("*")
                if path.is_file()
            } == image_bytes
    # Ordinary eviction is allowed again once use ends and policy ages it.
    _last_used(
        [root for root in (*roots.values(), child, control) if root.exists()],
        40 * DAY,
        now,
    )
    reclaimed = {Path(path) for path in _reclaimed(_gc(ext, env, now))}
    assert not chosen.exists()
    assert any(path == chosen or path in chosen.parents for path in reclaimed)
    if selected == "python-environment":
        assert not checkout_state._state_metadata_root(chosen).exists()


@pytest.mark.parametrize("operation", ["gc", "pressure"])
@pytest.mark.parametrize(
    "relative,globs",
    [
        ("target/lane-quiet", ("lane-*",)),
        ("target/custom-quiet", ("custom-*",)),
        ("cargo-target-quiet", ("lane-*",)),
    ],
)
def test_explicit_legacy_target_lifetime_and_timed_retention(
    tmp_path, relative, globs, operation
):
    import subprocess
    import time
    from molt import checkout_state
    from molt.process_spawn import ProcessStateUse

    ext, _projects, collector_env = _checkout_family(tmp_path)
    selected = ext / relative
    foreign = ext / "target" / "unselected-foreign"
    protected = ext / "target" / "debug"
    for path in (selected, foreign, protected):
        path.mkdir(parents=True, exist_ok=True)
        (path / "artifact").write_bytes(b"preserved")
    cfg = _cfg(lane_globs=globs)
    # The command does not know the independent collector's custom glob policy.
    command_env = {
        **os.environ,
        "CARGO_TARGET_DIR": str(selected),
        "MOLT_EXT_ROOT": str(ext),
    }
    command_env.pop(dg.ENV_LANE_GLOBS, None)

    def collect(now):
        if operation == "gc":
            return dg.gc(
                root=ext, env=collector_env, config=cfg, now_fn=lambda: now, log=False
            )
        return dg.ensure_free(
            root=ext,
            env=collector_env,
            config=cfg,
            now_fn=lambda: now,
            free_bytes_fn=lambda: 1,
            log=False,
        )

    with ProcessStateUse(ROOT, command_env, ROOT) as owner:
        with subprocess.Popen(
            [sys.executable, "-I", "-S", "-c", "import sys;sys.stdin.buffer.read(1)"],
            stdin=subprocess.PIPE,
        ) as child:
            owner.process = child
            assert child.poll() is None
            now = time.time()
            _last_used([selected, foreign, protected], 40 * DAY, now)
            assert str(selected.resolve()) not in _reclaimed(collect(now))
            assert selected.is_dir()
            child.communicate(b"x", timeout=10)
            assert child.returncode == 0
    assert not (selected / checkout_state.OWNER_RECORD).exists()
    # Completion refreshes use; exclusive admission must see that fresh fact.
    assert str(selected.resolve()) not in _reclaimed(collect(time.time()))
    now = time.time()
    _last_used([selected, foreign, protected], 40 * DAY, now)
    assert str(selected.resolve()) in _reclaimed(collect(now))
    assert not selected.exists()
    assert (foreign / "artifact").read_bytes() == b"preserved"
    assert (protected / "artifact").read_bytes() == b"preserved"


@pytest.mark.parametrize("primary", [None, "body-failure"])
def test_checkout_completion_preserves_primary_error_and_releases_use(
    tmp_path, monkeypatch, primary
):
    from molt import checkout_state

    selected = tmp_path / "target" / "lane-publication"
    selected.mkdir(parents=True)
    original = checkout_state._record_checkout_state_use
    calls = 0
    failure = OSError("completion disk full")
    body_error = ValueError("independent body failure")

    def publish(root):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise failure
        original(root)

    monkeypatch.setattr(checkout_state, "_record_checkout_state_use", publish)
    with pytest.raises(ValueError if primary else OSError) as caught:
        with checkout_state.checkout_state_use(((selected, None),)):
            if primary:
                raise body_error
    assert caught.value is (body_error if primary else failure)
    if primary:
        assert any("completion disk full" in note for note in body_error.__notes__)
    with checkout_state.checkout_state_retirement(selected) as available:
        assert available


@pytest.mark.parametrize("new_use", ["stamp", "held-lock"])
def test_gc_revalidates_live_use_after_policy_selection(tmp_path, monkeypatch, new_use):
    import time
    from molt import checkout_state, file_locks

    ext, projects, env = _checkout_family(tmp_path)
    source = projects / "source"
    source.mkdir()
    child, control = _session(ext, source, "started-after-selection")
    parent = child.parent.parent
    now = time.time()
    _last_used([child, parent, control], 40 * DAY, now)
    original = dg._retention_candidates
    handles = []
    planned = False

    def after_policy(*args, **kwargs):
        nonlocal planned
        candidates = original(*args, **kwargs)
        if candidates and not kwargs.get("build_control") and not planned:
            planned = True
            assert any(c.path == parent and not c.lock_held for c in candidates)
            if new_use == "stamp":
                checkout_state.record_checkout_state_use(child, now=now)
            else:
                handles.append(
                    file_locks._acquire_file_lock(
                        child / "debug" / ".cargo-lock",
                        timeout_s=1,
                        timeout_message="busy fixture",
                    )
                )
        return candidates

    monkeypatch.setattr(dg, "_retention_candidates", after_policy)
    try:
        result = _gc(ext, env, now)
        assert planned and parent.is_dir() and child.is_dir()
        assert not _reclaimed(result) & _resolved([parent, child])
    finally:
        for handle in handles:
            file_locks._release_file_lock(handle)


def test_retirement_finishes_before_later_user_recreates_root(tmp_path):
    import time
    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    source = projects / "source"
    source.mkdir()
    child, control = _session(ext, source, "later-user")
    parent = child.parent.parent
    now = time.time()
    _last_used([child, parent, control], 40 * DAY, now)
    assert str(parent.resolve()) in _reclaimed(_gc(ext, env, now))
    assert not child.exists()
    with checkout_state.checkout_state_use(
        ((child, checkout_state.CheckoutOwner(source, "cargo-target")),)
    ):
        child.mkdir(parents=True)
        (child / "new.bin").write_bytes(b"new user")
    assert checkout_state.bound_checkout_owner(child, ext).path == source.resolve()
    assert (child / "new.bin").read_bytes() == b"new user"


@pytest.mark.skipif(
    os.name != "nt", reason="actual Windows missing-component case aliases"
)
def test_managed_case_alias_use_contends_before_root_materialization(tmp_path):
    from molt.checkout_state import (
        CheckoutOwner,
        checkout_state_use,
        checkout_state_retirement,
    )
    from molt.dx import checkout_component

    checkout = tmp_path / "project"
    checkout.mkdir()
    artifact = tmp_path / "artifacts"
    artifact.mkdir()
    parent = artifact / "target" / "checkouts" / checkout_component(checkout)
    session = parent / "sessions" / "reader"
    alias = Path(str(session).swapcase())
    assert not session.exists()
    with checkout_state_use(((alias, CheckoutOwner(checkout, "cargo-target")),)):
        alias.mkdir(parents=True)
        for root in (parent, session):
            with checkout_state_retirement(root) as admitted:
                assert not admitted
    for root in (parent, session):
        with checkout_state_retirement(root) as admitted:
            assert admitted


@pytest.mark.parametrize(
    "relative,retained",
    [
        ("target/custom-scope", True),
        ("cargo-target-scope", True),
        ("target/.molt_state", False),
        ("target/debug", False),
        ("target/checkouts", False),
        ("target/.disk_guard", False),
        ("target/DEBUG", False),
        ("target/.MOLT_STATE", False),
        ("target/CACHEDIR.TAG", False),
        ("arbitrary/caller-output", False),
    ],
)
def test_structural_use_scope_never_adopts_an_owner_or_deletion_permission(
    tmp_path, relative, retained
):
    from molt import checkout_state

    selected = tmp_path / relative
    if sys.platform == "darwin" and relative == "target/DEBUG":
        # Default APFS may resolve this alias to an existing protected root.
        (tmp_path / "target/debug").mkdir(parents=True)
    selected.mkdir(parents=True, exist_ok=True)
    (selected / "payload").write_bytes(b"caller data")
    with checkout_state.checkout_state_use(((selected, None),)):
        assert (selected / checkout_state.LAST_USE_STAMP).exists() is retained
        with checkout_state.checkout_state_retirement(selected) as available:
            assert available is not retained
    assert (tmp_path / "target/.disk_guard/checkout-state-locks").exists() is retained
    assert not (selected / checkout_state.OWNER_RECORD).exists()
    assert (selected / "payload").read_bytes() == b"caller data"
    if not retained:
        assert dg._is_reclaimable(selected, tmp_path, ("*",)) is None


@pytest.fixture
def completed_incremental(tmp_path):
    from molt.checkout_state import record_checkout_owner

    root = tmp_path / "artifacts"
    source = tmp_path / "source"
    source.mkdir()
    session = (
        root / "target/checkouts" / dx.checkout_component(source) / "sessions/finished"
    )
    build = session / ".molt-toolchains" / ("a" * 64)
    cache = build / "release/incremental"
    unit = cache / "sample-session"
    unit.mkdir(parents=True)
    (unit / "query-cache.bin").write_bytes(b"discardable generated cache")
    (cache / "sample.lock").touch()
    (cache.parent / ".cargo-build-lock").touch()
    (session / "release").mkdir()
    (session / "release/.cargo-lock").touch()
    record_checkout_owner(session, owner=source, kind="cargo-target")
    for name in (
        "deps/keep.rlib",
        "build/keep",
        ".fingerprint/keep",
        "keep.generation.json",
    ):
        kept = cache.parent / name
        kept.parent.mkdir(exist_ok=True)
        kept.write_bytes(b"retain product and evidence")
    argv = [
        "/toolchain/bin/cargo",
        "build",
        "--profile",
        "release",
        "--locked",
        "--config",
        "build.build-dir=" + json.dumps(str(build)),
        "--config",
        "build.target-dir=" + json.dumps(str(session)),
    ]
    retired = root / "tmp/memory_guard/retired"
    retired.mkdir(parents=True)
    receipt = _guard_marker(retired, 761, "completed")
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    payload.update(
        cwd=str(source),
        command=argv,
        launch_command=argv,
        returncode=0,
        child_returncode=0,
        descendants_closed=True,
    )
    payload["child_process"]["command"] = argv
    receipt.write_text(json.dumps(payload), encoding="utf-8")
    return root, source, session, cache, receipt


def _incremental_plan(fixture, **kwargs):
    root, _, _, cache, receipt = fixture
    return dg.retire_cargo_incremental(
        cache, root=root, guard_receipt=receipt, env={}, **kwargs
    )


def test_incremental_retirement_removes_only_reviewed_cache(completed_incremental):
    root, _, session, cache, receipt = completed_incremental
    receipt_bytes = receipt.read_bytes()
    # A real hardlink into retained deps must remain byte-identical and its
    # storage must never be counted as reclaimed merely by unlinking the cache.
    kept = cache.parent / "deps/keep.rlib"
    os.link(kept, cache / "sample-session/external.rmeta")
    plan = _incremental_plan(completed_incremental)
    assert plan["applied"] is False and cache.is_dir()
    assert (
        plan["externally_linked_bytes_retained"]
        == getattr(kept.stat(), "st_blocks", 0) * 512
    )
    result = _incremental_plan(
        completed_incremental,
        apply=True,
        expected_inventory_sha256=plan["inventory_sha256"],
    )
    assert result["applied"] is True and not cache.exists()
    assert session.is_dir() and receipt.read_bytes() == receipt_bytes
    assert (
        kept.read_bytes() == b"retain product and evidence"
        and kept.stat().st_nlink == 1
    )
    for name in ("build/keep", ".fingerprint/keep", "keep.generation.json"):
        assert (cache.parent / name).read_bytes() == b"retain product and evidence"
    assert (cache.parent / ".cargo-build-lock").exists()


@pytest.mark.parametrize("relative", ["deps", "build", ".fingerprint", ".", "../.."])
def test_incremental_retirement_refuses_outside_scope(completed_incremental, relative):
    root, _, _, cache, receipt = completed_incremental
    with pytest.raises(ValueError):
        dg.retire_cargo_incremental(
            cache.parent / relative,
            root=root,
            guard_receipt=receipt,
            env={},
            apply=True,
            expected_inventory_sha256="0" * 64,
        )
    assert cache.is_dir()


@pytest.mark.parametrize(
    "field,value",
    [
        ("returncode", False),
        ("child_returncode", False),
        ("returncode", 1),
        ("descendants_closed", False),
        ("status", "child_running"),
        ("cwd", "/foreign"),
    ],
)
def test_incremental_retirement_requires_real_terminal_custody(
    completed_incremental, field, value
):
    receipt = completed_incremental[-1]
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    payload[field] = value
    receipt.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError):
        _incremental_plan(completed_incremental)
    assert completed_incremental[-2].is_dir()


@pytest.mark.parametrize("kind", ["managed-reader", "cargo-build", "cargo-output"])
def test_incremental_retirement_refuses_live_owners(completed_incremental, kind):
    from contextlib import ExitStack
    from molt.checkout_state import checkout_state_use
    from molt.file_locks import _try_acquire_file_lock, _release_file_lock

    _, _, session, cache, _ = completed_incremental
    with ExitStack() as stack:
        if kind == "managed-reader":
            stack.enter_context(checkout_state_use(((session, None),), publish=False))
        else:
            name = ".cargo-build-lock" if kind == "cargo-build" else ".cargo-lock"
            lock = (
                cache.parent if kind == "cargo-build" else session / "release"
            ) / name
            handle = _try_acquire_file_lock(lock)
            assert handle is not None
            stack.callback(_release_file_lock, handle)
        with pytest.raises(ValueError, match="live|active"):
            _incremental_plan(completed_incremental)
    assert cache.is_dir()


def test_incremental_retirement_does_not_require_a_surviving_source(
    completed_incremental,
):
    _, source, _, cache, _ = completed_incremental
    source.rmdir()
    plan = _incremental_plan(completed_incremental)
    _incremental_plan(
        completed_incremental,
        apply=True,
        expected_inventory_sha256=plan["inventory_sha256"],
    )
    assert not cache.exists()


@pytest.mark.parametrize(
    "change",
    [
        "payload",
        "receipt",
        "new-file",
        "symlink",
        "missing-lock",
        "missing-output-lock",
    ],
)
def test_incremental_retirement_rejects_changed_review(
    completed_incremental, change, tmp_path
):
    _, _, session, cache, receipt = completed_incremental
    plan = _incremental_plan(completed_incremental)
    if change == "payload":
        (cache / "sample-session/query-cache.bin").write_bytes(b"different")
    elif change == "receipt":
        receipt.write_text(receipt.read_text(encoding="utf-8") + "\n", encoding="utf-8")
    elif change == "new-file":
        (cache / "sample-session/added.o").write_bytes(b"new")
    elif change == "symlink":
        outside = tmp_path / "valuable.bin"
        outside.write_bytes(b"keep")
        selected = cache / "sample-session/query-cache.bin"
        selected.unlink()
        selected.symlink_to(outside)
    elif change == "missing-lock":
        (cache.parent / ".cargo-build-lock").unlink()
    else:
        (session / "release/.cargo-lock").unlink()
    with pytest.raises((ValueError, OSError)):
        _incremental_plan(
            completed_incremental,
            apply=True,
            expected_inventory_sha256=plan["inventory_sha256"],
        )
    assert cache.is_dir()
    if change == "symlink":
        assert outside.read_bytes() == b"keep"


@pytest.mark.parametrize("replacement", ["lock", "receipt", "owner"])
def test_incremental_retirement_rechecks_after_final_inventory(
    completed_incremental, monkeypatch, replacement
):
    _, _, session, cache, receipt = completed_incremental
    plan = _incremental_plan(completed_incremental)
    original = dg._incremental_inventory
    calls = 0

    def changed(path):
        nonlocal calls
        value = original(path)
        calls += 1
        if calls == 2:
            if replacement == "lock":
                lock = cache.parent / ".cargo-build-lock"
                lock.rename(lock.with_suffix(".old"))
                lock.touch()
            elif replacement == "receipt":
                receipt.write_text(
                    receipt.read_text(encoding="utf-8") + "\n", encoding="utf-8"
                )
            else:
                (session / ".molt-state-owner.json").write_text("{}", encoding="utf-8")
        return value

    monkeypatch.setattr(dg, "_incremental_inventory", changed)
    with pytest.raises(ValueError, match="changed|owner"):
        _incremental_plan(
            completed_incremental,
            apply=True,
            expected_inventory_sha256=plan["inventory_sha256"],
        )
    assert cache.is_dir()


def test_incremental_retirement_cli_requires_review_before_apply(
    completed_incremental, capsys
):
    root, _, _, cache, receipt = completed_incremental
    argv = [
        "--root",
        str(root),
        "--retire-cargo-incremental",
        str(cache),
        "--guard-receipt",
        str(receipt),
    ]
    assert dg.main(argv) == 0
    plan = json.loads(capsys.readouterr().out)
    assert not plan["applied"] and cache.exists()
    assert dg.main([*argv, "--apply"]) == 1
    assert "reviewed" in capsys.readouterr().out and cache.exists()


def test_incremental_retirement_targeted_profile(completed_incremental):
    _, _, session, cache, receipt = completed_incremental
    build = cache.parent.parent
    (build / "wasm32-wasip1").mkdir()
    cache.parent.rename(build / "wasm32-wasip1/release")
    (session / "wasm32-wasip1").mkdir()
    (session / "release").rename(session / "wasm32-wasip1/release")
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    for argv in (
        payload["command"],
        payload["launch_command"],
        payload["child_process"]["command"],
    ):
        argv.extend(["--target", "wasm32-wasip1"])
    receipt.write_text(json.dumps(payload), encoding="utf-8")
    fixture = (
        *completed_incremental[:3],
        build / "wasm32-wasip1/release/incremental",
        receipt,
    )
    plan = _incremental_plan(fixture)
    _incremental_plan(
        fixture, apply=True, expected_inventory_sha256=plan["inventory_sha256"]
    )
    assert not fixture[-2].exists()
    assert (
        fixture[-2].parent / "deps/keep.rlib"
    ).read_bytes() == b"retain product and evidence"


def test_incremental_retirement_refuses_indirect_ancestor(completed_incremental):
    root, _, session, cache, receipt = completed_incremental
    actual = session.with_name("retained-session")
    session.rename(actual)
    session.symlink_to(actual, target_is_directory=True)
    with pytest.raises(ValueError, match="link|junction"):
        dg.retire_cargo_incremental(cache, root=root, guard_receipt=receipt, env={})
    assert (actual / cache.relative_to(session)).is_dir()


def test_incremental_retirement_preserves_durable_failure_evidence(
    completed_incremental, monkeypatch
):
    from molt import file_publication

    _, _, _, cache, receipt = completed_incremental
    plan = _incremental_plan(completed_incremental)
    original = file_publication._reclaim_retired_leaf
    retained = []

    def deny_reclamation(path, identity):
        retained.append(path)
        raise file_publication.RetirementError(
            path, "physical reclamation", OSError("injected storage error")
        )

    monkeypatch.setattr(file_publication, "_reclaim_retired_leaf", deny_reclamation)
    with pytest.raises(file_publication.RetirementError):
        _incremental_plan(
            completed_incremental,
            apply=True,
            expected_inventory_sha256=plan["inventory_sha256"],
        )
    assert not cache.exists() and len(retained) == 1 and retained[0].is_dir()
    assert (
        retained[0] / "sample-session/query-cache.bin"
    ).read_bytes() == b"discardable generated cache"
    assert receipt.exists() and (cache.parent / "deps/keep.rlib").exists()
    # The canonical deletion owner remains responsible for explicit recovery;
    # the failed operation must not report success or recreate the live name.
    monkeypatch.setattr(file_publication, "_reclaim_retired_leaf", original)


@pytest.mark.skipif(os.name == "nt", reason="independent POSIX flock holder")
@pytest.mark.parametrize("name", [".cargo-lock", ".cargo-build-lock"])
def test_incremental_retirement_observes_native_cargo_locks(
    completed_incremental, name
):
    import fcntl

    _, _, session, cache, _ = completed_incremental
    lock = (cache.parent if name == ".cargo-build-lock" else session / "release") / name
    with lock.open("a+b") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        with pytest.raises(ValueError, match="active|live"):
            _incremental_plan(completed_incremental)
    assert cache.is_dir()


@pytest.mark.parametrize("missing", ["receipt", "cache"])
def test_incremental_retirement_refuses_missing_admission_inputs(
    completed_incremental, missing
):
    _, _, _, cache, receipt = completed_incremental
    plan = _incremental_plan(completed_incremental)
    selected = receipt if missing == "receipt" else cache
    retained = selected.with_name(selected.name + ".retained")
    selected.rename(retained)
    with pytest.raises((ValueError, OSError)):
        _incremental_plan(
            completed_incremental,
            apply=True,
            expected_inventory_sha256=plan["inventory_sha256"],
        )
    assert retained.exists()
    assert (
        cache.parent / "deps/keep.rlib"
    ).read_bytes() == b"retain product and evidence"


@pytest.mark.skipif(os.name == "nt", reason="independent POSIX flock oracle")
def test_incremental_retirement_holds_both_split_locks_through_deletion(
    completed_incremental, monkeypatch
):
    import fcntl
    from molt import file_publication

    _, _, session, cache, _ = completed_incremental
    plan = _incremental_plan(completed_incremental)
    original = file_publication.durable_remove_path
    observed = []

    def check_locks(path, **kwargs):
        for lock in (
            cache.parent / ".cargo-build-lock",
            session / "release/.cargo-lock",
        ):
            with lock.open("rb") as handle:
                with pytest.raises(BlockingIOError):
                    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            observed.append(lock)
        original(path, **kwargs)

    monkeypatch.setattr(file_publication, "durable_remove_path", check_locks)
    _incremental_plan(
        completed_incremental,
        apply=True,
        expected_inventory_sha256=plan["inventory_sha256"],
    )
    assert len(observed) == 2 and not cache.exists()


def _superseded_fixture(tmp_path: Path):
    """A live checkout whose target and build control hold the replaced layout.

    Returns the artifact root, its environment, the entries the GC must
    retire and the entries it must keep.
    """
    import time

    from molt import checkout_state

    ext, projects, env = _checkout_family(tmp_path)
    checkout = projects / "alpha"
    checkout.mkdir()
    roots = _checkout_state(ext, checkout)
    checkout_state.stamp_checkout_state(checkout, env)
    target, control = roots["cargo-target"], roots["build-control"]
    digest, sha = "a" * 64, "b" * 64

    def write(path: Path, data: bytes = b"x") -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    native = target / "dev-fast" / ".molt-native-generations"
    write(native / digest / "dev-fast" / "libmolt_runtime.stdlib_micro.a")
    receipt = write(
        target / "dev-fast" / "libmolt_runtime.stdlib_micro.a.generation.json"
    )
    backend = target / "release" / ".molt-backend-generations"
    write(backend / "molt-backend.native_backend" / sha / "molt-backend.native_backend")
    triple = target / "aarch64-apple-darwin" / "dev-fast" / ".molt-native-generations"
    write(triple / digest / "dev-fast" / "libmolt_runtime.stdlib_full.a")
    write(triple.parent / "libmolt_runtime.stdlib_full.a.generation.json")
    probes = control / "backend_probe_validations"
    write(probes / "molt-backend.native_backend.release.0123456789abcdef.json")
    cargo_receipt = write(
        control
        / "backend_fingerprints"
        / "molt-backend.release.0123456789abcdef.fingerprint"
    )
    retire = [
        receipt,
        native,
        backend,
        triple.parent / "libmolt_runtime.stdlib_full.a.generation.json",
        triple,
        cargo_receipt,
        probes,
    ]
    keep = [
        write(target / "dev-fast" / "libmolt_runtime.a"),
        write(target / "dev-fast" / "deps" / "molt_runtime-1.rlib"),
        write(target / "molt-runtime-wasm" / "molt_runtime.generation.json"),
        # The WASM pair's receipt name, beside a replaced generation directory.
        write(target / "dev-fast" / "molt_runtime.generation.json"),
        write(
            control
            / "backend_fingerprints"
            / "molt-backend.native_backend.release.0123456789abcdef.fingerprint"
        ),
        write(control / "runtime_fingerprints" / "runtime.fingerprint"),
    ]
    # A replaced entry written within the protection window stays this pass.
    recent = target / "release-fast" / ".molt-native-generations"
    write(recent / digest / "x")
    keep.append(recent)
    # Everything is idle except the recent entry, so only the frozen names
    # tell a replaced entry from a live one.
    old = time.time() - 3 * HOUR
    for path in (*retire, *keep[:-1]):
        for entry in (path, *(path.rglob("*") if path.is_dir() else ())):
            os.utime(entry, (old, old))
    return ext, env, target, retire, keep


def test_the_gc_retires_the_replaced_per_target_layout_inside_live_roots(tmp_path):
    """Design 56, 2.8: the product store replaced the per-target generations.

    The daily GC removes the replaced entries from roots that stay live, and
    nothing else: Cargo's outputs, the WASM pair, identity memos, and a
    replaced entry used within the protection window stay.
    """
    import time

    ext, env, target, retire, keep = _superseded_fixture(tmp_path)
    now = time.time()
    dry = dg.gc(
        root=ext, config=_cfg(), env=env, now_fn=lambda: now, apply=False, log=False
    )
    assert {row["path"] for row in dry.reclaimed} == {str(p) for p in retire}
    assert all(path.exists() for path in retire)

    result = _gc(ext, env, now)

    reclaimed = {row["path"] for row in result.reclaimed}
    assert reclaimed == {str(path) for path in retire}
    assert {row["kind"] for row in result.reclaimed} == {"superseded-layout"}
    assert not result.errors
    assert not any(path.exists() for path in retire)
    assert all(path.exists() for path in keep)
    assert target.is_dir()
    assert _gc(ext, env, now).reclaimed == []


def test_a_build_using_the_root_keeps_its_replaced_layout(tmp_path):
    """Old code in a checkout that has not taken the change may still read it.

    A managed build holds its target's use for its whole run, and raw Cargo
    holds a profile lock; either keeps every entry of that root.
    """
    import time

    from molt import checkout_state

    ext, env, target, retire, _keep = _superseded_fixture(tmp_path)
    now = time.time()
    with checkout_state.checkout_state_use(((target, None),)):
        result = _gc(ext, env, now)
    target_entries = [path for path in retire if path.is_relative_to(target)]
    assert all(path.exists() for path in target_entries)
    assert {"path": str(target), "reason": "state-in-use"} in result.skipped
    if os.name == "nt":
        return
    import fcntl

    with open(target / "dev-fast" / ".cargo-lock", "a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = _gc(ext, env, now)
    assert all(path.exists() for path in target_entries)
    assert {"path": str(target), "reason": "lock-held"} in result.skipped


def test_only_the_frozen_layout_names_pass_the_layout_choke_point(tmp_path):
    ext, env, target, retire, keep = _superseded_fixture(tmp_path)
    owner = ext.resolve()
    for path in retire:
        root = target if path.is_relative_to(target) else path.parent
        if root != target:
            root = next(p for p in path.parents if p.parent.name == "build-control")
        kind = "cargo-target" if root == target else "build-control"
        dg.assert_safe_to_retire_layout(path, root, owner, kind, dg.DEFAULT_LANE_GLOBS)
    # The last kept entry is a replaced one used within the window: its name
    # passes, and only the idle rule keeps it.
    for path in keep[:-1]:
        with pytest.raises(ValueError):
            dg.assert_safe_to_retire_layout(
                path, target, owner, "cargo-target", dg.DEFAULT_LANE_GLOBS
            )
    with pytest.raises(ValueError, match="allow-set"):
        dg.assert_safe_to_retire_layout(
            retire[0], target.parent, owner, "cargo-target", dg.DEFAULT_LANE_GLOBS
        )


def test_a_miscomputed_layout_entry_never_deletes_outside_its_root(
    tmp_path, monkeypatch
):
    """The layout choke point holds even if the entry list were wrong."""
    import time

    ext, env, target, _retire, _keep = _superseded_fixture(tmp_path)
    stranger = target.parent / "not-a-layout-entry"
    stranger.write_bytes(b"live data beside the target")
    old = time.time() - 3 * HOUR
    os.utime(stranger, (old, old))
    listed = dg.superseded_layout_paths

    def wrong(root, kind):
        found = listed(root, kind)
        return [stranger, *found] if Path(root) == target else found

    monkeypatch.setattr(dg, "superseded_layout_paths", wrong)
    result = _gc(ext, env, time.time())
    assert stranger.exists()
    assert any("outside its root" in error for error in result.errors)
