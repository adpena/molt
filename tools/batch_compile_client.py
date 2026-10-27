"""Client for Molt's internal batch build server."""

from __future__ import annotations

import atexit
import contextlib
import json
import os
import queue
import subprocess
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from dataclasses import dataclass

from tools import harness_memory_guard
from molt.process_spawn import ProcessGroupKwargs
from molt.source_root import compiler_source_root


@dataclass(frozen=True, slots=True)
class BatchCompileRequestCustody:
    child_process: harness_memory_guard.memory_guard.GuardedChildProcess
    request_started_at_ns: int
    returncode: int
    owned_process_identities: tuple = ()


class BatchCompileResponse(dict[str, object]):
    """Protocol response carrying the immutable local request observation."""

    def __init__(
        self, response: dict[str, object], custody: BatchCompileRequestCustody
    ):
        super().__init__(response)
        self.custody = custody


def _bounded_protocol_text(value: str, *, limit: int = 512) -> str:
    text = value.rstrip("\r\n")
    if len(text) <= limit:
        return text
    return f"... <truncated to last {limit} chars>{text[-limit:]}"


class BatchCompileProtocolError(RuntimeError):
    """Raised when the line protocol can no longer be correlated safely."""

    def __init__(
        self,
        message: str,
        *,
        expected_id: object | None = None,
        actual_id: object | None = None,
        op: str | None = None,
        raw_response: str | None = None,
    ) -> None:
        self.expected_id = expected_id
        self.actual_id = actual_id
        self.op = op
        self.raw_response = raw_response
        parts = [message]
        context: list[str] = []
        if expected_id is not None:
            context.append(f"expected id {expected_id!r}")
        if actual_id is not None:
            context.append(f"got id {actual_id!r}")
        if op is not None:
            context.append(f"op {op!r}")
        if context:
            parts.append("; ".join(context))
        if raw_response is not None:
            parts.append(f"raw response: {_bounded_protocol_text(raw_response)}")
        super().__init__(": ".join(parts))


