from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import importlib
import subprocess
from pathlib import Path

COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        ["git", *args],
        cwd=repo,
        check=True,
        text=True,
        capture_output=True,
    )


def test_pathspec_clean_source_state_uses_unguarded_git_status_and_listing(
    monkeypatch, tmp_path: Path
) -> None:
    COMPILER_METADATA._compiler_clean_pathspec_source_state_cached.cache_clear()
    calls: list[dict[str, object]] = []

    def fake_run(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        calls.append({"cmd": cmd, **kwargs})
        if "status" in cmd:
            return subprocess.CompletedProcess(cmd, 0, "", "")
        if "ls-files" in cmd:
            return subprocess.CompletedProcess(
                cmd,
                0,
                "100644 abc123 0\tsrc/molt/frontend/cfg_analysis.py\0",
                "",
            )
        raise AssertionError(f"unexpected git command: {cmd}")

    monkeypatch.setattr(
        COMPILER_METADATA, "_run_completed_command", fake_run, raising=True
    )

    try:
        state = COMPILER_METADATA._compiler_clean_pathspec_source_state(
            tmp_path,
            (
                str(tmp_path / "src" / "molt" / "frontend"),
                str(tmp_path / "src" / "molt" / "cli" / "module_source.py"),
            ),
        )
        again = COMPILER_METADATA._compiler_clean_pathspec_source_state(
            tmp_path,
            (
                str(tmp_path / "src" / "molt" / "frontend"),
                str(tmp_path / "src" / "molt" / "cli" / "module_source.py"),
            ),
        )
    finally:
        COMPILER_METADATA._compiler_clean_pathspec_source_state_cached.cache_clear()

    assert state is not None
    assert again == state
    assert state["kind"] == "git-clean-pathspec"
    assert state["pathspec_count"] == 2
    assert state["tracked_entry_count"] == 1
    assert len(calls) == 2
    assert calls[0]["cmd"] == [
        "git",
        "-C",
        str(tmp_path.resolve()),
        "status",
        "--porcelain=v2",
        "--untracked-files=all",
        "--",
        "src/molt/cli/module_source.py",
        "src/molt/frontend",
    ]
    assert calls[1]["cmd"] == [
        "git",
        "-C",
        str(tmp_path.resolve()),
        "ls-files",
        "-s",
        "-z",
        "--",
        "src/molt/cli/module_source.py",
        "src/molt/frontend",
    ]
    assert calls[0]["memory_guard_prefix"] is None
    assert calls[1]["memory_guard_prefix"] is None
    assert calls[0]["timeout"] == 5.0
    assert calls[1]["timeout"] == 5.0


def test_pathspec_clean_source_state_ignores_dirty_files_outside_scope(
    tmp_path: Path,
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init")
    _git(repo, "config", "user.email", "molt-test@example.invalid")
    _git(repo, "config", "user.name", "Molt Test")
    scoped = repo / "src" / "molt" / "frontend" / "cfg_analysis.py"
    outside_scope = repo / "README.md"
    scoped.parent.mkdir(parents=True, exist_ok=True)
    scoped.write_text("MARKER = 1\n", encoding="utf-8")
    outside_scope.write_text("clean\n", encoding="utf-8")
    _git(repo, "add", ".")
    _git(repo, "commit", "-m", "init")

    try:
        COMPILER_METADATA._compiler_clean_pathspec_source_state_cached.cache_clear()
        clean = COMPILER_METADATA._compiler_clean_pathspec_source_state(
            repo,
            (str(scoped.parent),),
        )
        assert clean is not None

        outside_scope.write_text("dirty\n", encoding="utf-8")
        COMPILER_METADATA._compiler_clean_pathspec_source_state_cached.cache_clear()
        still_clean = COMPILER_METADATA._compiler_clean_pathspec_source_state(
            repo,
            (str(scoped.parent),),
        )
        assert still_clean == clean

        scoped.write_text("dirty\n", encoding="utf-8")
        COMPILER_METADATA._compiler_clean_pathspec_source_state_cached.cache_clear()
        dirty = COMPILER_METADATA._compiler_clean_pathspec_source_state(
            repo,
            (str(scoped.parent),),
        )
        assert dirty is None
    finally:
        COMPILER_METADATA._compiler_clean_pathspec_source_state_cached.cache_clear()


def test_pathspec_clean_source_state_fails_closed_for_paths_outside_root(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    outside = tmp_path / "outside.py"
    outside.write_text("MARKER = 1\n", encoding="utf-8")

    assert (
        COMPILER_METADATA._compiler_clean_pathspec_source_state(root, (str(outside),))
        is None
    )
