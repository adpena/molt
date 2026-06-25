from __future__ import annotations

import importlib.util
import subprocess
from types import SimpleNamespace
import sys
from pathlib import Path

import pytest

from tools.memory_guard_core import windows_snapshot


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "tools" / "memory_guard.py"


def _load_memory_guard():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_memory_guard_windows_sampling", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_parse_windows_process_snapshot_rows_builds_process_samples() -> None:
    module = _load_memory_guard()
    rows = [
        (100, 42, 2, "python.exe", 7, 123456789),
        (101, 100, 0, "", None),
        (0, 0, 8192, "System Idle Process", None),
    ]

    samples = module.parse_windows_process_snapshot_rows(rows)

    assert sorted(samples) == [100, 101]
    assert samples[100].ppid == 42
    assert samples[100].rss_kb == 2
    assert samples[100].command == "python.exe"
    assert samples[100].elapsed_sec == 7
    assert samples[100].started_at_ns == 123456789
    assert samples[101].ppid == 100
    assert samples[101].rss_kb == 0
    assert samples[101].command == "pid:101"
    assert samples[101].elapsed_sec is None


def test_sample_processes_uses_windows_sampler_on_nt(monkeypatch) -> None:
    module = _load_memory_guard()
    sample = module.ProcessSample(pid=7, ppid=1, rss_kb=9, command="python.exe")

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "sample_processes_windows", lambda: {7: sample})
    monkeypatch.setattr(module, "sample_processes_posix", lambda: {})

    assert module.sample_processes() == {7: sample}


def test_sample_processes_posix_missing_ps_returns_empty(monkeypatch) -> None:
    module = _load_memory_guard()

    def missing_ps(*args, **kwargs):  # noqa: ANN002, ANN003
        raise FileNotFoundError("ps")

    monkeypatch.setattr(module.subprocess, "run", missing_ps)

    assert module.sample_processes_posix() == {}


