from __future__ import annotations

import os
import sys
import uuid
from pathlib import Path
from typing import Mapping, Sequence

from molt.cli.artifact_state import _build_state_root
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.diagnostic_text import strip_terminal_decoration
from molt.cli.models import _RuntimeArtifactState, _RuntimeWasmBuildFailure

_SUMMARY_LIMIT = 4096
_EVIDENCE_TEXT_LIMIT = 128 * 1024


def _bounded_failure_text(value: str) -> str:
    text = strip_terminal_decoration(value).strip()
    if len(text) <= _EVIDENCE_TEXT_LIMIT:
        return text
    half = _EVIDENCE_TEXT_LIMIT // 2
    return f"{text[:half]}\n... <truncated> ...\n{text[-half:]}"


def record_runtime_wasm_failure(
    runtime_state: _RuntimeArtifactState,
    *,
    project_root: Path,
    stage: str,
    summary: str,
    command: Sequence[str] = (),
    stdout: str = "",
    stderr: str = "",
    returncode: int | None = None,
    timed_out: bool = False,
    details: Mapping[str, object] | None = None,
) -> bool:
    """Publish one exact WASM failure authority without corrupting JSON stdout."""

    detail = _bounded_failure_text(stderr or stdout)
    compact = strip_terminal_decoration(summary).strip()
    if detail and detail not in compact:
        compact = f"{compact}: {detail}"
    compact = compact[:_SUMMARY_LIMIT]
    evidence_path: Path | None = None
    try:
        evidence_path = (
            _build_state_root(project_root)
            / "build_failures"
            / f"runtime-wasm-{stage}-{os.getpid()}-{uuid.uuid4().hex}.json"
        )
        _atomic_write_json(
            evidence_path,
            {
                "schema": "molt.runtime-wasm-build-failure.v1",
                "stage": stage,
                "summary": compact,
                "command": list(command),
                "cwd": str(project_root),
                "returncode": returncode,
                "timed_out": timed_out,
                "details": dict(details or {}),
                "stdout": _bounded_failure_text(stdout),
                "stderr": _bounded_failure_text(stderr),
            },
            indent=2,
            sort_keys=True,
        )
    except OSError:
        evidence_path = None
    runtime_state.runtime_wasm_build_failure = _RuntimeWasmBuildFailure(
        stage=stage,
        summary=compact,
        evidence_path=evidence_path,
        returncode=returncode,
        timed_out=timed_out,
        details=dict(details or {}),
    )
    print(compact, file=sys.stderr)
    if evidence_path is not None:
        print(f"Runtime WASM failure evidence: {evidence_path}", file=sys.stderr)
    return False
