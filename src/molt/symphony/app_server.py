from __future__ import annotations

import json
import re
import select
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from threading import Event
from typing import Any, Callable

from .errors import (
    AgentRunnerError,
    ResponseTimeoutError,
    TurnCancelledError,
    TurnFailedError,
    TurnInputRequiredError,
    TurnTimeoutError,
)
from .models import CodexConfig, Issue


EventCallback = Callable[[dict[str, Any]], None]
ToolHandler = Callable[[str, dict[str, Any] | str | None], dict[str, Any]]

_UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
_USAGE_KEYS = (
    "input_tokens",
    "inputtokens",
    "prompt_tokens",
    "prompttokens",
    "input_token_count",
    "inputtokencount",
    "output_tokens",
    "outputtokens",
    "completion_tokens",
    "completiontokens",
    "output_token_count",
    "outputtokencount",
    "total_tokens",
    "totaltokens",
    "token_count",
    "tokencount",
    "total_token_count",
    "totaltokencount",
)
_DELTA_USAGE_KEYS = (
    "input_tokens_delta",
    "inputtokensdelta",
    "delta_input_tokens",
    "deltainputtokens",
    "output_tokens_delta",
    "outputtokensdelta",
    "delta_output_tokens",
    "deltaoutputtokens",
    "total_tokens_delta",
    "totaltokensdelta",
    "delta_total_tokens",
    "deltatotaltokens",
    "delta_token_count",
    "deltatokencount",
    "token_count_delta",
    "tokencountdelta",
)


@dataclass(slots=True)
class SessionInfo:
    thread_id: str
    turn_id: str

    @property
    def session_id(self) -> str:
        return f"{self.thread_id}-{self.turn_id}"


