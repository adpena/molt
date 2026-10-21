"""Fixture repositories whose main carries this checkout's driver.

Every landing runs main's pinned driver (tools/molt_dev_controller.py), so a
fixture that lands through the real ``integrate`` needs that driver on its
main. The copy comes from the working tree, tracked and untracked alike, so a
test runs the code under test rather than the last commit.
"""

from __future__ import annotations

import shutil
from pathlib import Path

from tools.molt_dev_common import _run_fast_captured_command
from tools.molt_dev_controller import CONTROLLER_PATHS

REPO_ROOT = Path(__file__).resolve().parents[1]


def copy_driver_trees(target: Path) -> None:
    """Copy the controller's trees from this working tree into ``target``."""

    listed = _run_fast_captured_command(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            *CONTROLLER_PATHS,
            # The fixture is a checkout that provisions itself (uv sync
            # writes src/molt.egg-info): it keeps the repository's ignores.
            ".gitignore",
        ],
        cwd=REPO_ROOT,
    )
    if listed.returncode != 0:
        raise RuntimeError(f"cannot list the driver trees: {listed.stderr}")
    for relative in filter(None, listed.stdout.split("\0")):
        source = REPO_ROOT / relative
        if source.is_file():
            destination = target / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
