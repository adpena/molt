"""Failure-mode tests for the canonical integration driver (tools/molt_dev.py).

EVERY hazard countermeasure is proven by a test that FIRES the countermeasure
on a synthetic git repo built in tmp (the tests/test_check_suite_honesty.py
style: green-path test + a failure-mode test per guard direction). The repos
are real git repositories with a bare "origin" remote, so the fetch / rebase /
push / ancestor / patch-id flows are exercised end-to-end, not mocked.

Hazard -> proving test(s):
  1 rebase drops commits        test_integrate_detects_dropped_commit
                                test_integrate_accepts_already_upstream_by_patch_id
  2 push exit codes lie         test_verify_push_catches_non_landed_tip
                                test_verify_push_confirms_landed_tip
  3 cleanup loses work          test_cleanup_refuses_unpushed
                                test_cleanup_refuses_dirty_tracked
                                test_cleanup_allows_clean_pushed
                                test_cleanup_force_requires_matching_sha
                                test_cleanup_ignore_set_allows_wasm_sha
  4 partial WIP salvage         test_secure_wip_captures_staged_and_unstaged
                                test_secure_wip_honors_ignore_set
                                test_secure_wip_excludes_untracked_by_default
  5 diff/ls/stat lie            (all verdicts use plumbing/python: implicit in
                                every test; binaries-identical proven directly)
                                test_binaries_identical_helper
  6 stale-binary misattribution test_verify_toolchain_flags_stale_binary
                                test_verify_toolchain_fresh_binary_with_marker
                                test_verify_toolchain_missing_marker_fails
  7 .venv interpreter flips     test_python_oracle_pins_current_version
                                test_python_oracle_refuses_unavailable_version
  8 content-marker verify       test_integrate_marker_mismatch_fails
                                test_integrate_marker_match_passes
  9 liveness/recovery probes    test_probe_file_and_pid
                                test_probe_missing_file_is_nonzero
 10 gate selection by class     test_gate_manifest_selection_backend
                                test_gate_manifest_selection_python
                                test_gate_manifest_always_runs
                                test_committed_gate_manifest_is_valid
                                test_integrate_runs_selected_gate_and_halts_on_failure
 11 backgrounded runs die       test_detached_run_completes_and_records_rc
    silently                    test_detached_run_nonzero_rc_is_loud
                                test_detached_daemon_survives_spawner_and_runs_in_new_session
                                test_detached_run_refuses_live_duplicate_and_never_kills
                                test_detached_verify_detects_died_silent
                                test_detached_verify_too_young_is_not_trusted
                                test_detached_run_missing_command_is_usage_error
                                test_detached_run_exec_failure_records_sentinel_rc
"""

from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import harness_memory_guard  # noqa: E402

SCRIPT_PATH = REPO_ROOT / "tools" / "molt_dev.py"
COMMITTED_GATES = REPO_ROOT / "tools" / "molt_dev_gates.toml"


def _load_driver():
    spec = importlib.util.spec_from_file_location("molt_dev_under_test", SCRIPT_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def drv():
    return _load_driver()


# --------------------------------------------------------------------------
# git repo scaffolding (real repos, bare origin) — exercises the real flows
# --------------------------------------------------------------------------


def _git(repo: Path, *args: str, check: bool = True, input_text: str | None = None):
    proc = harness_memory_guard.guarded_completed_process(
        ["git", "-C", str(repo), *args],
        prefix="MOLT_TEST",
        cwd=REPO_ROOT,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=30.0,
    )
    if check and proc.returncode != 0:
        raise AssertionError(
            f"git {' '.join(args)} failed ({proc.returncode}): {proc.stderr}"
        )
    return proc


def _init_repo(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    _git(path, "init", "-q", "-b", "main")
    _git(path, "config", "user.email", "dev@molt.test")
    _git(path, "config", "user.name", "Molt Dev")
    # Deterministic identity so committer epochs are stable in tests.
    _git(path, "config", "commit.gpgsign", "false")


def _commit_file(repo: Path, rel: str, content: str, msg: str) -> str:
    target = repo / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")
    _git(repo, "add", "--", rel)
    _git(repo, "commit", "-q", "--no-verify", "-m", msg)
    return _git(repo, "rev-parse", "HEAD").stdout.strip()


@pytest.fixture
def origin_and_clone(tmp_path):
    """A bare `origin` plus a working clone tracking origin/main with one commit.

    Returns (origin_bare, work). The work clone is the "agent worktree" the
    driver acts on; origin is the upstream the push lands in.
    """
    origin = tmp_path / "origin.git"
    origin.mkdir()
    _git(origin, "init", "-q", "--bare", "-b", "main")

    seed = tmp_path / "seed"
    _init_repo(seed)
    _commit_file(seed, "README.md", "seed\n", "seed: initial commit")
    _git(seed, "remote", "add", "origin", str(origin))
    _git(seed, "push", "-q", "origin", "main")

    work = tmp_path / "work"
    _git(tmp_path, "clone", "-q", str(origin), str(work))
    _git(work, "config", "user.email", "dev@molt.test")
    _git(work, "config", "user.name", "Molt Dev")
    return origin, work


# --------------------------------------------------------------------------
# Hazard 10: gate manifest selection (pure, no git) — and committed-manifest lint
# --------------------------------------------------------------------------


def test_committed_gate_manifest_is_valid(drv):
    # The shipped manifest must parse and carry rules + always-gates.
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    assert cfg.always, "the committed manifest must define `always` gates"
    assert cfg.rules, "the committed manifest must define rules"
    names = {r.name for r in cfg.rules}
    # The dev-tooling self-gate must exist (the tool gates its own changes).
    assert "dev-tooling" in names
    assert "backend-native" in names


def _write_gates(tmp_path: Path) -> Path:
    cfg = tmp_path / "gates.toml"
    cfg.write_text(
        """
always = ["echo always-ran"]

[[rule]]
name = "backend-native"
globs = ["runtime/molt-backend/**/*.rs"]
gates = ["echo backend-gate"]

[[rule]]
name = "frontend-python"
globs = ["src/molt/**/*.py"]
gates = ["echo python-gate"]
""",
        encoding="utf-8",
    )
    return cfg


def test_gate_manifest_selection_backend(drv, tmp_path):
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["runtime/molt-backend/src/lib.rs"])
    assert "echo always-ran" in gates
    assert "echo backend-gate" in gates
    assert "echo python-gate" not in gates
    assert [r.name for r in matched] == ["backend-native"]


def test_gate_manifest_selection_python(drv, tmp_path):
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["src/molt/frontend.py"])
    assert "echo python-gate" in gates
    assert "echo backend-gate" not in gates
    assert [r.name for r in matched] == ["frontend-python"]


