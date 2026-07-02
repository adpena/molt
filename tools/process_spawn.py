from __future__ import annotations

import os
import subprocess
from types import ModuleType
from typing import Any


def _subprocess_flag(module: ModuleType | Any, name: str) -> int:
    return int(getattr(module, name, 0) or 0)


def hidden_windows_process_group_creationflags(
    *, subprocess_module: ModuleType | Any = subprocess
) -> int:
    """Return Windows flags for hidden, independently killable children."""
    return _subprocess_flag(
        subprocess_module, "CREATE_NEW_PROCESS_GROUP"
    ) | _subprocess_flag(subprocess_module, "CREATE_NO_WINDOW")


def hidden_windows_process_group_kwargs(
    *,
    windows: bool | None = None,
    subprocess_module: ModuleType | Any = subprocess,
) -> dict[str, object]:
    if windows is None:
        windows = os.name == "nt"
    if not windows:
        return {}
    creationflags = hidden_windows_process_group_creationflags(
        subprocess_module=subprocess_module
    )
    return {"creationflags": creationflags} if creationflags else {}


def detached_process_group_kwargs(
    *,
    windows: bool | None = None,
    subprocess_module: ModuleType | Any = subprocess,
) -> dict[str, object]:
    if windows is None:
        windows = os.name == "nt"
    if windows:
        return hidden_windows_process_group_kwargs(
            windows=True,
            subprocess_module=subprocess_module,
        )
    return {"start_new_session": True}
