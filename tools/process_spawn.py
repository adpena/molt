from __future__ import annotations

import os
import subprocess
import sys
from types import ModuleType
from typing import Any


def _subprocess_flag(module: ModuleType | Any, name: str) -> int:
    return int(getattr(module, name, 0) or 0)


def _stream_has_real_handle(stream: Any) -> bool:
    if stream is None:
        return False
    try:
        stream.fileno()
    except (AttributeError, OSError, ValueError):
        return False
    return True


def inherit_stdio_kwargs() -> dict[str, object]:
    """Explicit stdio passthrough for hidden-console Windows children.

    `CREATE_NO_WINDOW` children get a fresh hidden console instead of the
    parent's console, so any spawn that relies on console inheritance for
    stdout/stderr silently loses output. Passing the live stream handles
    explicitly keeps passthrough working under hidden process groups. Streams
    without a real OS handle (e.g. pythonw, closed streams) are omitted so the
    child falls back to the default behavior.
    """
    kwargs: dict[str, object] = {}
    if _stream_has_real_handle(sys.stdin):
        kwargs["stdin"] = sys.stdin
    if _stream_has_real_handle(sys.stdout):
        kwargs["stdout"] = sys.stdout
    if _stream_has_real_handle(sys.stderr):
        kwargs["stderr"] = sys.stderr
    return kwargs


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
