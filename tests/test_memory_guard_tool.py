from __future__ import annotations

from collections.abc import Mapping
import dataclasses
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

import pytest

import tools.memory_guard as memory_guard


def _guard_termination_report(
    *,
    reason: str = "test_cleanup",
    root_pid: int = 100,
    root_pgid: int | None = 100,
    watched_pids: tuple[int, ...] = (),
    actions: tuple[memory_guard.GuardTerminationAction, ...] = (),
) -> memory_guard.GuardTerminationReport:
    return memory_guard.GuardTerminationReport(
        reason=reason,
        started_at="2026-05-21T12:00:00Z",
        completed_at="2026-05-21T12:00:01Z",
        root_pid=root_pid,
        root_pgid=root_pgid,
        root_sid=None,
        grace_sec=0.125,
        watched_pids=watched_pids,
        protected_pgids=(),
        escaped_pids=(),
        remaining_pgids=(),
        remaining_pids=(),
        actions=actions,
    )


def test_termination_report_validator_rejects_fake_drift() -> None:
    with pytest.raises(TypeError, match="must return GuardTerminationReport"):
        memory_guard._validated_termination_report(
            None,
            caller="terminate_watched_processes",
        )


def test_termination_report_batch_validator_rejects_fake_drift() -> None:
    with pytest.raises(TypeError, match="must return GuardTerminationReport"):
        memory_guard._validated_termination_reports(
            (_guard_termination_report(), None),
            caller="cleanup_tracked_orphans",
        )


def test_parse_process_table_keeps_commands_with_spaces() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1  2048 python worker.py --flag value
          11    10  4096 /bin/sh -c echo hi
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
    )
    assert samples[11].command == "/bin/sh -c echo hi"


def test_active_guard_marker_records_death_capsule(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    marker_dir = tmp_path / "active"
    monkeypatch.setattr(memory_guard, "ACTIVE_GUARD_MARKER_DIR", marker_dir)

    token, marker = memory_guard._write_active_guard_marker(
        os.getpid(),
        command=("python", "-c", "print('ok')"),
        cwd=tmp_path,
    )

    payload = json.loads(marker.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 1
    assert payload["pid"] == os.getpid()
    assert payload["token"] == token
    assert payload["command"] == ["python", "-c", "print('ok')"]
    assert payload["cwd"] == str(tmp_path.resolve(strict=False))
    assert payload["status"] == "guard_starting"
    assert payload["created_at"]
    assert payload["updated_at"]

    memory_guard._update_active_guard_marker(
        marker,
        "wrong-token",
        status="corrupted",
    )
    assert json.loads(marker.read_text(encoding="utf-8"))["status"] == (
        "guard_starting"
    )

    memory_guard._update_active_guard_marker(
        marker,
        token,
        status="child_running",
        child_process={"pid": 123, "command": ["python"]},
    )
    updated = json.loads(marker.read_text(encoding="utf-8"))
    assert updated["status"] == "child_running"
    assert updated["child_process"]["pid"] == 123


def test_parse_process_table_reads_process_group_ids() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1    10  2048 python worker.py --flag value
          11    10    10  4096 /bin/sh -c echo hi
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
        pgid=10,
    )
    assert samples[11].pgid == 10


def test_parse_process_table_reads_process_elapsed_age() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1    10  2048  901 python worker.py --flag value
          11    10    10  4096  01:02:03 /bin/sh -c echo hi
          12    10    10  4096  2-03:04:05 python slow.py
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
        pgid=10,
        elapsed_sec=901,
    )
    assert samples[11].elapsed_sec == 3723
    assert samples[12].elapsed_sec == 183845


def test_parse_process_table_with_start_produces_creation_identity() -> None:
    samples = memory_guard.parse_process_table_with_start(
        "10 1 10 2048 Thu Jul 17 07:15:01 2026 python worker.py --flag value\n"
    )

    assert samples[10].command == "python worker.py --flag value"
    assert samples[10].started_at_ns is not None
    assert memory_guard.process_identity(samples[10]).started_at_ns == (
        samples[10].started_at_ns
    )


def _write_linux_proc_sample(
    root: Path,
    *,
    pid: int,
    ppid: int,
    pgid: int,
    start_ticks: int,
) -> None:
    proc = root / str(pid)
    proc.mkdir(parents=True)
    tail = ["S", str(ppid), str(pgid), *(["0"] * 16), str(start_ticks)]
    (proc / "stat").write_text(
        f"{pid} (worker) {' '.join(tail)}\n",
        encoding="utf-8",
    )
    (proc / "cmdline").write_bytes(b"python\0worker.py\0")
    (proc / "status").write_text("Name:\tworker\nVmRSS:\t1234 kB\n", encoding="utf-8")


def test_linux_proc_sampler_binds_lineage_identity_command_and_rss(
    tmp_path: Path,
) -> None:
    _write_linux_proc_sample(
        tmp_path,
        pid=200,
        ppid=100,
        pgid=200,
        start_ticks=321,
    )

    samples = memory_guard.sample_processes_linux_proc(tmp_path, uptime_sec=1000.0)

    assert samples[200].ppid == 100
    assert samples[200].pgid == 200
    assert samples[200].command == "python worker.py"
    assert samples[200].rss_kb == 1234
    assert samples[200].started_at_ns is not None
    assert samples[200].elapsed_sec is not None


def test_linux_proc_sampler_discards_reuse_between_bound_reads(
    tmp_path: Path,
) -> None:
    _write_linux_proc_sample(
        tmp_path,
        pid=200,
        ppid=100,
        pgid=200,
        start_ticks=321,
    )
    observations = iter(
        (
            (100, 200, 321_000, "worker"),
            (4, 200, 322_000, "System"),
        )
    )

    with pytest.raises(memory_guard.ProcessSnapshotError, match="no stable rows"):
        memory_guard.sample_processes_linux_proc(
            tmp_path,
            stat_reader=lambda _pid, _root: next(observations),
            uptime_sec=1000.0,
        )


def test_linux_proc_sampler_preserves_typed_enumeration_failure(
    tmp_path: Path,
) -> None:
    with pytest.raises(memory_guard.ProcessSnapshotError, match="enumeration failed"):
        memory_guard.sample_processes_linux_proc(tmp_path / "missing")


def test_darwin_sampler_keeps_bound_launcher_arguments_for_host_protection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model = memory_guard._process_model
    monkeypatch.setattr(model.sys, "platform", "darwin")
    monkeypatch.setattr(
        model.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout="7 3 7 2048 Thu Jul 17 07:15:01 2026 node\n",
            stderr="",
        ),
    )
    metadata = (3, 7, 123_456_789, "node")
    monkeypatch.setattr(model, "_darwin_proc_metadata", lambda _pid: metadata)
    monkeypatch.setattr(
        model,
        "_darwin_proc_command",
        lambda _pid: "node /opt/node_modules/@openai/codex/bin/codex.js app-server",
    )

    samples = model.sample_processes_posix()

    assert samples[7].ppid == 3
    assert samples[7].started_at_ns == 123_456_789
    assert "@openai/codex" in samples[7].command
    assert memory_guard.is_host_control_plane_process(samples[7])


def test_darwin_sampler_revokes_identity_when_native_binding_changes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model = memory_guard._process_model
    monkeypatch.setattr(model.sys, "platform", "darwin")
    monkeypatch.setattr(
        model.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout="7 3 7 2048 Thu Jul 17 07:15:01 2026 node\n",
            stderr="",
        ),
    )
    metadata = iter(
        (
            (3, 7, 123_456_789, "node"),
            (4, 7, 123_456_790, "node"),
        )
    )
    monkeypatch.setattr(model, "_darwin_proc_metadata", lambda _pid: next(metadata))
    monkeypatch.setattr(model, "_darwin_proc_command", lambda _pid: "node codex.js")

    samples = model.sample_processes_posix()

    assert samples[7].ppid == 0
    assert samples[7].started_at_ns is None


def test_descendant_pids_includes_grandchildren() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 20, "child"),
        102: memory_guard.ProcessSample(102, 101, 30, "grandchild"),
        200: memory_guard.ProcessSample(200, 1, 999_999, "unrelated"),
    }

    assert memory_guard.descendant_pids(samples, 100) == {100, 101, 102}


def test_timeout_sampler_uses_bounded_windows_snapshot_only_for_default_sampler(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def custom_sampler() -> dict[int, memory_guard.ProcessSample]:
        return {}

    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    assert (
        memory_guard._timeout_sampler(memory_guard.sample_processes)
        is memory_guard.sample_processes_windows_hard_timeout
    )
    assert memory_guard._timeout_sampler(custom_sampler) is custom_sampler

    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: False)
    assert memory_guard._timeout_sampler(memory_guard.sample_processes) is (
        memory_guard.sample_processes
    )


def test_watched_pids_excludes_unobserved_reparented_process_group_members() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=100),
        102: memory_guard.ProcessSample(102, 1, 30, "reparented", pgid=100),
        200: memory_guard.ProcessSample(200, 1, 999_999, "unrelated", pgid=200),
    }

    assert memory_guard.watched_pids(samples, 100) == {100, 101}


def test_watched_pids_excludes_host_control_plane_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
        200: memory_guard.ProcessSample(200, 1, 20, "unrelated", pgid=200),
    }

    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_plain_claude_control_plane_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "claude",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
        200: memory_guard.ProcessSample(200, 1, 20, "unrelated", pgid=200),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_claude_code_executable_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/opt/homebrew/bin/claude-code --continue",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_codex_app_and_cli_are_host_control_plane_on_all_platform_shapes() -> None:
    samples = [
        memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        memory_guard.ProcessSample(
            101,
            1,
            500_000,
            "/opt/homebrew/bin/codex exec --sandbox danger-full-access",
            pgid=101,
        ),
        memory_guard.ProcessSample(
            102,
            1,
            500_000,
            "/home/adpen/.local/bin/codex --continue",
            pgid=102,
        ),
        memory_guard.ProcessSample(
            103,
            1,
            500_000,
            "node /usr/local/lib/node_modules/@openai/codex/bin/codex.js",
            pgid=103,
        ),
        memory_guard.ProcessSample(
            104,
            1,
            500_000,
            r"C:\Users\adpen\AppData\Roaming\npm\codex.cmd exec",
            pgid=None,
        ),
        memory_guard.ProcessSample(
            105,
            1,
            500_000,
            r"powershell.exe -File C:\Users\adpen\AppData\Roaming\npm\codex.ps1",
            pgid=None,
        ),
    ]

    assert all(memory_guard.is_host_control_plane_process(sample) for sample in samples)


def test_watched_pids_excludes_node_launched_claude_code_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_process_tree_tracker_keeps_reparented_new_session_child_after_seen() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    first = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=101, started_at_ns=101
        ),
        102: memory_guard.ProcessSample(
            102, 101, 30, "grandchild", pgid=102, started_at_ns=102
        ),
    }

    assert tracker.update(first) == {100, 101, 102}

    reparented = {
        101: memory_guard.ProcessSample(
            101, 1, 20, "child", pgid=101, started_at_ns=101
        ),
        102: memory_guard.ProcessSample(
            102, 1, 30, "grandchild", pgid=102, started_at_ns=102
        ),
    }

    assert tracker.update(reparented) == {101, 102}
    violation = memory_guard.find_rss_violation(
        reparented,
        root_pid=100,
        max_rss_kb=25,
        tracker=tracker,
    )
    assert violation == memory_guard.RssViolation(
        pid=102,
        rss_kb=30,
        command="grandchild",
    )


def test_process_tree_tracker_stale_pid_cannot_admit_unrelated_child() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(100, 1, 10, "guard", started_at_ns=1),
        101: memory_guard.ProcessSample(101, 100, 20, "compiler", started_at_ns=2),
    }
    assert tracker.update(initial) == {100, 101}

    # PID 101 has exited. A later unrelated process reports the stale number as
    # its parent; an absent historical PID is not live custody authority.
    reused_parent_edge = {
        100: memory_guard.ProcessSample(100, 1, 10, "guard", started_at_ns=1),
        900: memory_guard.ProcessSample(
            900,
            101,
            500_000,
            "NVIDIA Overlay.exe",
            started_at_ns=99,
        ),
    }
    assert tracker.update(reused_parent_edge) == {100}


def test_process_tree_tracker_identity_ignores_mutable_command_and_group() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "guard", pgid=100, started_at_ns=1
        ),
        200: memory_guard.ProcessSample(
            200, 100, 20, "python worker.py", pgid=100, started_at_ns=2
        ),
    }
    assert tracker.update(initial) == {100, 200}

    execed_and_reparented = {
        100: initial[100],
        200: memory_guard.ProcessSample(
            200,
            1,
            20,
            "/opt/molt-backend --daemon",
            pgid=200,
            started_at_ns=2,
        ),
    }

    assert tracker.update(execed_and_reparented) == {100, 200}
    assert tracker.custody_identities({200}) == {
        200: memory_guard.process_identity(initial[200])
    }


def test_process_tree_tracker_revokes_same_command_pid_reuse() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "guard", pgid=100, started_at_ns=1
        ),
        200: memory_guard.ProcessSample(
            200, 100, 20, "worker", pgid=200, started_at_ns=2
        ),
    }
    tracker.update(initial)
    reused = {
        100: initial[100],
        200: memory_guard.ProcessSample(
            200, 1, 20, "worker", pgid=200, started_at_ns=3
        ),
    }

    assert tracker.update(reused) == {100}
    assert tracker.custody_identities({200}) == {}