def test_gate_manifest_always_runs(drv, tmp_path):
    # An unmatched change-class still runs `always` gates, nothing else.
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["docs/notes.txt"])
    assert gates == ["echo always-ran"]
    assert matched == []


def test_gate_manifest_dedups_across_rules(drv, tmp_path):
    cfg_path = tmp_path / "g.toml"
    cfg_path.write_text(
        """
always = []
[[rule]]
name = "a"
globs = ["x/**"]
gates = ["echo shared", "echo a-only"]
[[rule]]
name = "b"
globs = ["y/**"]
gates = ["echo shared", "echo b-only"]
""",
        encoding="utf-8",
    )
    cfg = drv.GateConfig.load(cfg_path)
    gates, _ = cfg.select(["x/1", "y/2"])
    # 'echo shared' appears once, preserving first-seen order.
    assert gates == ["echo shared", "echo a-only", "echo b-only"]


def test_gate_manifest_rejects_bad_toml(drv, tmp_path):
    bad = tmp_path / "bad.toml"
    bad.write_text("[[rule]]\nname = 5\nglobs = []\ngates = []\n", encoding="utf-8")
    with pytest.raises(drv.DriverError):
        drv.GateConfig.load(bad)


def test_gate_manifest_missing_is_usage_error(drv, tmp_path):
    with pytest.raises(drv.DriverError) as exc:
        drv.GateConfig.load(tmp_path / "does_not_exist.toml")
    assert exc.value.code == drv.EXIT_USAGE


# --------------------------------------------------------------------------
# Hazard 2: push confirmation by fetch+ancestor (not exit code)
# --------------------------------------------------------------------------


def test_verify_push_confirms_landed_tip(drv, origin_and_clone):
    origin, work = origin_and_clone
    tip = _commit_file(work, "a.py", "print(1)\n", "feat: a")
    _git(work, "push", "-q", "origin", "main")
    ns = _ns(drv, repo=str(work), remote="origin", branch="main", tip=tip, json=False)
    assert drv.cmd_verify_push(ns) == drv.EXIT_OK


def test_verify_push_catches_non_landed_tip(drv, origin_and_clone):
    # A tip that was committed but NOT pushed must be reported as not landed,
    # regardless of any (hypothetically swallowed) push exit code.
    origin, work = origin_and_clone
    tip = _commit_file(work, "a.py", "print(1)\n", "feat: a (unpushed)")
    ns = _ns(drv, repo=str(work), remote="origin", branch="main", tip=tip, json=False)
    assert drv.cmd_verify_push(ns) == drv.EXIT_FAIL


# --------------------------------------------------------------------------
# Hazard 1: rebase drops commits (patch-id survival)
# --------------------------------------------------------------------------


def test_integrate_detects_dropped_commit(drv, origin_and_clone, monkeypatch):
    """Simulate a rebase that DROPS a source commit and prove integrate fails.

    We monkeypatch Git.commits_in_range so that, for the post-rebase branch
    range, the source commit's patch-id is absent (as if a conflict resolution
    discarded it), while the source range still lists it. The patch-id survival
    check must then raise a DriverError naming the dangling sha.
    """
    origin, work = origin_and_clone
    git = drv.Git(work)
    dropped = _commit_file(work, "feature.py", "x = 1\n", "feat: important change")

    source_shas = [dropped]
    pre = {dropped: git.patch_id(dropped)}
    assert pre[dropped] is not None

    # New range contains NOTHING (the commit was 'dropped'); upstream too.
    monkeypatch.setattr(drv.Git, "commits_in_range", lambda self, rng: [], raising=True)
    with pytest.raises(drv.DriverError) as exc:
        drv._verify_no_dropped_commits(
            git, source_shas, pre, "origin/main..HEAD", git.rev_parse("origin/main")
        )
    assert "DROPPED COMMITS" in str(exc.value)
    assert git.short(dropped) in str(exc.value)


