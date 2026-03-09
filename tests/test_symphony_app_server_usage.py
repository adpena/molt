from __future__ import annotations

from pathlib import Path
from threading import Event
import sys

import pytest

from molt.symphony.app_server import (
    CodexAppServerClient,
    _build_request_user_input_result,
    _extract_notification_details,
    _extract_usage,
    _resolve_launch_command,
)
from molt.symphony.errors import TurnInputRequiredError
from molt.symphony.models import CodexConfig


def _codex_config() -> CodexConfig:
    return CodexConfig(
        command="codex --yolo app-server",
        approval_policy={"reject": {}},
        thread_sandbox="workspace-write",
        turn_sandbox_policy={"type": "workspaceWrite"},
        turn_timeout_ms=1000,
        read_timeout_ms=100,
        stall_timeout_ms=100,
    )


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


def test_extract_usage_prefers_total_token_usage_map_over_token_usage() -> None:
    payload = {
        "method": "codex/event/notification",
        "params": {
            "token_usage": {
                "input_tokens": 3,
                "output_tokens": 4,
                "total_tokens": 7,
            },
            "total_token_usage": {
                "input_tokens": 300,
                "output_tokens": 400,
                "total_tokens": 700,
            },
        },
    }
    usage = _extract_usage(payload)
    assert usage == {
        "input_tokens": 300,
        "output_tokens": 400,
        "total_tokens": 700,
        "delta": False,
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


def test_resolve_launch_command_windows_prefers_cmd_launcher(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "win32")
    monkeypatch.delenv("CODEX_BIN", raising=False)

    def fake_which(name: str) -> str | None:
        mapping = {
            "codex": r"C:\Users\adpen\AppData\Roaming\npm\codex.CMD",
            "codex.cmd": r"C:\Users\adpen\AppData\Roaming\npm\codex.CMD",
        }
        return mapping.get(name)

    monkeypatch.setattr("molt.symphony.app_server.shutil.which", fake_which)
    command = _resolve_launch_command("${CODEX_BIN:-codex} --yolo app-server")
    assert command == [
        r"C:\Users\adpen\AppData\Roaming\npm\codex.CMD",
        "--yolo",
        "app-server",
    ]


def test_resolve_launch_command_windows_expands_codex_bin_env(monkeypatch) -> None:
    monkeypatch.setattr(sys, "platform", "win32")
    monkeypatch.setenv("CODEX_BIN", r"D:\Tools\codex.cmd")
    monkeypatch.setattr(
        "molt.symphony.app_server.shutil.which",
        lambda name: r"D:\Tools\codex.cmd" if name == r"D:\Tools\codex.cmd" else None,
    )
    command = _resolve_launch_command("${CODEX_BIN:-codex} --yolo app-server")
    assert command == [r"D:\Tools\codex.cmd", "--yolo", "app-server"]


def test_resolve_launch_command_windows_expands_generic_env_defaults(
    monkeypatch,
) -> None:
    monkeypatch.setattr(sys, "platform", "win32")
    monkeypatch.setenv("CODEX_BIN", r"D:\Tools\codex.cmd")
    monkeypatch.setenv(
        "MOLT_SYMPHONY_CODEX_ARGS",
        "-c model_reasoning_effort=low -c model_reasoning_summary=none",
    )

    def fake_which(name: str) -> str | None:
        mapping = {
            r"D:\Tools\codex.cmd": r"D:\Tools\codex.cmd",
        }
        return mapping.get(name)

    monkeypatch.setattr("molt.symphony.app_server.shutil.which", fake_which)
    command = _resolve_launch_command(
        (
            "${CODEX_BIN:-codex} --yolo "
            "${MOLT_SYMPHONY_CODEX_ARGS:--c model_reasoning_effort=medium} app-server"
        )
    )
    assert command == [
        r"D:\Tools\codex.cmd",
        "--yolo",
        "-c",
        "model_reasoning_effort=low",
        "-c",
        "model_reasoning_summary=none",
        "app-server",
    ]


def test_build_request_user_input_result_includes_question_ids() -> None:
    payload = {
        "method": "item/tool/requestUserInput",
        "params": {
            "questions": [
                {"id": "confirm_path", "question": "Proceed?"},
                {"id": "branch_choice", "question": "Which branch?"},
            ]
        },
    }
    assert _build_request_user_input_result(payload) == {
        "answers": {
            "confirm_path": {"answers": []},
            "branch_choice": {"answers": []},
        }
    }


def test_handle_server_request_request_user_input_uses_answers_schema() -> None:
    sent: list[dict[str, object]] = []
    events: list[dict[str, object]] = []
    client = CodexAppServerClient(
        codex_config=_codex_config(),
        workspace_path=Path("."),
        stop_event=Event(),
        event_callback=events.append,
    )
    client._send_json = sent.append  # type: ignore[method-assign]

    message = {
        "id": 42,
        "method": "item/tool/requestUserInput",
        "params": {
            "threadId": "thr_123",
            "turnId": "turn_456",
            "itemId": "call1",
            "questions": [{"id": "confirm_path", "question": "Proceed?"}],
        },
    }

    with pytest.raises(TurnInputRequiredError):
        client._handle_server_request(message)

    assert sent == [
        {
            "id": 42,
            "result": {
                "answers": {
                    "confirm_path": {"answers": []},
                }
            },
        }
    ]
    assert events
    assert events[-1]["event"] == "request_user_input_required"


def test_start_uses_utf8_text_mode_for_windows_safe_stream_decode(monkeypatch) -> None:
    popen_kwargs: dict[str, object] = {}

    class _DummyProc:
        pid = 12345
        stdin = None
        stdout = None
        stderr = None

        def poll(self) -> int | None:
            return None

    def fake_popen(*args, **kwargs):  # type: ignore[no-untyped-def]
        _ = args
        popen_kwargs.update(kwargs)
        return _DummyProc()

    monkeypatch.setattr("molt.symphony.app_server.subprocess.Popen", fake_popen)

    client = CodexAppServerClient(
        codex_config=_codex_config(),
        workspace_path=Path("."),
        stop_event=Event(),
        event_callback=lambda _event: None,
    )

    request_ids = iter((1, 2))
    monkeypatch.setattr(
        client, "_send_request", lambda *_args, **_kwargs: next(request_ids)
    )
    monkeypatch.setattr(
        client,
        "_wait_for_response",
        lambda *_args, **_kwargs: {"result": {"thread": {"id": "thr_123"}}},
    )
    monkeypatch.setattr(client, "_send_notification", lambda *_args, **_kwargs: None)

    client.start()

    assert popen_kwargs["text"] is True
    assert popen_kwargs["encoding"] == "utf-8"
    assert popen_kwargs["errors"] == "replace"
