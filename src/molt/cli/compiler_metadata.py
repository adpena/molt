from __future__ import annotations

import functools
from pathlib import Path
import tomllib

from molt.cli.command_runtime import _CLI_MEMORY_GUARD_PREFIX, _run_completed_command


_CLI_PACKAGE_ROOT = Path(__file__).resolve().parent
_MOLT_PACKAGE_ROOT = _CLI_PACKAGE_ROOT.parent
_SRC_ROOT = _MOLT_PACKAGE_ROOT.parent
_COMPILER_ROOT = _SRC_ROOT.parent


def _compiler_root() -> Path:
    return _COMPILER_ROOT


def _git_rev(root: Path) -> str | None:
    try:
        result = _run_completed_command(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    value = result.stdout.strip()
    return value or None


def _compiler_metadata() -> tuple[str | None, str | None]:
    compiler_root = _compiler_root()
    try:
        data = tomllib.loads((compiler_root / "pyproject.toml").read_text())
    except (OSError, tomllib.TOMLDecodeError):
        data = {}
    project = data.get("project")
    version = project.get("version") if isinstance(project, dict) else None
    git_rev = _git_rev(compiler_root)
    return version if isinstance(version, str) else None, git_rev


@functools.lru_cache(maxsize=1)
def _rustc_version() -> str | None:
    try:
        result = _run_completed_command(
            ["rustc", "-Vv"],
            capture_output=True,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip()
