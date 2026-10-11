"""Portable process-launch options shared by packaged and repository guards."""

from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from types import ModuleType
from typing import IO, Any, TextIO, TypedDict


class ProcessGroupKwargs(TypedDict, total=False):
    creationflags: int
    start_new_session: bool
    preexec_fn: Callable[[], None]


class InheritedStdioKwargs(TypedDict, total=False):
    stdin: TextIO
    stdout: TextIO
    stderr: TextIO


class ProcessSpawnKwargs(ProcessGroupKwargs, total=False):
    cwd: str | os.PathLike[str] | None
    env: Mapping[str, str] | None
    text: bool
    encoding: str
    errors: str
    stdin: int | IO[str] | IO[bytes] | None
    stdout: int | IO[str] | IO[bytes] | None
    stderr: int | IO[str] | IO[bytes] | None
    pass_fds: tuple[int, ...]


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


def inherit_stdio_kwargs() -> InheritedStdioKwargs:
    """Explicit stdio passthrough for hidden-console Windows children.

    `CREATE_NO_WINDOW` children get a fresh hidden console instead of the
    parent's console, so any spawn that relies on console inheritance for
    stdout/stderr silently loses output. Passing the live stream handles
    explicitly keeps passthrough working under hidden process groups. Streams
    without a real OS handle (e.g. pythonw, closed streams) are omitted so the
    child falls back to the default behavior.
    """
    kwargs: InheritedStdioKwargs = {}
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
) -> ProcessGroupKwargs:
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
) -> ProcessGroupKwargs:
    if windows is None:
        windows = os.name == "nt"
    if windows:
        return hidden_windows_process_group_kwargs(
            windows=True,
            subprocess_module=subprocess_module,
        )
    return {"start_new_session": True}


def forward_command(
    command: Sequence[str],
    *,
    env: Mapping[str, str],
    cwd: str | os.PathLike[str] | None = None,
) -> int:
    """Transfer a CLI wrapper to its command without adding a custody owner.

    POSIX exec preserves process identity and signal delivery. Windows has no
    equivalent exec: wait for the child so its actual status reaches the caller.
    The command (or an existing outer guard) owns any descendant lifecycle.
    """
    if isinstance(command, (str, bytes)):
        raise TypeError("command must be typed argv, not shell text")
    argv = list(command)
    if not argv:
        raise ValueError("command is required")
    child_env = dict(env)
    if os.name == "nt":
        return subprocess.run(argv, cwd=cwd, env=child_env, check=False).returncode
    if cwd is None:
        os.execvpe(argv[0], argv, child_env)
    else:
        original_cwd = os.getcwd()
        try:
            os.chdir(cwd)
            os.execvpe(argv[0], argv, child_env)
        finally:
            # Only failed exec returns to Python; leave a reusable caller intact.
            os.chdir(original_cwd)
    raise AssertionError("os.execvpe returned")