def test_process_tree_tracker_weak_reused_parent_cannot_admit_child() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    root = memory_guard.ProcessSample(
        100, 1, 10, "guard", started_at_ns=100
    )
    tracker.update({100: root})

    weak_reused_root = memory_guard.ProcessSample(
        100,
        1,
        10,
        "unreadable.exe",
        started_at_ns=None,
    )
    unrelated_child = memory_guard.ProcessSample(
        200,
        100,
        20,
        "worker.exe",
        started_at_ns=200,
    )

    assert tracker.update({100: weak_reused_root, 200: unrelated_child}) == {100}
    assert tracker.custody_identities({100}) == {
        100: memory_guard.process_identity(root)
    }


def test_windows_termination_requires_creation_identity(monkeypatch) -> None:
    sample = memory_guard.ProcessSample(
        200,
        100,
        20,
        "worker.exe",
        started_at_ns=None,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent.append((pid, sig)),
    )

    report = memory_guard.terminate_watched_processes(
        100,
        samples={200: sample},
        watched={200},
        expected_identities={200: memory_guard.process_identity(sample)},
        root_owned=True,
    )

    assert sent == []
    assert any(
        action.target_id == 200 and action.result == "skipped_ambiguous_identity"
        for action in report.actions
    )


def test_windows_termination_uses_tracker_identity_not_fresh_pid_owner(
    monkeypatch,
) -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    original = memory_guard.ProcessSample(
        100,
        1,
        10,
        "guard.exe",
        started_at_ns=1,
    )
    child = memory_guard.ProcessSample(
        200,
        100,
        20,
        "rustc.exe",
        started_at_ns=2,
    )
    tracker.update({100: original, 200: child})
    reused = memory_guard.ProcessSample(
        200,
        4,
        20,
        "System",
        started_at_ns=3,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent.append((pid, sig)),
    )

    report = memory_guard.terminate_watched_processes(
        100,
        samples={200: reused},
        watched={200},
        tracker=tracker,
        root_owned=True,
    )

    assert sent == []
    assert any(
        action.target_id == 200 and action.result == "skipped_identity_mismatch"
        for action in report.actions
    )


def test_windows_termination_refuses_ambiguous_process_fanout(monkeypatch) -> None:
    root_pid = 100
    samples = {
        pid: memory_guard.ProcessSample(
            pid,
            root_pid if pid != root_pid else 1,
            10,
            f"process-{pid}",
            started_at_ns=pid,
        )
        for pid in range(
            root_pid,
            root_pid + memory_guard.MAX_TERMINATION_PID_FANOUT + 1,
        )
    }
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        memory_guard,
        "_terminate_pid_if_identity_action",
        lambda *_args, **_kwargs: pytest.fail("ambiguous tree must not be signaled"),
    )

    report = memory_guard.terminate_watched_processes(
        root_pid,
        samples=samples,
        watched=set(samples),
        root_owned=True,
    )

    assert report.reason == "windows_pid_tree_ambiguous_fanout"
    assert len(report.actions) == 1
    assert report.actions[0].result == "skipped_ambiguous_fanout"


def test_process_tree_tracker_does_not_absorb_root_ambient_process_group() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: memory_guard.ProcessSample(100, 50, 10, "pytest current", pgid=500),
        50: memory_guard.ProcessSample(
            50,
            1,
            20,
            "/Applications/Codex.app/Contents/MacOS/Codex app-server",
            pgid=500,
        ),
        200: memory_guard.ProcessSample(
            200,
            50,
            30,
            "/Users/adpena/Projects/molt/.venv/bin/python3 tests/molt_diff.py",
            pgid=200,
        ),
    }

    assert tracker.update(samples) == {100}
    assert tracker.known_pids == {100}
    assert tracker.known_pgids == {100}


def test_process_tree_tracker_does_not_absorb_learned_descendant_process_group_peer() -> (
    None
):
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=777, started_at_ns=101
        ),
        200: memory_guard.ProcessSample(
            200, 1, 999, "unrelated", pgid=777, started_at_ns=200
        ),
    }

    assert tracker.update(samples) == {100, 101}
    assert tracker.known_pids == {100, 101}
    assert tracker.known_pgids == {100, 777}


def test_find_rss_violation_ignores_unobserved_reparented_process_group_member() -> (
    None
):
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 1, 26_000_000, "reparented", pgid=100),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation is None


def test_terminate_watched_processes_kills_only_root_group_and_tracked_pids(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 1, 20, "child", pgid=101),
        102: memory_guard.ProcessSample(102, 1, 30, "grandchild", pgid=102),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        sent_pids.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(memory_guard.os, "kill", fake_kill)

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101, 102},
        grace=0.001,
    )

    assert (100, memory_guard.signal.SIGTERM) in sent_groups
    assert (101, memory_guard.signal.SIGTERM) not in sent_groups
    assert (102, memory_guard.signal.SIGTERM) not in sent_groups
    assert (101, memory_guard.signal.SIGTERM) in sent_pids
    assert (102, memory_guard.signal.SIGTERM) in sent_pids
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert (102, memory_guard.signal.SIGKILL) in sent_pids


def test_terminate_watched_processes_skips_host_control_plane_root_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert sent_groups == []
    assert sent_pids == []


def test_protected_process_groups_include_external_codex_descendant_not_owned_child() -> (
    None
):
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            10_000,
            "/bin/zsh -l",
            pgid=101,
        ),
        777: memory_guard.ProcessSample(
            777,
            101,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=777,
        ),
        999: memory_guard.ProcessSample(
            999,
            100,
            30_000,
            "python tools/memory_guard.py -- pytest",
            pgid=999,
        ),
        200: memory_guard.ProcessSample(
            200,
            999,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=200,
        ),
    }

    protected = memory_guard.protected_process_group_ids(
        samples,
        self_pid=999,
        self_pgid=999,
    )

    assert 100 in protected
    assert 777 in protected
    assert 999 in protected
    assert 200 not in protected


def test_protected_process_groups_include_external_claude_descendant_not_owned_child() -> (
    None
):
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "claude --dangerously-skip-permissions",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            10_000,
            "/bin/zsh -c source /Users/adpena/.claude/shell-snapshots/snapshot-zsh",
            pgid=101,
        ),
        777: memory_guard.ProcessSample(
            777,
            101,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=777,
        ),
        999: memory_guard.ProcessSample(
            999,
            1,
            30_000,
            "python tools/memory_guard.py -- pytest",
            pgid=999,
        ),
        200: memory_guard.ProcessSample(
            200,
            999,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=200,
        ),
    }

    protected = memory_guard.protected_process_group_ids(
        samples,
        self_pid=999,
        self_pgid=999,
    )

    assert 100 in protected
    assert 101 in protected
    assert 777 in protected
    assert 999 in protected
    assert 200 not in protected


def test_terminate_single_process_group_refuses_protected_group(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )

    assert memory_guard._terminate_single_process_group(100, grace=0.001) is True

    assert sent_groups == []


def test_escalation_pid_signal_revalidates_identity(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        return
    original = memory_guard.ProcessSample(
        101,
        100,
        20,
        "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
        pgid=101,
    )
    reused = memory_guard.ProcessSample(
        101,
        1,
        20,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        pgid=101,
    )
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    action = memory_guard._send_pid_signal_if_identity_action(
        101,
        memory_guard.process_identity(original),
        memory_guard.signal.SIGKILL,
        sampler=lambda: {101: reused},
    )

    assert action.result == "skipped_identity_mismatch"
    assert sent_pids == []


def test_escalation_group_signal_rechecks_protected_group(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        return
    original = memory_guard.ProcessSample(
        101,
        100,
        20,
        "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
        pgid=101,
    )
    protected = memory_guard.ProcessSample(
        101,
        1,
        20,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        pgid=101,
    )
    sent_groups: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )

    action = memory_guard._send_process_group_signal_if_identities_match_action(
        101,
        {101: memory_guard.process_identity(original)},
        memory_guard.signal.SIGKILL,
        sampler=lambda: {101: protected},
    )

    assert action.result == "skipped_protected_group"
    assert sent_groups == []


def test_sigterm_pid_helper_revalidates_identity_before_signal(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        return
    original = memory_guard.ProcessSample(
        101,
        100,
        20,
        "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
        pgid=101,
    )
    reused = memory_guard.ProcessSample(
        101,
        1,
        20,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        pgid=101,
    )
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    action = memory_guard._terminate_pid_if_identity_action(
        101,
        memory_guard.process_identity(original),
        sampler=lambda: {101: reused},
        grace=0.001,
    )

    assert action.result == "skipped_identity_mismatch"
    assert sent_pids == []


def test_terminate_watched_processes_revalidates_escaped_pid_before_sigterm(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    observed = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(
            101,
            100,
            20,
            "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
            pgid=777,
        ),
    }
    reused = {
        101: memory_guard.ProcessSample(
            101,
            1,
            20,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=777,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    report = memory_guard.terminate_watched_processes(
        100,
        samples=observed,
        watched={100, 101},
        sampler=lambda: reused,
        grace=0.001,
    )

    assert any(
        action.target_kind == "process"
        and action.target_id == 101
        and action.result == "skipped_identity_mismatch"
        for action in report.actions
    )
    assert sent_groups == []
    assert sent_pids == []


def test_terminate_watched_processes_revalidates_root_group_before_sigterm(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    observed = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(
            101,
            100,
            20,
            "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
            pgid=100,
        ),
    }
    protected = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
            pgid=100,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    report = memory_guard.terminate_watched_processes(
        100,
        samples=observed,
        watched={100, 101},
        sampler=lambda: protected,
        grace=0.001,
    )

    assert any(
        action.target_kind == "process_group"
        and action.target_id == 100
        and action.result == "skipped_protected_group"
        for action in report.actions
    )
    assert sent_groups == []
    assert sent_pids == []


def test_terminate_watched_processes_filters_protected_escaped_pid(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(
            101,
            100,
            500_000,
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            pgid=777,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert (100, memory_guard.signal.SIGTERM) in sent_groups
    assert all(pid != 101 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_killpgs_shared_child_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=777),
        200: memory_guard.ProcessSample(200, 1, 999, "unrelated", pgid=777),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        sent_pids.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(memory_guard.os, "kill", fake_kill)

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert (100, memory_guard.signal.SIGTERM) in sent_groups
    assert all(pgid != 777 for pgid, _sig in sent_groups)
    assert (101, memory_guard.signal.SIGTERM) in sent_pids
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_kills_learned_group_peer(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=777),
        200: memory_guard.ProcessSample(200, 1, 999, "unrelated", pgid=777),
    }
    assert tracker.update(samples) == {100, 101}
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        sent_pids.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(memory_guard.os, "kill", fake_kill)

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        tracker=tracker,
        grace=0.001,
    )

    assert all(pgid != 777 for pgid, _sig in sent_groups)
    assert (101, memory_guard.signal.SIGTERM) in sent_pids
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_killpgs_mixed_root_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=100),
        200: memory_guard.ProcessSample(200, 1, 999, "unrelated", pgid=100),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        sent_pids.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(memory_guard.os, "kill", fake_kill)

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert sent_groups == []
    assert (100, memory_guard.signal.SIGKILL) in sent_pids
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_kills_host_control_plane_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            27404,
            20,
            "uv run python tests/molt_diff.py --jobs 1",
            pgid=700,
        ),
        27404: memory_guard.ProcessSample(
            27404,
            27335,
            500_000,
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            pgid=700,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100},
        grace=0.001,
    )

    assert sent_groups == []
    assert sent_pids == []


def test_find_rss_violation_ignores_unrelated_processes() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 26_000_000, "child"),
        200: memory_guard.ProcessSample(200, 1, 40_000_000, "unrelated"),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation == memory_guard.RssViolation(
        pid=101,
        rss_kb=26_000_000,
        command="child",
    )


def test_find_rss_violation_returns_highest_descendant() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 28_000_000, "smaller"),
        102: memory_guard.ProcessSample(102, 100, 29_000_000, "larger"),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation is not None
    assert violation.pid == 102
    assert violation.rss_gb == pytest.approx(29_000_000 / (1024 * 1024))


def test_find_rss_violation_catches_aggregate_process_tree_rss() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 15_000_000, "child-a", pgid=100),
        102: memory_guard.ProcessSample(102, 100, 15_000_000, "child-b", pgid=100),
        200: memory_guard.ProcessSample(200, 1, 40_000_000, "unrelated", pgid=200),
    }

    violation = memory_guard.find_rss_violation(
        samples,
        root_pid=100,
        max_rss_kb=25_000_000,
        max_total_rss_kb=25_000_000,
    )

    assert violation == memory_guard.RssViolation(
        pid=100,
        rss_kb=30_000_010,
        command="process tree aggregate",
        scope="process_tree",
    )


def test_max_rss_gb_accepts_high_workstation_limits() -> None:
    assert memory_guard.max_rss_kb_from_gb(96) == 96 * 1024 * 1024


def test_max_rss_gb_must_leave_margin_below_hard_cap() -> None:
    with pytest.raises(ValueError, match="below 112"):
        memory_guard.max_rss_kb_from_gb(112)


