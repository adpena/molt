"""Subprocess process authority for ``asyncio.subprocess``."""

from __future__ import annotations

import logging as _logging
import os as _os
import subprocess as subprocess
from typing import Any

import asyncio as _asyncio
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    CancelledError,
    Future,
    _PROC_STDIO_PIPE,
    _molt_process_drop,
    _molt_process_kill,
    _molt_process_pid,
    _molt_process_returncode,
    _molt_process_spawn,
    _molt_process_stderr,
    _molt_process_stdin,
    _molt_process_stdout,
    _molt_process_terminate,
    _molt_process_wait_future,
    _normalize_proc_stdio,
    _require_asyncio_intrinsic,
)
from . import protocols as protocols
from . import streams as streams
from .streams import (
    ProcessStreamReader,
    ProcessStreamWriter,
    StreamReaderProtocol as SubprocessStreamProtocol,
)

PIPE = -1
STDOUT = -2
DEVNULL = -3
events: Any | None = None
logger = _logging.getLogger("asyncio")
tasks: Any | None = None

_PROCESS_WAIT_FUTURES: dict[int, Any] = {}

class Process:
    def __init__(
        self,
        handle: Any,
        stdin: ProcessStreamWriter | None,
        stdout: ProcessStreamReader | None,
        stderr: ProcessStreamReader | None,
    ) -> None:
        self._handle = handle
        self.stdin = stdin
        self.stdout = stdout
        self.stderr = stderr

    @property
    def pid(self) -> int:
        return int(
            _require_asyncio_intrinsic(_molt_process_pid, "process_pid")(self._handle)
        )

    @property
    def returncode(self) -> int | None:
        return _require_asyncio_intrinsic(
            _molt_process_returncode, "process_returncode"
        )(self._handle)

    def kill(self) -> None:
        _require_asyncio_intrinsic(_molt_process_kill, "process_kill")(self._handle)

    def terminate(self) -> None:
        _require_asyncio_intrinsic(_molt_process_terminate, "process_terminate")(
            self._handle
        )

    async def wait(self) -> int:
        key = id(self)
        wait_future = _PROCESS_WAIT_FUTURES.get(key)
        if wait_future is None:
            fut = _require_asyncio_intrinsic(
                _molt_process_wait_future, "process_wait_future"
            )(self._handle)
            _PROCESS_WAIT_FUTURES[key] = fut
            wait_future = fut
        code = int(await wait_future)
        watcher = getattr(_asyncio, "_CHILD_WATCHER", None)
        if watcher is not None and hasattr(watcher, "_notify_child_exit"):
            watcher._notify_child_exit(self.pid, code)
        return code

    async def communicate(
        self, input: bytes | None = None
    ) -> tuple[bytes | None, bytes | None]:
        if input is not None:
            if self.stdin is None:
                raise ValueError("stdin was not set to PIPE")
            self.stdin.write(input)
            await self.stdin.drain()
            self.stdin.close()

        tasks: list[Future] = []
        task_kinds: list[str] = []
        if self.stdout is not None:
            tasks.append(_asyncio.ensure_future(self.stdout.read()))
            task_kinds.append("stdout")
        if self.stderr is not None:
            tasks.append(_asyncio.ensure_future(self.stderr.read()))
            task_kinds.append("stderr")

        out: bytes | None = None
        err: bytes | None = None
        try:
            if tasks:
                while True:
                    current = _asyncio.current_task()
                    if current is not None and current.cancelling() > 0:
                        raise CancelledError()
                    all_done = True
                    for task in tasks:
                        if not task.done():
                            all_done = False
                            break
                    if all_done:
                        break
                    await _asyncio.sleep(0.0)
                results: list[Any] = []
                for task in tasks:
                    results.append(await task)
                for idx, result in enumerate(results):
                    kind = task_kinds[idx]
                    if kind == "stdout":
                        out = result
                    else:
                        err = result
            current = _asyncio.current_task()
            if current is not None and current.cancelling() > 0:
                raise CancelledError()
            await self.wait()
        except BaseException:
            for task in tasks:
                if not task.done():
                    task.cancel()
            raise
        return out, err

    def __del__(self) -> None:
        _PROCESS_WAIT_FUTURES.pop(id(self), None)
        if _molt_process_drop is not None:
            _molt_process_drop(self._handle)

async def create_subprocess_exec(
    *program: Any,
    stdin: Any | None = None,
    stdout: Any | None = None,
    stderr: Any | None = None,
    cwd: Any | None = None,
    env: Any | None = None,
) -> Process:
    if not program:
        raise ValueError("program must not be empty")
    stdin_mode = _normalize_proc_stdio(stdin, allow_stdout=False)
    stdout_mode = _normalize_proc_stdio(stdout, allow_stdout=False)
    stderr_mode = _normalize_proc_stdio(stderr, allow_stdout=True)
    spawn = _require_asyncio_intrinsic(_molt_process_spawn, "process_spawn")
    handle = spawn(list(program), env, cwd, stdin_mode, stdout_mode, stderr_mode)
    if handle is None:
        raise PermissionError("Missing process capability")
    stdin_stream = (
        ProcessStreamWriter(
            _require_asyncio_intrinsic(_molt_process_stdin, "process_stdin")(handle)
        )
        if stdin_mode == _PROC_STDIO_PIPE
        else None
    )
    stdout_stream = (
        ProcessStreamReader(
            _require_asyncio_intrinsic(_molt_process_stdout, "process_stdout")(handle)
        )
        if stdout_mode == _PROC_STDIO_PIPE
        else None
    )
    stderr_stream = (
        ProcessStreamReader(
            _require_asyncio_intrinsic(_molt_process_stderr, "process_stderr")(handle)
        )
        if stderr_mode == _PROC_STDIO_PIPE
        else None
    )
    return Process(handle, stdin_stream, stdout_stream, stderr_stream)

async def create_subprocess_shell(
    cmd: str,
    stdin: Any | None = None,
    stdout: Any | None = None,
    stderr: Any | None = None,
    cwd: Any | None = None,
    env: Any | None = None,
) -> Process:
    if _os.name == "nt":
        program = ["cmd.exe", "/c", cmd]
    else:
        program = ["/bin/sh", "-c", cmd]
    return await create_subprocess_exec(
        *program,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        cwd=cwd,
        env=env,
    )


__all__ = [
    "DEVNULL",
    "PIPE",
    "Process",
    "STDOUT",
    "SubprocessStreamProtocol",
    "create_subprocess_exec",
    "create_subprocess_shell",
    "events",
    "logger",
    "protocols",
    "streams",
    "subprocess",
    "tasks",
]

globals().pop("_require_intrinsic", None)
