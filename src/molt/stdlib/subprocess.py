"""Minimal intrinsic-backed `subprocess` subset for Molt."""

from __future__ import annotations

from collections.abc import Mapping as _Mapping
import os as _os
import time as _time
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_SUBPROCESS_RUNTIME_READY = _require_intrinsic(
    "molt_subprocess_runtime_ready", globals()
)
_MOLT_PROCESS_SPAWN = _require_intrinsic("molt_process_spawn", globals())
_MOLT_PROCESS_SPAWN_EX = _require_intrinsic("molt_process_spawn_ex", globals())
_MOLT_PROCESS_PID = _require_intrinsic("molt_process_pid", globals())
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
_MOLT_STREAM_READER_READLINE = _require_intrinsic(
    "molt_stream_reader_readline", globals()
)
_MOLT_STREAM_READER_DROP = _require_intrinsic("molt_stream_reader_drop", globals())
_MOLT_PENDING = _require_intrinsic("molt_pending", globals())
_MOLT_SUBPROCESS_PIPE_CONST = _require_intrinsic(
    "molt_subprocess_pipe_const", globals()
)
_MOLT_SUBPROCESS_STDOUT_CONST = _require_intrinsic(
    "molt_subprocess_stdout_const", globals()
)
_MOLT_SUBPROCESS_DEVNULL_CONST = _require_intrinsic(
    "molt_subprocess_devnull_const", globals()
)

PIPE = int(_MOLT_SUBPROCESS_PIPE_CONST())
DEVNULL = int(_MOLT_SUBPROCESS_DEVNULL_CONST())
STDOUT = int(_MOLT_SUBPROCESS_STDOUT_CONST())
_MODE_PIPE = 1
_MODE_DEVNULL = 2
_MODE_STDOUT = -2
_INHERIT = 0
_FD_BASE = 1 << 30
_FD_MAX = (1 << 30) - 1
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
    def __setattr__(self, name: str, value: Any) -> None:
        if name == "output" or name == "stdout":
            object.__setattr__(self, "output", value)
            object.__setattr__(self, "stdout", value)
            return
        object.__setattr__(self, name, value)

    def __init__(
        self,
        cmd: Any,
        timeout: float,
        output: Any | None = None,
        stderr: Any | None = None,
    ) -> None:
        super().__init__(f"Command {cmd!r} timed out after {timeout} seconds")
        self.cmd = cmd
        self.timeout = timeout
        self.output = output
        self.stderr = stderr


class CalledProcessError(SubprocessError):
    def __setattr__(self, name: str, value: Any) -> None:
        if name == "output" or name == "stdout":
            object.__setattr__(self, "output", value)
            object.__setattr__(self, "stdout", value)
            return
        object.__setattr__(self, name, value)

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


def _to_argv_item(value: Any) -> str | bytes:
    if isinstance(value, str):
        return value
    if isinstance(value, bytes):
        return value  # type: ignore[return-value]
    fspath = getattr(_os, "fspath", None)
    if callable(fspath):
        try:
            path_value = fspath(value)
            if isinstance(path_value, str):
                return path_value
            if isinstance(path_value, bytes):
                return path_value  # type: ignore[return-value]
        except TypeError:
            pass
    raise TypeError("args must be str, bytes, or os.PathLike")


def _to_env_item(value: Any) -> str | bytes:
    if isinstance(value, (str, bytes)):
        return value
    fspath = getattr(_os, "fspath", None)
    if callable(fspath):
        try:
            path_value = fspath(value)
            if isinstance(path_value, (str, bytes)):
                return path_value
        except TypeError:
            pass
    raise TypeError("env keys and values must be str, bytes, or os.PathLike")


def _coerce_argv(args: Any, shell: bool) -> list[str | bytes]:
    if shell:
        if isinstance(args, str):
            command = args
        elif isinstance(args, bytes):
            fsdecode = getattr(_os, "fsdecode", None)
            command = fsdecode(args) if callable(fsdecode) else args.decode("utf-8")
        elif hasattr(args, "__fspath__"):
            raise TypeError("path-like args is not allowed when shell is true")
        else:
            raise TypeError("args must be str, bytes, or os.PathLike when shell=True")
        return ["/bin/sh", "-c", command]
    if isinstance(args, (str, bytes)) or hasattr(args, "__fspath__"):
        return [_to_argv_item(args)]
    try:
        return [_to_argv_item(item) for item in args]
    except TypeError:
        raise TypeError("args must be str, bytes, os.PathLike, or a sequence")