def test_integrate_accepts_already_upstream_by_patch_id(drv, origin_and_clone):
    """A source change that is ALREADY upstream (same patch-id) is NOT dropped.

    Build a commit, push an equivalent change to origin under a different sha,
    then verify the survival check accounts for the source by upstream patch-id
    match (the legitimate "moved upstream" dedup) rather than flagging it.
    """
    origin, work = origin_and_clone
    git = drv.Git(work)

    # Local source commit.
    src = _commit_file(work, "shared.py", "value = 42\n", "feat: shared value")
    pre = {src: git.patch_id(src)}

    # Push an EQUIVALENT change to origin from a separate clone (different sha,
    # same diff -> same patch-id).
    other = work.parent / "other"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "dev2@molt.test")
    _git(other, "config", "user.name", "Other Dev")
    _commit_file(other, "shared.py", "value = 42\n", "feat: same shared value")
    _git(other, "push", "-q", "origin", "main")

    git.fetch("origin", "main")
    upstream = git.rev_parse("origin/main")

    # The branch range (origin/main..HEAD) no longer contains the source change
    # by patch-id once origin has it; but the upstream span does. The check must
    # pass (no DriverError) because the patch-id is found upstream.
    # We pass a new_range that excludes the source to force the upstream branch.
    drv._verify_no_dropped_commits(
        git, [src], pre, f"{upstream}..{src}", upstream
    )  # must NOT raise


def test_patch_id_stable_across_rebase(drv, origin_and_clone):
    # The same diff has the same patch-id even after the commit is replayed onto
    # a new base (the property the survival check relies on).
    origin, work = origin_and_clone
    git = drv.Git(work)
    src = _commit_file(work, "p.py", "a = 1\nb = 2\n", "feat: p")
    pid_before = git.patch_id(src)
    # Advance origin so a rebase is required, then rebase.
    other = work.parent / "other2"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "d@molt.test")
    _git(other, "config", "user.name", "D")
    _commit_file(other, "unrelated.py", "z = 9\n", "chore: unrelated")
    _git(other, "push", "-q", "origin", "main")
    git.fetch("origin", "main")
    _git(work, "rebase", "-q", "origin/main")
    new_head = git.head_sha()
    pid_after = git.patch_id(new_head)
    assert new_head != src  # the commit was replayed (new sha)
    assert pid_before == pid_after  # but the patch-id is invariant


# --------------------------------------------------------------------------
# Hazard 3: cleanup refuses on unpushed / dirty; --force needs the sha
# --------------------------------------------------------------------------


def _worktree_off(origin: Path, base: Path, name: str) -> Path:
    """Add a linked worktree off origin/main so cleanup can `worktree remove` it."""
    main_clone = base / "mainclone"
    if not main_clone.exists():
        _git(base, "clone", "-q", str(origin), str(main_clone))
        _git(main_clone, "config", "user.email", "d@molt.test")
        _git(main_clone, "config", "user.name", "D")
    wt = base / name
    _git(main_clone, "worktree", "add", "-q", "--detach", str(wt), "origin/main")
    _git(wt, "config", "user.email", "d@molt.test")
    _git(wt, "config", "user.name", "D")
    return wt


