"""Teeth for tools/claim_lane.py: the solo-lane claim protocol must be atomic
and fail-closed (back off on a live claim; allow claim on free/stale/released)."""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import datetime as dt
import subprocess
import sys
from pathlib import Path

import pytest

from tests.integration_driver_fixture import (
    clone_tree,
    copy_driver_trees,
    shared_session_tree,
)

TOOLS = Path(__file__).resolve().parents[2] / "tools"
CLAIM = TOOLS / "claim_lane.py"


def _git(repo: Path, *args: str) -> str:
    return run_guarded_test_process(
        ["git", *args],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def _run(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [sys.executable, str(CLAIM), *args],
        cwd=repo,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )


def _claims_body(rows: str) -> str:
    return (
        "# Solo-Owner Lane Claims\n\n"
        "## Log (append-only)\n\n"
        "| lane | agent-id | UTC (ISO) | status | note / evidence |\n"
        "|------|----------|-----------|--------|-----------------|\n" + rows
    )


def _build_seed(root: Path) -> None:
    """Minimal committed claims history for real fetch/read/refusal checks."""
    bare = root / "remote.git"
    bare.mkdir()
    _git(bare, "init", "--bare", "-q", "-b", "main")
    w = root / "work"
    (w / "docs" / "agent").mkdir(parents=True)
    _git(w, "init", "-q", "-b", "main")
    _git(w, "config", "user.email", "t@t")
    _git(w, "config", "user.name", "t")
    (w / "docs" / "agent" / "CLAIMS.md").write_text(_claims_body(""), encoding="utf-8")
    _git(w, "add", "-A")
    _git(w, "commit", "-q", "-m", "init")
    _git(w, "push", "-q", str(bare), "main")


def _build_integration_seed(root: Path, seed: Path, driver: Path) -> None:
    """The claims history with main's complete pinned controller and policy."""
    clone_tree(seed / "remote.git", root / "remote.git")
    clone_tree(seed / "work", root / "work")
    w = root / "work"
    copy_driver_trees(w, seed=driver)
    (w / "tools/proof_plan.toml").write_text(
        'always = ["echo claim-policy"]\n', encoding="utf-8"
    )
    _git(w, "add", "-A")
    _git(w, "commit", "-q", "-m", "pin integration controller")
    _git(w, "push", "-q", str(root / "remote.git"), "main")


@pytest.fixture(scope="session")
def _seed(tmp_path_factory: pytest.TempPathFactory, worker_id: str) -> Path:
    """One claims history per session, shared by every xdist worker."""
    return shared_session_tree(tmp_path_factory, worker_id, "claim-seed", _build_seed)


@pytest.fixture(scope="session")
def _integration_seed(
    tmp_path_factory: pytest.TempPathFactory,
    worker_id: str,
    _seed: Path,
    driver_seed: Path,
) -> Path:
    """Only landing tests need the driver; it is committed once per session."""
    return shared_session_tree(
        tmp_path_factory,
        worker_id,
        "claim-integration-seed",
        lambda root: _build_integration_seed(root, _seed, driver_seed),
    )


def _copy_repo(tmp_path: Path, seed: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    # These temporary repositories borrow the test runner's environment. Merely
    # deleting UV_PROJECT_ENVIRONMENT is insufficient: the outer guard supplies
    # its own checkout's managed default before launching the temporary driver.
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        monkeypatch.delenv(key, raising=False)
    # The guard reintroduces DX defaults. These shell-policy fixtures borrow
    # their already-running interpreter; they must never sync a temporary
    # editable checkout into the parent test environment.
    monkeypatch.setenv("MOLT_VENV", sys.prefix)
    # Clone (copy-on-write), never hardlink: each test mutates refs and objects.
    clone_tree(seed / "remote.git", tmp_path / "remote.git")
    clone_tree(seed / "work", tmp_path / "work")
    w = tmp_path / "work"
    _git(w, "remote", "add", "origin", str(tmp_path / "remote.git"))
    _git(w, "fetch", "-q", "origin")
    _git(w, "branch", "-q", "--set-upstream-to=origin/main", "main")
    return w


@pytest.fixture()
def repo(tmp_path: Path, _seed: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    return _copy_repo(tmp_path, _seed, monkeypatch)


@pytest.fixture()
def driver_repo(
    tmp_path: Path, _integration_seed: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    return _copy_repo(tmp_path, _integration_seed, monkeypatch)


def _set_claims(repo: Path, rows: str) -> None:
    (repo / "docs" / "agent" / "CLAIMS.md").write_text(
        _claims_body(rows), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-m", "seed claims", "--", "docs/agent/CLAIMS.md")
    _git(repo, "push", "-q", "origin", "main")


def _row(lane: str, agent: str, when: dt.datetime, status: str, note: str = "x") -> str:
    return f"| {lane} | {agent} | {when.strftime('%Y-%m-%dT%H:%M:%SZ')} | {status} | {note} |\n"


def test_check_unclaimed_is_claimable(repo: Path) -> None:
    res = _run(repo, "L1", "--check")
    assert res.returncode == 0 and "UNCLAIMED" in res.stdout


def test_check_live_claim_backs_off(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "other", now, "CLAIMED"))
    res = _run(repo, "L1", "--check")
    assert res.returncode == 1 and "CLAIMED-ALIVE" in res.stdout


def test_check_stale_claim_is_reclaimable(repo: Path) -> None:
    old = dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=5)
    _set_claims(repo, _row("L1", "other", old, "CLAIMED"))
    res = _run(repo, "L1", "--check")
    assert res.returncode == 0 and "STALE" in res.stdout


def test_claim_free_lane_lands(driver_repo: Path) -> None:
    res = _run(driver_repo, "L1", "--claim", "--agent", "me", "--note", "go")
    assert res.returncode == 0, res.stdout
    assert "CLAIMED L1 as me" in res.stdout
    _git(driver_repo, "fetch", "origin", "--quiet")
    assert "| L1 | me |" in _git(
        driver_repo, "show", "origin/main:docs/agent/CLAIMS.md"
    )


def test_claim_held_lane_refused(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "other", now, "CLAIMED"))
    before = _git(repo, "rev-parse", "origin/main")
    res = _run(repo, "L1", "--claim", "--agent", "me")
    assert res.returncode == 1 and "BACK OFF" in res.stdout
    _git(repo, "fetch", "origin", "--quiet")
    assert _git(repo, "rev-parse", "origin/main") == before  # nothing landed


def test_append_progress_by_claimant_lands(driver_repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(driver_repo, _row("L1", "me", now, "CLAIMED"))
    res = _run(
        driver_repo,
        "L1",
        "--append",
        "PROGRESS",
        "--agent",
        "me",
        "--note",
        "run-123",
    )
    assert res.returncode == 0, res.stdout
    assert "PROGRESS" in _git(driver_repo, "show", "origin/main:docs/agent/CLAIMS.md")


def test_live_takeover_by_other_refused(repo: Path) -> None:
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(repo, _row("L1", "owner", now, "CLAIMED"))
    res = _run(repo, "L1", "--append", "PROGRESS", "--agent", "intruder")
    assert res.returncode == 1 and "REFUSED" in res.stdout


def test_check_refuses_a_log_with_an_unknown_status(repo: Path) -> None:
    # The old reader skipped the newest row and reported the older CLAIMED one.
    now = dt.datetime.now(dt.timezone.utc)
    _set_claims(
        repo,
        _row("L1", "other", now - dt.timedelta(hours=1), "CLAIMED")
        + _row("L1", "other", now, "BLOCKED"),
    )
    res = _run(repo, "L1", "--check")
    assert res.returncode == 2, res.stdout
    assert "'BLOCKED'" in res.stderr


def test_append_refuses_a_note_that_breaks_the_table(repo: Path) -> None:
    before = _git(repo, "rev-parse", "origin/main")
    res = _run(repo, "L1", "--claim", "--agent", "me", "--note", "a | b")
    assert res.returncode == 2 and "--note" in res.stdout
    _git(repo, "fetch", "origin", "--quiet")
    assert _git(repo, "rev-parse", "origin/main") == before


@pytest.mark.skipif(sys.platform == "win32", reason="executable POSIX Git hook")
def test_claim_race_between_precheck_and_integration_preserves_winner(
    driver_repo: Path,
):
    import shlex

    other = driver_repo.parent / "competitor"
    _git(
        driver_repo.parent,
        "clone",
        "-q",
        str(driver_repo.parent / "remote.git"),
        str(other),
    )
    _git(other, "config", "user.email", "other@test")
    _git(other, "config", "user.name", "Other")
    (other / "docs/agent/CLAIMS.md").write_text(
        _claims_body(_row("L1", "winner", dt.datetime.now(dt.timezone.utc), "CLAIMED")),
        encoding="utf-8",
    )
    _git(other, "commit", "-qm", "winner", "--", "docs/agent/CLAIMS.md")
    winner = _git(other, "rev-parse", "HEAD")
    # The real claim command has already read origin when its commit-msg hook
    # publishes the competing claim. Integrate must not rebase across it.
    hook = driver_repo / ".git/hooks/commit-msg"
    hook.write_text(
        "#!/bin/sh\n"
        + shlex.join(["git", "-C", str(other), "push", "origin", "main"])
        + "\n",
        encoding="utf-8",
    )
    hook.chmod(0o755)
    result = _run(driver_repo, "L1", "--claim", "--agent", "loser")
    assert result.returncode != 0
    assert "expected upstream changed" in result.stderr
    assert (
        _git(driver_repo, "ls-remote", "origin", "refs/heads/main").split()[0] == winner
    )
    assert "| L1 | loser |" in (driver_repo / "docs/agent/CLAIMS.md").read_text(
        encoding="utf-8"
    )
    assert "| L1 | winner |" in _git(
        driver_repo, "show", "origin/main:docs/agent/CLAIMS.md"
    )


def test_claim_runs_committed_policy_and_reports_gate_failure(driver_repo: Path):
    (driver_repo / "tools/proof_plan.toml").write_text(
        'always = ["exit 17"]\n', encoding="utf-8"
    )
    _git(
        driver_repo,
        "commit",
        "-qm",
        "required gate",
        "--",
        "tools/proof_plan.toml",
    )
    _git(driver_repo, "push", "origin", "main")
    original = _git(driver_repo, "rev-parse", "origin/main")
    result = _run(driver_repo, "L1", "--claim", "--agent", "me")
    assert result.returncode != 0
    assert "gate failed (exit 17)" in result.stderr
    assert "lost the claim race" not in result.stdout
    assert (
        _git(driver_repo, "ls-remote", "origin", "refs/heads/main").split()[0]
        == original
    )


def test_claim_precheck_fetches_main_when_configured_fetch_excludes_it(repo: Path):
    old = _git(repo, "rev-parse", "origin/main")
    other = repo.parent / "fresh-claimant"
    _git(repo.parent, "clone", "-q", str(repo.parent / "remote.git"), str(other))
    _git(other, "config", "user.email", "other@test")
    _git(other, "config", "user.name", "Other")
    _git(other, "push", "origin", "HEAD:refs/heads/other")
    _set_claims(
        other, _row("L1", "winner", dt.datetime.now(dt.timezone.utc), "CLAIMED")
    )
    winner = _git(other, "rev-parse", "HEAD")
    _git(
        repo,
        "config",
        "--replace-all",
        "remote.origin.fetch",
        "+refs/heads/other:refs/remotes/origin/other",
    )
    assert _git(repo, "rev-parse", "origin/main") == old
    result = _run(repo, "L1", "--check")
    assert result.returncode == 1
    assert "CLAIMED-ALIVE" in result.stdout and "winner" in result.stdout
    assert _git(repo, "rev-parse", "origin/main") == winner
    assert _git(repo, "rev-parse", "HEAD") == old
    # Fresh precheck also prevents a local claim commit, not merely its push.
    result = _run(repo, "L1", "--claim", "--agent", "loser")
    assert result.returncode == 1 and "BACK OFF" in result.stdout
    assert _git(repo, "rev-parse", "HEAD") == old
