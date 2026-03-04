from __future__ import annotations

import pytest

from molt.symphony.errors import TemplateRenderError
from molt.symphony.template import render_prompt


ISSUE = {
    "identifier": "MT-1",
    "title": "Fix parser",
    "state": "Todo",
}


def test_template_renders_known_fields() -> None:
    rendered = render_prompt("Issue {{ issue.identifier }}", issue=ISSUE, attempt=None)
    assert rendered == "Issue MT-1"


def test_template_unknown_variable_fails() -> None:
    with pytest.raises(TemplateRenderError):
        render_prompt("{{ issue.missing }}", issue=ISSUE, attempt=None)


def test_template_unknown_filter_fails() -> None:
    with pytest.raises(TemplateRenderError):
        render_prompt("{{ issue.title | not_real }}", issue=ISSUE, attempt=None)


def test_template_default_filter() -> None:
    rendered = render_prompt(
        'Attempt {{ attempt | default: "first" }}',
        issue=ISSUE,
        attempt=None,
    )
    assert rendered == "Attempt first"
