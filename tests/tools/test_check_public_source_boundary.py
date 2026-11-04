"""The public-source boundary gate (HF-768)."""

from __future__ import annotations

from pathlib import Path
import subprocess

import pytest

from tools import check_public_source_boundary as boundary

REPO = Path(__file__).resolve().parents[2]


@pytest.mark.parametrize(
    "path",
    [
        # The leaks found on 2026-11-04.
        "logs/agents/lane-hf761-linux-identity/artifacts/design.md",
        "node_modules/.mf/cf.json",
        "node_modules/.cache/wrangler/wrangler-account.json",
        "examples/cloudflare-demo/.wrangler/cache/wrangler-account.json",
        "deploy/worker/node_modules/pkg/index.js",
        "deploy/worker/.mf/cf.json",
        "deploy/worker/.dev.vars",
        "deploy/worker/.dev.vars.staging",
        # The other local artifact roots and machine-local files.
        "tmp/scratch.txt",
        "target/debug/molt",
        "build/out.o",
        "dist/molt.whl",
        ".claude/settings.local.json",
        "bench/results/run/stdout.log",
        "bench/scoreboard/latest.log",
        "bench/results/run/memory_guard/outcome.json",
    ],
)
def test_local_artifact_paths_are_refused(path: str) -> None:
    assert boundary.boundary_violations([path]) == [
        boundary.Violation(path, "local artifact root")
    ]


@pytest.mark.parametrize(
    "path", ["artifacts/anatomy.md", "artifacts/design.md", "notes.md", "output.o"]
)
def test_root_entries_outside_the_contract_are_refused(path: str) -> None:
    # HF-685's lane committed artifacts/ at the root on 2026-11-04.
    assert boundary.boundary_violations([path]) == [
        boundary.Violation(path, "root entry outside ROOT_ENTRIES")
    ]


@pytest.mark.parametrize(
    "path",
    [
        "src/molt/__init__.py",
        "README.md",
        ".claude/settings.json",
        "bench/results/summary.json",
        "docs/ROOT_LAYOUT.md",
        "tools/check_public_source_boundary.py",
    ],
)
def test_public_source_is_accepted(path: str) -> None:
    assert boundary.boundary_violations([path]) == []


def test_the_checkout_is_inside_the_boundary() -> None:
    assert boundary.main(["--repo", str(REPO)]) == 0


def test_a_violation_fails_and_names_the_fix(monkeypatch, capsys) -> None:
    monkeypatch.setattr(
        boundary, "tracked_paths", lambda repo: ["src/ok.py", "logs/x.log"]
    )
    assert boundary.main(["--repo", str(REPO)]) == 1
    err = capsys.readouterr().err
    assert "logs/x.log: local artifact root" in err
    assert "git rm --cached" in err


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    ).stdout


def _repo_with_history(tmp_path: Path, *, leak: bool) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q")
    _git(repo, "config", "user.email", "test@example.invalid")
    _git(repo, "config", "user.name", "Test User")
    (repo / "README.md").write_text("x\n", encoding="utf-8")
    _git(repo, "add", "README.md")
    _git(repo, "commit", "-q", "-m", "base")
    if leak:
        cache = repo / "node_modules" / ".mf"
        cache.mkdir(parents=True)
        (cache / "cf.json").write_text("{}\n", encoding="utf-8")
        _git(repo, "add", "-f", "node_modules/.mf/cf.json")
        _git(repo, "commit", "-q", "-m", "leak")
        _git(repo, "rm", "-q", "--cached", "node_modules/.mf/cf.json")
        _git(repo, "commit", "-q", "-m", "untrack")
    (repo / "README.md").write_text("y\n", encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "clean tip")
    return repo


def test_history_that_touched_a_purged_path_is_refused(tmp_path) -> None:
    # HF-770: the tip tree is clean, but its history still holds the file.
    repo = _repo_with_history(tmp_path, leak=True)
    assert boundary.tracked_paths(repo) == ["README.md"]
    # The newest commit that touches the path: here the one that untracked it.
    untrack = _git(repo, "rev-parse", "HEAD~1").strip()
    assert boundary.purged_history(repo) == [
        boundary.Violation(untrack, "history touches a purged path (HF-770)")
    ]
    assert boundary.main(["--repo", str(repo)]) == 1


def test_clean_history_is_accepted(tmp_path) -> None:
    repo = _repo_with_history(tmp_path, leak=False)
    assert boundary.purged_history(repo) == []
    assert boundary.main(["--repo", str(repo)]) == 0
