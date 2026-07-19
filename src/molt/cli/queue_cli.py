from __future__ import annotations

import argparse
import os
from pathlib import Path
from subprocess import SubprocessError
import sys

from molt.cli.project_roots import _find_molt_root
from molt.dx import DxConfigError, development_artifact_env
from molt import process_guard

PROOF_QUEUE_SIZE_ENV = "MOLT_PROOF_QUEUE_SIZE"


def _positive_queue_size(value: object) -> str:
    try:
        parsed = int(str(value).strip())
    except (TypeError, ValueError) as exc:
        raise ValueError(
            f"--queue-size must be a positive integer, got {value!r}"
        ) from exc
    if parsed < 1:
        raise ValueError(f"--queue-size must be a positive integer, got {value!r}")
    return str(parsed)


def _queue_repo_root() -> Path:
    return _find_molt_root(Path.cwd()) or Path(__file__).resolve().parents[3]


def _queue_args_define_queue_size(queue_args: list[str]) -> bool:
    for token in queue_args:
        if token == "--":
            return False
        if token == "--queue-size" or token.startswith("--queue-size="):
            return True
    return False


def _valid_project_env(path: Path) -> bool:
    return (path / "pyvenv.cfg").exists()


def _resolve_candidate_venv(repo_root: Path, raw: str | None) -> Path | None:
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = repo_root / path
    path = path.resolve()
    return path if _valid_project_env(path) else None


def _main_worktree_venv(repo_root: Path) -> Path | None:
    if not (repo_root / ".git").exists():
        return None
    try:
        proc = process_guard.run_completed_command(
            ["git", "rev-parse", "--git-common-dir"],
            cwd=repo_root,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    common = Path(proc.stdout.strip())
    if not common.is_absolute():
        common = (repo_root / common).resolve()
    candidate = common.parent / ".venv"
    return candidate if _valid_project_env(candidate) else None


def _warm_project_env(repo_root: Path, env: dict[str, str]) -> Path | None:
    explicit = _resolve_candidate_venv(repo_root, env.get("UV_PROJECT_ENVIRONMENT"))
    if explicit is not None:
        return explicit
    active = _resolve_candidate_venv(repo_root, env.get("VIRTUAL_ENV"))
    if active is not None:
        return active
    local = repo_root / ".venv"
    if _valid_project_env(local):
        return local.resolve()
    main = _main_worktree_venv(repo_root)
    if main is not None:
        return main.resolve()
    return _resolve_candidate_venv(repo_root, env.get("MOLT_VENV"))


def _queue_child_env(repo_root: Path, *, queue_size: str | None) -> dict[str, str]:
    base_env = os.environ.copy()
    warm_env = _warm_project_env(repo_root, base_env)
    if warm_env is not None:
        base_env.setdefault("UV_PROJECT_ENVIRONMENT", str(warm_env))
    env = development_artifact_env(
        repo_root,
        base_env,
        session_prefix="queue",
        create_dirs=True,
    )
    if queue_size is not None:
        env[PROOF_QUEUE_SIZE_ENV] = queue_size
    return env


def handle_queue_command(args: argparse.Namespace) -> int:
    repo_root = _queue_repo_root()
    proof_queue = repo_root / "tools" / "proof_queue.py"
    if not proof_queue.exists():
        print(
            "molt queue requires a Molt source checkout containing "
            "tools/proof_queue.py",
            file=sys.stderr,
        )
        return 2
    queue_args = list(getattr(args, "queue_args", []) or [])
    if queue_args[:1] == ["--"]:
        queue_args = queue_args[1:]
    if not queue_args:
        queue_args = ["quickstart"]
    queue_size = getattr(args, "queue_size", None)
    if queue_size is not None and _queue_args_define_queue_size(queue_args):
        print(
            "molt queue: use either top-level --queue-size or proof_queue "
            "run --queue-size, not both",
            file=sys.stderr,
        )
        return 2
    if queue_size is not None:
        try:
            queue_size = _positive_queue_size(queue_size)
        except ValueError as exc:
            print(f"molt queue: {exc}", file=sys.stderr)
            return 2
    try:
        env = _queue_child_env(repo_root, queue_size=queue_size)
    except DxConfigError as exc:
        print(f"molt queue: {exc}", file=sys.stderr)
        return 2
    result = process_guard.run_completed_command(
        [sys.executable, str(proof_queue), *queue_args],
        cwd=repo_root,
        env=env,
    )
    return int(result.returncode)