def _coerce_env(
    env: _Mapping[Any, Any] | None,
) -> dict[str | bytes, str | bytes] | None:
    if env is None:
        return None
    if not isinstance(env, _Mapping):
        raise TypeError("env must be a mapping")
    out: dict[str | bytes, str | bytes] = {}
    for key, value in env.items():
        out[_to_env_item(key)] = _to_env_item(value)
    return out


def _normalize_timeout(timeout: float | None) -> float | None:
    if timeout is None:
        return None
    value = float(timeout)
    if value < 0.0:
        raise ValueError("timeout must be non-negative")
    return value


def _encode_fd(value: int) -> int:
    fd = int(value)
    if fd < 0:
        raise ValueError("file descriptor must be >= 0")
    if fd > _FD_MAX:
        raise ValueError("file descriptor is too large")
    return _FD_BASE + fd


def _stdio_mode(value: Any, name: str) -> int:
    if value is None:
        return _INHERIT
    if value == PIPE:
        return _MODE_PIPE
    if value == DEVNULL:
        return _MODE_DEVNULL
    if name == "stderr" and value == STDOUT:
        return _MODE_STDOUT
    if isinstance(value, int):
        return _encode_fd(value)
    fileno = getattr(value, "fileno", None)
    if callable(fileno):
        return _encode_fd(fileno())
    raise ValueError(f"unsupported {name} redirection value")


class _PopenPipeWriter:
    def __init__(
        self,
        stream: Any,
        *,
        text: bool,
        encoding: str,
        errors: str,
        sleeper: Any,
    ) -> None:
        self._stream = stream
        self._text = text
        self._encoding = encoding
        self._errors = errors
        self._sleep = sleeper
        self._closed = False

    @property
    def closed(self) -> bool:
        return self._closed

    def write(self, data: Any) -> int:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if self._text:
            if not isinstance(data, str):
                raise TypeError(
                    f"write() argument must be str, not {type(data).__name__}"
                )
            payload = data.encode(self._encoding, self._errors)
            written = len(data)
        else:
            if isinstance(data, str):
                raise TypeError("input must be bytes-like when text=False")
            if not isinstance(data, (bytes, bytearray, memoryview)):
                raise TypeError("input must be bytes-like")
            payload = bytes(data)
            written = len(payload)
        while True:
            out = _MOLT_STREAM_SEND_OBJ(self._stream, payload)
            if _is_pending(out):
                self._sleep()
                continue
            break
        return written

    def flush(self) -> None:
        return None

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        _MOLT_STREAM_CLOSE(self._stream)