def test_sample_processes_windows_uses_injected_snapshot_authority(monkeypatch) -> None:
    module = _load_memory_guard()

    def fail_run(*args, **kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("Windows sampler must not shell out")

    monkeypatch.setattr(module.subprocess, "run", fail_run)
    monkeypatch.setattr(
        module,
        "_windows_process_snapshot_rows",
        lambda: [(7, 1, 9, "python.exe", 3)],
    )

    samples = module.sample_processes_windows()

    assert sorted(samples) == [7]
    assert samples[7].ppid == 1
    assert samples[7].rss_kb == 9
    assert samples[7].command == "python.exe"


def test_sample_processes_windows_timeout_fails_closed(monkeypatch) -> None:
    module = _load_memory_guard()

    def timed_out():
        raise TimeoutError("snapshot deadline")

    monkeypatch.setattr(module, "_windows_process_snapshot_rows", timed_out)

    assert module.sample_processes_windows() == {}


def test_windows_process_snapshot_timeout_env_contract() -> None:
    name = windows_snapshot.WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_ENV

    assert (
        windows_snapshot._windows_process_snapshot_timeout_sec({})
        == windows_snapshot.DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC
    )
    assert windows_snapshot._windows_process_snapshot_timeout_sec({name: "0.25"}) == 0.25
    assert windows_snapshot._windows_process_snapshot_timeout_sec({name: "bad"}) == (
        windows_snapshot.DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC
    )
    assert windows_snapshot._windows_process_snapshot_timeout_sec({name: "0"}) is None
    assert windows_snapshot._windows_process_snapshot_timeout_sec({name: "off"}) is None


def test_windows_process_snapshot_hard_timeout_kills_helper(monkeypatch) -> None:
    monkeypatch.setattr(windows_snapshot.os, "name", "nt", raising=False)
    monkeypatch.setattr(
        windows_snapshot,
        "_windows_process_snapshot_timeout_sec",
        lambda: 0.25,
    )

    def timed_out(*args, **kwargs):  # noqa: ANN002, ANN003
        raise subprocess.TimeoutExpired(cmd=args[0], timeout=kwargs["timeout"])

    monkeypatch.setattr(windows_snapshot.subprocess, "run", timed_out)

    with pytest.raises(windows_snapshot.WindowsProcessSnapshotTimeout):
        windows_snapshot._windows_process_snapshot_rows_hard_timeout()


def test_windows_process_snapshot_hard_timeout_decodes_complete_rows(
    monkeypatch,
) -> None:
    monkeypatch.setattr(windows_snapshot.os, "name", "nt", raising=False)
    monkeypatch.setattr(
        windows_snapshot,
        "_windows_process_snapshot_timeout_sec",
        lambda: 0.25,
    )

    def fake_run(*args, **kwargs):  # noqa: ANN002, ANN003
        assert kwargs["timeout"] == 0.25
        assert kwargs["check"] is False
        assert kwargs["capture_output"] is True
        assert kwargs["text"] is True
        assert args[0][-1] == windows_snapshot.WINDOWS_PROCESS_SNAPSHOT_HELPER_ARG
        return SimpleNamespace(
            returncode=0,
            stdout='[[7,1,9,"python.exe",3,123456789]]',
            stderr="",
        )

    monkeypatch.setattr(windows_snapshot.subprocess, "run", fake_run)

    assert windows_snapshot._windows_process_snapshot_rows_hard_timeout() == [
        (7, 1, 9, "python.exe", 3, 123456789)
    ]


def test_windows_process_snapshot_hard_timeout_rejects_partial_payload(
    monkeypatch,
) -> None:
    monkeypatch.setattr(windows_snapshot.os, "name", "nt", raising=False)
    monkeypatch.setattr(
        windows_snapshot,
        "_windows_process_snapshot_timeout_sec",
        lambda: 0.25,
    )
    monkeypatch.setattr(
        windows_snapshot.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(  # noqa: ARG005, ANN002, ANN003
            returncode=0,
            stdout='[[7,1,9,"python.exe",3]]',
            stderr="",
        ),
    )

    assert windows_snapshot._windows_process_snapshot_rows_hard_timeout() == []


def test_windows_guarded_popen_uses_new_process_group(monkeypatch) -> None:
    module = _load_memory_guard()
    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )

    kwargs = module._guarded_popen_process_isolation_kwargs()

    assert kwargs == {"creationflags": 0x00000200}
    assert "start_new_session" not in kwargs


def test_posix_guarded_popen_uses_new_session(monkeypatch) -> None:
    module = _load_memory_guard()
    monkeypatch.setattr(module, "_is_windows_process_model", lambda: False)

    assert module._guarded_popen_process_isolation_kwargs() == {
        "start_new_session": True
    }


def test_windows_sampler_limits_full_command_line_reads_to_launcher_processes() -> None:
    module = _load_memory_guard()

    assert module._windows_process_needs_full_command_line("python.exe") is True
    assert module._windows_process_needs_full_command_line("UV.EXE") is True
    assert module._windows_process_needs_full_command_line("node.exe") is True
    assert module._windows_process_needs_full_command_line("explorer.exe") is False
    assert module._windows_process_needs_full_command_line("svchost.exe") is False


def test_command_executable_name_handles_windows_paths() -> None:
    module = _load_memory_guard()

    assert (
        module._command_executable_name(
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.616.10790.0_x64__2p2nqsd0c76g0\app\resources\codex.exe"
        )
        == "codex.exe"
    )
    assert (
        module._command_executable_name(
            r'"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0\app\Codex.exe"'
        )
        == "codex.exe"
    )
    assert (
        module._command_executable_name(
            r"C:\Users\adpen\AppData\Local\OpenAI\Codex\runtimes\cua_node\bin\node_repl.exe"
        )
        == "node_repl.exe"
    )


