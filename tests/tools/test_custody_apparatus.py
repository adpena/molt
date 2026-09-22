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

from tools.memory_guard_core.paths import (  # noqa: E402
    harness_command_profile_log_path,
    pytest_outer_guard_summary_dir,
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
    ],
)
def test_only_the_index_refresh_is_apparatus(
    tmp_path: Path, relative: str, expected: str | None
) -> None:
    assert classify_apparatus_event(tmp_path, tmp_path / relative) == expected


def test_paths_outside_the_root_are_never_apparatus(tmp_path: Path) -> None:
    other = tmp_path.parent / (tmp_path.name + "-other")
    assert classify_apparatus_event(tmp_path, other / ".git" / "index.lock") is None


def test_monitor_records_apparatus_events_apart_from_input_mutations(
    tmp_path: Path,
) -> None:
    spec = WatchSpec(root=tmp_path)
    monitor = LiveCustodyMonitor([spec])
    monitor._record_event(spec, "modified", tmp_path / ".git")
    monitor._record_event(spec, "added", tmp_path / ".git" / "index.lock")
    monitor._record_event(spec, "added", tmp_path / ".git" / "index.lock")
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
            "action": "added",
            "path": str(tmp_path / ".git" / "index.lock"),
            "apparatus": APPARATUS_GIT_INDEX_REFRESH,
        },
    ]
    assert receipt["events"] == [
        {"action": "modified", "path": str(tmp_path / ".git" / "index")},
        {"action": "modified", "path": str(tmp_path / "src" / "a.py")},
    ]


def test_apparatus_events_alone_leave_custody_stable(tmp_path: Path) -> None:
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
    spec = WatchSpec(root=tmp_path)
    quiet = LiveCustodyMonitor([spec])
    quiet._transition("CREATED", "DRAINED")
    refreshed = LiveCustodyMonitor([spec])
    refreshed._record_event(spec, "modified", tmp_path / ".git" / "index.lock")
    refreshed._transition("CREATED", "DRAINED")

    assert quiet.receipt()["identity_sha256"] != refreshed.receipt()["identity_sha256"]


def test_guard_summary_dir_defaults_to_the_repository_tmp_root(tmp_path: Path) -> None:
    assert pytest_outer_guard_summary_dir(tmp_path, environ={}) == (
        tmp_path / "tmp" / "pytest-memory-guard"
    )


def test_guard_summary_dir_follows_the_admitted_state_root(tmp_path: Path) -> None:
    state_root = tmp_path / "state"
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)}
    assert pytest_outer_guard_summary_dir(tmp_path / "repo", environ=environ) == (
        state_root.resolve() / "pytest-memory-guard"
    )


def test_guard_summary_dir_resolves_a_relative_state_root_against_the_repo(
    tmp_path: Path,
) -> None:
    repo = tmp_path / "repo"
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": "custody/state"}
    assert pytest_outer_guard_summary_dir(repo, environ=environ) == (
        (repo / "custody" / "state").resolve() / "pytest-memory-guard"
    )


def test_guard_summary_dir_ignores_a_blank_state_root(tmp_path: Path) -> None:
    environ = {"MOLT_MEMORY_GUARD_STATE_ROOT": "   "}
    assert pytest_outer_guard_summary_dir(tmp_path, environ=environ) == (
        tmp_path / "tmp" / "pytest-memory-guard"
    )


def test_harness_command_log_follows_the_admitted_state_root(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    assert harness_command_profile_log_path(repo, environ={}) == (
        repo / "logs" / "harness_memory_guard" / "commands.jsonl"
    )
    state_root = tmp_path / "state"
    assert harness_command_profile_log_path(
        repo, environ={"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)}
    ) == (state_root.resolve() / "harness_memory_guard" / "commands.jsonl")
