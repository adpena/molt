"""The public-source boundary gate (HF-768)."""

from __future__ import annotations

from pathlib import Path

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