def test_windows_codex_and_claude_processes_are_host_control_plane() -> None:
    module = _load_memory_guard()

    samples = [
        module.ProcessSample(
            pid=10,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
        ),
        module.ProcessSample(
            pid=11,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Users\adpen\AppData\Local\OpenAI\Codex\runtimes\cua_node"
                r"\789504f803e82e2b\bin\node_repl.exe"
            ),
        ),
        module.ProcessSample(
            pid=12,
            ppid=1,
            rss_kb=1,
            command=r"C:\Users\adpen\AppData\Local\Programs\Claude\claude.exe",
        ),
        module.ProcessSample(
            pid=13,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Program Files\nodejs\node.exe "
                r"C:\Users\adpen\AppData\Roaming\npm\node_modules\@anthropic-ai"
                r"\claude-code\cli.js"
            ),
        ),
    ]

    assert all(module.is_host_control_plane_process(sample) for sample in samples)


def test_windows_protects_external_codex_descendants_but_not_owned_children(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        10: module.ProcessSample(
            pid=10,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
        ),
        20: module.ProcessSample(
            pid=20,
            ppid=10,
            rss_kb=1,
            command=r"C:\Users\adpen\OneDrive\Documents\molt\target\dev-fast\molt-backend.exe",
        ),
        30: module.ProcessSample(
            pid=30,
            ppid=10,
            rss_kb=1,
            command=r"C:\Users\adpen\OneDrive\Documents\molt\.venv\Scripts\python.exe tools\memory_guard.py",
        ),
        31: module.ProcessSample(
            pid=31,
            ppid=30,
            rss_kb=1,
            command=r"C:\Users\adpen\OneDrive\Documents\molt\target\dev-fast\molt-backend.exe",
        ),
    }

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)

    protected = module.protected_process_group_ids(
        samples,
        self_pid=30,
        self_pgid=None,
    )

    assert 10 in protected
    assert 20 in protected
    assert 30 in protected
    assert 31 not in protected


def test_hidden_argv_uses_subprocess_worker_on_windows(monkeypatch) -> None:
    module = _load_memory_guard()
    calls: dict[str, object] = {}

    def fake_run(argv, *, env, check, creationflags=0):  # noqa: ANN001
        calls["argv"] = argv
        calls["env"] = env
        calls["check"] = check
        calls["creationflags"] = creationflags
        return SimpleNamespace(returncode=37)

    def fail_execve(*args, **kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("Windows hidden-argv path must not call execve")

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.subprocess, "run", fake_run)

    rc = module.main(
        ["--", "python", "-c", "print('ok')"],
        hide_command_argv=True,
        execve=fail_execve,
        environ={},
    )

    assert rc == 37
    assert calls["check"] is False
    assert calls["creationflags"] == getattr(
        module.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0,
    )
    assert calls["argv"][0] == sys.executable
    env = calls["env"]
    assert env[module.INTERNAL_WORKER_ENV] == "1"
    assert module.INTERNAL_COMMAND_ENV in env


def test_child_runner_uses_subprocess_on_windows(monkeypatch) -> None:
    module = _load_memory_guard()
    calls: dict[str, object] = {}

    def fake_run(argv, *, env, check, creationflags=0):  # noqa: ANN001
        calls["argv"] = argv
        calls["env"] = env
        calls["check"] = check
        calls["creationflags"] = creationflags
        return SimpleNamespace(returncode=23)

    def fail_execvpe(*args, **kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("Windows child runner must not call execvpe")

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.subprocess, "run", fake_run)
    monkeypatch.setattr(module.os, "execvpe", fail_execvpe)
    env = {
        module.INTERNAL_CHILD_COMMAND_ENV: '["python", "-c", "print(1)"]',
        module.INTERNAL_CHILD_RLIMIT_KB_ENV: "0",
    }

    rc = module._run_child_runner(env)

    assert rc == 23
    assert calls["argv"] == ["python", "-c", "print(1)"]
    assert calls["check"] is False
    assert calls["creationflags"] == getattr(
        module.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0,
    )
    child_env = calls["env"]
    assert module.INTERNAL_CHILD_COMMAND_ENV not in child_env