def test_max_global_rss_gb_must_leave_workstation_margin() -> None:
    assert memory_guard.max_global_rss_kb_from_gb(128) == 128 * 1024 * 1024
    with pytest.raises(ValueError, match="below 4096"):
        memory_guard.max_global_rss_kb_from_gb(4096)


def test_memory_guard_defaults_adapt_to_live_memory_budget() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "96",
        },
    )

    assert budget.reserve_gb == pytest.approx(7.68)
    assert budget.max_process_rss_gb == pytest.approx(46.262016)
    assert budget.max_total_rss_gb == pytest.approx(51.40224)
    assert budget.max_global_rss_gb == pytest.approx(85.6704)
    assert memory_guard.DEFAULT_POLL_INTERVAL_SEC == 0.10


def test_adaptive_budget_scales_up_and_down_with_live_available_memory() -> None:
    high = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "120",
        },
    )
    pressured = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "32",
        },
    )

    assert high.reserve_gb == pytest.approx(7.68)
    assert high.max_global_rss_gb == pytest.approx(108.9504)
    assert high.max_total_rss_gb == pytest.approx(65.37024)
    assert high.max_process_rss_gb == pytest.approx(58.833216)
    assert pressured.reserve_gb == pytest.approx(high.reserve_gb)
    assert pressured.max_global_rss_gb == pytest.approx(23.5904)
    assert pressured.max_total_rss_gb == pytest.approx(14.15424)
    assert pressured.max_process_rss_gb == pytest.approx(12.738816)
    assert high.max_global_rss_gb > pressured.max_global_rss_gb
    assert high.available_gb - high.max_global_rss_gb > high.reserve_gb
    assert pressured.available_gb - pressured.max_global_rss_gb > pressured.reserve_gb


def test_adaptive_budget_accounts_guarded_tree_rss_without_self_tightening() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "46",
        },
        accounted_rss_kb=50 * 1024 * 1024,
    )

    assert budget.accounted_rss_gb == pytest.approx(50.0)
    assert budget.available_gb == pytest.approx(96.0)
    assert budget.max_process_rss_gb == pytest.approx(46.262016)
    assert budget.max_total_rss_gb == pytest.approx(51.40224)
    assert budget.max_global_rss_gb == pytest.approx(85.6704)


def test_adaptive_budget_clamps_large_hosts_below_rss_conversion_cap() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "512",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "500",
        },
    )

    assert budget.reserve_gb == pytest.approx(12.0)
    assert budget.max_global_rss_gb == pytest.approx(473.36)
    assert budget.max_total_rss_gb == pytest.approx(
        memory_guard.DEFAULT_HARD_MAX_RSS_GB - 0.001
    )
    assert budget.max_process_rss_gb == pytest.approx(100.7991)
    assert memory_guard.max_rss_kb_from_gb(budget.max_total_rss_gb) > 0
    assert memory_guard.max_rss_kb_from_gb(budget.max_process_rss_gb) > 0


def test_parse_darwin_vm_stat_available_bytes() -> None:
    text = """
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                             10.
Pages active:                           99.
Pages inactive:                         20.
Pages speculative:                       3.
Pages purgeable:                         2.
Pages wired down:                       88.
Pages occupied by compressor:            7.
"""

    available = memory_guard._parse_darwin_vm_stat_available_bytes(text)

    assert available == (10 + 20 + 3 + 2) * 16_384


def test_available_memory_bytes_uses_darwin_vm_stat(monkeypatch) -> None:
    class Result:
        returncode = 0
        stdout = (
            "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n"
            "Pages free: 2.\n"
            "Pages inactive: 3.\n"
            "Pages speculative: 5.\n"
            "Pages purgeable: 7.\n"
        )

    monkeypatch.setattr(memory_guard.sys, "platform", "darwin")
    monkeypatch.setattr(
        memory_guard.subprocess,
        "run",
        lambda *args, **kwargs: Result(),
    )

    assert memory_guard.available_memory_bytes(environ={}) == 17 * 4096


def test_resolve_memory_limits_refreshes_dynamic_caps() -> None:
    seen_accounted: list[int] = []

    def provider(accounted_rss_kb: int) -> memory_guard.AdaptiveMemoryBudget:
        seen_accounted.append(accounted_rss_kb)
        return memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=4.0,
            max_total_rss_gb=6.0,
            max_global_rss_gb=8.0,
            reserve_gb=1.0,
            physical_gb=16.0,
            available_gb=12.0,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    limits = memory_guard.resolve_memory_limits(
        max_process_rss_kb=2 * 1024 * 1024,
        max_total_rss_kb=3 * 1024 * 1024,
        max_global_rss_kb=5 * 1024 * 1024,
        adaptive_budget_provider=provider,
        dynamic_process_rss=True,
        dynamic_total_rss=True,
        dynamic_global_rss=False,
        accounted_rss_kb=12345,
    )

    assert seen_accounted == [12345]
    assert limits.max_process_rss_kb == 4 * 1024 * 1024
    assert limits.max_total_rss_kb == 6 * 1024 * 1024
    assert limits.max_global_rss_kb == 5 * 1024 * 1024


def test_memory_guard_adaptive_defaults_do_not_starve_small_hosts() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "7",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "5",
        },
    )

    assert budget.reserve_gb == pytest.approx(1.0)
    assert budget.max_process_rss_gb == pytest.approx(2.0952)
    assert budget.max_total_rss_gb == pytest.approx(2.328)
    assert budget.max_global_rss_gb == pytest.approx(3.88)


def test_default_child_rlimit_tracks_process_rss_budget() -> None:
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
    ) == pytest.approx(2.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
        max_global_rss_gb=4.0,
    ) == pytest.approx(2.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=46.0,
        max_total_rss_gb=51.0,
        max_global_rss_gb=85.0,
    ) == pytest.approx(46.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=46.0,
        max_total_rss_gb=51.0,
    ) == pytest.approx(46.0)


def test_run_command_passes_through_success() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; print('ok'); time.sleep(0.2)"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.violation is None
    assert result.peak is not None
    assert result.peak.rss_kb > 0
    assert result.stdout == "ok\n"
    assert result.elapsed_s is not None
    assert result.elapsed_s > 0


def test_run_guarded_binary_capture_preserves_bytes() -> None:
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                "data = sys.stdin.buffer.read(); "
                "sys.stdout.buffer.write(data[::-1]); "
                "sys.stderr.buffer.write(b'err:' + data[:2])"
            ),
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        input=b"\xffabc",
        text=False,
    )

    assert result.returncode == 0
    assert result.stdout == b"cba\xff"
    assert result.stderr == b"err:\xffa"


def test_run_guarded_interrupt_during_sampling_terminates_child_tree() -> None:
    def interrupting_sampler():
        raise KeyboardInterrupt

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(30)"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=interrupting_sampler,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert "memory_guard: interrupted" in result.stderr
    assert result.elapsed_s < 10


def test_run_guarded_interrupt_reuses_last_successful_descendant_snapshot(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root_pid = 4242
    child_pid = 4243
    grandchild_pid = 4244
    samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid, 1, 64, "root", started_at_ns=root_pid
        ),
        child_pid: memory_guard.ProcessSample(
            child_pid, root_pid, 64, "child", started_at_ns=child_pid
        ),
        grandchild_pid: memory_guard.ProcessSample(
            grandchild_pid,
            child_pid,
            64,
            "grandchild",
            started_at_ns=grandchild_pid,
        ),
    }

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

    processes: list[FakePopen] = []

    def fake_popen(command: list[str], **kwargs: object) -> FakePopen:
        proc = FakePopen(command, **kwargs)
        processes.append(proc)
        return proc

    sample_calls = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_calls
        sample_calls += 1
        if sample_calls > 1:
            raise KeyboardInterrupt
        return samples

    terminations: list[dict[str, object]] = []

    def recording_terminate(
        root_pid: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        terminations.append({"root_pid": root_pid, **kwargs})
        processes[0].returncode = -15
        watched = kwargs.get("watched")
        return _guard_termination_report(
            reason=str(kwargs.get("reason", "test_cleanup")),
            root_pid=root_pid,
            root_pgid=root_pid,
            watched_pids=tuple(sorted(watched)) if isinstance(watched, set) else (),
        )

    monkeypatch.setattr(memory_guard.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(
        memory_guard, "terminate_watched_processes", recording_terminate
    )

    result = memory_guard.run_guarded(
        ["fake-python", "-c", "sleep"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=sampler,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert "memory_guard: interrupted" in result.stderr
    assert any(
        {root_pid, child_pid, grandchild_pid}.issubset(call.get("watched", set()))
        for call in terminations
    )
    assert all(call.get("root_owned") is True for call in terminations)


def test_run_guarded_sampler_failure_cleans_then_reraises(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root_pid = 5252
    child_pid = 5253
    samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid, 1, 64, "root", started_at_ns=root_pid
        ),
        child_pid: memory_guard.ProcessSample(
            child_pid, root_pid, 64, "child", started_at_ns=child_pid
        ),
    }

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

    processes: list[FakePopen] = []

    def fake_popen(command: list[str], **kwargs: object) -> FakePopen:
        proc = FakePopen(command, **kwargs)
        processes.append(proc)
        return proc

    sample_calls = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_calls
        sample_calls += 1
        if sample_calls > 1:
            raise RuntimeError("sampler failed after custody")
        return samples

    terminations: list[dict[str, object]] = []

    def recording_terminate(
        root_pid: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        terminations.append({"root_pid": root_pid, **kwargs})
        processes[0].returncode = -15
        watched = kwargs.get("watched")
        return _guard_termination_report(
            reason=str(kwargs.get("reason", "test_cleanup")),
            root_pid=root_pid,
            root_pgid=root_pid,
            watched_pids=tuple(sorted(watched)) if isinstance(watched, set) else (),
        )

    monkeypatch.setattr(memory_guard.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(
        memory_guard, "terminate_watched_processes", recording_terminate
    )

    with pytest.raises(RuntimeError, match="sampler failed after custody"):
        memory_guard.run_guarded(
            ["fake-python", "-c", "sleep"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=sampler,
        )

    assert any(
        {root_pid, child_pid}.issubset(call.get("watched", set()))
        for call in terminations
    )
    assert all(call.get("root_owned") is True for call in terminations)


def test_run_guarded_binds_root_identity_before_first_sampler(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root_pid = 6262

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = 123

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

    process = FakePopen(["fake-python"])
    reports: list[dict[str, object]] = []

    def fake_terminate(root: int, **kwargs: object):
        reports.append({"root": root, **kwargs})
        process.returncode = -15
        return _guard_termination_report(reason="sampler_failure", root_pid=root)

    monkeypatch.setattr(memory_guard.subprocess, "Popen", lambda *_a, **_kw: process)
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        memory_guard,
        "windows_process_handle_started_at_ns",
        lambda handle: 987_654_300 if handle == 123 else None,
    )
    monkeypatch.setattr(memory_guard, "terminate_watched_processes", fake_terminate)

    with pytest.raises(RuntimeError, match="first snapshot failed"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=lambda: (_ for _ in ()).throw(
                RuntimeError("first snapshot failed")
            ),
        )

    tracker = reports[0]["tracker"]
    assert isinstance(tracker, memory_guard.ProcessTreeTracker)
    assert tracker.custody_identities({root_pid}) == {
        root_pid: memory_guard.ProcessIdentity(987_654_300)
    }


def test_run_guarded_persistent_sampler_failure_reaps_owned_child_handle(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root_pid = 6363

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None
        terminate_calls = 0
        kill_calls = 0

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def terminate(self) -> None:
            self.terminate_calls += 1
            self.returncode = 0
            raise ProcessLookupError

        def kill(self) -> None:
            self.kill_calls += 1
            self.returncode = -9

    process = FakePopen(["fake-python"])
    root_sample = memory_guard.ProcessSample(
        root_pid,
        1,
        64,
        "fake-python",
        started_at_ns=root_pid,
    )
    sample_count = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_count
        sample_count += 1
        if sample_count == 1:
            return {root_pid: root_sample}
        raise RuntimeError("persistent snapshot failure")

    monkeypatch.setattr(memory_guard.subprocess, "Popen", lambda *_a, **_kw: process)
    monkeypatch.setattr(
        memory_guard,
        "terminate_watched_processes",
        lambda root, **kwargs: _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            actions=(
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=root,
                    signal=None,
                    signal_name=None,
                    result="skipped_sampler_failure",
                ),
            ),
        ),
    )

    with pytest.raises(RuntimeError, match="persistent snapshot failure"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=sampler,
        )

    assert process.terminate_calls == 1
    assert process.kill_calls == 0
    assert process.returncode == 0


def test_run_guarded_post_loop_sampler_failure_reaps_only_owned_child_handle(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root_pid = 6464
    unrelated_pid = 7474

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None
        terminate_calls = 0
        kill_calls = 0

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def terminate(self) -> None:
            self.terminate_calls += 1

        def kill(self) -> None:
            self.kill_calls += 1
            self.returncode = -9

    process = FakePopen(["fake-python"])
    samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid,
            1,
            2_000_000,
            "fake-python",
            started_at_ns=root_pid,
        ),
        unrelated_pid: memory_guard.ProcessSample(
            unrelated_pid,
            1,
            64,
            "unrelated",
            started_at_ns=unrelated_pid,
        ),
    }
    sample_count = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_count
        sample_count += 1
        if sample_count == 1:
            return samples
        raise RuntimeError("post-loop snapshot failure")

    watched_calls: list[set[int]] = []

    def record_termination(
        root: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        watched = set(kwargs.get("watched", set()))
        watched_calls.append(watched)
        return _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            watched_pids=tuple(sorted(watched)),
            actions=(
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=root,
                    signal=memory_guard.signal.SIGTERM,
                    signal_name="SIGTERM",
                    result="still_live",
                ),
            ),
        )

    monkeypatch.setattr(memory_guard.subprocess, "Popen", lambda *_a, **_kw: process)
    monkeypatch.setattr(memory_guard, "terminate_watched_processes", record_termination)

    with pytest.raises(RuntimeError, match="post-loop snapshot failure"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=sampler,
            cleanup_orphans=False,
        )

    assert process.terminate_calls == 1
    assert process.kill_calls == 1
    assert process.returncode == -9
    assert watched_calls
    assert all(root_pid in watched for watched in watched_calls)
    assert all(unrelated_pid not in watched for watched in watched_calls)


def test_run_guarded_weak_sampler_reaps_only_owned_child_handle(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root_pid = 6565
    unrelated_pid = 7575

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None
        terminate_calls = 0
        kill_calls = 0

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def terminate(self) -> None:
            self.terminate_calls += 1

        def kill(self) -> None:
            self.kill_calls += 1
            self.returncode = -9

    process = FakePopen(["fake-python"])
    weak_samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid,
            1,
            2_000_000,
            "fake-python",
            started_at_ns=None,
        ),
        unrelated_pid: memory_guard.ProcessSample(
            unrelated_pid,
            1,
            64,
            "unrelated",
            started_at_ns=None,
        ),
    }
    watched_calls: list[set[int]] = []

    def record_termination(
        root: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        watched = set(kwargs.get("watched", set()))
        watched_calls.append(watched)
        return _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            watched_pids=tuple(sorted(watched)),
            actions=(
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=root,
                    signal=None,
                    signal_name=None,
                    result="skipped_missing_identity",
                ),
            ),
        )

    monkeypatch.setattr(memory_guard.subprocess, "Popen", lambda *_a, **_kw: process)
    monkeypatch.setattr(memory_guard, "terminate_watched_processes", record_termination)

    result = memory_guard.run_guarded(
        ["fake-python"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=lambda: weak_samples,
        cleanup_orphans=False,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert process.terminate_calls == 1
    assert process.kill_calls == 1
    assert process.returncode == -9
    assert watched_calls
    assert all(unrelated_pid not in watched for watched in watched_calls)
    assert any(
        report.reason == "post_loop_unreaped_child_direct_child_handle"
        for report in result.termination_reports
    )


def test_cleanup_tracked_orphans_terminates_live_tracked_groups(monkeypatch) -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    assert tracker.known_pids is not None
    tracker.known_pids.update({200, 300})
    assert tracker.known_pgids is not None
    tracker.known_pgids.update({100, 300})
    samples = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=100,
            rss_kb=64,
            command="worker same group",
        ),
        300: memory_guard.ProcessSample(
            pid=300,
            ppid=1,
            pgid=300,
            rss_kb=64,
            command="worker new group",
        ),
    }
    calls: list[dict[str, object]] = []
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=300,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
        ),
    )

    def fake_terminate(root_pid, **kwargs):
        calls.append({"root_pid": root_pid, **kwargs})
        return report

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", fake_terminate)

    orphaned = memory_guard.cleanup_tracked_orphans(
        100,
        tracker=tracker,
        sampler=lambda: samples,
        grace=0.125,
    )

    assert orphaned.process_groups == (100, 300)
    assert orphaned.termination_reports == (report,)
    assert calls[0]["root_pid"] == 100
    assert calls[0]["watched"] == {200, 300}
    assert calls[0]["grace"] == 0.125
    assert calls[0]["reason"] == "tracked_orphan_cleanup"


