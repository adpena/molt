from __future__ import annotations

import json
import sys
from datetime import UTC, datetime
from typing import Any


def utc_now_iso() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def _render_value(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    text = str(value)
    if " " in text or "\n" in text or "\t" in text:
        return json.dumps(text)
    return text


def log(level: str, message: str, **fields: Any) -> None:
    parts = [f"ts={utc_now_iso()}", f"level={level}", f"msg={json.dumps(message)}"]
    for key in sorted(fields):
        parts.append(f"{key}={_render_value(fields[key])}")
    print(" ".join(parts), file=sys.stderr, flush=True)
