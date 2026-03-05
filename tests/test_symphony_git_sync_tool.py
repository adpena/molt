from __future__ import annotations

import os
import subprocess
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "tools" / "symphony_git_sync.sh"


def _git(
    cwd: Path,
    *args: str,
    env: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=check,
        text=True,
        capture_output=True,
        env=env,
    )


def _commit(cwd: Path, *, message: str, name: str, email: str) -> None:
    env = os.environ.copy()
    env["GIT_AUTHOR_NAME"] = name
    env["GIT_AUTHOR_EMAIL"] = email
    env["GIT_COMMITTER_NAME"] = name
    env["GIT_COMMITTER_EMAIL"] = email
    _git(cwd, "add", "-A")
    _git(cwd, "commit", "-m", message, env=env)


def _setup_repo(tmp_path: Path) -> tuple[Path, Path]:
    remote = tmp_path / "remote.git"
    seed = tmp_path / "seed"
    worker = tmp_path / "worker"

    _git(tmp_path, "init", "--bare", str(remote))
    _git(tmp_path, "init", "-b", "main", str(seed))

    (seed / "README.md").write_text("seed\n", encoding="utf-8")
    _commit(seed, message="seed", name="adpena", email="adpena@example.com")
    _git(seed, "remote", "add", "origin", str(remote))
    _git(seed, "push", "-u", "origin", "main")

    _git(tmp_path, "clone", str(remote), str(worker))
    return seed, worker


def test_sync_script_skips_dirty_workspace_without_failure(tmp_path: Path) -> None:
    _seed, worker = _setup_repo(tmp_path)
    head_before = _git(worker, "rev-parse", "HEAD").stdout.strip()
    (worker / "README.md").write_text("dirty\n", encoding="utf-8")

    proc = subprocess.run(
        ["bash", str(SCRIPT_PATH)],
        cwd=worker,
        check=False,
        text=True,
        capture_output=True,
        env=os.environ.copy(),
    )

    assert proc.returncode == 0
    assert "skip reason=dirty_workspace" in proc.stdout
    head_after = _git(worker, "rev-parse", "HEAD").stdout.strip()
    assert head_after == head_before


def test_sync_script_fast_forwards_when_incoming_authors_are_allowed(
    tmp_path: Path,
) -> None:
    seed, worker = _setup_repo(tmp_path)
    head_before = _git(worker, "rev-parse", "HEAD").stdout.strip()

    (seed / "README.md").write_text("seed\nallowed-update\n", encoding="utf-8")
    _commit(seed, message="allowed update", name="adpena", email="adpena@example.com")
    _git(seed, "push", "origin", "main")

    env = os.environ.copy()
    env["MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS"] = "adpena,symphony"
    proc = subprocess.run(
        ["bash", str(SCRIPT_PATH)],
        cwd=worker,
        check=False,
        text=True,
        capture_output=True,
        env=env,
    )

    assert proc.returncode == 0
    assert "ok status=fast_forward_applied" in proc.stdout
    head_after = _git(worker, "rev-parse", "HEAD").stdout.strip()
    assert head_after != head_before


def test_sync_script_blocks_auto_merge_for_disallowed_authors(tmp_path: Path) -> None:
    seed, worker = _setup_repo(tmp_path)
    head_before = _git(worker, "rev-parse", "HEAD").stdout.strip()

    (seed / "README.md").write_text("seed\ndisallowed-update\n", encoding="utf-8")
    _commit(
        seed, message="outsider update", name="outsider", email="outsider@example.com"
    )
    _git(seed, "push", "origin", "main")

    env = os.environ.copy()
    env["MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS"] = "adpena,symphony"
    proc = subprocess.run(
        ["bash", str(SCRIPT_PATH)],
        cwd=worker,
        check=False,
        text=True,
        capture_output=True,
        env=env,
    )

    assert proc.returncode == 0
    assert "skip reason=author_gate_blocked" in proc.stdout
    head_after = _git(worker, "rev-parse", "HEAD").stdout.strip()
    assert head_after == head_before