def test_cleanup_tracked_orphans_does_not_report_failed_actions_as_cleaned(
    monkeypatch,
) -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "guard.exe", started_at_ns=1
        ),
        200: memory_guard.ProcessSample(
            200, 100, 20, "worker.exe", started_at_ns=2
        ),
    }
    tracker.update(initial)
    live = {200: initial[200]}
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    monkeypatch.setattr(
        memory_guard,
        "terminate_watched_processes",
        lambda *_args, **_kwargs: report,
    )

    result = memory_guard.cleanup_tracked_orphans(
        100,
        tracker=tracker,
        sampler=lambda: live,
    )

    assert result.process_groups == ()
    assert result.termination_reports == (report,)


def test_cleanup_group_completion_requires_every_detected_member() -> None:
    completed = memory_guard.GuardTerminationAction(
        target_kind="process",
        target_id=200,
        signal=memory_guard.signal.SIGTERM,
        signal_name="SIGTERM",
        result="completed_or_missing",
    )
    failed = memory_guard.GuardTerminationAction(
        target_kind="process",
        target_id=201,
        signal=memory_guard.signal.SIGTERM,
        signal_name="SIGTERM",
        result="failed",
        error="access denied",
    )

    assert memory_guard._fully_completed_process_groups(
        {777: {200, 201}},
        (completed, failed),
    ) == set()
    assert memory_guard._fully_completed_process_groups(
        {777: {200, 201}},
        (
            completed,
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=201,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
        ),
    ) == {777}


def test_cleanup_repo_scoped_orphans_since_baseline_only_drains_tracked_orphans(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    root = memory_guard.ROOT.as_posix()
    tracker = memory_guard.ProcessTreeTracker(100)
    tracker.update(
        {
            100: memory_guard.ProcessSample(
                pid=100,
                ppid=1,
                pgid=100,
                rss_kb=64,
                command=f"{root}/.venv/bin/python3 -m pytest tests/root.py",
            ),
            200: memory_guard.ProcessSample(
                pid=200,
                ppid=100,
                pgid=200,
                rss_kb=64,
                command=f"{root}/.venv/bin/python3 -m molt.cli build main.py",
            ),
            300: memory_guard.ProcessSample(
                pid=300,
                ppid=200,
                pgid=300,
                rss_kb=64,
                command=f"{root}/target/dev-fast/molt-backend --ir-file ir.json",
            ),
        }
    )
    samples = {
        50: memory_guard.ProcessSample(
            pid=50,
            ppid=1,
            pgid=50,
            rss_kb=64,
            command="/bin/zsh -l",
        ),
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=64,
            command=f"{root}/.venv/bin/python3 -m molt.cli build main.py",
        ),
        300: memory_guard.ProcessSample(
            pid=300,
            ppid=200,
            pgid=300,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --ir-file ir.json",
        ),
        400: memory_guard.ProcessSample(
            pid=400,
            ppid=50,
            pgid=400,
            rss_kb=64,
            command=f"{root}/.venv/bin/python3 -m pytest tests/some_test.py",
        ),
        500: memory_guard.ProcessSample(
            pid=500,
            ppid=1,
            pgid=500,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --old",
        ),
        550: memory_guard.ProcessSample(
            pid=550,
            ppid=1,
            pgid=550,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --untracked",
        ),
        600: memory_guard.ProcessSample(
            pid=600,
            ppid=1,
            pgid=600,
            rss_kb=64,
            command="/Applications/Claude.app/Contents/MacOS/Claude",
        ),
        601: memory_guard.ProcessSample(
            pid=601,
            ppid=600,
            pgid=600,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --protected",
        ),
    }
    terminated: list[tuple[int, int]] = []

    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)

    def fake_kill(pid: int, sig: int) -> None:
        if sig == 0 and any(sent_pid == pid for sent_pid, _sig in terminated):
            raise ProcessLookupError
        if sig == memory_guard.signal.SIGTERM:
            terminated.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "kill", fake_kill)

    cleaned = memory_guard.cleanup_repo_scoped_orphans_since_baseline(
        baseline_pgids=frozenset({500}),
        tracker=tracker,
        sampler=lambda: samples,
        grace=0.125,
    )

    assert cleaned.process_groups == (200, 300)
    assert [report.reason for report in cleaned.termination_reports] == [
        "repo_scoped_orphan_cleanup",
        "repo_scoped_orphan_cleanup",
    ]
    assert [report.root_pgid for report in cleaned.termination_reports] == [200, 300]
    assert [
        action.target_id
        for report in cleaned.termination_reports
        for action in report.actions
    ] == [200, 300]
    assert all(
        action.result == "completed_or_missing"
        for report in cleaned.termination_reports
        for action in report.actions
    )
    assert terminated == [
        (200, memory_guard.signal.SIGTERM),
        (300, memory_guard.signal.SIGTERM),
    ]


def test_cleanup_repo_scoped_orphans_revalidates_identity_before_signal(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    root = memory_guard.ROOT.as_posix()
    tracker = memory_guard.ProcessTreeTracker(100)
    tracker.update(
        {
            100: memory_guard.ProcessSample(
                pid=100,
                ppid=1,
                pgid=100,
                rss_kb=64,
                command=f"{root}/.venv/bin/python3 -m pytest tests/root.py",
            ),
            200: memory_guard.ProcessSample(
                pid=200,
                ppid=100,
                pgid=200,
                rss_kb=64,
                command=f"{root}/target/dev-fast/molt-backend --owned",
            ),
        }
    )
    owned_orphan = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --owned",
        )
    }
    reused_pid = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=64,
            command="/Applications/Claude.app/Contents/MacOS/Claude",
        )
    }
    sampler_calls = 0

    def sampler():
        nonlocal sampler_calls
        sampler_calls += 1
        return owned_orphan if sampler_calls <= 2 else reused_pid

    terminated: list[tuple[int, float]] = []
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(
        memory_guard,
        "_terminate_single_pid",
        lambda pid, *, grace: terminated.append((pid, grace)) or True,
    )

    cleaned = memory_guard.cleanup_repo_scoped_orphans_since_baseline(
        baseline_pgids=frozenset(),
        tracker=tracker,
        sampler=sampler,
        grace=0.125,
    )

    assert cleaned.process_groups == ()
    assert len(cleaned.termination_reports) == 1
    assert cleaned.termination_reports[0].actions[0].result == (
        "skipped_identity_mismatch"
    )
    assert terminated == []


def test_terminate_verified_pid_revalidates_identity_before_fallback(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = memory_guard.ROOT.as_posix()
    original = memory_guard.ProcessSample(
        pid=200,
        ppid=1,
        pgid=200,
        rss_kb=64,
        command=f"{root}/target/dev-fast/molt-backend --owned",
        started_at_ns=111,
    )
    reused_pid = memory_guard.ProcessSample(
        pid=200,
        ppid=1,
        pgid=200,
        rss_kb=64,
        command="/Applications/Claude.app/Contents/MacOS/Claude",
        started_at_ns=222,
    )
    sample_sets = iter([{200: original}, {200: reused_pid}])
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999, raising=False)
    monkeypatch.setattr(
        memory_guard,
        "_pid_exited_or_unobservable",
        lambda pid, *, grace: False,
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: None if sig == 0 else sent.append((pid, sig)),
    )

    actions = memory_guard.terminate_verified_pid(
        200,
        memory_guard.process_identity(original),
        sampler=lambda: next(sample_sets),
        grace=0.125,
    )

    assert [action.result for action in actions] == [
        "still_live",
        "skipped_identity_mismatch",
    ]
    assert sent == [(200, memory_guard.signal.SIGTERM)]


def test_terminate_verified_pid_preserves_host_control_plane(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sample = memory_guard.ProcessSample(
        pid=300,
        ppid=1,
        pgid=300,
        rss_kb=64,
        command="codex exec --dangerously-skip-approvals",
        started_at_ns=333,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999, raising=False)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: None if sig == 0 else sent.append((pid, sig)),
    )

    actions = memory_guard.terminate_verified_pid(
        300,
        memory_guard.process_identity(sample),
        sampler=lambda: {300: sample},
        grace=0.125,
    )

    assert [action.result for action in actions] == ["skipped_host_control_plane"]
    assert sent == []


def test_cleanup_tracked_orphans_sampler_failure_uses_remembered_watched(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    assert tracker.known_pids is not None
    tracker.known_pids.add(200)
    remembered_samples = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=None,
            rss_kb=64,
            command="escaped worker",
        )
    }
    calls: list[dict[str, object]] = []

    def failing_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        raise RuntimeError("sampler unavailable")

    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def fake_terminate(
        root_pid: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        calls.append({"root_pid": root_pid, **kwargs})
        return report

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", fake_terminate)

    with pytest.raises(RuntimeError, match="sampler unavailable"):
        memory_guard.cleanup_tracked_orphans(
            100,
            tracker=tracker,
            sampler=failing_sampler,
            remembered_samples=remembered_samples,
            remembered_watched={200},
        )

    assert calls and calls[0]["watched"] == {200}
    assert calls[0]["reason"] == "tracked_orphan_cleanup"


def test_windows_cleanup_sampler_failure_never_signals_remembered_pid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    root = memory_guard.ProcessSample(
        100, 1, 10, "guard.exe", started_at_ns=100
    )
    child = memory_guard.ProcessSample(
        200, 100, 20, "worker.exe", started_at_ns=200
    )
    remembered = {100: root, 200: child}
    tracker.update(remembered)
    sent: list[tuple[int, int]] = []

    def failing_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        raise RuntimeError("live sampler unavailable")

    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent.append((pid, sig)),
    )

    with pytest.raises(RuntimeError, match="live sampler unavailable"):
        memory_guard.cleanup_tracked_orphans(
            100,
            tracker=tracker,
            sampler=failing_sampler,
            remembered_samples=remembered,
            remembered_watched={200},
        )

    assert sent == []