class CodexAppServerClient:
    def __init__(
        self,
        codex_config: CodexConfig,
        workspace_path: Path,
        stop_event: Event,
        event_callback: EventCallback,
        tool_handler: ToolHandler | None = None,
    ) -> None:
        self._config = codex_config
        self._workspace_path = workspace_path
        self._stop_event = stop_event
        self._event_callback = event_callback
        self._tool_handler = tool_handler

        self._proc: subprocess.Popen[str] | None = None
        self._next_id = 1
        self._response_cache: dict[int, dict[str, Any]] = {}
        self._thread_id: str | None = None
        self._stderr_thread: threading.Thread | None = None
        self._stderr_stop = Event()

    def start(self) -> None:
        if self._proc is not None:
            return
        self._proc = subprocess.Popen(
            ["bash", "-lc", self._config.command],
            cwd=self._workspace_path,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._stderr_stop.clear()
        if self._proc.stderr is not None:
            self._stderr_thread = threading.Thread(
                target=self._pump_stderr,
                args=(self._proc,),
                name="symphony-codex-stderr",
                daemon=True,
            )
            self._stderr_thread.start()
        self._emit("startup", message="codex process started")

        init_id = self._send_request(
            "initialize",
            {
                "clientInfo": {"name": "molt-symphony", "version": "1.0"},
                "capabilities": {},
            },
        )
        self._wait_for_response(init_id, self._config.read_timeout_ms / 1000.0)
        self._send_notification("initialized", {})

        thread_id = self._send_request(
            "thread/start",
            {
                "approvalPolicy": self._config.approval_policy,
                "sandbox": self._config.thread_sandbox,
                "cwd": str(self._workspace_path),
            },
        )
        thread_response = self._wait_for_response(
            thread_id, self._config.read_timeout_ms / 1000.0
        )
        self._thread_id = _extract_thread_id(thread_response)
        if not self._thread_id:
            raise AgentRunnerError("response_error missing thread id")

    def stop(self) -> None:
        proc = self._proc
        self._proc = None
        self._stderr_stop.set()
        if proc is None:
            return
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2.0)
        thread = self._stderr_thread
        self._stderr_thread = None
        if thread is not None and thread.is_alive():
            thread.join(timeout=0.25)
        self._emit("shutdown", message="codex process stopped")

    def run_turn(self, issue: Issue, prompt: str) -> SessionInfo:
        if self._proc is None or self._thread_id is None:
            raise AgentRunnerError("startup_failed codex process not initialized")

        request_id = self._send_request(
            "turn/start",
            {
                "threadId": self._thread_id,
                "input": [{"type": "text", "text": prompt}],
                "cwd": str(self._workspace_path),
                "title": f"{issue.identifier}: {issue.title}",
                "approvalPolicy": self._config.approval_policy,
                "sandboxPolicy": self._config.turn_sandbox_policy,
            },
        )
        response = self._wait_for_response(
            request_id, self._config.read_timeout_ms / 1000.0
        )
        turn_id = _extract_turn_id(response)
        if not turn_id:
            raise AgentRunnerError("response_error missing turn id")

        session_info = SessionInfo(thread_id=self._thread_id, turn_id=turn_id)
        self._emit(
            "session_started",
            message="turn started",
            thread_id=self._thread_id,
            turn_id=turn_id,
            session_id=session_info.session_id,
        )

        turn_deadline = time.monotonic() + (self._config.turn_timeout_ms / 1000.0)
        while True:
            if self._stop_event.is_set():
                self.stop()
                raise TurnCancelledError("turn_cancelled orchestrator requested stop")
            if time.monotonic() >= turn_deadline:
                self.stop()
                raise TurnTimeoutError("turn_timeout")

            message = self._read_one(self._config.read_timeout_ms / 1000.0)
            if message is None:
                continue

            if "id" in message and "method" not in message:
                response_id = _coerce_id(message.get("id"))
                if response_id is not None:
                    self._response_cache[response_id] = message
                continue

            if "id" in message and "method" in message:
                self._handle_server_request(message)
                continue

            method = str(message.get("method") or "")
            if method:
                usage = _extract_usage(message)
                rate_limits = _extract_rate_limits(message)
                details = _extract_notification_details(message)
                self._emit(
                    "notification",
                    message=method,
                    thread_id=self._thread_id,
                    turn_id=turn_id,
                    session_id=session_info.session_id,
                    usage=usage,
                    rate_limits=rate_limits,
                    details=details,
                )
                if _is_turn_completed(method):
                    self._emit(
                        "turn_completed",
                        message="turn completed",
                        thread_id=self._thread_id,
                        turn_id=turn_id,
                        session_id=session_info.session_id,
                    )
                    return session_info
                if _is_turn_failed(method):
                    raise TurnFailedError(f"turn_failed method={method}")
                if _is_turn_cancelled(method):
                    raise TurnCancelledError(f"turn_cancelled method={method}")
                if _is_input_required(method, message):
                    raise TurnInputRequiredError("turn_input_required")
                continue

            self._emit("other_message", message=json.dumps(message, ensure_ascii=True))

    def _send_notification(self, method: str, params: dict[str, Any]) -> None:
        self._send_json({"method": method, "params": params})

    def _send_request(self, method: str, params: dict[str, Any]) -> int:
        request_id = self._next_id
        self._next_id += 1
        self._send_json({"id": request_id, "method": method, "params": params})
        return request_id

    def _wait_for_response(
        self, request_id: int, timeout_seconds: float
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout_seconds
        cached = self._response_cache.pop(request_id, None)
        if cached is not None:
            return cached

        while True:
            if self._stop_event.is_set():
                raise TurnCancelledError("turn_cancelled orchestrator requested stop")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ResponseTimeoutError("response_timeout")

            message = self._read_one(min(remaining, timeout_seconds))
            if message is None:
                continue

            if "id" in message and "method" not in message:
                response_id = _coerce_id(message.get("id"))
                if response_id == request_id:
                    return message
                if response_id is not None:
                    self._response_cache[response_id] = message
                continue

            if "id" in message and "method" in message:
                self._handle_server_request(message)
                continue

            method = str(message.get("method") or "")
            if method:
                usage = _extract_usage(message)
                rate_limits = _extract_rate_limits(message)
                details = _extract_notification_details(message)
                self._emit(
                    "notification",
                    message=method,
                    usage=usage,
                    rate_limits=rate_limits,
                    details=details,
                )
                if _is_input_required(method, message):
                    raise TurnInputRequiredError("turn_input_required")
                continue

    def _send_json(self, payload: dict[str, Any]) -> None:
        proc = self._proc
        if proc is None or proc.stdin is None:
            raise AgentRunnerError("port_exit")
        line = json.dumps(payload, ensure_ascii=True)
        proc.stdin.write(line + "\n")
        proc.stdin.flush()

    def _read_one(self, timeout_seconds: float) -> dict[str, Any] | None:
        proc = self._proc
        if proc is None or proc.stdout is None:
            raise AgentRunnerError("port_exit")

        if proc.poll() is not None:
            raise AgentRunnerError("port_exit")

        fd = proc.stdout.fileno()
        ready, _, _ = select.select([fd], [], [], max(timeout_seconds, 0.0))
        if not ready:
            return None

        line = proc.stdout.readline()
        if line == "":
            raise AgentRunnerError("port_exit")

        text = line.rstrip("\n")
        if not text.strip():
            return None
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            self._emit("malformed", message=text)
            return None

    def _handle_server_request(self, message: dict[str, Any]) -> None:
        request_id = message.get("id")
        method = str(message.get("method") or "")

        if _is_input_required(method, message):
            self._send_json(
                {
                    "id": request_id,
                    "result": {
                        "success": False,
                        "error": "turn_input_required",
                    },
                }
            )
            raise TurnInputRequiredError("turn_input_required")

        if "approval" in method.lower():
            self._send_json({"id": request_id, "result": {"approved": True}})
            self._emit("approval_auto_approved", message=method)
            return

        if "tool/call" in method.lower():
            params = message.get("params") or {}
            tool_name = _extract_tool_name(params)
            tool_input = _extract_tool_input(params)
            if tool_name and self._tool_handler is not None:
                result = self._tool_handler(tool_name, tool_input)
                self._send_json({"id": request_id, "result": result})
            else:
                self._send_json(
                    {
                        "id": request_id,
                        "result": {
                            "success": False,
                            "error": "unsupported_tool_call",
                        },
                    }
                )
                self._emit("unsupported_tool_call", message=tool_name or method)
            return

        self._send_json(
            {
                "id": request_id,
                "result": {
                    "success": False,
                    "error": "unsupported_request",
                },
            }
        )

    def _emit(self, event: str, **payload: Any) -> None:
        proc = self._proc
        base = {
            "event": event,
            "timestamp": time.time(),
            "codex_app_server_pid": proc.pid if proc else None,
        }
        base.update(payload)
        self._event_callback(base)

    def _pump_stderr(self, proc: subprocess.Popen[str]) -> None:
        stderr = proc.stderr
        if stderr is None:
            return
        while not self._stderr_stop.is_set():
            line = stderr.readline()
            if line == "":
                if proc.poll() is not None:
                    return
                time.sleep(0.01)
                continue
            text = line.strip()
            if not text:
                continue
            self._emit("stderr_output", message=text[:4000])


def _coerce_id(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _extract_thread_id(response: dict[str, Any]) -> str | None:
    result = response.get("result") or {}
    thread_obj = result.get("thread") if isinstance(result, dict) else None
    if isinstance(thread_obj, dict):
        thread_id = thread_obj.get("id")
        if isinstance(thread_id, str) and thread_id:
            return thread_id
    return None


def _extract_turn_id(response: dict[str, Any]) -> str | None:
    result = response.get("result") or {}
    turn_obj = result.get("turn") if isinstance(result, dict) else None
    if isinstance(turn_obj, dict):
        turn_id = turn_obj.get("id")
        if isinstance(turn_id, str) and turn_id:
            return turn_id
    return None


def _extract_usage(message: dict[str, Any]) -> dict[str, int | bool] | None:
    method = str(message.get("method") or "").lower()
    prefer_delta = ("token_count" in method) or ("delta" in method)
    candidates: list[dict[str, Any]] = []
    seen: set[int] = set()

    for path in (
        ("total_token_usage",),
        ("totalTokenUsage",),
        ("usage",),
        ("token_usage",),
        ("params", "total_token_usage"),
        ("params", "totalTokenUsage"),
        ("params", "usage"),
        ("params", "token_usage"),
        ("params", "event", "total_token_usage"),
        ("params", "event", "totalTokenUsage"),
        ("params", "event", "usage"),
        ("params", "event", "token_usage"),
        ("params", "event", "metrics"),
        ("params", "event", "stats"),
        ("params", "metrics"),
        ("params", "stats"),
    ):
        candidate = _resolve_map_path(message, path)
        if candidate is None:
            continue
        key = id(candidate)
        if key in seen:
            continue
        seen.add(key)
        candidates.append(candidate)

    params = message.get("params")
    if isinstance(params, dict):
        for candidate in _iter_token_maps(params):
            key = id(candidate)
            if key in seen:
                continue
            seen.add(key)
            candidates.append(candidate)
    if not candidates:
        for candidate in _iter_token_maps(message):
            key = id(candidate)
            if key in seen:
                continue
            seen.add(key)
            candidates.append(candidate)

    for candidate in candidates:
        parsed = _coerce_usage_candidate(candidate, prefer_delta=prefer_delta)
        if parsed is not None:
            return parsed
    return None


def _resolve_map_path(root: Any, path: tuple[str, ...]) -> dict[str, Any] | None:
    current = root
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    if isinstance(current, dict):
        return current
    return None


def _iter_token_maps(value: Any, depth: int = 0) -> list[dict[str, Any]]:
    if depth > 5:
        return []
    rows: list[dict[str, Any]] = []
    if isinstance(value, dict):
        lowered_keys = {str(key).lower() for key in value}
        if lowered_keys & (set(_USAGE_KEYS) | set(_DELTA_USAGE_KEYS)):
            rows.append(value)
        for key, child in value.items():
            key_lower = str(key).lower()
            if depth > 0 and not any(
                token in key_lower
                for token in (
                    "token",
                    "usage",
                    "metric",
                    "stats",
                    "event",
                    "turn",
                    "thread",
                    "data",
                )
            ):
                continue
            rows.extend(_iter_token_maps(child, depth + 1))
    elif isinstance(value, list):
        for child in value:
            rows.extend(_iter_token_maps(child, depth + 1))
    return rows


def _coerce_usage_candidate(
    candidate: Any, *, prefer_delta: bool = False
) -> dict[str, int | bool] | None:
    if not isinstance(candidate, dict):
        return None
    input_total_keys = (
        "input_tokens",
        "inputTokens",
        "prompt_tokens",
        "promptTokens",
        "input_token_count",
        "inputTokenCount",
    )
    output_total_keys = (
        "output_tokens",
        "outputTokens",
        "completion_tokens",
        "completionTokens",
        "output_token_count",
        "outputTokenCount",
    )
    total_keys = (
        "total_tokens",
        "totalTokens",
        "token_count",
        "tokenCount",
        "total_token_count",
        "totalTokenCount",
    )
    input_delta_keys = (
        "input_tokens_delta",
        "inputTokensDelta",
        "delta_input_tokens",
        "deltaInputTokens",
    )
    output_delta_keys = (
        "output_tokens_delta",
        "outputTokensDelta",
        "delta_output_tokens",
        "deltaOutputTokens",
    )
    total_delta_keys = (
        "total_tokens_delta",
        "totalTokensDelta",
        "delta_total_tokens",
        "delta_total_token_count",
        "deltaTokenCount",
        "delta_token_count",
        "tokenCountDelta",
        "token_count_delta",
    )
    input_tokens = _first_int(candidate, input_total_keys)
    output_tokens = _first_int(candidate, output_total_keys)
    total_tokens = _first_int(candidate, total_keys)
    delta_input = _first_int(candidate, input_delta_keys)
    delta_output = _first_int(candidate, output_delta_keys)
    delta_total = _first_int(candidate, total_delta_keys)

    has_totals = _has_any_key(
        candidate, input_total_keys + output_total_keys + total_keys
    )
    has_deltas = _has_any_key(
        candidate, input_delta_keys + output_delta_keys + total_delta_keys
    )
    if not has_totals and not has_deltas:
        return None
    delta_nonzero = any(value > 0 for value in (delta_input, delta_output, delta_total))
    totals_nonzero = any(
        value > 0 for value in (input_tokens, output_tokens, total_tokens)
    )
    choose_delta = has_deltas and (
        (prefer_delta and (delta_nonzero or not totals_nonzero)) or not has_totals
    )

    if choose_delta:
        if delta_total == 0 and (delta_input or delta_output):
            delta_total = delta_input + delta_output
        return {
            "input_tokens": delta_input,
            "output_tokens": delta_output,
            "total_tokens": delta_total,
            "delta": True,
        }

    if has_totals:
        if total_tokens == 0 and (input_tokens or output_tokens):
            total_tokens = input_tokens + output_tokens
        return {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": total_tokens,
            "delta": False,
        }
    if delta_total == 0 and (delta_input or delta_output):
        delta_total = delta_input + delta_output
    return {
        "input_tokens": delta_input,
        "output_tokens": delta_output,
        "total_tokens": delta_total,
        "delta": True,
    }


def _has_any_key(candidate: dict[str, Any], keys: tuple[str, ...]) -> bool:
    present = {str(key).lower() for key in candidate}
    return any(str(key).lower() in present for key in keys)


def _extract_rate_limits(message: dict[str, Any]) -> dict[str, Any] | None:
    for key in ("rate_limits", "rateLimits"):
        value = message.get(key)
        if isinstance(value, dict):
            return value
    params = message.get("params")
    if isinstance(params, dict):
        for key in ("rate_limits", "rateLimits"):
            value = params.get(key)
            if isinstance(value, dict):
                return value
    return None


def _extract_notification_details(message: dict[str, Any]) -> dict[str, Any] | None:
    params = message.get("params")
    if not isinstance(params, dict):
        return None

    text = _extract_text_preview(params)
    details: dict[str, Any] = {}
    if text:
        details["text_preview"] = text
    usage = _extract_usage(message)
    if isinstance(usage, dict):
        details["usage"] = usage
    tool_name = _extract_tool_name(params)
    if tool_name:
        details["tool_name"] = tool_name

    return details or None


def _extract_text_preview(value: Any, depth: int = 0) -> str | None:
    if depth > 5:
        return None
    if isinstance(value, str):
        cleaned = " ".join(value.split())
        if not cleaned:
            return None
        if _UUID_RE.match(cleaned.lower()):
            return None
        if len(cleaned) < 4:
            return None
        return cleaned[:320]
    if isinstance(value, list):
        for item in value:
            candidate = _extract_text_preview(item, depth + 1)
            if candidate:
                return candidate
        return None
    if isinstance(value, dict):
        preferred_keys = (
            "text",
            "content",
            "delta",
            "message",
            "reasoning",
            "analysis",
            "output",
            "summary",
            "title",
        )
        for key in preferred_keys:
            if key in value:
                candidate = _extract_text_preview(value.get(key), depth + 1)
                if candidate:
                    return candidate
        for child in value.values():
            candidate = _extract_text_preview(child, depth + 1)
            if candidate:
                return candidate
    return None


def _as_int(value: Any) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


def _first_int(candidate: dict[str, Any], keys: tuple[str, ...]) -> int:
    for key in keys:
        if key in candidate:
            parsed = _as_int(candidate.get(key))
            if parsed:
                return parsed
    return 0


def _is_turn_completed(method: str) -> bool:
    lowered = method.lower()
    return lowered.endswith("turn/completed") or "turn/completed" in lowered


def _is_turn_failed(method: str) -> bool:
    lowered = method.lower()
    return lowered.endswith("turn/failed") or "turn/failed" in lowered


def _is_turn_cancelled(method: str) -> bool:
    lowered = method.lower()
    return lowered.endswith("turn/cancelled") or "turn/cancelled" in lowered


def _is_input_required(method: str, payload: dict[str, Any]) -> bool:
    lowered = method.lower()
    if "requestuserinput" in lowered:
        return True
    if "input_required" in lowered:
        return True
    params = payload.get("params")
    if isinstance(params, dict) and params.get("inputRequired") is True:
        return True
    return False


def _extract_tool_name(params: dict[str, Any]) -> str | None:
    for key in ("name", "toolName", "tool_name"):
        value = params.get(key)
        if isinstance(value, str) and value:
            return value
    tool = params.get("tool")
    if isinstance(tool, dict):
        value = tool.get("name")
        if isinstance(value, str) and value:
            return value
    return None


def _extract_tool_input(params: dict[str, Any]) -> dict[str, Any] | str | None:
    for key in ("input", "arguments", "args"):
        if key in params:
            return params[key]
    tool = params.get("tool")
    if isinstance(tool, dict) and "input" in tool:
        return tool["input"]
    return None
