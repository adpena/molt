from __future__ import annotations

from tools import anti_recurrence_gate as gate


CLAIM = "## Log\n| BUG-CLASS | agent | 2026-07-11T00:00:00Z | COMPLETE | closes recurring bug class |\n"


def test_teeth_warns_when_bug_class_learning_is_not_captured():
    findings = gate.inspect(CLAIM, ["src/molt/x.py"])
    assert findings and "anti-recurrence" in findings[0].message


def test_complete_class_with_teeth_and_lesson_is_clean():
    assert not gate.inspect(CLAIM, ["tests/test_x.py", "docs/agent/x_lesson.md"])


def test_non_bug_class_complete_is_ignored():
    text = CLAIM.replace("closes recurring bug class", "routine cleanup")
    assert not gate.inspect(text, ["src/molt/x.py"])


def test_advisory_cli_self_test_is_live():
    assert gate.self_test()


def test_advisory_cli_read_error_fails_open(tmp_path):
    assert gate.main(["--claims", str(tmp_path / "missing.md")]) == 0
