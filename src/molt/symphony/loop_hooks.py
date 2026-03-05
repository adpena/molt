from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping


@dataclass(frozen=True, slots=True)
class HookDecision:
    action: str
    reason: str = ""
    command: list[str] | None = None
    metadata: dict[str, Any] | None = None


def _normalize_action(value: Any) -> str:
    text = str(value or "").strip().lower()
    if text in {"allow", "block", "replace"}:
        return text
    return "allow"


def _normalize_command(value: Any) -> list[str] | None:
    if value is None:
        return None
    if isinstance(value, list):
        result = [str(part).strip() for part in value if str(part).strip()]
        return result or None
    text = str(value).strip()
    return [text] if text else None


def decode_hook_decision(payload: Mapping[str, Any] | None) -> HookDecision:
    row = dict(payload or {})
    return HookDecision(
        action=_normalize_action(row.get("action")),
        reason=str(row.get("reason") or "").strip(),
        command=_normalize_command(row.get("command")),
        metadata=row.get("metadata") if isinstance(row.get("metadata"), dict) else None,
    )


class LoopHookRunner:
    def __init__(self, command: list[str] | None) -> None:
        self._command = [part for part in (command or []) if str(part).strip()]

    @property
    def enabled(self) -> bool:
        return bool(self._command)

    def run(
        self,
        *,
        event: str,
        payload: dict[str, Any],
        cwd: Path,
        env: Mapping[str, str],
    ) -> HookDecision:
        if not self.enabled:
            return HookDecision(action="allow", reason="disabled")
        request = {"event": event, "payload": payload}
        proc = subprocess.run(
            self._command,
            cwd=cwd,
            env=dict(env),
            input=json.dumps(request, sort_keys=True),
            text=True,
            capture_output=True,
            check=False,
        )
        if proc.returncode != 0:
            return HookDecision(
                action="allow",
                reason=f"hook_rc_{proc.returncode}",
                metadata={"stderr": (proc.stderr or "").strip()[-1000:]},
            )
        stdout = (proc.stdout or "").strip()
        if not stdout:
            return HookDecision(action="allow", reason="empty_response")
        try:
            parsed = json.loads(stdout)
        except Exception:
            return HookDecision(
                action="allow",
                reason="invalid_json",
                metadata={"stdout": stdout[-1000:]},
            )
        if not isinstance(parsed, dict):
            return HookDecision(action="allow", reason="non_object_response")
        return decode_hook_decision(parsed)
