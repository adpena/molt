from __future__ import annotations

from dataclasses import asdict, dataclass
from enum import StrEnum
from pathlib import Path
import platform
import sys
from typing import Any, Mapping


class DebugSubcommand(StrEnum):
    REPRO = "repro"
    IR = "ir"
    VERIFY = "verify"
    TRACE = "trace"
    REDUCE = "reduce"
    BISECT = "bisect"
    DIFF = "diff"
    PERF = "perf"


class DebugStatus(StrEnum):
    OK = "ok"
    UNSUPPORTED = "unsupported"
    ERROR = "error"


class DebugFailureClass(StrEnum):
    NOT_YET_WIRED = "not_yet_wired"
    INVALID_REQUEST = "invalid_request"
    CAPABILITY_DENIED = "capability_denied"
    INTERNAL_ERROR = "internal_error"


@dataclass(frozen=True)
class DebugCapabilityRecord:
    name: str
    granted: bool
    source: str | None = None
    detail: str | None = None


def _normalize_path(value: Path | str | None) -> str | None:
    if value is None:
        return None
    return str(Path(value))


def normalize_debug_payload(
    *,
    subcommand: DebugSubcommand | str,
    status: DebugStatus | str,
    run_id: str,
    artifact_root: Path | str,
    manifest_path: Path | str,
    selectors: Mapping[str, Any] | None = None,
    failure_class: DebugFailureClass | str | None = None,
    message: str | None = None,
    retained_output: Path | str | None = None,
    capabilities: list[DebugCapabilityRecord] | None = None,
    data: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    normalized_capabilities = [
        asdict(record) for record in (capabilities or [])
    ]
    normalized_selectors = dict(selectors or {})
    dimensions = {
        "python_tag": f"py{sys.version_info.major}{sys.version_info.minor}",
        "host_os": (platform.system() or "unknown").lower(),
        "backend": normalized_selectors.get("backend"),
        "target": normalized_selectors.get("target"),
    }
    return {
        "schema_version": 1,
        "command": "debug",
        "subcommand": str(DebugSubcommand(subcommand)),
        "status": str(DebugStatus(status)),
        "run_id": run_id,
        "artifact_root": _normalize_path(artifact_root),
        "manifest_path": _normalize_path(manifest_path),
        "selectors": normalized_selectors,
        "dimensions": dimensions,
        "failure_class": (
            str(DebugFailureClass(failure_class))
            if failure_class is not None
            else None
        ),
        "message": message,
        "capabilities": normalized_capabilities,
        "artifacts": {
            "manifest": _normalize_path(manifest_path),
            "retained_output": _normalize_path(retained_output),
        },
        "data": dict(data or {}),
    }
