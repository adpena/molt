from __future__ import annotations

import json
import sys
from typing import Any, TextIO

from molt.cli import progress


JSON_SCHEMA_VERSION = "1.0"
CliFailure = int


def success(message: str, *, file: TextIO | None = None) -> None:
    """Emit human success text without crossing JSON or quiet presentation."""
    progress.finish()
    if progress.success_is_visible():
        print(message, file=file)


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


def read_json_payload(stdout: str) -> dict[str, Any]:
    """The payload a ``--json`` command printed: its last payload line.

    Raises ``ValueError`` when no line of ``stdout`` is a payload of this
    schema version.
    """
    for line in reversed(stdout.splitlines()):
        text = line.strip()
        if not text.startswith("{"):
            continue
        try:
            payload = json.loads(text)
        except json.JSONDecodeError:
            continue
        if (
            isinstance(payload, dict)
            and payload.get("schema_version") == JSON_SCHEMA_VERSION
        ):
            return payload
    raise ValueError("the command printed no molt JSON payload")


def json_command_failure(stdout: str, stderr: str, *, limit: int = 2000) -> str:
    """Why a ``--json`` command failed, in at most ``limit`` characters.

    The payload's ``errors`` are the CLI's own failure messages. Without a
    payload the failure is at the end of stderr (else stdout); the start holds
    banners such as the sccache notice.
    """
    try:
        errors = read_json_payload(stdout).get("errors")
    except ValueError:
        errors = None
    if isinstance(errors, list) and errors:
        detail = "; ".join(str(error) for error in errors)
    else:
        detail = stderr.strip() or stdout.strip() or "(no output)"
    return detail if len(detail) <= limit else "..." + detail[-limit:]


def build_output_path(payload: dict[str, Any]) -> str:
    """The artifact a successful ``molt build --json`` wrote: ``data.output``.

    Every emit mode names its artifact there (``--emit obj`` names the object).
    """
    if payload.get("status") != "ok":
        raise ValueError(f"build reported status {payload.get('status')!r}")
    data = payload.get("data")
    output = data.get("output") if isinstance(data, dict) else None
    if not isinstance(output, str) or not output:
        raise ValueError("the build payload names no output path")
    return output


def fail(
    message: str,
    json_output: bool,
    code: int = 2,
    command: str = "molt",
    *,
    data: dict[str, Any] | None = None,
) -> int:
    progress.finish()
    if json_output:
        failure_data = dict(data or {})
        # The process return code is owned by this failure authority; structured
        # diagnostic extensions may not contradict it.
        failure_data["returncode"] = code
        payload = json_payload(
            command,
            "error",
            data=failure_data,
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