def test_terminate_watched_processes_windows_kills_owned_descendants(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(pid=100, ppid=50, rss_kb=1, command="uv.exe"),
        101: module.ProcessSample(pid=101, ppid=100, rss_kb=1, command="python.exe"),
        900: module.ProcessSample(pid=900, ppid=50, rss_kb=1, command="unrelated.exe"),
    }
    sent: list[tuple[int, int]] = []

    def fake_kill(pid: int, sig: int) -> None:
        sent.append((pid, sig))

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_current_protected_process_group_ids", lambda _s: set())
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", fake_kill)

    module.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.0,
    )

    assert (101, module.signal.SIGTERM) in sent
    assert (100, module.signal.SIGTERM) in sent
    assert (900, module.signal.SIGTERM) not in sent
    assert (101, module.fallback_kill_signal()) in sent
    assert (100, module.fallback_kill_signal()) in sent


def test_terminate_watched_processes_windows_refuses_codex_root(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=42,
            rss_kb=1,
            command=(
                r'"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0'
                r'\app\resources\codex.exe" app-server --analytics-default-enabled'
            ),
        ),
        101: module.ProcessSample(
            pid=101,
            ppid=100,
            rss_kb=1,
            command=(
                r'"C:\Users\adpen\AppData\Local\OpenAI\Codex\runtimes'
                r'\cua_node\bin\node_repl.exe"'
            ),
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    module.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.0,
    )

    assert sent == []


def test_terminate_watched_processes_windows_kills_owned_root_with_empty_samples(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    module.terminate_watched_processes(
        100,
        samples={},
        watched=set(),
        grace=0.0,
        root_owned=True,
    )

    assert (100, module.signal.SIGTERM) in sent
    assert (100, module.fallback_kill_signal()) in sent


def test_cleanup_tracked_orphans_windows_passes_live_descendants_to_terminator(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    tracker = module.ProcessTreeTracker(root_pid=100)
    initial = {
        100: module.ProcessSample(pid=100, ppid=50, rss_kb=1, command="uv.exe"),
        101: module.ProcessSample(pid=101, ppid=100, rss_kb=1, command="python.exe"),
    }
    tracker.update(initial)
    live = {
        101: module.ProcessSample(pid=101, ppid=100, rss_kb=1, command="python.exe"),
    }
    terminated: dict[str, object] = {}

    def fake_terminate(  # noqa: ANN001
        root_pid,
        *,
        samples,
        watched,
        tracker,
        grace,
        reason,
        sampler,
        root_owned,
    ):
        terminated["root_pid"] = root_pid
        terminated["samples"] = samples
        terminated["watched"] = set(watched)
        terminated["tracker"] = tracker
        terminated["grace"] = grace
        terminated["reason"] = reason
        terminated["sampler"] = sampler
        terminated["root_owned"] = root_owned
        return module.GuardTerminationReport(
            reason=reason,
            started_at="2026-06-17T00:00:00Z",
            completed_at="2026-06-17T00:00:01Z",
            root_pid=root_pid,
            root_pgid=None,
            root_sid=None,
            grace_sec=grace,
            watched_pids=tuple(sorted(watched)),
            protected_pgids=(),
            escaped_pids=(),
            remaining_pgids=(),
            remaining_pids=(),
            actions=(),
        )

    monkeypatch.setattr(module, "_current_protected_process_group_ids", lambda _s: set())
    monkeypatch.setattr(module, "terminate_watched_processes", fake_terminate)

    orphans = module.cleanup_tracked_orphans(
        100,
        tracker=tracker,
        sampler=lambda: live,
        grace=0.5,
    )

    assert orphans.process_groups == (101,)
    assert len(orphans.termination_reports) == 1
    assert terminated["root_pid"] == 100
    assert terminated["samples"] == live
    assert terminated["watched"] == {101}
    assert terminated["tracker"] is tracker
    assert terminated["grace"] == 0.5
    assert terminated["reason"] == "tracked_orphan_cleanup"
    assert terminated["sampler"] is not None
    assert terminated["root_owned"] is True
