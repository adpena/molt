"""Teeth for tools/ff_land.py: prove it fails CLOSED on every unsafe land."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

TOOL = Path(__file__).resolve().parents[2] / "tools" / "ff_land.py"


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


def _run(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(TOOL), *args], cwd=repo, capture_output=True, text=True
    )


def _init_commit(repo: Path, name: str = "a.txt", content: str = "v1\n") -> None:
    (repo / name).write_text(content)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", f"add {name}")


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    bare = tmp_path / "remote.git"
    bare.mkdir()
    _git(bare, "init", "--bare", "-q", "-b", "main")
    w = tmp_path / "work"
    w.mkdir()
    _git(w, "init", "-q", "-b", "main")
    _git(w, "config", "user.email", "t@t")
    _git(w, "config", "user.name", "t")
    _git(w, "remote", "add", "origin", str(bare))
    _init_commit(w)
    _git(w, "push", "-q", "origin", "main")
    return w


def _second_clone_pushes(work: Path, tmp_path: Path) -> None:
    """Advance origin/main out-of-band so `work` becomes non-fast-forwardable."""
    origin_url = _git(work, "remote", "get-url", "origin")
    other = tmp_path / "other"
    _git(tmp_path, "clone", "-q", origin_url, str(other))
    _git(other, "config", "user.email", "o@o")
    _git(other, "config", "user.name", "o")
    _init_commit(other, "remote_only.txt", "remote\n")
    _git(other, "push", "-q", "origin", "main")


def test_clean_ff_lands(work: Path) -> None:
    _init_commit(work, "b.txt")
    res = _run(work)
    assert res.returncode == 0, res.stdout
    assert "FF-LAND OK: pushed" in res.stdout
    # remote actually advanced to our HEAD
    assert _git(work, "rev-parse", "HEAD") == _git(work, "rev-parse", "origin/main")


def test_nothing_to_land(work: Path) -> None:
    res = _run(work)
    assert res.returncode == 0, res.stdout
    assert "nothing to land" in res.stdout


def test_dirty_tree_refused(work: Path) -> None:
    (work / "a.txt").write_text("locally edited\n")
    res = _run(work)
    assert res.returncode == 2, res.stdout
    assert "REFUSED" in res.stdout and "uncommitted" in res.stdout


def test_non_fast_forward_refused(work: Path, tmp_path: Path) -> None:
    _second_clone_pushes(work, tmp_path)
    _init_commit(work, "c.txt")  # our own divergent commit
    res = _run(work)
    assert res.returncode == 1, res.stdout
    assert "REFUSED (DRIFT)" in res.stdout
    # our commit must NOT have landed
    assert _git(work, "rev-parse", "HEAD") != _git(work, "rev-parse", "origin/main")


def test_stale_proof_plan_projection_is_refused_before_push(work: Path) -> None:
    generator = work / "tools" / "gen_proof_plan.py"
    generator.parent.mkdir()
    generator.write_text(
        "import sys\nprint('proof-plan projection stale: generated.json')\nsys.exit(1)\n",
        encoding="utf-8",
    )
    _init_commit(work, "b.txt")
    before = _git(work, "rev-parse", "origin/main")

    res = _run(work)

    assert res.returncode == 4, res.stdout
    assert "REFUSED (GENERATED DRIFT)" in res.stdout
    assert "proof-plan projection stale" in res.stdout
    _git(work, "fetch", "origin", "--quiet")
    assert _git(work, "rev-parse", "origin/main") == before


def test_dry_run_does_not_push(work: Path) -> None:
    _init_commit(work, "d.txt")
    before = _git(work, "rev-parse", "origin/main")
    res = _run(work, "--dry-run")
    assert res.returncode == 0, res.stdout
    assert "dry-run" in res.stdout
    _git(work, "fetch", "origin", "--quiet")
    assert _git(work, "rev-parse", "origin/main") == before
