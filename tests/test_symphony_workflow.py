from __future__ import annotations

from pathlib import Path

import pytest

from molt.symphony.errors import WorkflowFrontMatterNotMapError, WorkflowParseError
from molt.symphony.workflow import load_workflow


def test_load_workflow_with_front_matter(tmp_path: Path) -> None:
    workflow = tmp_path / "WORKFLOW.md"
    workflow.write_text(
        """---
tracker:
  kind: linear
  project_slug: proj
---
Hello {{ issue.identifier }}
""",
        encoding="utf-8",
    )

    loaded = load_workflow(workflow)
    assert loaded.config["tracker"]["kind"] == "linear"
    assert loaded.prompt_template == "Hello {{ issue.identifier }}"


def test_workflow_parse_error_on_unclosed_front_matter(tmp_path: Path) -> None:
    workflow = tmp_path / "WORKFLOW.md"
    workflow.write_text("---\ntracker: {}\n", encoding="utf-8")

    with pytest.raises(WorkflowParseError):
        load_workflow(workflow)


def test_workflow_front_matter_requires_mapping(tmp_path: Path) -> None:
    workflow = tmp_path / "WORKFLOW.md"
    workflow.write_text("---\n- a\n- b\n---\nbody\n", encoding="utf-8")

    with pytest.raises(WorkflowFrontMatterNotMapError):
        load_workflow(workflow)
