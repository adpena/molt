from __future__ import annotations

import json
import sys
from typing import Any


JSON_SCHEMA_VERSION = "1.0"
CliFailure = int


def emit_json(payload: dict[str, Any], json_output: bool) -> None:
    if json_output:
        print(json.dumps(payload))


def json_payload(
    command: str,
    status: str,
    *,
    data: dict[str, Any] | None = None,
    warnings: list[str] | None = None,
    errors: list[str] | None = None,
) -> dict[str, Any]:
    return {
        "schema_version": JSON_SCHEMA_VERSION,
        "command": command,
        "status": status,
        "data": data or {},
        "warnings": warnings or [],
        "errors": errors or [],
    }


def fail(
    message: str,
    json_output: bool,
    code: int = 2,
    command: str = "molt",
) -> int:
    if json_output:
        payload = json_payload(
            command,
            "error",
            data={"returncode": code},
            errors=[message],
        )
        emit_json(payload, json_output=True)
    else:
        print(message, file=sys.stderr)
    return code


def coerce_process_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


subprocess_output_text = coerce_process_text
