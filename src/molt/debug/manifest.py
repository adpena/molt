from __future__ import annotations

import json
import os
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from secrets import token_hex
from typing import Any, cast

from .contracts import DebugSubcommand


@dataclass(frozen=True)
class DebugPaths:
    run_id: str
    artifact_root: Path
    manifest_path: Path
    retained_output: Path | None


def canonical_debug_root(*, retained: bool) -> Path:
    base = Path(os.getcwd()) / ("logs" if retained else "tmp")
    return base / "debug"


def new_debug_run_id(now: datetime | None = None) -> str:
    instant = now if now is not None else datetime.now(UTC)
    return f"{instant.strftime('%Y%m%dT%H%M%SZ')}-{token_hex(6)}"


def allocate_debug_paths(
    subcommand: DebugSubcommand | str,
    *,
    out: Path | str | None = None,
    output_extension: str | None = None,
    run_id: str | None = None,
) -> DebugPaths:
    debug_subcommand = DebugSubcommand(subcommand)
    retained = out is not None
    current_run_id = run_id or new_debug_run_id()
    artifact_root = (
        canonical_debug_root(retained=retained)
        / debug_subcommand.value
        / current_run_id
    )
    manifest_path = artifact_root / "manifest.json"
    retained_output: Path | None = None
    if retained:
        requested = Path(out) if out is not None else Path("summary")
        suffix = requested.suffix
        if not suffix and output_extension:
            normalized_extension = output_extension.removeprefix(".")
            suffix = f".{normalized_extension}"
        filename = requested.name or f"{debug_subcommand.value}{suffix or ''}"
        if suffix and not filename.endswith(suffix):
            filename = f"{filename}{suffix}"
        retained_output = artifact_root / filename
    return DebugPaths(
        run_id=current_run_id,
        artifact_root=artifact_root,
        manifest_path=manifest_path,
        retained_output=retained_output,
    )


def write_debug_manifest(path: Path, payload: dict[str, object]) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return path


def render_debug_json_summary(payload: dict[str, object]) -> str:
    return json.dumps(payload, indent=2, sort_keys=True) + "\n"


def render_debug_text_summary(payload: dict[str, object]) -> str:
    lines = [
        "Molt Debug Summary",
        f"Subcommand: {payload.get('subcommand', 'unknown')}",
        f"Status: {payload.get('status', 'unknown')}",
        f"Run ID: {payload.get('run_id', 'unknown')}",
        f"Manifest: {payload.get('manifest_path', '')}",
    ]
    message = payload.get("message")
    if isinstance(message, str) and message:
        lines.append(f"Message: {message}")
    retained_output = None
    artifacts = payload.get("artifacts")
    if isinstance(artifacts, dict):
        retained_output = cast(dict[str, Any], artifacts).get("retained_output")
    if isinstance(retained_output, str) and retained_output:
        lines.append(f"Retained Output: {retained_output}")
    return "\n".join(lines) + "\n"
