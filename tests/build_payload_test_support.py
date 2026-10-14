"""Stdout lines that fake ``molt build --json`` exactly as the CLI prints them."""

from __future__ import annotations

import json

from molt.cli.output import json_payload


def build_payload_line(output: object, *, status: str = "ok") -> str:
    """The payload line of a build that names ``output`` as its artifact."""
    return json.dumps(json_payload("build", status, data={"output": output}))


# Lines a completed build might print that name no usable artifact: not a
# payload at all, the pre-envelope shape, and payloads without a string output
# or with a failed status.
MALFORMED_BUILD_OUTPUTS = (
    "[]",
    "null",
    json.dumps({"output": "app"}),
    build_payload_line(23),
    json.dumps(json_payload("build", "ok", data={})),
    build_payload_line("app", status="error"),
)
