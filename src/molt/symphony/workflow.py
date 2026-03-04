from __future__ import annotations

from collections.abc import Mapping
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import yaml

from .errors import (
    MissingWorkflowFileError,
    WorkflowFrontMatterNotMapError,
    WorkflowParseError,
)
from .models import WorkflowDefinition


FRONT_MATTER_DELIM = "---"


def discover_workflow_path(explicit_path: str | None) -> Path:
    if explicit_path:
        return Path(explicit_path).expanduser()
    return Path.cwd() / "WORKFLOW.md"


def load_workflow(path: Path) -> WorkflowDefinition:
    if not path.exists():
        raise MissingWorkflowFileError(f"missing_workflow_file path={path}")
    try:
        content = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise MissingWorkflowFileError(
            f"missing_workflow_file path={path} error={exc}"
        ) from exc

    config, prompt = _parse_front_matter(content)
    stat = path.stat()
    return WorkflowDefinition(
        path=path,
        config=config,
        prompt_template=prompt.strip(),
        loaded_at=datetime.now(UTC),
        mtime_ns=stat.st_mtime_ns,
    )


def maybe_reload_workflow(previous: WorkflowDefinition) -> WorkflowDefinition | None:
    try:
        stat = previous.path.stat()
    except OSError:
        return None
    if stat.st_mtime_ns == previous.mtime_ns:
        return None
    return load_workflow(previous.path)


def _parse_front_matter(content: str) -> tuple[dict[str, Any], str]:
    lines = content.splitlines()
    if not lines:
        return {}, ""

    if lines[0].strip() != FRONT_MATTER_DELIM:
        return {}, content

    closing_index = None
    for index in range(1, len(lines)):
        if lines[index].strip() == FRONT_MATTER_DELIM:
            closing_index = index
            break
    if closing_index is None:
        raise WorkflowParseError(
            "workflow_parse_error missing closing front matter delimiter"
        )

    yaml_text = "\n".join(lines[1:closing_index])
    prompt_body = "\n".join(lines[closing_index + 1 :])

    try:
        parsed = yaml.safe_load(yaml_text) if yaml_text.strip() else {}
    except yaml.YAMLError as exc:
        raise WorkflowParseError(f"workflow_parse_error {exc}") from exc

    if parsed is None:
        parsed = {}
    if not isinstance(parsed, Mapping):
        raise WorkflowFrontMatterNotMapError("workflow_front_matter_not_a_map")

    return dict(parsed), prompt_body
