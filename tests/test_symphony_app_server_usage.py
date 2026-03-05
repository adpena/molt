from __future__ import annotations

from molt.symphony.app_server import _extract_notification_details, _extract_usage


def test_extract_usage_from_explicit_payload() -> None:
    payload = {
        "method": "codex/event/turn/completed",
        "params": {
            "usage": {
                "input_tokens": 120,
                "output_tokens": 30,
                "total_tokens": 150,
            }
        },
    }
    usage = _extract_usage(payload)
    assert usage == {
        "input_tokens": 120,
        "output_tokens": 30,
        "total_tokens": 150,
        "delta": False,
    }


def test_extract_usage_from_nested_token_count_delta() -> None:
    payload = {
        "method": "codex/event/token_count",
        "params": {
            "event": {
                "metrics": {
                    "tokenCountDelta": 42,
                    "inputTokensDelta": 24,
                    "outputTokensDelta": 18,
                }
            }
        },
    }
    usage = _extract_usage(payload)
    assert usage == {
        "input_tokens": 24,
        "output_tokens": 18,
        "total_tokens": 42,
        "delta": True,
    }


def test_extract_usage_from_nested_total_token_count() -> None:
    payload = {
        "method": "codex/event/token_count",
        "params": {
            "stats": {
                "totalTokenCount": 999,
            }
        },
    }
    usage = _extract_usage(payload)
    assert usage == {
        "input_tokens": 0,
        "output_tokens": 0,
        "total_tokens": 999,
        "delta": False,
    }


def test_extract_usage_prefers_delta_for_token_count_events() -> None:
    payload = {
        "method": "codex/event/token_count",
        "params": {
            "event": {
                "metrics": {
                    "totalTokenCount": 1200,
                    "inputTokenCount": 700,
                    "outputTokenCount": 500,
                    "tokenCountDelta": 42,
                    "inputTokensDelta": 24,
                    "outputTokensDelta": 18,
                }
            }
        },
    }
    usage = _extract_usage(payload)
    assert usage == {
        "input_tokens": 24,
        "output_tokens": 18,
        "total_tokens": 42,
        "delta": True,
    }


def test_extract_usage_ignores_unrelated_token_strings() -> None:
    payload = {
        "method": "codex/event/notification",
        "params": {
            "message": {
                "tokenizer": "noop",
                "token_hint": "metadata only",
            }
        },
    }
    usage = _extract_usage(payload)
    assert usage is None


def test_extract_notification_details_ignores_uuidish_text() -> None:
    payload = {
        "method": "codex/event/agent_message_delta",
        "params": {
            "message": {"id": "019cbb3d-1024-79d1-a2f5-7a83cb0e9e75"},
            "content": {"text": "Implemented parser and tests for throughput bug."},
        },
    }
    details = _extract_notification_details(payload)
    assert details is not None
    assert (
        details.get("text_preview")
        == "Implemented parser and tests for throughput bug."
    )