def test_pid_permission_error_is_live_unknown_not_completed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sample = memory_guard.ProcessSample(
        pid=300,
        ppid=1,
        pgid=300,
        rss_kb=64,
        command="worker",
        started_at_ns=333,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999, raising=False)

    def permission_liveness(pid: int, sig: int) -> None:
        if sig == 0:
            raise PermissionError("EPERM")
        sent.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "kill", permission_liveness)

    action = memory_guard._terminate_pid_if_identity_action(
        300,
        memory_guard.process_identity(sample),
        sampler=lambda: {300: sample},
        grace=0.01,
    )

    assert action.result == "still_live"
    assert sent == [(300, memory_guard.signal.SIGTERM)]


def test_process_group_permission_error_is_live_unknown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda _pgid, _sig: (_ for _ in ()).throw(PermissionError("EPERM")),
        raising=False,
    )

    assert not memory_guard._process_group_exited_or_unobservable(300, grace=0.01)


def test_completed_process_group_does_not_emit_redundant_member_kill(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    custody = memory_guard._process_custody
    samples = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=100, started_at_ns=101
        ),
    }
    monkeypatch.setattr(custody, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(custody.os, "name", "posix", raising=False)
    monkeypatch.setattr(custody.os, "getpid", lambda: 999)
    monkeypatch.setattr(custody, "_safe_getpgrp", lambda: 999)
    monkeypatch.setattr(custody, "_safe_getpgid", lambda _pid: 100)
    monkeypatch.setattr(custody, "_safe_getsid", lambda _pid: 100)
    monkeypatch.setattr(
        custody,
        "_current_protected_process_group_ids",
        lambda _samples, **_kwargs: set(),
    )
    monkeypatch.setattr(
        custody,
        "_terminate_process_group_if_identities_match_action",
        lambda pgid, _identities, **_kwargs: memory_guard.GuardTerminationAction(
            target_kind="process_group",
            target_id=pgid,
            signal=memory_guard.signal.SIGTERM,
            signal_name="SIGTERM",
            result="completed_or_missing",
        ),
    )
    monkeypatch.setattr(
        custody,
        "_send_pid_signal_if_identity_action",
        lambda *_args, **_kwargs: pytest.fail(
            "completed group must not emit redundant member SIGKILL"
        ),
    )

    report = custody.terminate_watched_processes(
        100,
        samples=samples,
        watched=set(samples),
        expected_identities={
            pid: memory_guard.process_identity(sample)
            for pid, sample in samples.items()
        },
        sampler=lambda: samples,
        root_owned=True,
    )

    assert [action.result for action in report.actions] == ["completed_or_missing"]


def test_run_command_cleans_tracked_orphans_by_default(monkeypatch) -> None:
    calls: list[dict[str, object]] = []
    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def fake_cleanup(root_pid, **kwargs):
        calls.append({"root_pid": root_pid, **kwargs})
        return memory_guard.GuardOrphanCleanupResult(
            process_groups=(777,),
            termination_reports=(report,),
        )

    monkeypatch.setattr(memory_guard, "cleanup_tracked_orphans", fake_cleanup)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.orphaned_process_groups == (777,)
    assert result.termination_reports == (report,)
    assert len(calls) == 1


def test_run_command_timeout_reports_post_baseline_repo_orphan_cleanup(
    monkeypatch,
) -> None:
    report = _guard_termination_report(
        reason="repo_scoped_orphan_cleanup",
        root_pid=222,
        root_pgid=222,
    )

    def fake_cleanup(**kwargs):
        assert kwargs["baseline_pgids"] == frozenset()
        return memory_guard.GuardOrphanCleanupResult(
            process_groups=(222,),
            termination_reports=(report,),
        )

    monkeypatch.setattr(
        memory_guard,
        "cleanup_repo_scoped_orphans_since_baseline",
        fake_cleanup,
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=0.01,
        sampler=lambda: {},
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert result.orphaned_process_groups == (222,)
    assert report in result.termination_reports


def test_run_guarded_observes_child_exit_before_timeout_race(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FakePopen:
        pid = 4242
        stdin = None
        returncode: int | None = None
        _handle = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                if timeout is not None and timeout <= 0.02:
                    time.sleep(0.06)
                    self.returncode = 0
                    raise subprocess.TimeoutExpired(self.command, timeout)
                self.returncode = 0
            return self.returncode

    monkeypatch.setattr(memory_guard.os, "name", "nt", raising=False)
    monkeypatch.setattr(memory_guard.subprocess, "Popen", FakePopen)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "pass"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=lambda: {},
        timeout=0.05,
        cleanup_orphans=False,
    )

    assert result.returncode == 0
    assert result.timed_out is False


def test_run_command_captures_large_stdout_without_pipe_deadlock() -> None:
    payload_size = 512 * 1024
    script = (
        "import sys; "
        f"sys.stdout.write('x' * {payload_size}); "
        "sys.stdout.flush(); "
        "sys.stderr.write('done\\n')"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=5.0,
    )

    assert result.returncode == 0
    assert len(result.stdout) == payload_size
    assert result.stderr == "done\n"


def test_run_command_feeds_stdin_under_guard() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import sys; print(sys.stdin.read().upper())"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        input="guarded stdin",
    )

    assert result.returncode == 0
    assert result.stdout == "GUARDED STDIN\n"


def test_run_command_elapsed_excludes_guard_child_runner_startup() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(0.03); print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=1.0,
        child_rlimit_kb=1_000_000,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.elapsed_s is not None
    assert result.elapsed_s >= 0.02
    nested_guard_budget = memory_guard.ACTIVE_ENV in os.environ
    elapsed_ceiling = 8.0 if nested_guard_budget else (2.0 if os.name == "nt" else 0.5)
    assert result.elapsed_s < elapsed_ceiling


def test_run_command_ignores_samples_without_root_pid() -> None:
    def sampler() -> dict[int, memory_guard.ProcessSample]:
        return {
            999_999: memory_guard.ProcessSample(999_999, 1, 1, "missing-root"),
        }

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=sampler,
    )

    assert result.returncode == 0
    assert result.violation is None


def test_run_command_returns_guard_code_on_real_low_limit() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1,
        poll_interval=0.01,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.rss_kb > 1


def test_run_command_fast_start_poll_catches_allocator_before_slow_poll() -> None:
    # Hold the allocation beyond the configured slow poll; otherwise Windows
    # full-table sampling under load can turn this into a scheduler race.
    script = (
        "import time; "
        "buf = bytearray(192 * 1024 * 1024); "
        "time.sleep(10.0); "
        "print(len(buf))"
    )
    sampler = memory_guard.sample_processes if os.name == "nt" else (lambda: {})

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=96 * 1024,
        max_total_rss_kb=160 * 1024,
        poll_interval=5.0,
        child_rlimit_kb=None,
        sampler=sampler,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.elapsed_s is not None
    assert result.elapsed_s < 5.0


def test_run_command_rusage_catches_short_lived_allocator_spike() -> None:
    if memory_guard.os.name != "posix" or not hasattr(memory_guard.os, "wait4"):
        return
    script = "import os\nbuf = bytearray(192 * 1024 * 1024)\nos._exit(0)"

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=96 * 1024,
        max_total_rss_kb=160 * 1024,
        poll_interval=1.0,
        child_rlimit_kb=None,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.scope == "process_rusage"


def test_run_command_returns_timeout_code_when_wall_clock_expires() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=0.01,
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert "timeout after" in result.stderr


def test_run_command_timeout_teardown_uses_bounded_wait(monkeypatch) -> None:
    waits: list[float | None] = []

    class FakeProc:
        pid = 987654
        returncode: int | None = None
        stdin = None

        def __init__(self, command, **_kwargs):  # type: ignore[no-untyped-def]
            self.command = list(command)

        def wait(self, timeout=None):  # type: ignore[no-untyped-def]
            waits.append(timeout)
            if timeout is None:
                raise AssertionError("memory guard attempted an unbounded wait")
            raise subprocess.TimeoutExpired(self.command, timeout)

        def poll(self):  # type: ignore[no-untyped-def]
            return self.returncode

        def terminate(self) -> None:
            pass

        def kill(self) -> None:
            pass

    monkeypatch.setattr(memory_guard.subprocess, "Popen", FakeProc)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1_000_000,
        poll_interval=0.001,
        timeout=0.001,
        env={"MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC": "0.001"},
        sampler=lambda: {},
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert "termination wait expired" in result.stderr
    assert waits
    assert None not in waits


def test_exit_signal_payload_classifies_direct_signal_status() -> None:
    assert memory_guard._exit_signal_payload(-15) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": False,
    }


def test_exit_signal_payload_names_posix_only_signal_numbers() -> None:
    assert memory_guard._exit_signal_payload(-9) == {
        "signal": 9,
        "name": "SIGKILL",
        "conventional_shell_status": False,
    }


def test_exit_signal_payload_classifies_shell_signal_status() -> None:
    assert memory_guard._exit_signal_payload(143) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }


def test_exit_signal_payload_classifies_windows_sigterm_status(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(memory_guard, "_is_windows_process_model", lambda: True)
    assert memory_guard._exit_signal_payload(15) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": False,
    }


