"""Dependency manifests the repository tracks, by tracked path.

The one authority for which lockfiles exist: Dependabot's watch list
(`tests/test_dependabot_config.py`) and the Cargo security audit
(`tools/cargo_lock_audit.py`) both read it, so a new lockfile is watched and
audited at once. Fixture projects keep their pins on purpose and are excluded.
"""

from __future__ import annotations

from pathlib import Path, PurePosixPath
import subprocess

FIXTURE_PREFIX = "tests/fixtures/"


def tracked_manifest_dirs(root: Path, *patterns: str) -> list[PurePosixPath]:
    """Directories, relative to `root`, of tracked files matching `patterns`.

    The repository root is `PurePosixPath(".")`. Paths under `tests/fixtures/`
    are excluded.
    """
    output = subprocess.run(
        ["git", "ls-files", "-z", "--", *patterns],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    ).stdout
    return sorted(
        {
            PurePosixPath(path).parent
            for path in output.split("\0")
            if path and not path.startswith(FIXTURE_PREFIX)
        }
    )


def cargo_workspace_dirs(root: Path) -> list[PurePosixPath]:
    """Every tracked Cargo workspace: the directory of each tracked `Cargo.lock`."""
    return tracked_manifest_dirs(root, "Cargo.lock", "*/Cargo.lock")
