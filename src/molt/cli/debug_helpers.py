from __future__ import annotations

import argparse
import importlib
import json
from pathlib import Path
from typing import Any

from molt.debug import (
    render_debug_json_summary,
    render_debug_text_summary,
    write_debug_manifest,
)
from molt.debug.reduce import normalize_failure_oracle


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _atomic_write_text(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._atomic_write_text(*args, **kwargs)


def _emit_debug_payload(
    *,
    payload: dict[str, Any],
    format_name: str,
    retained_output: Path | None,
    rendered_text: str | None = None,
) -> int:
    write_debug_manifest(Path(payload["manifest_path"]), payload)
    if format_name == "json":
        summary = render_debug_json_summary(payload)
    else:
        summary = (
            rendered_text
            if rendered_text is not None
            else render_debug_text_summary(payload)
        )
    if retained_output is not None:
        _atomic_write_text(retained_output, summary)
    print(summary, end="")
    return 0


def _load_debug_oracle(args: argparse.Namespace) -> dict[str, Any]:
    oracle_json = getattr(args, "oracle_json", None)
    oracle_file = getattr(args, "oracle_file", None)
    if oracle_json and oracle_file:
        raise ValueError("use --oracle-json or --oracle-file, not both")
    if oracle_file:
        oracle_payload = json.loads(Path(oracle_file).read_text(encoding="utf-8"))
    elif oracle_json:
        oracle_payload = json.loads(oracle_json)
    else:
        raise ValueError("missing oracle; use --oracle-json or --oracle-file")
    return normalize_failure_oracle(oracle_payload)
