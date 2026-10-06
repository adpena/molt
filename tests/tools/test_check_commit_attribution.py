"""The commit-attribution policy: no commit is attributed to Claude/Anthropic."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import check_commit_attribution as policy

TRAILER = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"


def _git(repo: Path, *args: str) -> str:
    return run_guarded_test_process(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


def _commit(repo: Path, name: str, message: str) -> str:
    (repo / name).write_text(name, encoding="utf-8")
    _git(repo, "add", "--", name)
    _git(repo, "commit", "-q", "-m", message)
    return _git(repo, "rev-parse", "HEAD")


@pytest.fixture()
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    root.mkdir()
    _git(root, "init", "-q", "-b", "main")
    _git(root, "config", "user.email", "dev@example.com")
    _git(root, "config", "user.name", "Dev")
    _git(root, "config", "commit.gpgsign", "false")
    return root


@pytest.mark.parametrize(
    "line",
    [
        TRAILER,
        "co-authored-by: Claude Fable 5.1 <noreply@anthropic.com>",
        "Co-Authored-By: someone <noreply@anthropic.com>",
        "🤖 Generated with [Claude Code](https://claude.com/claude-code)",
        "Generated with Claude Code",
    ],
)
def test_attribution_lines_are_detected(line: str) -> None:
    assert policy.attribution_lines(f"Fix the parser\n\n{line}\n") == [line.strip()]


@pytest.mark.parametrize(
    "message",
    [
        "Fix the parser\n\nCo-Authored-By: Ada Lovelace <ada@example.com>\n",
        "Document the Claude-to-CPython parity notes\n",
        "Fix the parser\n# Co-Authored-By: Claude <noreply@anthropic.com>\n",
    ],
)
def test_ordinary_messages_and_comment_lines_pass(message: str) -> None:
    assert policy.attribution_lines(message) == []


def test_message_file_mode_fails_closed_on_attribution(tmp_path: Path) -> None:
    clean = tmp_path / "clean"
    clean.write_text("Fix the parser\n", encoding="utf-8")
    dirty = tmp_path / "dirty"
    dirty.write_text(f"Fix the parser\n\n{TRAILER}\n", encoding="utf-8")

    assert policy.main(["--message-file", str(clean)]) == 0
    assert policy.main(["--message-file", str(dirty)]) == 1


def test_range_mode_reports_only_attributed_commits(
    repo: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    base = _commit(repo, "a", "Base")
    clean = _commit(repo, "b", "Clean change")
    dirty = _commit(repo, "c", f"Attributed change\n\n{TRAILER}")

    assert policy.main(["--repo", str(repo), "--range", f"{base}..{clean}"]) == 0
    assert policy.main(["--repo", str(repo), "--range", f"{base}..{dirty}"]) == 1
    assert dirty[:12] in capsys.readouterr().err


def _event(tmp_path: Path, payload: dict[str, object]) -> Path:
    path = tmp_path / "event.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def test_github_push_event_checks_before_to_after(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    before = _commit(repo, "a", "Base")
    _commit(repo, "b", f"Attributed\n\n{TRAILER}")
    after = _commit(repo, "c", "Clean")
    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    monkeypatch.setenv(
        "GITHUB_EVENT_PATH", str(_event(tmp_path, {"before": before, "after": after}))
    )
    assert policy.main(["--repo", str(repo), "--github-event"]) == 1

    monkeypatch.setenv(
        "GITHUB_EVENT_PATH", str(_event(tmp_path, {"before": after, "after": after}))
    )
    assert policy.main(["--repo", str(repo), "--github-event"]) == 0


def test_github_new_branch_push_checks_commits_absent_from_remotes(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    head = _commit(repo, "a", f"Attributed\n\n{TRAILER}")
    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    monkeypatch.setenv(
        "GITHUB_EVENT_PATH", str(_event(tmp_path, {"before": "0" * 40, "after": head}))
    )
    assert policy.main(["--repo", str(repo), "--github-event"]) == 1


@pytest.mark.parametrize("event_name", ["pull_request", "merge_group"])
def test_github_pull_request_and_merge_group_check_base_to_head(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, event_name: str
) -> None:
    base = _commit(repo, "a", "Base")
    head = _commit(repo, "b", f"Attributed\n\n{TRAILER}")
    payload: dict[str, object] = (
        {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
        if event_name == "pull_request"
        else {"merge_group": {"base_sha": base, "head_sha": head}}
    )
    monkeypatch.setenv("GITHUB_EVENT_NAME", event_name)
    monkeypatch.setenv("GITHUB_EVENT_PATH", str(_event(tmp_path, payload)))
    assert policy.main(["--repo", str(repo), "--github-event"]) == 1


def test_all_mode_audits_commits_reachable_only_from_a_tag(repo: Path) -> None:
    _commit(repo, "a", "Base")
    _git(repo, "checkout", "-q", "--detach")
    _commit(repo, "b", f"Tagged only\n\n{TRAILER}")
    _git(repo, "tag", "archive/only-here")
    _git(repo, "checkout", "-q", "main")

    assert policy.main(["--repo", str(repo), "--range", "main"]) == 0
    assert policy.main(["--repo", str(repo), "--all"]) == 1
