"""Portable process-launch options shared by packaged and repository guards."""

from __future__ import annotations

import atexit
import os
import ntpath
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from contextlib import ExitStack
from dataclasses import dataclass, field
from types import ModuleType
from pathlib import Path
from typing import IO, Any, Self, TextIO, TypedDict


PROCESS_STATE_HANDOFF_TIMEOUT_S = 30.0


@dataclass(slots=True, weakref_slot=True)
class ProcessStateUse:
    """Retain selected managed state before birth through observed child death.

    Callers still create and clean up their own Popen object. The registered
    bound method keeps this exact owner, child and entered use alive when a
    failed wait and its exception are collected. Verified handoffs may release
    the same owner explicitly; no parallel handoff/release flag is needed.
    """

    source_root: Path
    environment: Mapping[str, str]
    cwd: str | os.PathLike[str] | None
    process: subprocess.Popen[Any] | None = field(default=None, init=False)
    state_use: ExitStack = field(default_factory=ExitStack, init=False, repr=False)
    released: bool = field(default=False, init=False)
    _registered: bool = field(default=False, init=False, repr=False)

    def __enter__(self) -> Self:
        from .checkout_state import checkout_state_command_roots, checkout_state_use

        if self._registered or self.released:
            raise RuntimeError("process state owner is single-use")
        try:
            self.state_use.enter_context(
                checkout_state_use(
                    checkout_state_command_roots(
                        self.source_root, self.environment, cwd=self.cwd
                    )
                )
            )
            # Register before any caller can create a child, including callers
            # whose Popen/reader setup later fails after actual process birth.
            atexit.register(self._close_at_exit)
            self._registered = True
        except BaseException as error:
            try:
                self.state_use.close()
            except BaseException as cleanup_error:
                error.add_note(f"prebirth state cleanup failed: {cleanup_error!r}")
            self.released = True
            raise
        return self

    def _release(self) -> None:
        if self.released:
            return
        self.state_use.close()
        self.released = True
        if self._registered:
            atexit.unregister(self._close_at_exit)
            self._registered = False

    def _close_at_exit(self) -> None:
        if self.released:
            return
        if self.process is not None:
            self.process.wait()
            if self.process.poll() is None:
                return
        self._release()

    def _annotate_error(self, error: BaseException) -> None:
        setattr(error, "state_owner", self)
        setattr(error, "state_use", self.state_use)
        setattr(error, "owned_process", self.process)
        error.add_note("managed state remains retained until child closure is observed")

    def __exit__(
        self, exc_type: object, error: BaseException | None, tb: object
    ) -> None:
        del exc_type, tb
        if self.released:
            return
        try:
            closed = self.process is None or self.process.poll() is not None
            if closed:
                self._release()
                return
        except BaseException as cleanup_error:
            if error is None:
                self._annotate_error(cleanup_error)
                raise
            error.add_note(f"managed state cleanup failed: {cleanup_error!r}")
        if error is not None:
            self._annotate_error(error)


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


def native_popen_command(
    command: Sequence[str], *, windows: bool | None = None
) -> Sequence[str] | str:
    """Serialize an explicit cmd.exe /c payload using its shell grammar.

    Ordinary argv retains CPython's CRT serialization. The single final field
    after cmd.exe /c (or /k) is shell source, not a CRT argument: escaping its
    quotes with list2cmdline changes the program. Match CPython's shell=True
    outer quoting here, at the native spawn boundary, while custody continues
    to carry the original argument vector. No shell is inferred or added.
    """
    if windows is None:
        windows = os.name == "nt"
    if (
        windows
        and len(command) >= 3
        and ntpath.basename(command[0]).lower() in {"cmd", "cmd.exe"}
        and command[-2].lower() in {"/c", "/k"}
    ):
        return subprocess.list2cmdline(command[:-1]) + ' "' + command[-1] + '"'
    return command


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
    from .checkout_state import checkout_state_command_roots, checkout_state_use
    from .file_locks import _file_lock_descriptor_action, _file_lock_owned_operation
    from .source_root import compiler_source_root

    child_env = dict(env)
    source = compiler_source_root()
    if os.name == "nt":
        with ProcessStateUse(source, child_env, cwd) as state_owner:
            process = subprocess.Popen(
                native_popen_command(argv), cwd=cwd, env=child_env
            )
            state_owner.process = process
            error: BaseException | None = None
            try:
                try:
                    process.communicate()
                except BaseException as failure:
                    try:
                        process.kill()
                    except BaseException as cleanup_error:
                        failure.add_note(f"child kill failed: {cleanup_error!r}")
                    raise
                returncode = process.poll()
                if returncode is None:
                    raise RuntimeError("forwarded command has no observed exit status")
                return returncode
            except BaseException as failure:
                error = failure
                raise
            finally:
                # Reuse Popen's stream/wait and KeyboardInterrupt policy. A
                # failed cleanup must not replace the original child error.
                try:
                    process.__exit__(
                        type(error) if error is not None else None,
                        error,
                        error.__traceback__ if error is not None else None,
                    )
                except BaseException as cleanup_error:
                    if error is None:
                        raise
                    error.add_note(f"child context cleanup failed: {cleanup_error!r}")
    with checkout_state_use(
        checkout_state_command_roots(source, child_env, cwd=cwd)
    ) as handles:
        with ExitStack() as custody:
            for handle in handles:
                custody.enter_context(
                    _file_lock_owned_operation(
                        handle,
                        expected_lock_path=Path(handle.registry_key),
                        shared=True,
                    )
                )
            # Prevent a concurrent Python fork from observing the temporary
            # inheritable descriptors. Exec itself replaces this process.
            with _file_lock_descriptor_action():
                inherited = [
                    (handle.file.fileno(), os.get_inheritable(handle.file.fileno()))
                    for handle in handles
                ]
                original_cwd = os.getcwd()
                try:
                    for fd, _ in inherited:
                        os.set_inheritable(fd, True)
                    if cwd is not None:
                        os.chdir(cwd)
                    os.execvpe(argv[0], argv, child_env)
                finally:
                    # Successful exec never returns. Failed transfer restores
                    # the reusable caller before its shared handles are closed.
                    for fd, was_inheritable in inherited:
                        os.set_inheritable(fd, was_inheritable)
                    if cwd is not None:
                        os.chdir(original_cwd)
    raise AssertionError("os.execvpe returned")
