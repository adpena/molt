"""Teeth for tools/tree_drift_check.py: prove it flags stale/dirty trees LOUD."""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import subprocess
import sys
from pathlib import Path

import pytest

TOOL = Path(__file__).resolve().parents[2] / "tools" / "tree_drift_check.py"


def _git(repo: Path, *args: str) -> str:
    return run_guarded_test_process(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


def _run_tool(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [sys.executable, str(TOOL), *args],
        cwd=repo,
        capture_output=True,
        text=True,
    )


@pytest.fixture()
def repo(tmp_path: Path) -> Path:
    r = tmp_path / "repo"
    r.mkdir()
    _git(r, "init", "-q")
    _git(r, "config", "user.email", "t@t")
    _git(r, "config", "user.name", "t")
    (r / "a.txt").write_text("v1\n")
    (r / "b.txt").write_text("keep\n")
    _git(r, "add", "-A")
    _git(r, "commit", "-q", "-m", "c1")
    # advance the base one commit past HEAD so HEAD is STALE for a.txt
    (r / "a.txt").write_text("v2\n")
    _git(r, "add", "-A")
    _git(r, "commit", "-q", "-m", "c2")
    _git(r, "branch", "base")
    # move HEAD back to c1 (detached): now HEAD is 1 behind base, a.txt stale
    _git(r, "checkout", "-q", "--detach", "HEAD~1")
    return r


def test_stale_file_is_loud_and_fails(repo: Path) -> None:
    res = _run_tool(repo, "--base", "base", "--files", "a.txt")
    assert res.returncode == 1, res.stdout
    assert "DRIFT" in res.stdout
    assert "STALE" in res.stdout and "a.txt" in res.stdout


def test_clean_file_passes(repo: Path) -> None:
    # b.txt is identical on HEAD and base -> not masking
    res = _run_tool(repo, "--base", "base", "--files", "b.txt")
    assert res.returncode == 0, res.stdout
    assert "OK" in res.stdout


def test_dirty_file_is_loud(repo: Path) -> None:
    (repo / "b.txt").write_text("locally-edited\n")
    res = _run_tool(repo, "--base", "base", "--files", "b.txt")
    assert res.returncode == 1, res.stdout
    assert "DIRTY" in res.stdout


def test_untracked_file_is_loud(repo: Path) -> None:
    (repo / "c.txt").write_text("new\n")
    res = _run_tool(repo, "--base", "base", "--files", "c.txt")
    assert res.returncode == 1, res.stdout
    assert "UNTRACKED" in res.stdout


def test_unscoped_behind_tree_fails(repo: Path) -> None:
    # HEAD is 1 behind base with no file scope -> materially stale -> DRIFT
    res = _run_tool(repo, "--base", "base")
    assert res.returncode == 1, res.stdout
    assert "behind=1" in res.stdout


def test_up_to_date_tree_passes(repo: Path) -> None:
    _git(repo, "checkout", "-q", "base")
    res = _run_tool(repo, "--base", "base")
    assert res.returncode == 0, res.stdout
    assert "behind=0" in res.stdout
