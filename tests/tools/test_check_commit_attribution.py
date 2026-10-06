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
    assert policy.main(["--repo", str(repo), "--introduced"]) == 1

    monkeypatch.setenv(
        "GITHUB_EVENT_PATH", str(_event(tmp_path, {"before": after, "after": after}))
    )
    assert policy.main(["--repo", str(repo), "--introduced"]) == 0


def test_github_new_branch_push_checks_commits_absent_from_remotes(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    head = _commit(repo, "a", f"Attributed\n\n{TRAILER}")
    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    monkeypatch.setenv(
        "GITHUB_EVENT_PATH", str(_event(tmp_path, {"before": "0" * 40, "after": head}))
    )
    assert policy.main(["--repo", str(repo), "--introduced"]) == 1


def _ci_clone(origin: Path, tmp_path: Path) -> Path:
    """Clone `origin` the way a CI checkout does: every branch is fetched."""
    clone = tmp_path / "ci-clone"
    run_guarded_test_process(
        ["git", "clone", "-q", str(origin), str(clone)],
        check=True,
        capture_output=True,
        text=True,
    )
    return clone


def test_github_new_branch_push_checks_commits_even_when_ci_fetched_the_branch(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _commit(repo, "a", "Base")
    _git(repo, "checkout", "-q", "-b", "feature")
    head = _commit(repo, "b", f"Attributed\n\n{TRAILER}")
    _git(repo, "checkout", "-q", "main")
    clone = _ci_clone(repo, tmp_path)
    assert _git(clone, "rev-parse", "origin/feature") == head

    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    monkeypatch.setenv(
        "GITHUB_EVENT_PATH",
        str(
            _event(
                tmp_path,
                {"ref": "refs/heads/feature", "before": "0" * 40, "after": head},
            )
        ),
    )
    assert policy.main(["--repo", str(clone), "--introduced"]) == 1


def test_github_forced_push_checks_commits_when_the_old_tip_is_gone(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _commit(repo, "a", "Base")
    head = _commit(repo, "b", f"Rewritten\n\n{TRAILER}")
    clone = _ci_clone(repo, tmp_path)
    # A history rewrite force-pushed `main`; the clone never had the old tip,
    # and `origin/HEAD` points at the pushed branch.
    vanished = "1234567" * 5 + "89abc"
    assert _git(clone, "rev-parse", "origin/HEAD") == head

    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    monkeypatch.setenv(
        "GITHUB_EVENT_PATH",
        str(
            _event(
                tmp_path,
                {"ref": "refs/heads/main", "before": vanished, "after": head},
            )
        ),
    )
    assert policy.main(["--repo", str(clone), "--introduced"]) == 1


def test_github_new_branch_push_skips_commits_already_on_other_branches(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _commit(repo, "a", f"Old attributed history\n\n{TRAILER}")
    _git(repo, "checkout", "-q", "-b", "feature")
    head = _commit(repo, "b", "Clean")
    _git(repo, "checkout", "-q", "main")
    clone = _ci_clone(repo, tmp_path)

    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    monkeypatch.setenv(
        "GITHUB_EVENT_PATH",
        str(
            _event(
                tmp_path,
                {"ref": "refs/heads/feature", "before": "0" * 40, "after": head},
            )
        ),
    )
    assert policy.main(["--repo", str(clone), "--introduced"]) == 0


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
    assert policy.main(["--repo", str(repo), "--introduced"]) == 1


def _without_github_event(monkeypatch: pytest.MonkeyPatch) -> None:
    # The suite itself may run inside GitHub Actions.
    monkeypatch.delenv("GITHUB_EVENT_NAME", raising=False)
    monkeypatch.delenv("GITHUB_EVENT_PATH", raising=False)


def test_local_introduced_mode_checks_only_commits_no_remote_has(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _without_github_event(monkeypatch)
    _commit(repo, "a", f"Already published\n\n{TRAILER}")
    clone = _ci_clone(repo, tmp_path)
    _git(clone, "config", "user.email", "dev@example.com")
    _git(clone, "config", "user.name", "Dev")
    _git(clone, "config", "commit.gpgsign", "false")

    _commit(clone, "b", "Local clean change")
    assert policy.main(["--repo", str(clone), "--introduced"]) == 0
    _commit(clone, "c", f"Local attributed change\n\n{TRAILER}")
    assert policy.main(["--repo", str(clone), "--introduced"]) == 1


def test_introduced_mode_rejects_half_a_github_event(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _without_github_event(monkeypatch)
    _commit(repo, "a", "Base")
    monkeypatch.setenv("GITHUB_EVENT_NAME", "push")
    with pytest.raises(SystemExit, match="must both be set"):
        policy.main(["--repo", str(repo), "--introduced"])


@pytest.mark.parametrize("mode", ["--introduced", "--all"])
def test_history_modes_fail_closed_in_a_shallow_clone(
    repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    _without_github_event(monkeypatch)
    _commit(repo, "a", f"Hidden below the shallow boundary\n\n{TRAILER}")
    _commit(repo, "b", "Clean tip")
    shallow = tmp_path / "shallow"
    run_guarded_test_process(
        ["git", "clone", "-q", "--depth=1", repo.as_uri(), str(shallow)],
        check=True,
        capture_output=True,
        text=True,
    )
    with pytest.raises(SystemExit, match="clone is shallow"):
        policy.main(["--repo", str(shallow), mode])


def test_all_mode_audits_commits_reachable_only_from_a_tag(repo: Path) -> None:
    _commit(repo, "a", "Base")
    _git(repo, "checkout", "-q", "--detach")
    _commit(repo, "b", f"Tagged only\n\n{TRAILER}")
    _git(repo, "tag", "archive/only-here")
    _git(repo, "checkout", "-q", "main")

    assert policy.main(["--repo", str(repo), "--range", "main"]) == 0
    assert policy.main(["--repo", str(repo), "--all"]) == 1
