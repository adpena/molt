from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

from tools import forbidden_checkout_guard as guard
from tools.hooks import path_guard


def test_teeth_refuses_write_under_retired_checkout():
    decision = guard.decide("Write", {"file_path": guard.FORBIDDEN_ROOT + r"\src\x.py"})
    assert decision.block and decision.rule == "forbidden-checkout-mutation"


def test_allows_write_under_canonical_checkout():
    assert not guard.decide("Write", {"file_path": r"C:\Molt\molt-src\src\x.py"}).block


def test_teeth_refuses_build_when_cwd_reset_to_retired_checkout():
    assert guard.decide("Bash", {"command": "cargo check"}, guard.FORBIDDEN_ROOT).block


def test_reads_under_retired_checkout_remain_allowed():
    assert not guard.decide(
        "Read", {"file_path": guard.FORBIDDEN_ROOT + r"\README.md"}
    ).block


def test_classifier_error_fails_open(monkeypatch):
    monkeypatch.setattr(
        guard, "_tool_paths", lambda value: (_ for _ in ()).throw(RuntimeError("boom"))
    )
    assert not guard.decide("Write", {"file_path": guard.FORBIDDEN_ROOT + r"\x"}).block


def test_hook_teeth_returns_blocking_exit(monkeypatch):
    monkeypatch.setattr(
        sys,
        "stdin",
        io.StringIO(
            json.dumps(
                {
                    "tool_name": "Write",
                    "tool_input": {"file_path": guard.FORBIDDEN_ROOT + r"\x"},
                }
            )
        ),
    )
    assert path_guard.run() == 2


def test_hook_wrapper_error_fails_open(monkeypatch, tmp_path):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    monkeypatch.setattr(
        path_guard, "run", lambda: (_ for _ in ()).throw(RuntimeError("boom"))
    )
    with pytest.raises(SystemExit) as exc:
        path_guard.main()
    assert exc.value.code == 0


def test_settings_wire_path_guard_before_mutations():
    settings = json.loads(
        (Path(__file__).resolve().parents[2] / ".claude" / "settings.json").read_text(
            encoding="utf-8"
        )
    )
    hooks = settings["hooks"]["PreToolUse"]
    assert any(
        "path_guard.py" in item["hooks"][0]["command"] and "Write" in item["matcher"]
        for item in hooks
    )
