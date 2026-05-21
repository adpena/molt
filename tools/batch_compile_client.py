"""Client for Molt's internal batch build server."""

from __future__ import annotations

import contextlib
import json
import queue
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path

try:
    from tools import harness_memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct import from tools/
    import harness_memory_guard  # type: ignore


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
        process_group_kwargs: Mapping[str, object] | None = None,
        force_close: Callable[[subprocess.Popen[str]], None] | None = None,
        reader_name: str = "molt-batch-server-reader",
    ) -> None:
        self._guard_context = guard_context
        if self._guard_context is None and memory_guard_prefix is not None:
            self._guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
                memory_guard_prefix,
                env,
                repo_root=cwd,
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
        self._guard_sentinel = None
        if self._guard_context is not None:
            label = reader_name.removesuffix("-reader").replace("-", "_")
            self._guard_sentinel = self._guard_context.start_repo_sentinel(
                label=label,
                drain_until_clean_sec=0.1,
                drain_max_runtime_sec=2.0,
            )
        try:
            self._proc = subprocess.Popen(
                list(cmd),
                cwd=str(cwd),
                env=launch_env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                errors="replace",
                **dict(process_group_kwargs or {}),
            )
        except BaseException:
            if self._guard_sentinel is not None:
                self._guard_sentinel.__exit__(*sys.exc_info())
            raise
        self._next_id = 1
        self._response_queue: queue.Queue[str | BaseException | None] = queue.Queue()
        self._response_reader = threading.Thread(
            target=self._stdout_reader_loop,
            name=reader_name,
            daemon=True,
        )
        self._response_reader.start()

    def _stdout_reader_loop(self) -> None:
        if self._proc.stdout is None:
            self._response_queue.put(
                RuntimeError("batch compile server stdout pipe unavailable")
            )
            return
        try:
            while True:
                line = self._proc.stdout.readline()
                if not line:
                    self._response_queue.put(None)
                    return
                self._response_queue.put(line)
        except Exception as exc:
            self._response_queue.put(exc)

    def _readline(self, timeout: float) -> str:
        deadline = time.monotonic() + max(0.01, timeout)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("batch compile server response timed out")
            try:
                item = self._response_queue.get(timeout=remaining)
            except queue.Empty as exc:
                raise TimeoutError("batch compile server response timed out") from exc
            if isinstance(item, BaseException):
                raise RuntimeError(
                    "batch compile server response reader failed"
                ) from item
            if item is None:
                error_detail = ""
                if self._proc.stderr is not None:
                    with contextlib.suppress(Exception):
                        error_detail = self._proc.stderr.read().strip()
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
        timeout: float,
    ) -> dict[str, object]:
        if self._proc.poll() is not None:
            raise RuntimeError("batch compile server process is not running")
        if self._proc.stdin is None:
            raise RuntimeError("batch compile server stdin pipe unavailable")
        req_id = self._next_id
        self._next_id += 1
        request: dict[str, object] = {"id": req_id, "op": op}
        if params is not None:
            request["params"] = params
        payload = json.dumps(request, sort_keys=True) + "\n"
        try:
            self._proc.stdin.write(payload)
            self._proc.stdin.flush()
        except OSError as exc:
            raise RuntimeError(f"failed to write batch compile request: {exc}") from exc
        raw = self._readline(timeout)
        try:
            response = json.loads(raw)
        except json.JSONDecodeError as exc:
            raise RuntimeError(f"invalid batch compile response JSON: {exc}") from exc
        if not isinstance(response, dict):
            raise RuntimeError("batch compile response must be an object")
        if response.get("id") != req_id:
            raise RuntimeError("batch compile response id mismatch")
        return response

    def force_close(self) -> None:
        if self._force_close is not None:
            self._force_close(self._proc)
            return
        if self._proc.poll() is not None:
            return
        with contextlib.suppress(ProcessLookupError, OSError):
            self._proc.terminate()
        with contextlib.suppress(subprocess.TimeoutExpired):
            self._proc.wait(timeout=0.35)
        if self._proc.poll() is None:
            with contextlib.suppress(ProcessLookupError, OSError):
                self._proc.kill()
            with contextlib.suppress(subprocess.TimeoutExpired):
                self._proc.wait(timeout=0.35)

    def close(self, *, force: bool = False, timeout: float = 60.0) -> None:
        try:
            if not force and self._proc.poll() is None:
                with contextlib.suppress(Exception):
                    self.request("shutdown", timeout=timeout)
            self.force_close()
        finally:
            if self._guard_sentinel is not None:
                self._guard_sentinel.__exit__(None, None, None)
                self._guard_sentinel = None