def test_cargo_incremental_quarantine_moves_only_incremental_dirs(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    debug_file = target / "debug" / "incremental" / "unit-a" / "work.o"
    triple_file = (
        target
        / "aarch64-apple-darwin"
        / "dev-fast"
        / "incremental"
        / "unit-b"
        / "work.o"
    )
    non_incremental = target / "debug" / "deps" / "libmolt.rlib"
    for path in (debug_file, triple_file, non_incremental):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(path.name, encoding="utf-8")

    receipt = memory_guard._quarantine_cargo_incremental_state(
        reason="signal_exit",
        target_dir=target,
        command=["cargo", "test"],
        cwd=tmp_path,
    )

    assert not (target / "debug" / "incremental").exists()
    assert not (target / "aarch64-apple-darwin" / "dev-fast" / "incremental").exists()
    assert non_incremental.exists()
    assert len(receipt.moved_paths) == 2
    assert receipt.errors == ()
    assert receipt.quarantine_dir is not None
    quarantine_dir = Path(receipt.quarantine_dir)
    assert (quarantine_dir / "debug" / "incremental" / "unit-a" / "work.o").exists()
    assert (
        quarantine_dir
        / "aarch64-apple-darwin"
        / "dev-fast"
        / "incremental"
        / "unit-b"
        / "work.o"
    ).exists()
    assert receipt.receipt_path is not None
    payload = json.loads(Path(receipt.receipt_path).read_text(encoding="utf-8"))
    assert payload["reason"] == "signal_exit"
    assert payload["target_dir"] == str(target)
    assert payload["command"] == ["cargo", "test"]
    assert len(payload["moved_paths"]) == 2


def test_cargo_incremental_quarantine_skips_sibling_session_targets(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    root_incremental = target / "release-fast" / "incremental" / "unit-a" / "work.o"
    session_incremental = (
        target
        / "sessions"
        / "proof-rust"
        / "debug"
        / "incremental"
        / "unit-b"
        / "work.o"
    )
    session_triple_incremental = (
        target
        / "sessions"
        / "proof-wasm"
        / "wasm32-wasip1"
        / "release-output"
        / "incremental"
        / "unit-c"
        / "work.o"
    )
    old_receipt_incremental = (
        target
        / ".molt_state"
        / "quarantine"
        / "cargo_incremental"
        / "old"
        / "debug"
        / "incremental"
        / "unit-d"
        / "work.o"
    )
    nested_session_receipt_incremental = (
        target
        / "sessions"
        / "proof-wasm"
        / ".molt_state"
        / "quarantine"
        / "cargo_incremental"
        / "old"
        / "debug"
        / "incremental"
        / "unit-e"
        / "work.o"
    )
    for path in (
        root_incremental,
        session_incremental,
        session_triple_incremental,
        old_receipt_incremental,
        nested_session_receipt_incremental,
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(path.name, encoding="utf-8")

    receipt = memory_guard._quarantine_cargo_incremental_state(
        reason="orphaned_processes_cleaned",
        target_dir=target,
        command=["cargo", "test"],
        cwd=tmp_path,
    )

    assert not (target / "release-fast" / "incremental").exists()
    assert session_incremental.exists()
    assert session_triple_incremental.exists()
    assert old_receipt_incremental.exists()
    assert nested_session_receipt_incremental.exists()
    assert receipt.errors == ()
    assert [Path(move.original_path) for move in receipt.moved_paths] == [
        target / "release-fast" / "incremental"
    ]


def test_cargo_incremental_quarantine_moves_explicit_session_target(
    tmp_path: Path,
) -> None:
    sessions_root = tmp_path / "target" / "sessions"
    target = sessions_root / "proof-rust"
    session_incremental = target / "debug" / "incremental" / "unit-a" / "work.o"
    sibling_incremental = (
        sessions_root / "proof-wasm" / "debug" / "incremental" / "unit-b" / "work.o"
    )
    for path in (session_incremental, sibling_incremental):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(path.name, encoding="utf-8")

    receipt = memory_guard._quarantine_cargo_incremental_state(
        reason="signal_exit",
        target_dir=target,
        command=["cargo", "build"],
        cwd=tmp_path,
    )

    assert not (target / "debug" / "incremental").exists()
    assert sibling_incremental.exists()
    assert receipt.errors == ()
    assert [Path(move.original_path) for move in receipt.moved_paths] == [
        target / "debug" / "incremental"
    ]


def test_cargo_incremental_quarantine_prunes_old_receipts(tmp_path: Path) -> None:
    target = tmp_path / "target"
    parent = target / ".molt_state" / "quarantine" / "cargo_incremental"
    base_mtime = time.time() - 600
    for index in range(3):
        stale = parent / f"stale-{index}"
        stale.mkdir(parents=True)
        stale_mtime = base_mtime + index
        os.utime(stale, (stale_mtime, stale_mtime))
    live_file = target / "debug" / "incremental" / "unit" / "work.o"
    live_file.parent.mkdir(parents=True, exist_ok=True)
    live_file.write_text("work", encoding="utf-8")

    receipt = memory_guard._quarantine_cargo_incremental_state(
        reason="timeout",
        target_dir=target,
        command=["cargo", "build"],
        cwd=tmp_path,
        retention_keep=2,
    )

    assert receipt.quarantine_dir is not None
    remaining = sorted(path.name for path in parent.iterdir() if path.is_dir())
    assert len(remaining) == 2
    assert Path(receipt.quarantine_dir).name in remaining
    assert receipt.pruned_quarantine_dirs


def test_run_guarded_signal_exit_quarantines_cargo_incremental(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    live_file = target / "debug" / "incremental" / "unit" / "work.o"
    live_file.parent.mkdir(parents=True, exist_ok=True)
    live_file.write_text("work", encoding="utf-8")

    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            "import os, signal; os.kill(os.getpid(), signal.SIGTERM)",
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={"CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )

    assert result.returncode == (15 if os.name == "nt" else -15)
    assert result.cargo_incremental_quarantine is None
    assert live_file.exists()

    fake_cargo = tmp_path / ("cargo.cmd" if os.name == "nt" else "cargo")
    if os.name == "nt":
        fake_cargo.write_text(
            f'@echo off\r\n"{sys.executable}" -c "import os, signal; '
            'os.kill(os.getpid(), signal.SIGTERM)"\r\n',
            encoding="utf-8",
        )
    else:
        fake_cargo.write_text(
            f"#!{sys.executable}\n"
            "import os, signal\n"
            "os.kill(os.getpid(), signal.SIGTERM)\n",
            encoding="utf-8",
        )
    fake_cargo.chmod(0o755)
    result = memory_guard.run_guarded(
        [str(fake_cargo)],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={"CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )

    assert result.returncode != 0
    assert result.cargo_incremental_quarantine is not None
    assert "quarantined Cargo incremental state" in result.stderr
    assert not (target / "debug" / "incremental").exists()


def _run_guarded_cargo_with_fake_orphan_cleanup(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    *,
    exit_code: int,
) -> tuple[
    memory_guard.GuardResult,
    list[dict[str, object]],
    memory_guard.GuardTerminationReport,
]:
    target = tmp_path / "target"
    calls: list[dict[str, object]] = []
    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def fake_cleanup(root_pid: int, **kwargs: object):
        return memory_guard.GuardOrphanCleanupResult(
            process_groups=(777,),
            termination_reports=(report,),
        )

    def fake_quarantine(**kwargs: object):
        calls.append(kwargs)
        return memory_guard.CargoIncrementalQuarantine(
            reason=str(kwargs["reason"]),
            recorded_at="2026-07-09T00:00:00Z",
            target_dir=str(target),
            quarantine_dir=None,
            command=tuple(kwargs["command"]),
            cwd=str(kwargs["cwd"]),
        )

    monkeypatch.setattr(memory_guard, "cleanup_tracked_orphans", fake_cleanup)
    monkeypatch.setattr(
        memory_guard,
        "_quarantine_cargo_incremental_state",
        fake_quarantine,
    )

    script = "print('ok')" if exit_code == 0 else f"import sys; sys.exit({exit_code})"
    result = memory_guard.run_guarded(
        [sys.executable, "-c", script, "cargo"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={"CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )
    return result, calls, report


def test_successful_cargo_orphan_cleanup_does_not_quarantine_incremental(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    result, calls, report = _run_guarded_cargo_with_fake_orphan_cleanup(
        monkeypatch,
        tmp_path,
        exit_code=0,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.orphaned_process_groups == (777,)
    assert result.termination_reports == (report,)
    assert result.cargo_incremental_quarantine is None
    assert calls == []


def test_failed_cargo_orphan_cleanup_quarantines_incremental(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    result, calls, _report = _run_guarded_cargo_with_fake_orphan_cleanup(
        monkeypatch,
        tmp_path,
        exit_code=3,
    )

    assert result.returncode == 3
    assert result.orphaned_process_groups == (777,)
    assert result.cargo_incremental_quarantine is not None
    assert result.cargo_incremental_quarantine.reason == "orphaned_processes_cleaned"
    assert [call["reason"] for call in calls] == ["orphaned_processes_cleaned"]


def test_main_enforces_timeout_and_writes_summary(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    summary_path = tmp_path / "timeout-summary.json"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--timeout",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "import time; time.sleep(10)",
        ]
    )

    assert rc == memory_guard.TIMEOUT_RETURN_CODE
    assert "timeout after" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == memory_guard.TIMEOUT_RETURN_CODE
    assert payload["timed_out"] is True
    assert payload["violation"] is None
    assert payload["exit_signal"] is None
    assert payload["incident"]["reason"] == "timeout"
    assert payload["incident"]["cleanup"].startswith("terminated tracked process tree")


def test_main_writes_summary_when_guard_parent_receives_sigterm(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    if memory_guard.os.name != "posix":
        return
    summary_path = tmp_path / "guard-sigterm-summary.json"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--timeout",
            "5",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            (
                "import os, signal, time; "
                "os.kill(os.getppid(), signal.SIGTERM); "
                "time.sleep(10)"
            ),
        ]
    )

    assert rc == 143
    assert "guard parent received SIGTERM" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 143
    assert payload["timed_out"] is False
    assert payload["violation"] is None
    assert payload["exit_signal"] is None
    assert payload["guard_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "guard_interrupted"
    assert payload["incident"]["cleanup"] == "terminated tracked process tree"
    assert payload["incident"]["signal"] == payload["guard_signal"]


def test_run_guarded_restores_signal_handlers_after_post_launch_exception() -> None:
    if memory_guard.os.name != "posix":
        return
    watched_signals = [
        sig
        for sig in (
            getattr(signal, "SIGTERM", None),
            getattr(signal, "SIGINT", None),
            getattr(signal, "SIGHUP", None),
        )
        if sig is not None
    ]
    previous_handlers = {sig: signal.getsignal(sig) for sig in watched_signals}
    sampler_calls = 0

    def failing_sampler():
        nonlocal sampler_calls
        sampler_calls += 1
        if sampler_calls == 1:
            return {}
        raise RuntimeError("injected sampler failure")

    with pytest.raises(RuntimeError, match="injected sampler failure"):
        memory_guard.run_guarded(
            [sys.executable, "-c", "import time; time.sleep(5)"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            timeout=5,
            sampler=failing_sampler,
        )

    assert sampler_calls >= 2
    assert {sig: signal.getsignal(sig) for sig in watched_signals} == previous_handlers


def test_summary_json_keeps_rss_incident_primary_when_guard_signal_is_secondary(
    tmp_path,
) -> None:
    summary_path = tmp_path / "rss-plus-guard-signal.json"
    violation = memory_guard.RssViolation(
        pid=123,
        rss_kb=2_000_000,
        command="python worker.py",
        scope="process",
    )

    memory_guard._write_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        result=memory_guard.GuardResult(
            returncode=memory_guard.GUARD_RETURN_CODE,
            violation=violation,
            peak=violation,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.1,
            guard_signal=signal.SIGTERM,
        ),
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["exit_signal"] is None
    assert payload["guard_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "rss_limit_exceeded"
    assert payload["incident"]["guard_signal"] == payload["guard_signal"]


def test_summary_json_keeps_timeout_primary_when_guard_signal_is_secondary(
    tmp_path,
) -> None:
    summary_path = tmp_path / "timeout-plus-guard-signal.json"

    memory_guard._write_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        result=memory_guard.GuardResult(
            returncode=memory_guard.TIMEOUT_RETURN_CODE,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            timed_out=True,
            elapsed_s=5.0,
            guard_signal=signal.SIGTERM,
        ),
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["exit_signal"] is None
    assert payload["incident"]["reason"] == "timeout"
    assert payload["incident"]["guard_signal"] == payload["guard_signal"]


def test_main_writes_running_summary_before_launch_result(
    tmp_path, monkeypatch
) -> None:
    summary_path = tmp_path / "running-summary.json"

    def fake_run_guarded(_command, **_kwargs):
        assert _kwargs["running_summary_json"] == str(summary_path)
        payload = json.loads(summary_path.read_text(encoding="utf-8"))
        assert payload["status"] == "running"
        assert payload["returncode"] is None
        assert payload["child_process"] is None
        assert payload["incident"]["reason"] == "guard_started"
        assert payload["repro"]["summary_json"] == str(summary_path)
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    final_payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert final_payload["returncode"] == 0
    assert "status" not in final_payload


def test_running_summary_refresh_records_spawned_child_process(tmp_path) -> None:
    summary_path = tmp_path / "running-summary-child.json"
    child = memory_guard.GuardedChildProcess(
        pid=1234,
        pgid=None,
        sid=None,
        command=(sys.executable, "-c", "pass"),
        started_at="2026-07-02T17:03:11Z",
    )

    memory_guard._write_running_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        child_process=child,
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["status"] == "running"
    assert payload["child_process"]["pid"] == 1234
    assert payload["child_process"]["command"] == [sys.executable, "-c", "pass"]
    assert payload["incident"]["reason"] == "child_running"
    assert payload["repro"]["summary_json"] == str(summary_path)


def test_main_reports_signal_status_without_guard_violation(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "signal-summary.json"

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=143,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(143)",
        ]
    )

    assert rc == 143
    assert "SIGTERM status" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 143
    assert payload["child_rlimit_gb"] == pytest.approx(1.0)
    assert payload["timed_out"] is False
    assert payload["violation"] is None
    assert payload["exit_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "signal_exit"
    assert payload["incident"]["elapsed_s"] == pytest.approx(0.3)


def test_main_reports_guard_signal_name_from_guard_signal_not_returncode(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "rss-plus-guard-signal-summary.json"
    violation = memory_guard.RssViolation(
        pid=123,
        rss_kb=2_000_000,
        command="python worker.py",
        scope="process",
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=137,
            violation=violation,
            peak=violation,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
            guard_signal=signal.SIGTERM,
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(137)",
        ]
    )

    assert rc == 137
    stderr = capsys.readouterr().err
    assert "guard parent received SIGTERM" in stderr
    assert "guard parent received SIGKILL" not in stderr
    assert "not classified as an RSS limit trip" not in stderr
    assert "RSS limit incident remains the primary classification" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 137
    assert payload["guard_signal"]["name"] == "SIGTERM"
    assert payload["incident"]["reason"] == "rss_limit_exceeded"


def test_main_reports_cargo_incremental_quarantine_summary(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "signal-summary.json"
    target = tmp_path / "target"
    quarantine = target / ".molt_state" / "quarantine" / "cargo_incremental" / "q"
    receipt = memory_guard.CargoIncrementalQuarantine(
        reason="signal_exit",
        recorded_at="2026-06-12T00:00:00Z",
        target_dir=str(target),
        quarantine_dir=str(quarantine),
        command=("cargo", "test"),
        cwd=str(tmp_path),
        moved_paths=(
            memory_guard.CargoIncrementalQuarantineMove(
                original_path=str(target / "debug" / "incremental"),
                quarantined_path=str(quarantine / "debug" / "incremental"),
            ),
        ),
        receipt_path=str(quarantine / "receipt.json"),
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=143,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
            cargo_incremental_quarantine=receipt,
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            "cargo",
            "test",
        ]
    )

    assert rc == 143
    stderr = capsys.readouterr().err
    assert "quarantined Cargo incremental state after signal_exit" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["cargo_incremental_quarantine"]["reason"] == "signal_exit"
    assert payload["cargo_incremental_quarantine"]["target_dir"] == str(target)
    assert len(payload["cargo_incremental_quarantine"]["moved_paths"]) == 1
    assert payload["incident"]["cleanup"] == "quarantined Cargo incremental state"


def test_main_reports_incident_repro_context(
    tmp_path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch,
) -> None:
    summary_path = tmp_path / "rss-summary.json"
    current_root = tmp_path / "pytest-memory-guard"
    current_test_path = current_root / "pytest-current-test.json"
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", current_root)
    current_root.mkdir(parents=True)
    current_test_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "nodeid": "tests/test_memory_guard_tool.py::live_unit",
                "phase": "call",
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    env = {
        "CARGO_BUILD_JOBS": "2",
        "CARGO_INCREMENTAL": "1",
        "PATH": "/usr/bin",
        "PYTEST_CURRENT_TEST": "tests/test_memory_guard_tool.py::unit (call)",
        "MOLT_PYTEST_CURRENT_TEST_FILE": str(current_test_path),
        "MOLT_SESSION_ID": "unit-session",
        "SECRET_TOKEN": "must-not-leak",
        "UV_LINK_MODE": "copy",
        "UV_PROJECT_ENVIRONMENT": str(tmp_path / "uv-project-env"),
    }

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=memory_guard.GUARD_RETURN_CODE,
            violation=memory_guard.RssViolation(
                pid=321,
                rss_kb=4 * 1024 * 1024,
                command="python hungry.py",
                scope="process_tree",
            ),
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=1.25,
            limit_at_violation=memory_guard.ResolvedMemoryLimits(
                max_process_rss_kb=2 * 1024 * 1024,
                max_total_rss_kb=3 * 1024 * 1024,
            ),
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "2",
            "--max-total-rss-gb",
            "3",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "pass",
        ],
        environ=env,
    )

    assert rc == memory_guard.GUARD_RETURN_CODE
    stderr = capsys.readouterr().err
    assert "memory_guard: repro context:" in stderr
    assert "tests/test_memory_guard_tool.py::unit" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    repro = payload["repro"]
    assert repro["command"] == [sys.executable, "-c", "pass"]
    assert repro["pytest"]["current_test"] == env["PYTEST_CURRENT_TEST"]
    assert (
        repro["pytest"]["current_test_file"]["payload"]["nodeid"]
        == "tests/test_memory_guard_tool.py::live_unit"
    )
    assert repro["env"]["MOLT_SESSION_ID"] == "unit-session"
    assert repro["env"]["CARGO_BUILD_JOBS"] == "2"
    assert repro["env"]["CARGO_INCREMENTAL"] == "1"
    assert repro["env"]["UV_LINK_MODE"] == "copy"
    assert repro["env"]["UV_PROJECT_ENVIRONMENT"] == str(tmp_path / "uv-project-env")
    assert "SECRET_TOKEN" not in repro["env"]
    assert repro["limits"]["max_total_rss_gb"] == pytest.approx(3.0)

    env_delta = memory_guard._safe_repro_env_delta(
        env,
        baseline={
            "CARGO_BUILD_JOBS": "8",
            "CARGO_INCREMENTAL": "0",
            "UV_PROJECT_ENVIRONMENT": str(tmp_path / "old-uv-project-env"),
            "SECRET_TOKEN": "baseline-secret",
        },
    )
    assert env_delta["changed"]["CARGO_BUILD_JOBS"] == {"from": "8", "to": "2"}
    assert env_delta["changed"]["CARGO_INCREMENTAL"] == {"from": "0", "to": "1"}
    assert env_delta["changed"]["UV_PROJECT_ENVIRONMENT"] == {
        "from": str(tmp_path / "old-uv-project-env"),
        "to": str(tmp_path / "uv-project-env"),
    }
    assert "SECRET_TOKEN" not in env_delta["changed"]


def test_repro_context_platform_detail_does_not_spawn_subprocess(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    def forbidden_platform_detail() -> str:
        raise AssertionError("platform.platform must not run in summary emission")

    monkeypatch.setattr(memory_guard.platform, "platform", forbidden_platform_detail)

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-c", "pass"],
        cwd=tmp_path,
        environ={},
    )

    assert repro["host"]["platform"] == sys.platform
    assert repro["host"]["platform_detail"]


def test_repro_context_reads_xdist_worker_current_test_sidecars(
    tmp_path: Path,
    monkeypatch,
) -> None:
    current_root = tmp_path / "pytest-memory-guard"
    aggregate_path = current_root / "pytest-current-test.json"
    worker_dir = aggregate_path.with_name(f"{aggregate_path.name}.d")
    worker_dir.mkdir(parents=True)
    worker_path = worker_dir / "gw0-4321_current-test.json"
    worker_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "pid": 4321,
                "nodeid": "tests/test_xdist.py::test_memory",
                "phase": "call",
                "xdist_worker": "gw0",
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", current_root)
    monkeypatch.setattr(
        memory_guard,
        "sample_processes",
        lambda: {
            4321: memory_guard.ProcessSample(
                pid=4321,
                ppid=100,
                rss_kb=1,
                command="pytest worker gw0",
            ),
            9876: memory_guard.ProcessSample(
                pid=9876,
                ppid=4321,
                rss_kb=4 * 1024 * 1024,
                command="python hungry.py",
            ),
        },
    )

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest", "-n", "2"],
        cwd=tmp_path,
        environ={
            "MOLT_PYTEST_CURRENT_TEST_FILE": str(aggregate_path),
            "PYTEST_XDIST_WORKER": "",
        },
        incident_pid=9876,
    )

    current_test = repro["pytest"]["current_test_file"]
    assert current_test["missing"] is True
    records = current_test["worker_records"]
    assert records[0]["incident_match"] == "pid_lineage"
    assert records[0]["payload"]["nodeid"] == "tests/test_xdist.py::test_memory"


def test_repro_context_rejects_noncanonical_current_test_file(
    tmp_path: Path,
    monkeypatch,
) -> None:
    current_root = tmp_path / "pytest-memory-guard"
    outside_path = tmp_path / "outside" / "pytest-current-test.json"
    outside_path.parent.mkdir()
    outside_path.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", current_root)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={"MOLT_PYTEST_CURRENT_TEST_FILE": str(outside_path)},
    )

    current_test = repro["pytest"]["current_test_file"]
    assert current_test["rejected"] == "noncanonical"
    assert current_test["canonical_root"] == str(current_root)


def test_repro_context_includes_bounded_host_control_plane(
    monkeypatch, tmp_path: Path
) -> None:
    long_command = "/Applications/Codex.app/Contents/MacOS/Codex " + ("x" * 800)
    samples = {
        10: memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=500_000,
            command=long_command,
        ),
        11: memory_guard.ProcessSample(
            pid=11,
            ppid=10,
            pgid=10,
            rss_kb=200_000,
            command="/Users/adpena/Projects/molt/target/release-fast/molt-backend",
        ),
        999: memory_guard.ProcessSample(
            pid=999,
            ppid=10,
            pgid=999,
            rss_kb=10,
            command="python tools/memory_guard.py",
        ),
    }
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getppid", lambda: 10)
    monkeypatch.setattr(memory_guard, "_safe_getpgrp", lambda: 999)

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={},
    )

    host = repro["host_control_plane"]
    assert host["host_pgids"] == [10]
    assert 10 in host["protected_pgids"]
    assert host["samples"][0]["pid"] == 10
    assert host["samples"][0]["command"].endswith("...<truncated>")
    assert len(host["samples"][0]["command"]) < len(long_command)


def test_main_rejects_unsafe_threshold(capsys: pytest.CaptureFixture[str]) -> None:
    rc = memory_guard.main(["--max-rss-gb", "112", "--", sys.executable, "-c", "pass"])

    assert rc == 2
    assert "below 112" in capsys.readouterr().err


def test_main_rejects_unsafe_total_threshold(
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc = memory_guard.main(
        ["--max-total-rss-gb", "112", "--", sys.executable, "-c", "pass"]
    )

    assert rc == 2
    assert "below 112" in capsys.readouterr().err


def test_parser_accepts_process_and_tree_rss_aliases() -> None:
    args = memory_guard._parser().parse_args(
        [
            "--max-process-rss-gb",
            "1.5",
            "--max-tree-rss-gb",
            "2.5",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )
    group_args = memory_guard._parser().parse_args(
        [
            "--max-group-rss-gb",
            "3.5",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )

    assert args.max_rss_gb == 1.5
    assert args.max_total_rss_gb == 2.5
    assert group_args.max_total_rss_gb == 3.5


def test_main_reexec_hides_guarded_command_from_guard_argv(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    marker = "molt-backend-marker"
    captured: dict[str, object] = {}
    stdio = {"stdin": "in", "stdout": "out", "stderr": "err"}

    def fake_execve(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(73)

    def fake_subprocess_run(argv, *, env, check, **kwargs):
        assert check is False
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        captured["run_kwargs"] = dict(kwargs)
        return subprocess.CompletedProcess(argv, 73)

    main_argv = [
        "--max-rss-gb",
        "1",
        "--poll-interval",
        "0.01",
        "--",
        sys.executable,
        "-c",
        f"print({marker!r})",
    ]
    if os.name == "nt":
        monkeypatch.setattr(memory_guard, "inherit_stdio_kwargs", lambda: stdio)
        monkeypatch.setattr(memory_guard.subprocess, "run", fake_subprocess_run)
        assert (
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
            == 73
        )
    else:
        with pytest.raises(SystemExit) as exc:
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
        assert exc.value.code == 73
    worker_argv = captured["argv"]
    assert isinstance(worker_argv, list)
    assert all(marker not in arg for arg in worker_argv)
    env = captured["env"]
    assert isinstance(env, dict)
    encoded = env[memory_guard.INTERNAL_COMMAND_ENV]
    assert json.loads(encoded) == [sys.executable, "-c", f"print({marker!r})"]
    assert env[memory_guard.INTERNAL_WORKER_ENV] == "1"
    if os.name == "nt":
        run_kwargs = captured["run_kwargs"]
        assert isinstance(run_kwargs, dict)
        assert run_kwargs["creationflags"] == (
            getattr(memory_guard.subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            | getattr(memory_guard.subprocess, "CREATE_NO_WINDOW", 0)
        )
        assert run_kwargs["stdin"] == "in"
        assert run_kwargs["stdout"] == "out"
        assert run_kwargs["stderr"] == "err"


def test_run_guarded_marks_child_environment_as_guarded() -> None:
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import json, os, pathlib; "
                "marker = pathlib.Path(os.environ['MOLT_MEMORY_GUARD_MARKER']); "
                "payload = json.loads(marker.read_text()); "
                "print(os.environ.get('MOLT_MEMORY_GUARD_ACTIVE')); "
                "print(bool(os.environ.get('MOLT_MEMORY_GUARD_PID'))); "
                "print(bool(os.environ.get('MOLT_MEMORY_GUARD_TOKEN'))); "
                "print(marker.exists()); "
                "print(payload['pid'] == int(os.environ['MOLT_MEMORY_GUARD_PID'])); "
                "print(payload['token'] == os.environ['MOLT_MEMORY_GUARD_TOKEN'])"
            ),
        ],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=None,
    )

    assert result.returncode == 0
    assert result.stdout.splitlines() == ["1", "True", "True", "True", "True", "True"]


def test_run_guarded_exports_backend_memory_contract() -> None:
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import os; "
                "print(os.environ.get('MOLT_BACKEND_MEMORY_AVAILABLE_GB')); "
                "print(os.environ.get('MOLT_BACKEND_MAX_RSS_GB'))"
            ),
        ],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=768 * 1024,
    )

    assert result.returncode == 0
    assert result.stdout.splitlines() == ["0.500000", "0.500000"]


def test_main_reexec_preserves_stream_and_sample_rotation_options(
    monkeypatch, tmp_path
) -> None:
    captured: dict[str, object] = {}
    samples_path = tmp_path / "samples.jsonl"
    stdio = {"stdin": "in", "stdout": "out", "stderr": "err"}

    def fake_execve(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(74)

    def fake_subprocess_run(argv, *, env, check, **kwargs):
        assert check is False
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        captured["creationflags"] = kwargs.get("creationflags", 0)
        captured["run_kwargs"] = dict(kwargs)
        return subprocess.CompletedProcess(argv, 74)

    main_argv = [
        "--max-rss-gb",
        "1",
        "--poll-interval",
        "0.01",
        "--samples-jsonl",
        str(samples_path),
        "--samples-max-mb",
        "0.5",
        "--stream",
        "json-stderr",
        "--child-rlimit-gb",
        "0.75",
        "--",
        sys.executable,
        "-c",
        "print('ok')",
    ]
    if os.name == "nt":
        monkeypatch.setattr(memory_guard, "inherit_stdio_kwargs", lambda: stdio)
        monkeypatch.setattr(memory_guard.subprocess, "run", fake_subprocess_run)
        assert (
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
            == 74
        )
    else:
        with pytest.raises(SystemExit) as exc:
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
        assert exc.value.code == 74
    worker_argv = captured["argv"]
    assert isinstance(worker_argv, list)
    assert "--samples-jsonl" in worker_argv
    assert str(samples_path) in worker_argv
    assert "--samples-max-mb" in worker_argv
    assert "0.5" in worker_argv
    assert "--stream" in worker_argv
    assert "json-stderr" in worker_argv
    assert "--child-rlimit-gb" in worker_argv
    assert "0.75" in worker_argv
    if os.name == "nt":
        assert captured["creationflags"] == (
            getattr(memory_guard.subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            | getattr(memory_guard.subprocess, "CREATE_NO_WINDOW", 0)
        )
        run_kwargs = captured["run_kwargs"]
        assert isinstance(run_kwargs, dict)
        assert run_kwargs["stdin"] == "in"
        assert run_kwargs["stdout"] == "out"
        assert run_kwargs["stderr"] == "err"


def test_internal_worker_loads_command_and_strips_internal_env(monkeypatch) -> None:
    command = [sys.executable, "-c", "print('worker')"]
    observed: dict[str, object] = {}

    def fake_run_guarded(seen_command, **kwargs):
        observed["command"] = list(seen_command)
        observed["env"] = dict(kwargs["env"])
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
        )

    monkeypatch.setenv(memory_guard.INTERNAL_WORKER_ENV, "1")
    monkeypatch.setenv(memory_guard.INTERNAL_COMMAND_ENV, json.dumps(command))
    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
        ],
        hide_command_argv=True,
    )

    assert rc == 0
    assert observed["command"] == command
    child_env = observed["env"]
    assert isinstance(child_env, dict)
    assert memory_guard.INTERNAL_COMMAND_ENV not in child_env
    assert memory_guard.INTERNAL_WORKER_ENV not in child_env
    assert memory_guard.INTERNAL_CHILD_RUNNER_ENV not in child_env
    assert memory_guard.INTERNAL_CHILD_COMMAND_ENV not in child_env
    assert memory_guard.INTERNAL_CHILD_RLIMIT_KB_ENV not in child_env


def test_child_runner_env_wraps_command_without_leaking_guard_keys() -> None:
    command = [sys.executable, "-c", "print('child')"]
    env = memory_guard._child_runner_env(
        {
            "KEEP": "1",
            memory_guard.INTERNAL_WORKER_ENV: "1",
            memory_guard.INTERNAL_COMMAND_ENV: json.dumps(["hidden"]),
        },
        command,
        child_rlimit_kb=12345,
    )

    assert env[memory_guard.INTERNAL_CHILD_RUNNER_ENV] == "1"
    assert json.loads(env[memory_guard.INTERNAL_CHILD_COMMAND_ENV]) == command
    assert env[memory_guard.INTERNAL_CHILD_RLIMIT_KB_ENV] == "12345"
    assert memory_guard.INTERNAL_CHILD_STARTED_FD_ENV not in env
    child_env = memory_guard._child_env_without_internal_keys(env)
    assert child_env == {"KEEP": "1"}


def test_resolve_relative_executable_leaves_absolute_and_bare_names() -> None:
    # Absolute paths and bare program names (no separator) are untouched so
    # PATH lookup still works and an explicit absolute command is preserved.
    absolute = [sys.executable, "-c", "print('x')"]
    assert memory_guard._resolve_relative_executable(absolute) == absolute
    bare = ["python3", "-c", "print('x')"]
    assert memory_guard._resolve_relative_executable(bare) == bare
    assert memory_guard._resolve_relative_executable([]) == []


def test_resolve_relative_executable_resolves_against_parent_cwd(
    monkeypatch, tmp_path
) -> None:
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp_name = "python.exe" if os.name == "nt" else "python3"
    rel_interp = rel_dir / rel_interp_name
    if os.name == "nt":
        shutil.copy2(Path(sys.executable).resolve(), rel_interp)
    else:
        rel_interp.symlink_to(Path(sys.executable).resolve())
    monkeypatch.chdir(tmp_path)

    resolved = memory_guard._resolve_relative_executable(
        [f"relbin/{rel_interp_name}", "-c", "print('x')"]
    )

    assert resolved[0] == str(rel_interp.resolve())
    assert resolved[1:] == ["-c", "print('x')"]


def test_resolve_relative_executable_skips_nonexistent_relative_path(
    monkeypatch, tmp_path
) -> None:
    # A relative path that does not exist under the parent cwd is left as-is so
    # an intentionally child-relative command is never clobbered.
    monkeypatch.chdir(tmp_path)
    command = ["does/not/exist", "arg"]
    assert memory_guard._resolve_relative_executable(command) == command


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="relative venv interpreter symlink chain is a POSIX concern",
)
def test_run_guarded_execs_relative_interpreter_with_other_cwd(
    monkeypatch, tmp_path
) -> None:
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp = rel_dir / "python3"
    rel_interp.symlink_to(Path(sys.executable).resolve())
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    monkeypatch.chdir(tmp_path)

    result = memory_guard.run_guarded(
        ["relbin/python3", "-c", "print('relrun')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=str(other_cwd),
        child_rlimit_kb=1_000_000,
    )

    assert result.returncode == 0
    assert result.stdout == "relrun\n"


def test_guarded_launch_applies_resource_limit_before_exec_on_posix() -> None:
    command = [sys.executable, "-c", "print('child')"]
    launch = memory_guard._guarded_launch(
        command,
        {"KEEP": "1"},
        child_rlimit_kb=12345,
    )

    if memory_guard.os.name == "posix":
        assert launch.command == command
        assert launch.env == {"KEEP": "1"}
        assert launch.preexec_fn is not None
        assert launch.started_read_fd is not None
        assert launch.pass_fds == launch.close_fds
    else:
        assert launch.command == [
            sys.executable,
            str(Path(memory_guard.__file__).resolve()),
        ]
        launch_env = launch.env
        assert launch_env is not None
        assert (
            json.loads(launch_env[memory_guard.INTERNAL_CHILD_COMMAND_ENV]) == command
        )
        assert launch_env[memory_guard.INTERNAL_CHILD_RLIMIT_KB_ENV] == "12345"
        assert memory_guard.INTERNAL_CHILD_STARTED_FD_ENV not in launch_env
        assert launch.started_read_fd is None
    memory_guard._close_fds((*launch.close_fds, launch.started_read_fd))


def test_main_writes_summary_json(tmp_path) -> None:
    summary_path = tmp_path / "summary.json"
    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "import time; print('ok'); time.sleep(0.2)",
        ]
    )

    assert rc == 0
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 0
    assert payload["violation"] is None
    assert payload["peak"]["rss_kb"] > 0
    expected_peak_scopes = {"process", "process_rusage"}
    expected_total_scopes = {"process_tree", "process_tree_rusage"}
    if os.name == "nt":
        expected_peak_scopes.add("process_handle")
        expected_total_scopes.add("process_tree_handle")
    assert payload["peak"]["scope"] in expected_peak_scopes
    assert payload["peak_total"]["rss_kb"] >= payload["peak"]["rss_kb"]
    assert payload["peak_total"]["scope"] in expected_total_scopes
    assert payload["max_total_rss_gb"] == pytest.approx(18.0)
    assert payload["child_rlimit_gb"] is None
    assert payload["orphaned_process_groups"] == []
    assert payload["incident"] is None


def test_run_guarded_keeps_windows_handle_peak_when_sampler_misses_child(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "nt":
        return
    monkeypatch.setattr(
        memory_guard,
        "windows_process_handle_rss_kb",
        lambda _handle: 12_345,
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "pass"],
        max_rss_kb=1_000_000,
        max_total_rss_kb=18 * 1024 * 1024,
        poll_interval=0.01,
        sampler=lambda: {},
        timeout=5.0,
    )

    assert result.returncode == 0
    assert result.peak is not None
    assert result.peak.rss_kb == 12_345
    assert result.peak.scope == "process_handle"
    assert result.peak_total is not None
    assert result.peak_total.rss_kb == 12_345
    assert result.peak_total.scope == "process_tree_handle"


def test_main_reports_orphan_cleanup_with_operator_signal(
    tmp_path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch,
) -> None:
    summary_path = tmp_path / "orphan-summary.json"
    report = _guard_termination_report(
        reason="repo_scoped_orphan_cleanup",
        root_pid=44,
        root_pgid=44,
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.4,
            orphaned_process_groups=(44,),
            termination_reports=(report,),
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    stderr = capsys.readouterr().err
    assert "orphaned child processes detected after command exit" in stderr
    assert "elapsed=0.40s" in stderr
    assert "pgids=44" in stderr
    assert "next action: inspect child process lifecycle and logs" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["orphaned_process_groups"] == [44]
    assert payload["incident"]["reason"] == "orphaned_processes_cleaned"
    assert payload["incident"]["elapsed_s"] == pytest.approx(0.4)
    assert payload["incident"]["process_groups"] == [44]
    assert payload["termination_reports"][0]["reason"] == "repo_scoped_orphan_cleanup"
    assert payload["incident"]["termination_reports"][0]["root_pgid"] == 44


def test_incident_reports_incomplete_orphan_cleanup_without_false_success() -> None:
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=100,
                signal=None,
                signal_name=None,
                result="skipped_missing_identity",
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=1,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        elapsed_s=1.0,
        # Even if another group was fully cleaned, this incomplete group must
        # dominate the incident classification and quarantine authority.
        orphaned_process_groups=(777,),
        termination_reports=(report,),
    )

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["reason"] == "orphan_cleanup_incomplete"
    assert incident["candidate_pids"] == [200]
    assert "reported as cleaned" in str(incident["cleanup"])
    assert incident["termination_reports"][0]["actions"][1]["result"] == "failed"


@pytest.mark.parametrize(
    ("result_overrides", "expected_reason", "report_reason"),
    [
        (
            {"guard_signal": int(signal.SIGTERM)},
            "guard_interrupted",
            "guard_signal",
        ),
        ({"timed_out": True}, "timeout", "timeout"),
        (
            {
                "violation": memory_guard.RssViolation(
                    pid=200,
                    rss_kb=10,
                    command="worker",
                )
            },
            "rss_limit_exceeded",
            "rss_limit",
        ),
    ],
)
def test_primary_incidents_preserve_incomplete_cleanup_truth(
    result_overrides: dict[str, object],
    expected_reason: str,
    report_reason: str,
) -> None:
    report = _guard_termination_report(
        reason=report_reason,
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    kwargs: dict[str, object] = {
        "returncode": 1,
        "violation": None,
        "peak": None,
        "peak_total": None,
        "stdout": "",
        "stderr": "",
        "elapsed_s": 1.0,
        "termination_reports": (report,),
    }
    kwargs.update(result_overrides)
    result = memory_guard.GuardResult(**kwargs)  # type: ignore[arg-type]

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["reason"] == expected_reason
    assert "cleanup incomplete" in str(incident["cleanup"])
    assert incident["process_tree_cleanup_status"] == "incomplete"
    assert incident["process_tree_cleanup_candidate_pids"] == [200]


def test_owned_child_handle_success_reconciles_only_direct_child_failure() -> None:
    primary = _guard_termination_report(
        reason="timeout",
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=100,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="still_live",
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    handle = _guard_termination_report(
        reason="post_loop_unreaped_child_direct_child_handle",
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="owned_child_handle",
                target_id=100,
                signal=memory_guard.fallback_kill_signal(),
                signal_name=memory_guard._signal_name(
                    memory_guard.fallback_kill_signal()
                ),
                result="completed_or_missing",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=memory_guard.TIMEOUT_RETURN_CODE,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        timed_out=True,
        termination_reports=(primary, handle),
    )

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["process_tree_cleanup_status"] == "incomplete"
    assert incident["process_tree_cleanup_candidate_pids"] == [200]

    fully_reaped = dataclasses.replace(
        result,
        termination_reports=(
            dataclasses.replace(primary, actions=(primary.actions[0],)),
            handle,
        ),
    )
    fully_reaped_incident = memory_guard._incident_payload(fully_reaped)
    assert fully_reaped_incident is not None
    assert "process_tree_cleanup_status" not in fully_reaped_incident
    assert fully_reaped_incident["cleanup"] == "terminated tracked process tree"


def test_completed_group_outcome_supersedes_preliminary_root_group_skip() -> None:
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        root_pid=100,
        root_pgid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process_group",
                target_id=100,
                signal=None,
                signal_name=None,
                result="skipped_protected_root_group",
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process_group",
                target_id=100,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=0,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        orphaned_process_groups=(100,),
        termination_reports=(report,),
    )

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["reason"] == "orphaned_processes_cleaned"
    assert "orphan_cleanup_status" not in incident


def test_main_writes_samples_jsonl(tmp_path) -> None:
    samples_path = tmp_path / "samples.jsonl"
    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--samples-jsonl",
            str(samples_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    lines = samples_path.read_text(encoding="utf-8").splitlines()
    assert lines
    payload = json.loads(lines[-1])
    assert payload["root_pid"] > 0
    assert "peak" in payload
    assert "total" in payload


def test_sample_jsonl_rotation_bounds_artifacts(tmp_path) -> None:
    samples_path = tmp_path / "samples.jsonl"
    peak = memory_guard.RssViolation(pid=100, rss_kb=10, command="root")

    for _ in range(8):
        memory_guard._append_sample_jsonl(
            str(samples_path),
            root_pid=100,
            peak=peak,
            total=peak,
            violation=None,
            max_bytes=1024,
        )

    assert samples_path.exists()
    assert samples_path.with_name("samples.jsonl.1").exists()
    assert samples_path.stat().st_size <= 1024
    assert samples_path.with_name("samples.jsonl.1").stat().st_size <= 1024


def test_main_streams_samples_without_sample_artifact(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    samples_path = tmp_path / "samples.jsonl"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--stream",
            "stderr",
            "--",
            sys.executable,
            "-c",
            "import time; time.sleep(0.05)",
        ]
    )

    captured = capsys.readouterr()
    assert rc == 0
    assert "memory_guard sample:" in captured.err
    assert not samples_path.exists()
