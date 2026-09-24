"""Live custody must separate the apparatus's own writes from input mutations.

`git status` (run by custody's source snapshot and by tree validators) refreshes
the index through `.git/index.lock`, and the memory guard writes its state
files. Neither carries information about the proof's inputs, so a proof must
not fail closed as "transient-input-mutation" on them: the git refresh is
classified under its own receipt field and the guard state is routed to the
admitted state root outside the watched tree.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt.memory_guard_paths import (  # noqa: E402
    harness_guard_artifact_dir,
    pytest_guard_summary_dir,
)
from tools.proof_queue_pkg.execution_custody import (  # noqa: E402
    APPARATUS_GIT_INDEX_REFRESH,
    LiveCustodyMonitor,
    WatchSpec,
    classify_apparatus_event,
)


@pytest.mark.parametrize(
    ("relative", "expected"),
    [
        (".git", APPARATUS_GIT_INDEX_REFRESH),
        (".git/index.lock", APPARATUS_GIT_INDEX_REFRESH),
        (".git/index", None),
        (".git/HEAD", None),
        (".git/refs/heads/main", None),
        (".git/objects/ab/cdef", None),
        ("src/molt/__init__.py", None),
        ("index.lock", None),
        ("bench/friends/repos/numpy/.git", APPARATUS_GIT_INDEX_REFRESH),
        ("bench/friends/repos/numpy/.git/index.lock", APPARATUS_GIT_INDEX_REFRESH),
        ("bench/friends/repos/numpy/.git/worktrees/wt", APPARATUS_GIT_INDEX_REFRESH),
        (
            "bench/friends/repos/numpy/.git/worktrees/wt/index.lock",
            APPARATUS_GIT_INDEX_REFRESH,
        ),
        ("bench/friends/repos/numpy/.git/worktrees/wt/HEAD", None),
        ("bench/friends/repos/numpy/.git/worktrees", None),
        ("bench/friends/repos/numpy/.git/index", None),
    ],
)
def test_only_the_index_refresh_is_apparatus(
    tmp_path: Path, relative: str, expected: str | None
) -> None:
    (tmp_path / ".git").mkdir()
    (tmp_path / "bench/friends/repos/numpy/.git/worktrees/wt").mkdir(parents=True)
    assert (
        classify_apparatus_event(tmp_path, tmp_path / relative, "modified") == expected
    )


def test_paths_outside_the_root_are_never_apparatus(tmp_path: Path) -> None:
    other = tmp_path.parent / (tmp_path.name + "-other")
    assert (
        classify_apparatus_event(tmp_path, other / ".git" / "index.lock", "modified")
        is None
    )


def test_monitor_records_apparatus_events_apart_from_input_mutations(
    tmp_path: Path,
) -> None:
    (tmp_path / ".git").mkdir()
    spec = WatchSpec(root=tmp_path)
    monitor = LiveCustodyMonitor([spec])
    monitor._record_event(spec, "modified", tmp_path / ".git")
    monitor._record_event(spec, "created", tmp_path / ".git" / "index.lock")
    monitor._record_event(spec, "created", tmp_path / ".git" / "index.lock")
    monitor._record_event(spec, "modified", tmp_path / ".git" / "index")
    monitor._record_event(spec, "modified", tmp_path / "src" / "a.py")

    receipt = monitor.receipt()
    assert receipt["apparatus_events"] == [
        {
            "action": "modified",
            "path": str(tmp_path / ".git"),
            "apparatus": APPARATUS_GIT_INDEX_REFRESH,
        },
        {
            "action": "created",
            "path": str(tmp_path / ".git" / "index.lock"),
            "apparatus": APPARATUS_GIT_INDEX_REFRESH,
        },
    ]
    assert receipt["events"] == [
        {"action": "modified", "path": str(tmp_path / ".git" / "index")},
        {"action": "modified", "path": str(tmp_path / "src" / "a.py")},
    ]


def test_apparatus_events_alone_leave_custody_stable(tmp_path: Path) -> None:
    (tmp_path / ".git").mkdir()
    spec = WatchSpec(root=tmp_path)
    monitor = LiveCustodyMonitor([spec])
    monitor._record_event(spec, "modified", tmp_path / ".git" / "index.lock")
    monitor._transition("CREATED", "DRAINED")

    receipt = monitor.receipt()
    assert receipt["events"] == []
    assert receipt["errors"] == []
    assert receipt["stable"] is True
    assert len(receipt["apparatus_events"]) == 1


def test_apparatus_events_are_part_of_the_receipt_identity(tmp_path: Path) -> None:
    (tmp_path / ".git").mkdir()
    spec = WatchSpec(root=tmp_path)
    quiet = LiveCustodyMonitor([spec])
    quiet._transition("CREATED", "DRAINED")
    refreshed = LiveCustodyMonitor([spec])
    refreshed._record_event(spec, "modified", tmp_path / ".git" / "index.lock")
    refreshed._transition("CREATED", "DRAINED")

    assert quiet.receipt()["identity_sha256"] != refreshed.receipt()["identity_sha256"]


def test_guard_summary_dir_defaults_to_the_repository_tmp_root(tmp_path: Path) -> None:
    assert pytest_guard_summary_dir(tmp_path, environ={}) == (
        tmp_path / "tmp" / "pytest-memory-guard"
    )


def test_guard_summary_dir_follows_the_admitted_state_root(tmp_path: Path) -> None:
    state_root = tmp_path / "state"
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)}
    assert pytest_guard_summary_dir(tmp_path / "repo", environ=environ) == (
        state_root.resolve().parent / "pytest-memory-guard"
    )


def test_guard_summary_dir_resolves_a_relative_state_root_against_the_repo(
    tmp_path: Path,
) -> None:
    repo = tmp_path / "repo"
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": "custody/state"}
    assert pytest_guard_summary_dir(repo, environ=environ) == (
        (repo / "custody").resolve() / "pytest-memory-guard"
    )


def test_guard_summary_dir_ignores_a_blank_state_root(tmp_path: Path) -> None:
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": "   "}
    assert pytest_guard_summary_dir(tmp_path, environ=environ) == (
        tmp_path / "tmp" / "pytest-memory-guard"
    )


def test_harness_command_log_follows_the_admitted_state_root(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    assert harness_guard_artifact_dir(repo, environ={}) == (
        repo / "tmp" / "harness_memory_guard"
    )
    state_root = tmp_path / "state"
    assert harness_guard_artifact_dir(
        repo, environ={"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)}
    ) == (state_root.resolve().parent / "harness_memory_guard")


@pytest.mark.parametrize(
    "action",
    [
        "created",
        "deleted",
        "renamed-from",
        "renamed-to",
        "unknown",
        "inotify:0x40000200",
        "fsevents:0x20200",
        "inotify:bad-mask",
    ],
)
def test_destructive_or_unknown_git_directory_events_are_input_mutations(
    tmp_path: Path, action: str
) -> None:
    git = tmp_path / ".git"
    git.mkdir()
    assert classify_apparatus_event(tmp_path, git, action) is None


@pytest.mark.parametrize(
    "action", ["modified", "inotify:0x40000004", "fsevents:0x20400"]
)
def test_real_directory_metadata_refresh_is_apparatus(
    tmp_path: Path, action: str
) -> None:
    git = tmp_path / ".git"
    git.mkdir()
    assert (
        classify_apparatus_event(tmp_path, git, action) == APPARATUS_GIT_INDEX_REFRESH
    )


def test_linked_worktree_git_file_is_an_input(tmp_path: Path) -> None:
    git = tmp_path / ".git"
    git.write_text("gitdir: elsewhere", encoding="utf-8")
    assert classify_apparatus_event(tmp_path, git, "modified") is None
    git.unlink()
    assert classify_apparatus_event(tmp_path, git, "deleted") is None


def test_symlinked_git_directory_is_not_excused(tmp_path: Path) -> None:
    actual = tmp_path / "actual"
    actual.mkdir()
    git = tmp_path / ".git"
    try:
        git.symlink_to(actual, target_is_directory=True)
    except OSError:
        pytest.skip("symlinks unavailable")
    assert classify_apparatus_event(tmp_path, git, "modified") is None
    assert classify_apparatus_event(tmp_path, git / "index.lock", "created") is None
