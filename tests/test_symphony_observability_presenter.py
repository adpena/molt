from __future__ import annotations

import json
from pathlib import Path

from molt.symphony.observability_presenter import (
    load_security_events_summary,
    project_state_payload,
)


def test_project_state_payload_redacts_and_injects_http_security() -> None:
    linear_token = "lin_api_" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    openai_token = "sk-" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    github_token = "ghp_" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    slack_token = "xoxb-" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    payload = {
        "generated_at": "2026-03-04T00:00:00Z",
        "runtime": {
            "api_token": linear_token,
            "nested": {
                "authorization": openai_token,
                "message": github_token,
            },
        },
        "rate_limits": {"secret": slack_token},
    }
    projected = project_state_payload(
        payload,
        http_security={"profile": "local", "counters": {"unauthorized": 1}},
    )
    encoded = json.dumps(projected, sort_keys=True)
    assert linear_token not in encoded
    assert openai_token not in encoded
    assert github_token not in encoded
    assert slack_token not in encoded
    assert "<redacted>" in encoded
    assert projected["runtime"]["http_security"]["profile"] == "local"
    assert projected["runtime"]["http_security"]["counters"]["unauthorized"] == 1


def test_load_security_events_summary_counts_recent_rows(tmp_path: Path) -> None:
    events_file = tmp_path / "events.jsonl"
    rows = [
        {"kind": "other", "at": "2026-03-04T01:00:00Z"},
        {"kind": "secret_guard_blocked", "at": "2026-03-04T01:01:00Z"},
        {"kind": "secret_guard_blocked", "at": "2026-03-04T01:03:00Z"},
    ]
    events_file.write_text(
        "not-json\n" + "\n".join(json.dumps(row) for row in rows) + "\n",
        encoding="utf-8",
    )
    summary = load_security_events_summary(events_file, max_lines=100)
    secret_guard = summary["secret_guard_blocked"]
    assert secret_guard["total"] == 2
    assert secret_guard["last_at"] == "2026-03-04T01:03:00Z"
