from __future__ import annotations

from tools import apparatus_agent_safety as safety


def test_teeth_detects_real_process_actuation_calls():
    violations = safety.scan_source(
        "import os\nimport subprocess\nos.kill(1, 9)\nsubprocess.Popen(['x'])\n"
    )
    assert {violation.symbol for violation in violations} == {"kill", "Popen"}


def test_read_only_subprocess_observation_is_allowed():
    assert not safety.scan_source(
        "import subprocess\nsubprocess.run(['git', 'status'], timeout=1)\n"
    )


def test_existing_hook_spine_has_no_process_termination_capability():
    assert not safety.scan_tree()


def test_self_test_is_live():
    assert safety.self_test()


def test_internal_scanner_error_fails_open(monkeypatch):
    monkeypatch.setattr(
        safety, "scan_tree", lambda: (_ for _ in ()).throw(RuntimeError("boom"))
    )
    assert safety.main([]) == 0