class BatchCompileServerClient:
    """Line-delimited JSON client for `molt.cli internal-batch-build-server`."""

    def __init__(
        self,
        cmd: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
        guard_context: harness_memory_guard.HarnessExecutionContext | None = None,
        memory_guard_prefix: str | None = "MOLT",
        process_group_kwargs: ProcessGroupKwargs | None = None,
        force_close: Callable[[subprocess.Popen[str]], None] | None = None,
        reader_name: str = "molt-batch-server-reader",
    ) -> None:
        self._proc: subprocess.Popen[str] | None = None
        self._guard_sentinel = None
        self._force_close: Callable[[subprocess.Popen[str]], None] | None = None
        self._response_reader: threading.Thread | None = None
        self._stderr_reader: threading.Thread | None = None
        self._stderr_tail = b""
        self._stderr_error: Exception | None = None
        self._request_lock = threading.Lock()
        self._closing = threading.Event()
        self._closed = False
        self._next_id = 1
        self._response_queue: queue.Queue[str | BaseException | None] = queue.Queue()
        self._state_use = contextlib.ExitStack()
        self._poisoned = False
        atexit.register(self.close)
        try:
            source_root = (
                guard_context.repo_root
                if guard_context is not None
                else compiler_source_root()
            )
            self._guard_context = guard_context
            if self._guard_context is None and memory_guard_prefix is not None:
                self._guard_context = (
                    harness_memory_guard.HarnessExecutionContext.from_env(
                        memory_guard_prefix,
                        env,
                        repo_root=source_root,
                    )
                )
            launch_env = (
                dict(self._guard_context.env)
                if self._guard_context is not None
                else dict(env)
            )
            if process_group_kwargs is None and self._guard_context is not None:
                process_group_kwargs = self._guard_context.process_group_kwargs()
            if (
                force_close is None
                and self._guard_context is not None
                and self._guard_context.limits.enabled
            ):
                force_close = self._guard_context.force_close_process_group
            self._force_close = force_close
            from molt.checkout_state import (
                checkout_state_command_roots,
                checkout_state_use,
            )

            selected_environment = (
                self._guard_context.env if self._guard_context is not None else env
            )
            self._state_use.enter_context(
                checkout_state_use(
                    checkout_state_command_roots(
                        source_root, selected_environment, cmd, cwd=cwd
                    )
                )
            )
            self._guard_sentinel = None
            if self._guard_context is not None:
                label = reader_name.removesuffix("-reader").replace("-", "_")
                self._guard_sentinel = self._guard_context.start_repo_sentinel(
                    label=label,
                    drain_until_clean_sec=0.1,
                    drain_max_runtime_sec=2.0,
                )
            process = subprocess.Popen(
                list(cmd),
                cwd=str(cwd),
                env=launch_env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                errors="replace",
                **(process_group_kwargs or {}),
            )
            self._proc = process
            # Capture the launch birth before any poll can reap a fast server exit.
            guard = harness_memory_guard.memory_guard
            self.child_process = guard.GuardedChildProcess(
                pid=process.pid,
                pgid=guard._safe_getpgid(process.pid),
                sid=guard._safe_getsid(process.pid),
                command=tuple(cmd),
                started_at=guard._utc_timestamp(),
                started_at_ns=(
                    guard.windows_process_handle_started_at_ns(
                        getattr(process, "_handle", None)
                    )
                    if os.name == "nt"
                    else guard._process_model.process_started_at_ns(process.pid)
                ),
            )
            if process.stdin is None:
                raise RuntimeError("batch compile server stdin pipe unavailable")
            # Writes below use the descriptor directly; never buffer a partial
            # protocol line in TextIOWrapper where flush could outlive its budget.
            os.set_blocking(process.stdin.fileno(), False)
            self._stderr_reader = threading.Thread(
                target=self._stderr_reader_loop,
                name=f"{reader_name}-stderr",
                daemon=True,
            )
            self._stderr_reader.start()
            self._response_reader = threading.Thread(
                target=self._stdout_reader_loop,
                name=reader_name,
                daemon=True,
            )
            self._response_reader.start()
        except BaseException as error:
            try:
                self.close(force=True, timeout=0.35)
            except BaseException as cleanup_error:
                setattr(error, "batch_client", self)
                error.add_note(f"batch startup closure unresolved: {cleanup_error}")
            raise

    def _require_process(self) -> subprocess.Popen[str]:
        if self._proc is None:
            raise RuntimeError("batch compiler child has not been launched")
        return self._proc

    def _stdout_reader_loop(self) -> None:
        stdout = self._require_process().stdout
        if stdout is None:
            self._response_queue.put(
                RuntimeError("batch compile server stdout pipe unavailable")
            )
            return
        try:
            while True:
                line = stdout.readline()
                if not line:
                    self._response_queue.put(None)
                    return
                self._response_queue.put(line)
        except Exception as exc:
            self._response_queue.put(exc)

    def _stderr_reader_loop(self) -> None:
        stderr = self._require_process().stderr
        if stderr is None:
            return
        try:
            while chunk := os.read(stderr.fileno(), 4096):
                # One reader publishes immutable snapshots. Diagnostics must
                # never wait for stderr EOF, nor retain an unbounded transcript.
                self._stderr_tail = (self._stderr_tail + chunk)[-8192:]
        except Exception as exc:
            self._stderr_error = exc

    def _request_remaining(
        self, deadline: float, phase: str, *, allow_closing: bool
    ) -> float:
        if self._closing.is_set() and not allow_closing:
            raise RuntimeError("batch compile server is closing")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError(f"batch compile server {phase} timed out")
        return remaining

    def _write_request(
        self, payload: bytes, deadline: float, *, allow_closing: bool
    ) -> None:
        stdin = self._require_process().stdin
        if stdin is None:
            raise RuntimeError("batch compile server stdin pipe unavailable")
        pending = memoryview(payload)
        while pending:
            remaining = self._request_remaining(
                deadline, "request write", allow_closing=allow_closing
            )
            try:
                written = os.write(stdin.fileno(), pending)
            except BlockingIOError:
                # Pipe readiness selectors are not portable to Windows. This
                # bounded wait also lets forced close wake the active writer.
                if allow_closing:
                    time.sleep(min(remaining, 0.01))
                else:
                    self._closing.wait(min(remaining, 0.01))
                continue
            except InterruptedError:
                continue
            if written <= 0:
                raise OSError("batch compile stdin write made no progress")
            pending = pending[written:]

    def _readline(self, deadline: float, *, allow_closing: bool = False) -> str:
        while True:
            remaining = self._request_remaining(
                deadline, "response", allow_closing=allow_closing
            )
            try:
                item = self._response_queue.get(timeout=min(remaining, 0.05))
            except queue.Empty:
                continue
            if isinstance(item, BaseException):
                raise RuntimeError(
                    "batch compile server response reader failed"
                ) from item
            if item is None:
                stderr = self._require_process().stderr
                encoding = stderr.encoding if stderr is not None else "utf-8"
                error_detail = self._stderr_tail.decode(
                    encoding or "utf-8", errors="replace"
                ).strip()
                if self._stderr_error is not None:
                    error_detail += f" (stderr reader failed: {self._stderr_error})"
                raise RuntimeError(
                    "batch compile server closed response pipe"
                    + (f": {error_detail}" if error_detail else "")
                )
            return item

    def request(
        self,
        op: str,
        *,
        params: dict[str, object] | None = None,
        env_overrides: Mapping[str, str] | None = None,
        timeout: float,
    ) -> BatchCompileResponse:
        deadline = time.monotonic() + max(0.0, timeout)
        if self._closing.is_set():
            raise RuntimeError("batch compile server is closing")
        if not self._request_lock.acquire(
            timeout=max(0.0, deadline - time.monotonic())
        ):
            # No exclusive admission: do not attribute another request's work
            # to this caller or poison its still-synchronized server.
            raise TimeoutError("batch compile server request admission timed out")
        try:
            if self._closing.is_set():
                raise RuntimeError("batch compile server is closing")
            # One persistent server, one active request. This boundary also
            # excludes suite evidence published for an earlier request.
            request_started_at_ns = time.monotonic_ns()
            try:
                response = self._request(
                    op, params=params, env_overrides=env_overrides, deadline=deadline
                )
            except Exception as exc:
                setattr(
                    exc,
                    "batch_request_custody",
                    BatchCompileRequestCustody(
                        self.child_process, request_started_at_ns, 127
                    ),
                )
                raise
            returncode = response.get("returncode")
            if type(returncode) is not int:
                returncode = 0 if response.get("ok") else 1
            return BatchCompileResponse(
                response,
                BatchCompileRequestCustody(
                    self.child_process, request_started_at_ns, returncode
                ),
            )
        finally:
            self._request_lock.release()

    def _request(
        self,
        op: str,
        *,
        params: dict[str, object] | None = None,
        env_overrides: Mapping[str, str] | None = None,
        deadline: float,
        allow_closing: bool = False,
    ) -> dict[str, object]:
        self._request_remaining(deadline, "request", allow_closing=allow_closing)
        if self._poisoned:
            raise RuntimeError(
                "batch compile server protocol state is unsynchronized; restart required"
            )
        if self._require_process().poll() is not None:
            raise RuntimeError("batch compile server process is not running")
        stdin = self._require_process().stdin
        if stdin is None:
            raise RuntimeError("batch compile server stdin pipe unavailable")
        req_id = self._next_id
        self._next_id += 1
        request: dict[str, object] = {"id": req_id, "op": op}
        if params is not None:
            request["params"] = params
        if env_overrides is not None:
            request["env_overrides"] = dict(env_overrides)
        payload = (
            json.dumps(request, sort_keys=True, ensure_ascii=True) + "\n"
        ).encode("ascii")
        try:
            try:
                self._write_request(payload, deadline, allow_closing=allow_closing)
            except TimeoutError:
                raise
            except OSError as exc:
                raise RuntimeError(
                    f"failed to write batch compile request: {exc}"
                ) from exc
            raw = self._readline(deadline, allow_closing=allow_closing)
        except BaseException:
            # Even a failed write can leave a prefix of the line in the server.
            # Reusing that connection would destroy request/response correlation.
            self._poisoned = True
            raise
        try:
            response = json.loads(raw)
        except json.JSONDecodeError as exc:
            self._poisoned = True
            raise BatchCompileProtocolError(
                f"invalid batch compile response JSON: {exc}",
                expected_id=req_id,
                op=op,
                raw_response=raw,
            ) from exc
        if not isinstance(response, dict):
            self._poisoned = True
            raise BatchCompileProtocolError(
                "batch compile response must be an object",
                expected_id=req_id,
                op=op,
                raw_response=raw,
            )
        actual_id = response.get("id")
        if type(actual_id) is not int or actual_id != req_id:
            self._poisoned = True
            raise BatchCompileProtocolError(
                "batch compile response id mismatch",
                expected_id=req_id,
                actual_id=actual_id,
                op=op,
                raw_response=raw,
            )
        return response

    def force_close(self) -> None:
        self._closing.set()
        process = self._proc
        if process is None:
            return
        if self._force_close is not None:
            self._force_close(process)
        elif process.poll() is None:
            with contextlib.suppress(ProcessLookupError, OSError):
                process.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                process.wait(timeout=0.35)
            if process.poll() is None:
                with contextlib.suppress(ProcessLookupError, OSError):
                    process.kill()
                with contextlib.suppress(subprocess.TimeoutExpired):
                    process.wait(timeout=0.35)
        if process.poll() is None:
            raise RuntimeError(
                "batch compile child closure unresolved; use owner retained"
            )

    def close(self, *, force: bool = False, timeout: float = 60.0) -> None:
        self._closing.set()
        if self._closed:
            return
        deadline = time.monotonic() + max(0.0, timeout)
        acquired = False
        try:
            if not force and not self._poisoned:
                acquired = self._request_lock.acquire(
                    timeout=max(0.0, deadline - time.monotonic())
                )
                if acquired and not self._closed:
                    process = self._proc
                    if process is not None and process.poll() is None:
                        with contextlib.suppress(Exception):
                            # External admission is closed, but shutdown remains
                            # one serialized request under the same lock.
                            self._request(
                                "shutdown", deadline=deadline, allow_closing=True
                            )
            # Forced close must stop the child before waiting for an active
            # writer. The existing closure callback has its own teardown budget.
            self.force_close()
            cleanup_deadline = time.monotonic() + min(max(0.0, timeout), 0.35)
            if not acquired:
                acquired = self._request_lock.acquire(
                    timeout=max(0.0, cleanup_deadline - time.monotonic())
                )
            if not acquired:
                raise RuntimeError(
                    "batch request closure unresolved; use owner retained"
                )
            if self._closed:
                return
            for reader in (self._response_reader, self._stderr_reader):
                if reader is not None and reader.ident is not None:
                    reader.join(timeout=max(0.0, cleanup_deadline - time.monotonic()))
                    if reader.is_alive():
                        raise RuntimeError(
                            "batch reader closure unresolved; use owner retained"
                        )
            if self._guard_sentinel is not None:
                self._guard_sentinel.__exit__(None, None, None)
                self._guard_sentinel = None
            process = self._proc
            if process is not None:
                # Neither writer nor readers may still use these descriptors.
                for stream in (process.stdin, process.stdout, process.stderr):
                    if stream is not None:
                        stream.close()
            self._state_use.close()
            self._closed = True
            atexit.unregister(self.close)
        finally:
            if acquired:
                self._request_lock.release()