class _PopenPipeReader:
    def __init__(
        self,
        stream: Any,
        reader: Any,
        *,
        text: bool,
        encoding: str,
        errors: str,
        sleeper: Any,
    ) -> None:
        self._stream = stream
        self._reader = reader
        self._text = text
        self._encoding = encoding
        self._errors = errors
        self._sleep = sleeper
        self._closed = False

    @property
    def closed(self) -> bool:
        return self._closed

    def _decode(self, payload: bytes) -> Any:
        if self._text:
            return payload.decode(self._encoding, self._errors)
        return payload

    def read(self, size: int | None = -1) -> Any:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if size is None:
            size = -1
        while True:
            out = _MOLT_STREAM_READER_READ(self._reader, int(size))
            if _is_pending(out):
                self._sleep()
                continue
            if out is None:
                return self._decode(b"")
            return self._decode(bytes(out))

    def readline(self, size: int | None = -1) -> Any:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if size == 0:
            return self._decode(b"")
        if size is not None and int(size) > 0:
            remaining = int(size)
            chunks = bytearray()
            while remaining > 0:
                out = _MOLT_STREAM_READER_READ(self._reader, 1)
                if _is_pending(out):
                    self._sleep()
                    continue
                if out is None:
                    break
                chunk = bytes(out)
                if not chunk:
                    break
                chunks.extend(chunk)
                remaining -= len(chunk)
                if chunk.endswith(b"\n"):
                    break
            return self._decode(bytes(chunks))
        while True:
            out = _MOLT_STREAM_READER_READLINE(self._reader)
            if _is_pending(out):
                self._sleep()
                continue
            if out is None:
                return self._decode(b"")
            return self._decode(bytes(out))

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        _MOLT_STREAM_CLOSE(self._stream)


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
        start_new_session: bool = False,
        process_group: int | None = None,
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
        self._communication_started = False

        argv = _coerce_argv(args, bool(shell))
        env_map = _coerce_env(env)
        use_ex = bool(start_new_session) or process_group is not None
        if use_ex:
            self._handle = _MOLT_PROCESS_SPAWN_EX(
                argv,
                env_map,
                cwd,
                self._stdin_mode,
                self._stdout_mode,
                self._stderr_mode,
                bool(start_new_session),
                process_group,
            )
        else:
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
            _MOLT_PROCESS_STDIN(self._handle)
            if self._stdin_mode == _MODE_PIPE
            else None
        )
        self._stdout_stream = (
            _MOLT_PROCESS_STDOUT(self._handle)
            if self._stdout_mode == _MODE_PIPE
            else None
        )
        self._stderr_stream = (
            _MOLT_PROCESS_STDERR(self._handle)
            if self._stderr_mode == _MODE_PIPE
            else None
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
        self.stdin = (
            _PopenPipeWriter(
                self._stdin_stream,
                text=self._text,
                encoding=self._encoding,
                errors=self._errors,
                sleeper=self._sleep,
            )
            if self._stdin_stream is not None
            else None
        )
        self.stdout = (
            _PopenPipeReader(
                self._stdout_stream,
                self._stdout_reader,
                text=self._text,
                encoding=self._encoding,
                errors=self._errors,
                sleeper=self._sleep,
            )
            if self._stdout_stream is not None
            else None
        )
        self.stderr = (
            _PopenPipeReader(
                self._stderr_stream,
                self._stderr_reader,
                text=self._text,
                encoding=self._encoding,
                errors=self._errors,
                sleeper=self._sleep,
            )
            if self._stderr_stream is not None
            else None
        )

    @property
    def pid(self) -> int:
        """Return the process ID of the child process."""
        return int(_MOLT_PROCESS_PID(self._handle))

    def send_signal(self, sig: int) -> None:
        """Send a signal to the child process."""
        _os.kill(self.pid, sig)

    def _sleep(self) -> None:
        _time.sleep(_POLL_SLEEP)

    def _write_input(self, data: Any) -> None:
        if self.stdin is None:
            return
        if data is None:
            return
        if self._text and not isinstance(data, str):
            # Mirror CPython's text-mode communicate() normalization path,
            # which calls input.encode(...) before writing to the child pipe.
            encoded = data.encode(self._encoding, self._errors)
            if isinstance(encoded, (bytes, bytearray, memoryview)):
                data = bytes(encoded).decode(self._encoding, self._errors)
            else:
                data = encoded
        self.stdin.write(data)

    def _read_stream(self, reader: Any | None) -> bytes | None:
        if reader is None:
            return None
        chunks = bytearray()
        while True:
            out = _MOLT_STREAM_READER_READ(reader, 65536)
            if _is_pending(out):
                self._sleep()
                continue
            if out is None:
                break
            chunk = bytes(out)
            if not chunk:
                break
            chunks.extend(chunk)
        return bytes(chunks)

    def _read_available_stream(self, reader: Any | None) -> bytes | None:
        if reader is None:
            return None
        chunks = bytearray()
        while True:
            out = _MOLT_STREAM_READER_READ(reader, 65536)
            if _is_pending(out):
                break
            if out is None:
                break
            chunk = bytes(out)
            if not chunk:
                break
            chunks.extend(chunk)
            if len(chunk) < 65536:
                continue
        if not chunks:
            return None
        return bytes(chunks)

    def _finalize_text(self, payload: bytes | None) -> str | None:
        if payload is None:
            return None
        if self._text:
            return payload.decode(self._encoding, self._errors)
        return payload  # type: ignore[return-value]

    def _as_bytes(self, payload: Any) -> bytes:
        if payload is None:
            return b""
        if isinstance(payload, (bytes, bytearray, memoryview)):
            return bytes(payload)
        return b""

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
        if self.stdin is not None and self._communication_started and input is not None:
            raise ValueError("Cannot send input after starting communication")
        self._communication_started = True
        if input is not None:
            self._write_input(input)
        if self.stdin is not None:
            self.stdin.close()
        # Read stdout and stderr BEFORE waiting for the process to exit.
        # Reading blocks until the child closes its write end of each pipe
        # (which happens when the child exits or closes the fd explicitly).
        # This prevents the classic pipe-buffer deadlock where:
        #   - child fills its stdout/stderr pipe buffer (typically 64 KB) and
        #     blocks waiting for the parent to drain it, while
        #   - parent is blocked in wait() waiting for the child to exit.
        # By draining the pipes first, the child is never blocked on a full
        # buffer, so it can exit, and the subsequent wait() returns immediately.
        out = self._read_stream(self._stdout_reader)
        err = self._read_stream(self._stderr_reader)
        # The child should already be done since it closed its pipe ends, but
        # we still need to reap the exit code.  Apply any caller-supplied
        # timeout here (only to the wait, not to the reads above).
        try:
            self.wait(limit)
        except TimeoutExpired as exc:
            exc.output = out
            exc.stderr = err
            raise
        return self._finalize_text(out), self._finalize_text(err)

    def kill(self) -> None:
        _MOLT_PROCESS_KILL(self._handle)

    def terminate(self) -> None:
        _MOLT_PROCESS_TERMINATE(self._handle)

    def __enter__(self) -> Popen:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self.stdin is not None:
            self.stdin.close()
        if self.stdout is not None:
            self.stdout.close()
        if self.stderr is not None and self.stderr is not self.stdout:
            self.stderr.close()
        self.wait()
        return None

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
    start_new_session: bool = False,
    process_group: int | None = None,
) -> CompletedProcess:
    if capture_output:
        if stdout is not None or stderr is not None:
            raise ValueError(
                "stdout and stderr arguments may not be used with capture_output."
            )
        stdout = PIPE
        stderr = PIPE
    if input is not None and stdin is not None:
        raise ValueError("stdin and input arguments may not both be used.")
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
        start_new_session=start_new_session,
        process_group=process_group,
    )
    try:
        out, err = proc.communicate(input=input, timeout=timeout)
    except TimeoutExpired as exc:
        partial_out = proc._as_bytes(getattr(exc, "output", None))
        partial_err = proc._as_bytes(getattr(exc, "stderr", None))
        proc.kill()
        try:
            proc.wait(None)
        except Exception:
            pass
        tail_out = proc._as_bytes(proc._read_stream(proc._stdout_reader))
        tail_err = proc._as_bytes(proc._read_stream(proc._stderr_reader))
        full_out = partial_out + tail_out
        full_err = partial_err + tail_err
        exc.output = full_out if full_out else None
        exc.stderr = full_err if full_err else None
        raise

    code = proc.returncode if proc.returncode is not None else proc.wait(None)
    completed = CompletedProcess(args, code, stdout=out, stderr=err)
    if check:
        completed.check_returncode()
    return completed


