"""tools/check_typed_facts.py: new untyped facts block; old lines do not.

The oracle is a real Git repository and the real ruff: a plausible wrong
checker either flags the whole file (blocking the backlog it must leave alone)
or misses a new unannotated signature.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import check_typed_facts
from tools.check_typed_facts import Severity

OLD = "def old(value):\n    return value\n"


def _git(root: Path, *args: str) -> None:
    run_guarded_test_process(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
    )


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / "tools").mkdir(parents=True)
    (root / "pyproject.toml").write_text(
        '[tool.ty.src]\nexclude = ["vendored/**"]\n', encoding="utf-8"
    )
    (root / "tools" / "generator_manifest.toml").write_text(
        '[[generator]]\noutputs = ["generated.py"]\n', encoding="utf-8"
    )
    (root / "lib.py").write_text(OLD, encoding="utf-8")
    _git(root, "init", "-q", "-b", "main")
    _git(root, "add", "-A")
    _git(
        root,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "-m",
        "base",
    )
    return root


def test_a_new_untyped_signature_blocks_and_the_old_backlog_does_not(
    repo: Path,
) -> None:
    (repo / "lib.py").write_text(
        OLD
        + "\n\ndef new(value):\n    return value\n\n\ndef typed(value: int) -> int:\n"
        "    return value\n",
        encoding="utf-8",
    )
    findings = check_typed_facts.check(repo, "HEAD", (Path("lib.py"),))
    blocking = [f for f in findings if f.severity is Severity.BLOCKING]
    assert {(f.line, f.rule) for f in blocking} == {(5, "ANN001"), (5, "ANN201")}


def test_an_identity_shaped_str_is_advisory_and_other_strings_are_not(
    repo: Path,
) -> None:
    (repo / "lib.py").write_text(
        OLD
        + "\n\nclass Row:\n    kind: str\n    reason: str\n    surface_kind: str | None\n",
        encoding="utf-8",
    )
    findings = check_typed_facts.check(repo, "HEAD", (Path("lib.py"),))
    advisory = {(f.line, f.rule) for f in findings if f.severity is Severity.ADVISORY}
    assert advisory == {(6, "TYPED-IDENTITY"), (8, "TYPED-IDENTITY")}


def test_excluded_and_generated_files_are_not_checked(repo: Path) -> None:
    (repo / "vendored").mkdir()
    for path in ("vendored/mirror.py", "generated.py", "lib.py"):
        (repo / path).write_text("def f(x):\n    return x\n", encoding="utf-8")
    assert check_typed_facts.checked_python_paths(
        repo, ["vendored/mirror.py", "generated.py", "lib.py", "notes.md"]
    ) == (Path("lib.py"),)


def test_the_hook_reports_the_edited_files_new_findings(repo: Path) -> None:
    (repo / "lib.py").write_text(
        OLD + "\n\ndef new(value):\n    pass\n", encoding="utf-8"
    )
    event = {"tool_name": "Edit", "tool_input": {"file_path": str(repo / "lib.py")}}
    context = check_typed_facts.hook_context(repo, event)
    assert context is not None
    assert "lib.py:5: ANN001" in context
    assert (
        check_typed_facts.hook_context(repo, {"tool_input": {"file_path": "x.md"}})
        is None
    )
    json.dumps({"additionalContext": context})  # serializable as hook output
