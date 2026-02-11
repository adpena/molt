"""Minimal intrinsic-backed `subprocess` subset for Molt."""

from __future__ import annotations

import os as _os
import time as _time
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_SUBPROCESS_RUNTIME_READY = _require_intrinsic(
    "molt_subprocess_runtime_ready", globals()
)
_MOLT_PROCESS_SPAWN = _require_intrinsic("molt_process_spawn", globals())
_MOLT_PROCESS_RETURNCODE = _require_intrinsic("molt_process_returncode", globals())
_MOLT_PROCESS_KILL = _require_intrinsic("molt_process_kill", globals())
_MOLT_PROCESS_TERMINATE = _require_intrinsic("molt_process_terminate", globals())
_MOLT_PROCESS_STDIN = _require_intrinsic("molt_process_stdin", globals())
_MOLT_PROCESS_STDOUT = _require_intrinsic("molt_process_stdout", globals())
_MOLT_PROCESS_STDERR = _require_intrinsic("molt_process_stderr", globals())
_MOLT_PROCESS_DROP = _require_intrinsic("molt_process_drop", globals())
_MOLT_STREAM_SEND_OBJ = _require_intrinsic("molt_stream_send_obj", globals())
_MOLT_STREAM_CLOSE = _require_intrinsic("molt_stream_close", globals())
_MOLT_STREAM_DROP = _require_intrinsic("molt_stream_drop", globals())
_MOLT_STREAM_READER_NEW = _require_intrinsic("molt_stream_reader_new", globals())
_MOLT_STREAM_READER_READ = _require_intrinsic("molt_stream_reader_read", globals())
_MOLT_STREAM_READER_DROP = _require_intrinsic("molt_stream_reader_drop", globals())
_MOLT_PENDING = _require_intrinsic("molt_pending", globals())

PIPE = 1
DEVNULL = 2
STDOUT = -2
_INHERIT = 0
_FD_BASE = 1 << 30
_POLL_SLEEP = 0.001
_PENDING_SENTINEL: Any | None = None


def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is None:
        _PENDING_SENTINEL = _MOLT_PENDING()
    return _PENDING_SENTINEL


def _is_pending(value: Any) -> bool:
    pending = _pending_sentinel()
    return value is pending or value == pending


class SubprocessError(Exception):
    pass


class TimeoutExpired(SubprocessError):
    def __init__(self, cmd: Any, timeout: float) -> None:
        super().__init__(f"Command {cmd!r} timed out after {timeout} seconds")
        self.cmd = cmd
        self.timeout = timeout
        self.output: Any | None = None
        self.stderr: Any | None = None


class CalledProcessError(SubprocessError):
    def __init__(
        self,
        returncode: int,
        cmd: Any,
        output: Any | None = None,
        stderr: Any | None = None,
    ) -> None:
        super().__init__(f"Command {cmd!r} returned non-zero exit status {returncode}.")
        self.returncode = int(returncode)
        self.cmd = cmd
        self.output = output
        self.stderr = stderr


class CompletedProcess:
    def __init__(
        self,
        args: Any,
        returncode: int,
        stdout: Any | None = None,
        stderr: Any | None = None,
    ) -> None:
        self.args = args
        self.returncode = int(returncode)
        self.stdout = stdout
        self.stderr = stderr

    def check_returncode(self) -> None:
        if self.returncode != 0:
            raise CalledProcessError(
                self.returncode,
                self.args,
                output=self.stdout,
                stderr=self.stderr,
            )


def _to_argv_item(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, (bytes, bytearray, memoryview)):
        return bytes(value).decode("utf-8")
    fspath = getattr(_os, "fspath", None)
    if callable(fspath):
        try:
            path_value = fspath(value)
            if isinstance(path_value, str):
                return path_value
            if isinstance(path_value, (bytes, bytearray, memoryview)):
                return bytes(path_value).decode("utf-8")
        except TypeError:
            pass
    raise TypeError("args must be str, bytes, or os.PathLike")


def _coerce_argv(args: Any, shell: bool) -> list[str]:
    if shell:
        if isinstance(args, str):
            command = args
        elif isinstance(args, (bytes, bytearray, memoryview)):
            command = bytes(args).decode("utf-8")
        else:
            raise TypeError("args must be str, bytes, or os.PathLike when shell=True")
        return ["/bin/sh", "-c", command]
    if isinstance(args, (str, bytes, bytearray, memoryview)):
        return [_to_argv_item(args)]
    try:
        return [_to_argv_item(item) for item in args]
    except TypeError:
        raise TypeError("args must be str, bytes, os.PathLike, or a sequence")