def test_cleanup_allows_clean_pushed(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_clean")
    # No commits beyond origin/main, no dirty tree -> cleanup proceeds.
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert not wt.exists()


def test_cleanup_refuses_unpushed(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_unpushed")
    _commit_file(wt, "local.py", "n = 1\n", "feat: unpushed local")
    git = drv.Git(wt)
    rc = drv._cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()  # NOT removed


def test_cleanup_refuses_dirty_tracked(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_dirty")
    # Modify a TRACKED file without committing -> dirty -> refuse.
    (wt / "README.md").write_text("seed\nlocal edit\n", encoding="utf-8")
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()


def test_cleanup_ignore_set_allows_wasm_sha(drv, origin_and_clone, tmp_path):
    # A change confined to the wasm sha256 ignore set must NOT block cleanup.
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_wasm")
    sha_file = wt / "wasm" / "molt_runtime.wasm.sha256"
    sha_file.parent.mkdir(parents=True, exist_ok=True)
    sha_file.write_text("deadbeef\n", encoding="utf-8")
    _git(wt, "add", "--", "wasm/molt_runtime.wasm.sha256")
    # Staged change to an IGNORED path only -> cleanup still allowed.
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert not wt.exists()


def test_cleanup_force_requires_matching_sha(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_force")
    _commit_file(wt, "local.py", "n = 1\n", "feat: unpushed")
    git = drv.Git(wt)
    head = git.head_sha()
    # Wrong force sha -> refused.
    rc_wrong = drv._cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha="0" * 40
    )
    assert rc_wrong == drv.EXIT_FAIL
    assert wt.exists()
    # Correct force sha (the real HEAD) -> abandon proceeds.
    rc_right = drv._cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=head
    )
    assert rc_right == drv.EXIT_OK
    assert not wt.exists()


def test_cleanup_refuses_main_worktree(drv, origin_and_clone, tmp_path):
    # Targeting the MAIN worktree must be refused (this is for disposable ones).
    origin, _work = origin_and_clone
    main_clone = tmp_path / "main_only"
    _git(tmp_path, "clone", "-q", str(origin), str(main_clone))
    _git(main_clone, "config", "user.email", "d@molt.test")
    _git(main_clone, "config", "user.name", "D")
    rc = drv._cleanup_worktree(
        drv.Git(main_clone),
        main_clone,
        "origin/main",
        drv.DEFAULT_IGNORE_GLOBS,
        force_sha=None,
    )
    assert rc == drv.EXIT_FAIL
    assert main_clone.exists()


# --------------------------------------------------------------------------
# Hazard 4: secure-wip captures staged + unstaged in one recovery commit
# --------------------------------------------------------------------------


def test_secure_wip_captures_staged_and_unstaged(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    # A STAGED modification (index) ...
    (work / "README.md").write_text("seed\nstaged edit\n", encoding="utf-8")
    _git(work, "add", "--", "README.md")
    # ... and an UNSTAGED new tracked file added then edited without re-staging.
    (work / "new_tracked.py").write_text("x = 1\n", encoding="utf-8")
    _git(work, "add", "--", "new_tracked.py")
    (work / "new_tracked.py").write_text("x = 1\ny = 2\n", encoding="utf-8")
    # Now: README.md staged, new_tracked.py has staged+unstaged hunks.

    head_before = git.head_sha()
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    head_after = git.head_sha()
    assert head_after != head_before

    # The single recovery commit must contain BOTH paths at their FULL content
    # (the unstaged y=2 hunk too), proving staged+unstaged were both captured.
    subject = _git(work, "log", "-1", "--format=%s").stdout.strip()
    assert subject.startswith(drv.WIP_MARKER)
    files = set(_git(work, "show", "--name-only", "--format=", "HEAD").stdout.split())
    assert "README.md" in files
    assert "new_tracked.py" in files
    blob = _git(work, "show", "HEAD:new_tracked.py").stdout
    assert "y = 2" in blob  # the UNSTAGED hunk made it in
    # Tree is clean afterward (everything was committed).
    assert git.status_porcelain() == []


def test_secure_wip_honors_ignore_set(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    # A real tracked edit ...
    (work / "README.md").write_text("seed\nedit\n", encoding="utf-8")
    # ... plus a churn to an IGNORED wasm sha file that must be left behind.
    sha = work / "wasm" / "molt_runtime.wasm.sha256"
    sha.parent.mkdir(parents=True, exist_ok=True)
    sha.write_text("cafef00d\n", encoding="utf-8")
    _git(work, "add", "--", "wasm/molt_runtime.wasm.sha256")

    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message="salvage",
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    files = set(_git(work, "show", "--name-only", "--format=", "HEAD").stdout.split())
    assert "README.md" in files
    assert "wasm/molt_runtime.wasm.sha256" not in files  # excluded
    # The ignored file is still pending (not swept into the recovery commit).
    remaining = {p for _xy, p in git.status_porcelain()}
    assert "wasm/molt_runtime.wasm.sha256" in remaining


def test_secure_wip_excludes_untracked_by_default(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    (work / "README.md").write_text("seed\nedit\n", encoding="utf-8")
    (work / "scratch.txt").write_text("scratch\n", encoding="utf-8")  # untracked
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    files = set(_git(work, "show", "--name-only", "--format=", "HEAD").stdout.split())
    assert "README.md" in files
    assert "scratch.txt" not in files
    # The untracked file remains untracked (not captured, not deleted).
    assert (work / "scratch.txt").exists()
    remaining = {p for _xy, p in git.status_porcelain()}
    assert "scratch.txt" in remaining


def test_secure_wip_dry_run_does_not_commit(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    (work / "README.md").write_text("seed\nedit\n", encoding="utf-8")
    head_before = git.head_sha()
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=True,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    assert git.head_sha() == head_before  # no commit made


def test_secure_wip_clean_tree_is_noop(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    head_before = git.head_sha()
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    assert git.head_sha() == head_before


# --------------------------------------------------------------------------
# Hazard 5: comparison verdicts use python/plumbing (binaries-identical)
# --------------------------------------------------------------------------


def test_binaries_identical_helper(drv, tmp_path):
    a = tmp_path / "a.bin"
    b = tmp_path / "b.bin"
    c = tmp_path / "c.bin"
    a.write_bytes(b"\x00\x01\x02molt")
    b.write_bytes(b"\x00\x01\x02molt")
    c.write_bytes(b"\x00\x01\x02MOLT")
    assert drv._binaries_identical(a, b) is True
    assert drv._binaries_identical(a, c) is False
    assert drv._binaries_identical(a, tmp_path / "missing.bin") is False


# --------------------------------------------------------------------------
# Hazard 6: stale-binary misattribution (freshness + behavior marker)
# --------------------------------------------------------------------------


def _fake_binary(path: Path, prints: str) -> None:
    """A tiny executable python 'binary' that prints a marker, runnable under
    safe_run.py (which the driver always uses to run a binary)."""
    path.write_text("#!/usr/bin/env python3\nprint(%r)\n" % prints, encoding="utf-8")
    path.chmod(0o755)


def test_verify_toolchain_flags_stale_binary(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    git = drv.Git(work)
    # Make a binary, then make a NEWER Rust-source commit so the binary is stale.
    binary = tmp_path / "stale_bin"
    _fake_binary(binary, "SETATTR marker present")
    old = binary.stat().st_mtime
    os.utime(binary, (old - 10_000, old - 10_000))  # backdate the binary
    _commit_file(work, "runtime/molt-backend/src/x.rs", "// change\n", "rt: change")

    report = drv.verify_toolchain(
        git, binary, marker=None, probe_args=[], rss_mb=256, timeout=10
    )
    assert report.fresh is False  # mtime < newest rust commit


def test_verify_toolchain_fresh_binary_with_marker(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    git = drv.Git(work)
    _commit_file(work, "runtime/molt-backend/src/y.rs", "// y\n", "rt: y")
    binary = tmp_path / "fresh_bin"
    _fake_binary(binary, "native=148 ready")  # marker in output
    # Binary is created AFTER the commit -> fresh.
    report = drv.verify_toolchain(
        git, binary, marker="native=148", probe_args=[], rss_mb=256, timeout=10
    )
    assert report.fresh is True
    assert report.marker_found is True
    assert report.probe_exit == 0


def test_verify_toolchain_missing_marker_fails(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    git = drv.Git(work)
    _commit_file(work, "runtime/molt-backend/src/z.rs", "// z\n", "rt: z")
    binary = tmp_path / "nomarker_bin"
    _fake_binary(binary, "some other output")
    report = drv.verify_toolchain(
        git, binary, marker="native=148", probe_args=[], rss_mb=256, timeout=10
    )
    assert report.marker_found is False


def test_verify_toolchain_reference_detects_noop_rebuild(
    drv, origin_and_clone, tmp_path
):
    origin, work = origin_and_clone
    git = drv.Git(work)
    a = tmp_path / "bin_a"
    b = tmp_path / "bin_b"
    _fake_binary(a, "v1")
    _fake_binary(b, "v1")  # byte-identical to a (a no-op rebuild)
    report = drv.verify_toolchain(
        git, a, marker=None, probe_args=[], rss_mb=256, timeout=10, reference=b
    )
    assert report.differs_from_reference is False  # identical -> suspicious


# --------------------------------------------------------------------------
# Hazard 7: python-oracle pins and verifies the interpreter version
# --------------------------------------------------------------------------


def test_python_oracle_pins_current_version(drv):
    # The running interpreter's own version must resolve+verify successfully.
    mm = "%d.%d" % sys.version_info[:2]
    exe = drv.resolve_python(mm, prefer_uv=False)
    ok, full = drv._verify_interpreter_version(exe, mm)
    assert ok
    assert full


def test_python_oracle_refuses_unavailable_version(drv):
    # An impossible version must raise LOUDLY (never silently fall back).
    with pytest.raises(drv.DriverError) as exc:
        drv.resolve_python("2.0", prefer_uv=False)
    assert "could not resolve" in str(exc.value)


def test_python_oracle_rejects_bad_format(drv):
    with pytest.raises(drv.DriverError) as exc:
        drv.resolve_python("3.12.1", prefer_uv=False)
    assert exc.value.code == drv.EXIT_USAGE


def test_verify_interpreter_version_mismatch(drv):
    # The current interpreter does NOT report a wrong version.
    ok, _ = drv._verify_interpreter_version(sys.executable, "9.99")
    assert ok is False


# --------------------------------------------------------------------------
# Hazard 8: content-marker verification within integrate
# --------------------------------------------------------------------------


def test_marker_parse_and_check(drv, tmp_path):
    f = tmp_path / "probe.txt"
    f.write_text("hello SETATTR world\n", encoding="utf-8")
    m_exists = drv.Marker.parse("exists:probe.txt")
    ok, _ = m_exists.check(tmp_path)
    assert ok is True
    m_contains = drv.Marker.parse("contains:probe.txt::SETATTR")
    ok, _ = m_contains.check(tmp_path)
    assert ok is True
    m_absent = drv.Marker.parse("contains:probe.txt::NOPE")
    ok, _ = m_absent.check(tmp_path)
    assert ok is False
    m_missing = drv.Marker.parse("exists:nope.txt")
    ok, _ = m_missing.check(tmp_path)
    assert ok is False


def test_marker_parse_rejects_bad_spec(drv):
    with pytest.raises(drv.DriverError):
        drv.Marker.parse("contains:onlypath-no-needle")
    with pytest.raises(drv.DriverError):
        drv.Marker.parse("garbage:foo")


def test_integrate_marker_mismatch_fails(drv, origin_and_clone):
    # A declared marker that is NOT satisfied post-rebase halts integrate before
    # push, LOUDLY (a DriverError — the fail-loud contract). We isolate the
    # marker step with --no-gates/--no-push.
    origin, work = origin_and_clone
    _commit_file(work, "impl.py", "def f():\n    return 1\n", "feat: impl")
    ns = _integrate_ns(
        drv,
        repo=str(work),
        marker=["contains:impl.py::NONEXISTENT_TOKEN"],
        no_gates=True,
        no_push=True,
    )
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_integrate(ns)
    assert "content marker" in str(exc.value)
    # And origin is untouched (no push happened).
    origin_main = _git(work, "rev-parse", "origin/main").stdout.strip()
    seed = _git(work, "rev-list", "--max-parents=0", "origin/main").stdout.strip()
    assert origin_main == seed


def test_integrate_marker_mismatch_via_main_returns_fail(drv, origin_and_clone):
    # The same failure through the top-level main() must translate the
    # DriverError into EXIT_FAIL (the CI-facing exit-code contract).
    origin, work = origin_and_clone
    _commit_file(work, "impl.py", "def f():\n    return 1\n", "feat: impl")
    rc = drv.main(
        [
            "integrate",
            "--repo",
            str(work),
            "--no-gates",
            "--no-push",
            "--marker",
            "contains:impl.py::NONEXISTENT_TOKEN",
        ]
    )
    assert rc == drv.EXIT_FAIL


def test_integrate_marker_match_passes(drv, origin_and_clone):
    origin, work = origin_and_clone
    _commit_file(work, "impl.py", "def f():\n    return 1\n", "feat: impl")
    ns = _integrate_ns(
        drv,
        repo=str(work),
        marker=["exists:impl.py", "contains:impl.py::return 1"],
        no_gates=True,
        no_push=True,  # stop before push so the test stays local
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK


# --------------------------------------------------------------------------
# Hazard 9: probe (file size+mtime, pid liveness)
# --------------------------------------------------------------------------


def test_probe_file_and_pid(drv, tmp_path):
    f = tmp_path / "x.log"
    f.write_text("data\n", encoding="utf-8")
    info = drv.probe_path(f)
    assert info["exists"] is True
    assert info["size"] == 5
    assert "mtime" in info
    live = drv.probe_pid(os.getpid())
    assert live["alive"] is True
    # A pid that is almost certainly dead.
    dead = drv.probe_pid(2_000_000_000)
    assert dead["alive"] is False


def test_probe_missing_file_is_nonzero(drv, tmp_path):
    ns = _ns(drv, file=[str(tmp_path / "nope.log")], pid=None, json=False)
    assert drv.cmd_probe(ns) == drv.EXIT_FAIL


def test_probe_live_pid_is_zero(drv):
    ns = _ns(drv, file=None, pid=[os.getpid()], json=False)
    assert drv.cmd_probe(ns) == drv.EXIT_OK


def test_probe_requires_target(drv):
    ns = _ns(drv, file=None, pid=None, json=False)
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_probe(ns)
    assert exc.value.code == drv.EXIT_USAGE


# --------------------------------------------------------------------------
# integrate: end-to-end gate selection + halt-on-failure, and full push path
# --------------------------------------------------------------------------


def test_integrate_runs_selected_gate_and_halts_on_failure(
    drv, origin_and_clone, tmp_path
):
    # A gate that exits non-zero must halt integrate BEFORE push.
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/m.py", "x = 1\n", "feat: py change")
    gates = tmp_path / "g.toml"
    gates.write_text(
        """
always = []
[[rule]]
name = "frontend-python"
globs = ["src/molt/**/*.py"]
gates = ["exit 3"]
""",
        encoding="utf-8",
    )
    ns = _integrate_ns(
        drv,
        repo=str(work),
        no_gates=False,
        no_push=True,
        gates_config=str(gates),
    )
    # A failing gate halts integrate LOUDLY (DriverError) BEFORE push.
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_integrate(ns)
    assert "gate failed" in str(exc.value)
    # Nothing was pushed (origin unchanged).
    origin_main = _git(work, "rev-parse", "origin/main").stdout.strip()
    seed = _git(work, "rev-list", "--max-parents=0", "origin/main").stdout.strip()
    assert origin_main == seed  # still just the seed commit


def test_integrate_full_push_and_confirm(drv, origin_and_clone, tmp_path):
    # The happy path end-to-end: a passing gate, real push, fetch+ancestor
    # confirm. Proves the push-confirm gate accepts a genuinely-landed tip.
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/ok.py", "ok = True\n", "feat: ok")
    gates = tmp_path / "g.toml"
    gates.write_text(
        """
always = ["true"]
[[rule]]
name = "frontend-python"
globs = ["src/molt/**/*.py"]
gates = ["true"]
""",
        encoding="utf-8",
    )
    git = drv.Git(work)
    tip = git.head_sha()
    ns = _integrate_ns(
        drv,
        repo=str(work),
        no_gates=False,
        no_push=False,
        gates_config=str(gates),
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK
    # origin/main now contains the tip (confirmed by the same ancestor check).
    git.fetch("origin", "main")
    assert git.is_ancestor(tip, "origin/main")


def test_integrate_dry_run_mutates_nothing(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/d.py", "d = 1\n", "feat: d")
    git = drv.Git(work)
    head_before = git.head_sha()
    origin_before = _git(work, "rev-parse", "origin/main").stdout.strip()
    gates = tmp_path / "g.toml"
    gates.write_text("always = []\n", encoding="utf-8")
    ns = _integrate_ns(
        drv,
        repo=str(work),
        dry_run=True,
        no_gates=False,
        no_push=False,
        gates_config=str(gates),
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK
    assert git.head_sha() == head_before
    assert _git(work, "rev-parse", "origin/main").stdout.strip() == origin_before


def test_integrate_nothing_to_do_is_green(drv, origin_and_clone):
    # HEAD already on origin/main -> integrate is a green no-op.
    origin, work = origin_and_clone
    ns = _integrate_ns(drv, repo=str(work), no_gates=True, no_push=True)
    assert drv.cmd_integrate(ns) == drv.EXIT_OK


# --------------------------------------------------------------------------
# namespace builders (argparse.Namespace with the fields each cmd reads)
# --------------------------------------------------------------------------


def _ns(drv, **kwargs):
    import argparse

    return argparse.Namespace(**kwargs)


def _integrate_ns(drv, **overrides):
    base = dict(
        repo=None,
        remote="origin",
        branch="main",
        dry_run=False,
        no_gates=False,
        no_push=False,
        gates_config=None,
        marker=None,
        extra_gate=None,
        cleanup_worktree=False,
        ignore=None,
        session_id="devtool-test",
        toolchain_binary=None,
        toolchain_marker=None,
        toolchain_probe_arg=None,
        rss_mb=512,
        timeout=10,
    )
    base.update(overrides)
    import argparse

    return argparse.Namespace(**base)


# --------------------------------------------------------------------------
# Hazard 11: backgrounded long-runs die silently (detached-run / detached-verify)
# --------------------------------------------------------------------------


def _dr_ns(drv, name, state_root, command, *, cwd=None, env=None, replace=False):
    import argparse

    return argparse.Namespace(
        name=name,
        state_dir=str(state_root),
        cwd=cwd,
        env=env,
        replace=replace,
        verify_min_age_hint=30,
        json=False,
        command=["--", *command],
    )


def _dv_ns(drv, name, state_root, *, min_age_s=0.0, as_json=True):
    import argparse

    return argparse.Namespace(
        name=name, state_dir=str(state_root), min_age_s=min_age_s, json=as_json
    )


def _wait_rc(state: Path, timeout_s: float = 15.0) -> int:
    import time

    deadline = time.monotonic() + timeout_s
    rc_f = state / "rc"
    while time.monotonic() < deadline:
        if rc_f.exists():
            return int(rc_f.read_text().strip())
        time.sleep(0.05)
    raise AssertionError(f"rc file never appeared in {state} within {timeout_s}s")


def _verify_json(drv, capsys, ns) -> tuple[int, dict]:
    import json as _json

    rc = drv.cmd_detached_verify(ns)
    out = capsys.readouterr().out
    payload = next(line for line in out.splitlines() if line.strip().startswith("{"))
    return rc, _json.loads(payload)


def test_detached_run_completes_and_records_rc(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    rc = drv.cmd_detached_run(
        _dr_ns(
            drv,
            "ok-run",
            state_root,
            [sys.executable, "-c", "print('MARKER-OUT', flush=True)"],
        )
    )
    assert rc == drv.EXIT_OK
    state = state_root / "ok-run"
    assert _wait_rc(state) == 0
    # Unbuffered log captured the child's stdout despite daemonization.
    assert "MARKER-OUT" in (state / "run.log").read_text()
    # cmd.json records the exact argv for postmortems.
    import json as _json

    recorded = _json.loads((state / "cmd.json").read_text())
    assert recorded["argv"][0] == sys.executable
    vrc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "ok-run", state_root))
    assert vrc == drv.EXIT_OK
    assert verdict["status"] == "done" and verdict["rc"] == 0


def test_detached_run_nonzero_rc_is_loud(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "fail-run",
            state_root,
            [sys.executable, "-c", "import sys; sys.exit(7)"],
        )
    )
    state = state_root / "fail-run"
    assert _wait_rc(state) == 7
    vrc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "fail-run", state_root))
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "done" and verdict["rc"] == 7


def test_detached_daemon_survives_spawner_and_runs_in_new_session(
    drv, tmp_path, capsys
):
    state_root = tmp_path / "detached"
    rc = drv.cmd_detached_run(
        _dr_ns(
            drv,
            "sleeper",
            state_root,
            [sys.executable, "-c", "import time; time.sleep(2); print('woke')"],
        )
    )
    assert rc == drv.EXIT_OK  # spawner returned while the daemon still runs
    state = state_root / "sleeper"
    assert not (state / "rc").exists()  # still running -> detachment is real
    # New session: daemon sid differs from the test process's sid.
    daemon_sid = int((state / "sid").read_text().strip())
    assert daemon_sid != os.getsid(0)
    vrc, verdict = _verify_json(
        drv, capsys, _dv_ns(drv, "sleeper", state_root, min_age_s=0.0)
    )
    assert vrc == drv.EXIT_OK and verdict["status"] == "running"
    assert _wait_rc(state) == 0
    vrc2, verdict2 = _verify_json(drv, capsys, _dv_ns(drv, "sleeper", state_root))
    assert vrc2 == drv.EXIT_OK and verdict2["status"] == "done"


def test_detached_run_refuses_live_duplicate_and_never_kills(drv, tmp_path):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "dup",
            state_root,
            [sys.executable, "-c", "import time; time.sleep(3)"],
        )
    )
    state = state_root / "dup"
    live_pid = int((state / "pid").read_text().strip())
    # A second spawn under the same name must REFUSE (never kill), even
    # with --replace (replace only clears DEAD state).
    for replace in (False, True):
        with pytest.raises(drv.DriverError, match="RUNNING"):
            drv.cmd_detached_run(
                _dr_ns(
                    drv,
                    "dup",
                    state_root,
                    [sys.executable, "-c", "print('imposter')"],
                    replace=replace,
                )
            )
    # The original daemon was not harmed by the refusals.
    assert drv.probe_pid(live_pid)["alive"]
    assert _wait_rc(state) == 0
    # After completion: same name still refuses WITHOUT --replace...
    with pytest.raises(drv.DriverError, match="--replace"):
        drv.cmd_detached_run(
            _dr_ns(drv, "dup", state_root, [sys.executable, "-c", "print('x')"])
        )
    # ...and respawns cleanly WITH --replace.
    rc = drv.cmd_detached_run(
        _dr_ns(
            drv,
            "dup",
            state_root,
            [sys.executable, "-c", "print('second')"],
            replace=True,
        )
    )
    assert rc == drv.EXIT_OK
    assert _wait_rc(state) == 0
    assert "second" in (state / "run.log").read_text()


def test_detached_verify_detects_died_silent(drv, tmp_path, capsys):
    # Fabricate the hazard-11 signature: a pid that is GONE and no rc file.
    dead_pid = 999_999_999
    state = tmp_path / "detached" / "ghost"
    state.mkdir(parents=True)
    (state / "pid").write_text(str(dead_pid))
    (state / "run.log").write_text("")
    vrc, verdict = _verify_json(
        drv, capsys, _dv_ns(drv, "ghost", tmp_path / "detached")
    )
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "died-silent"


def test_detached_verify_too_young_is_not_trusted(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "young",
            state_root,
            [sys.executable, "-c", "import time; time.sleep(3)"],
        )
    )
    vrc, verdict = _verify_json(
        drv, capsys, _dv_ns(drv, "young", state_root, min_age_s=9999.0)
    )
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "too-young"
    assert _wait_rc(state_root / "young") == 0


def test_detached_run_missing_command_is_usage_error(drv, tmp_path):
    with pytest.raises(drv.DriverError) as exc_info:
        drv.cmd_detached_run(_dr_ns(drv, "noop", tmp_path / "detached", []))
    assert exc_info.value.code == drv.EXIT_USAGE


def test_detached_run_exec_failure_records_sentinel_rc(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "noexec",
            state_root,
            ["/nonexistent/binary/definitely-not-here"],
        )
    )
    state = state_root / "noexec"
    assert _wait_rc(state) == 127  # exec-failure sentinel, NOT died-silent
    assert "exec failed" in (state / "run.log").read_text()
    vrc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "noexec", state_root))
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "done" and verdict["rc"] == 127


# --------------------------------------------------------------------------
# Hazard 12: split-root toolchain (difftest)
# --------------------------------------------------------------------------


def test_difftest_toolchain_env_roots_frontend_and_backend_together(drv, tmp_path):
    """The hazard-12 core: ONE --root drives BOTH the frontend import path
    (PYTHONPATH) and the runtime/backend build root (MOLT_PROJECT_ROOT), so a
    worktree edit can never be silently compiled out of one but not the other.
    """
    root = tmp_path / "wt"
    env = drv._difftest_toolchain_env(root, "sess-x", None)
    assert env["MOLT_PROJECT_ROOT"] == str(root)
    assert env["PYTHONPATH"].split(os.pathsep)[0] == str(root / "src")
    assert env["MOLT_SESSION_ID"] == "sess-x"


def test_difftest_toolchain_env_preserves_existing_pythonpath(
    drv, tmp_path, monkeypatch
):
    monkeypatch.setenv("PYTHONPATH", "/pre/existing")
    env = drv._difftest_toolchain_env(tmp_path, "s", None)
    parts = env["PYTHONPATH"].split(os.pathsep)
    assert parts[0] == str(tmp_path / "src")
    assert "/pre/existing" in parts  # prepended, not clobbered


def test_difftest_toolchain_env_passthrough_and_bad_kv(drv, tmp_path):
    env = drv._difftest_toolchain_env(tmp_path, "s", ["MOLT_TRACE_X=1"])
    assert env["MOLT_TRACE_X"] == "1"
    with pytest.raises(drv.DriverError) as exc:
        drv._difftest_toolchain_env(tmp_path, "s", ["NOEQUALS"])
    assert exc.value.code == drv.EXIT_USAGE


def _difftest_ns(drv, program, root):
    return drv.build_parser().parse_args(
        ["difftest", str(program), "--root", str(root)]
    )


def test_difftest_refuses_non_molt_root(drv, tmp_path):
    """A --root without the Molt CLI package is a LOUD usage refusal, never a
    build that silently uses the canonical checkout's runtime."""
    prog = tmp_path / "p.py"
    prog.write_text("print('hi')\n", encoding="utf-8")
    not_a_checkout = tmp_path / "empty"
    not_a_checkout.mkdir()
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_difftest(_difftest_ns(drv, prog, not_a_checkout))
    assert exc.value.code == drv.EXIT_USAGE
    assert "not a molt checkout" in str(exc.value)


def test_difftest_refuses_missing_program(drv):
    """A missing program file refuses before any build (root is the real repo
    so the root check passes and the program check fires)."""
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_difftest(_difftest_ns(drv, REPO_ROOT / "does-not-exist.py", REPO_ROOT))
    assert exc.value.code == drv.EXIT_USAGE
    assert "not found" in str(exc.value)


def test_difftest_roots_relative_output_dir_before_safe_run(
    drv, tmp_path, monkeypatch
):
    """A relative --out-dir is part of the rooted toolchain, not a process-cwd
    accident.

    On Windows, handing safe_run a relative extensionless native binary path can
    fail at spawn time even though the build wrote the artifact successfully.
    The difftest runner must pass an absolute, --root-scoped artifact path to
    both build verification and safe_run.
    """
    root = tmp_path / "wt"
    (root / "src" / "molt" / "cli").mkdir(parents=True)
    (root / "tools").mkdir()
    (root / "src" / "molt" / "cli" / "__init__.py").write_text("", encoding="utf-8")
    (root / "tools" / "safe_run.py").write_text("", encoding="utf-8")
    program = root / "case.py"
    program.write_text("print('ok')\n", encoding="utf-8")

    monkeypatch.setattr(
        drv, "_verify_interpreter_version", lambda _python, _version: (True, "3.12.0")
    )

    class Probe:
        returncode = 0
        stderr = ""

    monkeypatch.setattr(drv, "_run_driver_command", lambda *a, **kw: Probe())

    captured: list[list[str]] = []

    def fake_capture(cmd, *, env, cwd, timeout):
        captured.append(list(cmd))
        if len(cmd) >= 2 and cmd[1] == str(program):
            return 0, b"ok\r\n", b""
        if cmd[1:3] == ["-m", "molt"]:
            output = Path(cmd[cmd.index("--output") + 1])
            assert output.is_absolute()
            assert output.parent == root / "tmp" / "difftest-rel"
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"MZ")
            return 0, b"", b""
        if cmd[1:2] == [str(root / "tools" / "safe_run.py")]:
            artifact = Path(cmd[cmd.index("--") + 1])
            assert artifact.is_absolute()
            assert artifact.parent == root / "tmp" / "difftest-rel"
            assert artifact.exists()
            return 0, b"ok\n", b""
        return 0, b"", b""

    monkeypatch.setattr(drv, "_difftest_capture", fake_capture)

    ns = drv.build_parser().parse_args(
        [
            "difftest",
            str(program),
            "--root",
            str(root),
            "--target",
            "native",
            "--python-version",
            "3.12",
            "--out-dir",
            "tmp/difftest-rel",
        ]
    )
    assert drv.cmd_difftest(ns) == drv.EXIT_OK

    run_cmds = [cmd for cmd in captured if str(root / "tools" / "safe_run.py") in cmd]
    assert len(run_cmds) == 1
