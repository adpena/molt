from __future__ import annotations

import sys
from pathlib import Path

from molt.symphony.loop_hooks import LoopHookRunner, decode_hook_decision


def test_decode_hook_decision_normalizes_values() -> None:
    decision = decode_hook_decision(
        {"action": "replace", "reason": "policy", "command": ["echo", "ok"]}
    )
    assert decision.action == "replace"
    assert decision.reason == "policy"
    assert decision.command == ["echo", "ok"]


def test_loop_hook_runner_returns_replace_decision(tmp_path: Path) -> None:
    hook_script = tmp_path / "hook.py"
    hook_script.write_text(
        (
            "import json,sys\n"
            "request=json.loads(sys.stdin.read())\n"
            "payload=request.get('payload') or {}\n"
            "name=payload.get('name','')\n"
            "print(json.dumps({'action':'replace','reason':name,'command':['echo','hooked']}))\n"
        ),
        encoding="utf-8",
    )
    runner = LoopHookRunner([sys.executable, str(hook_script)])
    decision = runner.run(
        event="before_step",
        payload={"name": "readiness_audit"},
        cwd=tmp_path,
        env={},
    )
    assert decision.action == "replace"
    assert decision.command == ["echo", "hooked"]
    assert decision.reason == "readiness_audit"