def _coerce_env(env: dict[Any, Any] | None) -> dict[str, str] | None:
    if env is None:
        return None
    if not isinstance(env, dict):
        raise TypeError("env must be a mapping")
    out: dict[str, str] = {}
    for key, value in env.items():
        out[str(key)] = str(value)
    return out


def _normalize_timeout(timeout: float | None) -> float | None:
    if timeout is None:
        return None
    value = float(timeout)
    if value < 0.0:
        raise ValueError("timeout must be non-negative")
    return value


def _stdio_mode(value: Any, name: str) -> int:
    if value is None:
        return _INHERIT
    if value is PIPE:
        return PIPE
    if value is DEVNULL:
        return DEVNULL
    if name == "stderr" and value is STDOUT:
        return STDOUT
    if isinstance(value, int) and value >= 0:
        return _FD_BASE + int(value)
    raise ValueError(f"unsupported {name} redirection value")


class Popen:
    def __init__(
        self,
        args: Any,
        bufsize: int = -1,
        executable: Any | None = None,
        stdin: Any | None = None,
        stdout: Any | None = None,
        stderr: Any | None = None,
        preexec_fn: Any | None = None,
        close_fds: bool = True,
        shell: bool = False,
        cwd: str | None = None,
        env: dict[Any, Any] | None = None,
        text: bool = False,
        encoding: str | None = None,
        errors: str | None = None,
    ) -> None:
        del bufsize, executable, preexec_fn, close_fds
        _MOLT_SUBPROCESS_RUNTIME_READY()
        self.args = args
        self.returncode: int | None = None
        self._text = bool(text)
        self._encoding = encoding or "utf-8"
        self._errors = errors or "strict"
        self._stdin_mode = _stdio_mode(stdin, "stdin")
        self._stdout_mode = _stdio_mode(stdout, "stdout")
        self._stderr_mode = _stdio_mode(stderr, "stderr")

        argv = _coerce_argv(args, bool(shell))
        env_map = _coerce_env(env)
        self._handle = _MOLT_PROCESS_SPAWN(
            argv,
            env_map,
            cwd,
            self._stdin_mode,
            self._stdout_mode,
            self._stderr_mode,
        )
        if self._handle is None:
            raise RuntimeError("process spawn failed")

        self._stdin_stream = (
            _MOLT_PROCESS_STDIN(self._handle) if self._stdin_mode == PIPE else None
        )
        self._stdout_stream = (
            _MOLT_PROCESS_STDOUT(self._handle) if self._stdout_mode == PIPE else None
        )
        self._stderr_stream = (
            _MOLT_PROCESS_STDERR(self._handle) if self._stderr_mode == PIPE else None
        )
        self._stdout_reader = (
            _MOLT_STREAM_READER_NEW(self._stdout_stream)
            if self._stdout_stream is not None
            else None
        )
        self._stderr_reader = (
            _MOLT_STREAM_READER_NEW(self._stderr_stream)
            if self._stderr_stream is not None
            else None
        )

        # Keep API-compatible attributes.
        self.stdin = self._stdin_stream
        self.stdout = self._stdout_stream
        self.stderr = self._stderr_stream

    def _sleep(self) -> None:
        _time.sleep(_POLL_SLEEP)

    def _write_input(self, data: Any) -> None:
        if self._stdin_stream is None:
            raise ValueError("stdin was not set to PIPE")
        if data is None:
            return
        if self._text:
            if isinstance(data, str):
                payload = data.encode(self._encoding, self._errors)
            elif isinstance(data, (bytes, bytearray, memoryview)):
                payload = bytes(data)
            else:
                raise TypeError("input must be str or bytes-like")
        else:
            if isinstance(data, str):
                raise TypeError("input must be bytes-like when text=False")
            if not isinstance(data, (bytes, bytearray, memoryview)):
                raise TypeError("input must be bytes-like")
            payload = bytes(data)
        while True:
            out = _MOLT_STREAM_SEND_OBJ(self._stdin_stream, payload)
            if _is_pending(out):
                self._sleep()
                continue
            break

    def _read_stream(self, reader: Any | None) -> bytes | None:
        if reader is None:
            return None
        chunks = bytearray()
        while True:
            out = _MOLT_STREAM_READER_READ(reader, -1)
            if _is_pending(out):
                self._sleep()
                continue
            if out is None:
                break
            chunks.extend(bytes(out))
            break
        return bytes(chunks)

    def _finalize_text(self, payload: bytes | None) -> str | None:
        if payload is None:
            return None
        if self._text:
            return payload.decode(self._encoding, self._errors)
        return payload  # type: ignore[return-value]

    def poll(self) -> int | None:
        out = _MOLT_PROCESS_RETURNCODE(self._handle)
        if out is None:
            return None
        self.returncode = int(out)
        return self.returncode

    def wait(self, timeout: float | None = None) -> int:
        limit = _normalize_timeout(timeout)
        deadline = None if limit is None else (_time.monotonic() + limit)
        while True:
            code = self.poll()
            if code is not None:
                return code
            if deadline is not None and _time.monotonic() >= deadline:
                raise TimeoutExpired(self.args, limit if limit is not None else 0.0)
            self._sleep()

    def communicate(
        self, input: Any | None = None, timeout: float | None = None
    ) -> tuple[Any | None, Any | None]:
        limit = _normalize_timeout(timeout)
        if input is not None:
            self._write_input(input)
        if self._stdin_stream is not None:
            _MOLT_STREAM_CLOSE(self._stdin_stream)
        self.wait(limit)
        out = self._read_stream(self._stdout_reader)
        err = self._read_stream(self._stderr_reader)
        return self._finalize_text(out), self._finalize_text(err)

    def kill(self) -> None:
        _MOLT_PROCESS_KILL(self._handle)

    def terminate(self) -> None:
        _MOLT_PROCESS_TERMINATE(self._handle)

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            if getattr(self, "_stdin_stream", None) is not None:
                _MOLT_STREAM_DROP(self._stdin_stream)
            if getattr(self, "_stdout_stream", None) is not None:
                _MOLT_STREAM_DROP(self._stdout_stream)
            if getattr(self, "_stderr_stream", None) is not None:
                _MOLT_STREAM_DROP(self._stderr_stream)
            if getattr(self, "_stdout_reader", None) is not None:
                _MOLT_STREAM_READER_DROP(self._stdout_reader)
            if getattr(self, "_stderr_reader", None) is not None:
                _MOLT_STREAM_READER_DROP(self._stderr_reader)
            _MOLT_PROCESS_DROP(handle)
        except Exception:
            pass