def check_output(args: Any, *, timeout: float | None = None, **kwargs: Any) -> Any:
    if "stdout" in kwargs:
        raise ValueError("stdout argument not allowed, it will be overridden.")
    if "check" in kwargs:
        raise ValueError("check argument not allowed, it will be overridden.")
    if "input" in kwargs and kwargs["input"] is None:
        if kwargs.get("text") or kwargs.get("encoding") or kwargs.get("errors"):
            kwargs["input"] = ""
        else:
            kwargs["input"] = b""
    kwargs["stdout"] = PIPE
    kwargs["check"] = True
    result = run(args, timeout=timeout, **kwargs)
    return result.stdout


def check_call(args: Any, *, timeout: float | None = None, **kwargs: Any) -> int:
    """Run command with arguments and return returncode; raise on non-zero exit.

    Equivalent to ``run(..., check=True).returncode``.
    """
    for key in ("input", "capture_output", "check"):
        if key in kwargs:
            raise TypeError(
                f"Popen.__init__() got an unexpected keyword argument '{key}'"
            )
    result = run(args, timeout=timeout, check=True, **kwargs)
    return result.returncode


def getstatusoutput(
    cmd: str,
    *,
    encoding: str | None = None,
    errors: str | None = None,
) -> tuple[int, str]:
    """Return ``(exitcode, output)`` of executing *cmd* in a shell.

    The locale encoding is used for decoding; trailing newlines are stripped
    from *output*.  The combined stdout and stderr is returned.
    """
    try:
        data = check_output(
            cmd,
            shell=True,
            stderr=STDOUT,
            text=True,
            encoding=encoding,
            errors=errors,
        )
        status = 0
    except CalledProcessError as exc:
        data = exc.output
        status = int(exc.returncode)
    if isinstance(data, bytes):
        data = data.decode(encoding or "utf-8", errors or "strict")
    if data and data[-1] == "\n":
        data = data[:-1]
    return status, data


def getoutput(cmd: str) -> str:
    """Return the output of executing *cmd* in a shell."""
    return getstatusoutput(cmd)[1]


__all__ = [
    "CalledProcessError",
    "CompletedProcess",
    "DEVNULL",
    "PIPE",
    "Popen",
    "STDOUT",
    "SubprocessError",
    "TimeoutExpired",
    "check_call",
    "check_output",
    "getoutput",
    "getstatusoutput",
    "run",
]


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's subprocess API.
# ---------------------------------------------------------------------------
for _name in ("Any",):
    globals().pop(_name, None)
