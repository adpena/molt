from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

_REDACT_VALUE_PATTERNS: tuple[re.Pattern[str], ...] = (
    re.compile(r"\blin_api_[A-Za-z0-9]{20,}\b"),
    re.compile(r"\bsk-[A-Za-z0-9]{20,}\b"),
    re.compile(r"\b(?:ghp|github_pat)_[A-Za-z0-9_]{20,}\b"),
    re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
)
_REDACT_SENSITIVE_KEYS = (
    "token",
    "api_key",
    "apikey",
    "secret",
    "password",
    "authorization",
)


def project_state_payload(
    state_payload: dict[str, Any], *, http_security: dict[str, Any]
) -> dict[str, Any]:
    payload = redact_payload(state_payload)
    runtime_payload_raw = payload.get("runtime")
    runtime_payload = dict(runtime_payload_raw) if isinstance(runtime_payload_raw, dict) else {}
    runtime_payload["http_security"] = http_security
    payload["runtime"] = runtime_payload
    return payload


def redact_payload(payload: dict[str, Any]) -> dict[str, Any]:
    return _redact_value(payload, depth=0)


def _redact_value(value: Any, *, depth: int) -> Any:
    if depth >= 10:
        return value
    if isinstance(value, dict):
        redacted: dict[str, Any] = {}
        for raw_key, raw_value in value.items():
            key = str(raw_key)
            key_norm = key.strip().lower()
            if any(marker in key_norm for marker in _REDACT_SENSITIVE_KEYS):
                redacted[key] = "<redacted>"
                continue
            redacted[key] = _redact_value(raw_value, depth=depth + 1)
        return redacted
    if isinstance(value, list):
        return [_redact_value(item, depth=depth + 1) for item in value]
    if isinstance(value, tuple):
        return tuple(_redact_value(item, depth=depth + 1) for item in value)
    if isinstance(value, str):
        redacted = value
        for pattern in _REDACT_VALUE_PATTERNS:
            redacted = pattern.sub("<redacted>", redacted)
        return redacted
    return value


def load_security_events_summary(path: Path, *, max_lines: int) -> dict[str, Any]:
    if not path.exists():
        return {"secret_guard_blocked": {"total": 0, "last_at": None}}
    try:
        with path.open("rb") as handle:
            handle.seek(0, 2)
            end = handle.tell()
            pos = end
            block = 4096
            chunks: list[bytes] = []
            newline_budget = max(max_lines * 2, 200)
            seen_newlines = 0
            while pos > 0 and seen_newlines <= newline_budget:
                read_size = min(block, pos)
                pos -= read_size
                handle.seek(pos)
                chunk = handle.read(read_size)
                chunks.insert(0, chunk)
                seen_newlines += chunk.count(b"\n")
            content = b"".join(chunks).decode("utf-8", errors="replace")
    except OSError:
        return {"secret_guard_blocked": {"total": 0, "last_at": None}}
    total = 0
    last_at: str | None = None
    for raw_line in content.splitlines()[-max(max_lines, 50) :]:
        line = raw_line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(row, dict):
            continue
        if str(row.get("kind") or "") != "secret_guard_blocked":
            continue
        total += 1
        at_value = row.get("at")
        if isinstance(at_value, str) and at_value:
            if last_at is None or at_value > last_at:
                last_at = at_value
    return {"secret_guard_blocked": {"total": total, "last_at": last_at}}
