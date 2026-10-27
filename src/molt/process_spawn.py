"""Portable process-launch options shared by packaged and repository guards."""

from __future__ import annotations

import atexit
import json
import os
import ntpath
import stat
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from contextlib import ExitStack
from dataclasses import dataclass, field
from types import ModuleType
from pathlib import Path
from typing import IO, TYPE_CHECKING, Any, Self, TextIO, TypedDict

if TYPE_CHECKING:
    from .file_locks import _FileLockHandle


PROCESS_STATE_HANDOFF_TIMEOUT_S = 30.0


@dataclass(slots=True, weakref_slot=True)
class ProcessStateUse:
    """Retain selected managed state before birth through observed child death.

    Callers still create and clean up their own Popen object. The registered
    bound method keeps this exact owner, child and entered use alive when a
    failed wait and its exception are collected. Verified handoffs may release
    the same owner explicitly; no parallel handoff/release flag is needed.
    The actual command is required: activation variables alone cannot retain
    an unactivated foreign environment selected by its executable or PATH.
    """

    source_root: Path
    environment: Mapping[str, str]
    cwd: str | os.PathLike[str] | None
    command: Sequence[str]
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
                        self.source_root, self.environment, self.command, cwd=self.cwd
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


@dataclass(frozen=True, slots=True)
class ExecSharedDescriptor:
    fd: int
    path: Path
    device: int
    inode: int


@dataclass(frozen=True, slots=True)
class ExecStateHandoff:
    """One same-process exec's exact shared descriptors.

    The receiver makes these close-on-exec after validating their identity.
    No scan of unrelated descriptors or retained historical root set exists.
    """

    pid: int
    descriptors: tuple[ExecSharedDescriptor, ...]

    def to_json(self) -> str:
        return json.dumps(
            {
                "schema": "molt.exec-state-handoff.v1",
                "pid": self.pid,
                "descriptors": [
                    {
                        "fd": item.fd,
                        "path": str(item.path),
                        "device": item.device,
                        "inode": item.inode,
                    }
                    for item in self.descriptors
                ],
            },
            separators=(",", ":"),
        )

    @classmethod
    def from_json(cls, raw: str) -> ExecStateHandoff:
        from .exact_json import loads_exact

        value = loads_exact(raw)
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "pid", "descriptors"}
            or value["schema"] != "molt.exec-state-handoff.v1"
            or type(value["pid"]) is not int
            or value["pid"] <= 0
            or not isinstance(value["descriptors"], list)
        ):
            raise ValueError("invalid exec state handoff")
        descriptors = []
        for item in value["descriptors"]:
            if (
                not isinstance(item, dict)
                or set(item) != {"fd", "path", "device", "inode"}
                or type(item["fd"]) is not int
                or item["fd"] < 3
                or type(item["device"]) is not int
                or item["device"] < 0
                or type(item["inode"]) is not int
                or item["inode"] < 0
                or not isinstance(item["path"], str)
                or not Path(item["path"]).is_absolute()
            ):
                raise ValueError("invalid exec shared descriptor")
            descriptors.append(
                ExecSharedDescriptor(
                    item["fd"], Path(item["path"]), item["device"], item["inode"]
                )
            )
        if len(descriptors) != len({item.fd for item in descriptors}):
            raise ValueError("duplicate exec shared descriptors")
        return cls(value["pid"], tuple(descriptors))

    def receive(self, source_handle: _FileLockHandle) -> None:
        """Keep this invocation's use; exclude it from subsequent execs.

        Validate every descriptor before changing any. The next forward owns
        its current canonical command resources before exec closes these old
        descriptions. Closing or unlocking here would weaken current use.
        """
        from .checkout_state import is_checkout_state_lock_file
        from .file_locks import _file_lock_descriptor_action, _file_lock_owned_operation

        if os.name != "posix" or self.pid != os.getpid():
            raise ValueError("exec state handoff requires the original POSIX process")
        source_path = Path(source_handle.registry_key)
        with (
            _file_lock_owned_operation(
                source_handle, expected_lock_path=source_path, shared=True
            ),
            _file_lock_descriptor_action(),
        ):
            for item in self.descriptors:
                if item.fd == source_handle.file.fileno() or (
                    item.path != source_path
                    and not is_checkout_state_lock_file(item.path)
                ):
                    raise ValueError("exec descriptor is not a handed-off shared lock")
                opened = os.fstat(item.fd)
                named = item.path.stat()
                if (
                    not stat.S_ISREG(opened.st_mode)
                    or (opened.st_dev, opened.st_ino) != (item.device, item.inode)
                    or not os.path.samestat(opened, named)
                    or not os.get_inheritable(item.fd)
                ):
                    raise ValueError("exec descriptor identity or inheritance changed")
            changed = []
            try:
                for item in self.descriptors:
                    os.set_inheritable(item.fd, False)
                    changed.append(item.fd)
            except BaseException:
                for fd in changed:
                    os.set_inheritable(fd, True)
                raise


def forward_command(
    command: Sequence[str],
    *,
    env: Mapping[str, str],
    cwd: str | os.PathLike[str] | None = None,
    retained_shared_handles: Sequence[_FileLockHandle] = (),
    handoff_argument: str | None = None,
) -> int:
    """Transfer a CLI wrapper to its command without adding a custody owner.

    POSIX exec preserves process identity and signal delivery. Windows has no
    equivalent exec: wait for the child so its actual status reaches the caller.
    The command (or an existing outer guard) owns any descendant lifecycle.
    Caller-owned shared handles transfer through the same audited POSIX exec
    boundary as managed state; the caller retains them during a Windows wait.
    A receiving controller requests an explicit typed argv handoff. Generic
    wrappers retain the existing exec-to-exit resource lifetime.
    """
    if isinstance(command, (str, bytes)):
        raise TypeError("command must be typed argv, not shell text")
    argv = list(command)
    if not argv:
        raise ValueError("command is required")
    if handoff_argument is not None and (
        not handoff_argument.startswith("--")
        or "=" in handoff_argument
        or any(char.isspace() for char in handoff_argument)
        or handoff_argument in argv
    ):
        raise ValueError("exec handoff requires one unbound option name")
    from .checkout_state import checkout_state_command_roots, checkout_state_use
    from .file_locks import (
        _file_lock_descriptor_action,
        _file_lock_is_owned,
        _file_lock_owned_operation,
    )
    from .source_root import compiler_source_root

    child_env = dict(env)
    source = compiler_source_root()
    for handle in retained_shared_handles:
        if not handle.shared or not _file_lock_is_owned(handle):
            raise ValueError("forwarded source use requires live shared handles")
    if os.name == "nt":
        if handoff_argument is not None:
            raise ValueError("exec state handoff is POSIX-only")
        with ProcessStateUse(source, child_env, cwd, argv) as state_owner:
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
        checkout_state_command_roots(
            source,
            child_env,
            argv,
            cwd=cwd,
            include_current_process=handoff_argument is None,
        )
    ) as handles:
        handles = (*handles, *retained_shared_handles)
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
                    if handoff_argument is not None:
                        descriptors = []
                        for handle in handles:
                            metadata = os.fstat(handle.file.fileno())
                            descriptors.append(
                                ExecSharedDescriptor(
                                    handle.file.fileno(),
                                    Path(handle.registry_key),
                                    metadata.st_dev,
                                    metadata.st_ino,
                                )
                            )
                        transfer = ExecStateHandoff(os.getpid(), tuple(descriptors))
                        # Apply the same closed wire admission to the producer.
                        payload = transfer.to_json()
                        ExecStateHandoff.from_json(payload)
                        argv.extend((handoff_argument, payload))
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