def run(
    args: Any,
    *,
    input: Any | None = None,
    capture_output: bool = False,
    timeout: float | None = None,
    check: bool = False,
    shell: bool = False,
    cwd: str | None = None,
    env: dict[Any, Any] | None = None,
    text: bool = False,
    encoding: str | None = None,
    errors: str | None = None,
    stdin: Any | None = None,
    stdout: Any | None = None,
    stderr: Any | None = None,
) -> CompletedProcess:
    if capture_output:
        if stdout is not None or stderr is not None:
            raise ValueError("stdout and stderr may not be used with capture_output")
        stdout = PIPE
        stderr = PIPE
    if input is not None and stdin is not None:
        raise ValueError("stdin and input arguments may not both be used")
    if input is not None and stdin is None:
        stdin = PIPE

    proc = Popen(
        args,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        shell=shell,
        cwd=cwd,
        env=env,
        text=text,
        encoding=encoding,
        errors=errors,
    )
    try:
        out, err = proc.communicate(input=input, timeout=timeout)
    except TimeoutExpired as exc:
        proc.kill()
        try:
            proc.wait(None)
        except Exception:
            pass
        exc.output = proc._finalize_text(proc._read_stream(proc._stdout_reader))
        exc.stderr = proc._finalize_text(proc._read_stream(proc._stderr_reader))
        raise

    code = proc.returncode if proc.returncode is not None else proc.wait(None)
    completed = CompletedProcess(args, code, stdout=out, stderr=err)
    if check:
        completed.check_returncode()
    return completed


def check_output(args: Any, *, timeout: float | None = None, **kwargs: Any) -> Any:
    if "stdout" in kwargs:
        raise ValueError("stdout argument not allowed, it will be overridden.")
    kwargs["stdout"] = PIPE
    kwargs.setdefault("check", True)
    result = run(args, timeout=timeout, **kwargs)
    return result.stdout


__all__ = [
    "PIPE",
    "DEVNULL",
    "STDOUT",
    "Popen",
    "CompletedProcess",
    "SubprocessError",
    "CalledProcessError",
    "TimeoutExpired",
    "run",
    "check_output",
]
