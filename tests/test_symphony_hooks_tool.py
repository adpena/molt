from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "tools" / "symphony_hooks.py"


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
    _git(worker, "config", "user.name", "adpena")
    _git(worker, "config", "user.email", "adpena@example.com")
    return seed, worker


def test_python_before_run_skips_dirty_workspace_without_failure(
    tmp_path: Path,
) -> None:
    _seed, worker = _setup_repo(tmp_path)
    head_before = _git(worker, "rev-parse", "HEAD").stdout.strip()
    (worker / "README.md").write_text("dirty\n", encoding="utf-8")

    proc = subprocess.run(
        [sys.executable, str(SCRIPT_PATH), "before_run"],
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


def test_python_after_run_autolands_and_sets_guard_pythonpath(tmp_path: Path) -> None:
    _seed, worker = _setup_repo(tmp_path)
    (worker / "app.py").write_text("print('ok')\n", encoding="utf-8")
    tools_dir = worker / "tools"
    tools_dir.mkdir(parents=True, exist_ok=True)
    (tools_dir / "secret_guard.py").write_text(
        "import os\n"
        "import sys\n"
        "sys.exit(0 if os.environ.get('PYTHONPATH') == 'src' else 2)\n",
        encoding="utf-8",
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = ""
    env["MOLT_SYMPHONY_AUTOLAND_ENABLED"] = "1"
    env["MOLT_SYMPHONY_AUTOLAND_MODE"] = "direct-main"
    env["MOLT_SYMPHONY_SYNC_REMOTE"] = "origin"
    env["MOLT_SYMPHONY_SYNC_BRANCH"] = "main"
    env["MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS"] = "adpena,symphony"
    env["MOLT_SYMPHONY_TRUSTED_USERS"] = "adpena,symphony"
    env["MOLT_SYMPHONY_TRUSTED_MACHINES"] = ""

    proc = subprocess.run(
        [sys.executable, str(SCRIPT_PATH), "after_run"],
        cwd=worker,
        check=False,
        text=True,
        capture_output=True,
        env=env,
    )

    assert proc.returncode == 0
    assert "ok status=pushed mode=direct-main branch=main" in proc.stdout
    last_subject = _git(worker, "log", "-1", "--pretty=%s").stdout.strip()
    assert last_subject == "chore: sync all changes"
