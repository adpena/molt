"""Canonical filesystem deletion primitive for Molt-owned artifact cleanup.

Callers remain responsible for proving that a path is in their deletion scope.
This module owns only the cross-platform mechanics: do not follow directory
symlinks, tolerate an already-absent path, and clear a Windows read-only bit
only after the operating system has rejected the deletion for permission.
"""

from __future__ import annotations

import os
import shutil
import stat
from pathlib import Path
from typing import Callable


def _retry_readonly(
    operation: Callable[[str], object], raw_path: str, error: BaseException
) -> None:
    """Retry one failed rmtree operation after clearing owner read-only state."""
    if not isinstance(error, PermissionError):
        raise error
    path = Path(raw_path)
    path.chmod(path.stat().st_mode | stat.S_IWUSR)
    operation(raw_path)


def delete_path(path: Path) -> tuple[bool, str]:
    """Delete one already-authorized path and report failure without hiding it."""
    try:
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path, onexc=_retry_readonly)
        else:
            try:
                path.unlink(missing_ok=True)
            except PermissionError as error:
                _retry_readonly(os.unlink, str(path), error)
        return True, ""
    except OSError as error:
        return False, str(error)
