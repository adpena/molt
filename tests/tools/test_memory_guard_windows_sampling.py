from __future__ import annotations

import importlib.util
import json
import os
import subprocess
from types import SimpleNamespace
import sys
from pathlib import Path

import pytest

from tools.memory_guard_core import windows_snapshot
from tools.memory_guard_core import process_model


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


def _windows_guard_creationflags(module) -> int:
    from tools.process_spawn import hidden_windows_process_group_creationflags

    return hidden_windows_process_group_creationflags(
        subprocess_module=module.subprocess
    )


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


@pytest.mark.skipif(
    sys.platform.startswith("linux"),
    reason="Linux process sampling uses native /proc rather than ps",
)
def test_sample_processes_posix_missing_ps_is_typed_failure(monkeypatch) -> None:
    module = _load_memory_guard()

    def missing_ps(*args, **kwargs):  # noqa: ANN002, ANN003
        raise FileNotFoundError("ps")

    monkeypatch.setattr(module.subprocess, "run", missing_ps)

    with pytest.raises(module.ProcessSnapshotError, match="POSIX process snapshot"):
        module.sample_processes_posix()


def test_darwin_process_authority_binds_once_for_all_pid_reads(monkeypatch) -> None:
    class FakeAuthority:
        def metadata(self, pid: int) -> tuple[int, int, int, str]:
            return (1, pid, 123_000, "node")

        def command(self, pid: int) -> str:
            return f"node codex-{pid}.js"

    authority = FakeAuthority()
    loads: list[None] = []
    monkeypatch.setattr(process_model.sys, "platform", "darwin")
    monkeypatch.setattr(
        process_model,
        "_darwin_process_authority_cache",
        process_model._DARWIN_PROCESS_AUTHORITY_UNSET,
    )
    monkeypatch.setattr(
        process_model,
        "_load_darwin_process_authority",
        lambda: loads.append(None) or authority,
    )

    assert process_model._darwin_proc_metadata(7) == (1, 7, 123_000, "node")
    assert process_model._darwin_proc_command(7) == "node codex-7.js"
    assert process_model._darwin_proc_metadata(8) == (1, 8, 123_000, "node")
    assert process_model._darwin_proc_command(8) == "node codex-8.js"
    assert loads == [None]


def test_darwin_process_authority_retains_one_library_binding_set(
    monkeypatch,
) -> None:
    import ctypes

    class FakeFunction:
        argtypes = None
        restype = None

    libproc = SimpleNamespace(proc_pidinfo=FakeFunction())
    libsystem = SimpleNamespace(sysctl=FakeFunction())
    loads: list[str] = []

    def fake_cdll(path: str, *, use_errno: bool) -> object:
        assert use_errno
        loads.append(path)
        return libproc if path.endswith("libproc.dylib") else libsystem

    monkeypatch.setattr(process_model.sys, "platform", "darwin")
    monkeypatch.setattr(
        process_model,
        "_darwin_process_authority_cache",
        process_model._DARWIN_PROCESS_AUTHORITY_UNSET,
    )
    monkeypatch.setattr(ctypes, "CDLL", fake_cdll)

    first = process_model._darwin_process_authority()
    second = process_model._darwin_process_authority()

    assert first is second
    assert first is not None
    assert first.libproc is libproc
    assert first.libsystem is libsystem
    assert loads == [
        "/usr/lib/libproc.dylib",
        "/usr/lib/libSystem.B.dylib",
    ]


def test_darwin_cached_authority_preserves_bound_command_and_identity(
    monkeypatch,
) -> None:
    class FakeAuthority:
        metadata_calls = 0
        command_calls = 0

        def metadata(self, pid: int) -> tuple[int, int, int, str]:
            assert pid == 200
            self.metadata_calls += 1
            return (100, 200, 987_654_321_000, "node")

        def command(self, pid: int) -> str:
            assert pid == 200
            self.command_calls += 1
            return "node /usr/local/lib/node_modules/@openai/codex/bin/codex.js"

    authority = FakeAuthority()
    monkeypatch.setattr(process_model.sys, "platform", "darwin")
    monkeypatch.setattr(
        process_model,
        "_darwin_process_authority_cache",
        process_model._DARWIN_PROCESS_AUTHORITY_UNSET,
    )
    monkeypatch.setattr(
        process_model,
        "_load_darwin_process_authority",
        lambda: authority,
    )
    monkeypatch.setattr(
        process_model.subprocess,
        "run",
        lambda *_args, **_kwargs: SimpleNamespace(
            returncode=0,
            stdout=("200 1 200 64 Thu Jul 17 07:15:01 2026 node placeholder.js\n"),
        ),
    )

    sample = process_model.sample_processes_posix()[200]

    assert sample.ppid == 100
    assert sample.pgid == 200
    assert sample.started_at_ns == 987_654_321_000
    assert process_model.is_host_control_plane_process(sample)
    assert authority.metadata_calls == 2
    assert authority.command_calls == 1


def test_darwin_cached_authority_keeps_reuse_fail_closed(monkeypatch) -> None:
    class FakeAuthority:
        def __init__(self) -> None:
            self.metadata_rows = iter(
                (
                    (100, 200, 111_000, "node"),
                    (4, 200, 222_000, "node"),
                )
            )

        def metadata(self, pid: int) -> tuple[int, int, int, str]:
            assert pid == 200
            return next(self.metadata_rows)

        def command(self, pid: int) -> str:
            assert pid == 200
            return "node /usr/local/lib/node_modules/@openai/codex/bin/codex.js"

    monkeypatch.setattr(process_model.sys, "platform", "darwin")
    monkeypatch.setattr(
        process_model,
        "_darwin_process_authority_cache",
        process_model._DARWIN_PROCESS_AUTHORITY_UNSET,
    )
    monkeypatch.setattr(
        process_model,
        "_load_darwin_process_authority",
        FakeAuthority,
    )
    monkeypatch.setattr(
        process_model.subprocess,
        "run",
        lambda *_args, **_kwargs: SimpleNamespace(
            returncode=0,
            stdout="200 1 200 64 Thu Jul 17 07:15:01 2026 node codex.js\n",
        ),
    )

    sample = process_model.sample_processes_posix()[200]

    assert sample.ppid == 0
    assert sample.started_at_ns is None


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

    with pytest.raises(module.ProcessSnapshotError, match="Windows process snapshot"):
        module.sample_processes_windows()


def test_windows_process_snapshot_timeout_env_contract() -> None:
    name = windows_snapshot.WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_ENV

    assert (
        windows_snapshot._windows_process_snapshot_timeout_sec({})
        == windows_snapshot.DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC
    )
    assert (
        windows_snapshot._windows_process_snapshot_timeout_sec({name: "0.25"}) == 0.25
    )
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

    with pytest.raises(windows_snapshot.ProcessSnapshotError, match="invalid payload"):
        windows_snapshot._windows_process_snapshot_rows_hard_timeout()


@pytest.mark.parametrize(
    ("returncode", "stdout", "stderr", "message"),
    [
        (7, "", "helper failed", "exit code 7"),
        (0, "not-json", "", "invalid payload"),
    ],
)
def test_windows_process_snapshot_hard_timeout_preserves_failure_authority(
    monkeypatch,
    returncode: int,
    stdout: str,
    stderr: str,
    message: str,
) -> None:
    monkeypatch.setattr(windows_snapshot.os, "name", "nt", raising=False)
    monkeypatch.setattr(
        windows_snapshot.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(  # noqa: ARG005, ANN002, ANN003
            returncode=returncode,
            stdout=stdout,
            stderr=stderr,
        ),
    )

    with pytest.raises(windows_snapshot.ProcessSnapshotError, match=message):
        windows_snapshot._windows_process_snapshot_rows_hard_timeout()


@pytest.mark.parametrize(
    ("bound_pid", "bound_ppid", "started_at_ns"),
    [
        (None, None, 123),
        (8, 1, 123),
        (7, None, 123),
        (7, 1, None),
    ],
)
def test_windows_snapshot_rejects_unbound_lineage_identity(
    bound_pid: int | None,
    bound_ppid: int | None,
    started_at_ns: int | None,
) -> None:
    assert windows_snapshot._validated_windows_process_binding(
        7,
        bound_pid,
        bound_ppid,
        started_at_ns,
    ) == (0, None)


def test_windows_snapshot_accepts_parent_and_identity_from_same_handle() -> None:
    assert windows_snapshot._validated_windows_process_binding(
        7,
        7,
        3,
        123,
    ) == (3, 123)


def test_windows_process_handle_rss_fails_closed_when_psapi_unavailable(
    monkeypatch,
) -> None:
    import ctypes

    monkeypatch.setattr(windows_snapshot.os, "name", "nt", raising=False)

    def missing_psapi(*_args, **_kwargs):
        raise OSError("psapi unavailable")

    monkeypatch.setattr(ctypes, "WinDLL", missing_psapi, raising=False)

    assert windows_snapshot.windows_process_handle_rss_kb(1234) is None


def test_windows_process_handle_rss_rejects_invalid_handle_values(
    monkeypatch,
) -> None:
    monkeypatch.setattr(windows_snapshot.os, "name", "nt", raising=False)

    assert windows_snapshot.windows_process_handle_rss_kb(None) is None
    assert windows_snapshot.windows_process_handle_rss_kb("not-a-handle") is None


def test_windows_guarded_popen_uses_new_process_group(monkeypatch) -> None:
    module = _load_memory_guard()
    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        module.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )

    kwargs = module._guarded_popen_process_isolation_kwargs()

    assert kwargs == {"creationflags": 0x08000200}
    assert "start_new_session" not in kwargs


def test_windows_wrapper_terminalizes_worker_exit_running_summary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_memory_guard()
    summary_path = tmp_path / "memory_guard.json"
    child_process = module.GuardedChildProcess(
        pid=40132,
        pgid=None,
        sid=None,
        command=("python", "worker.py"),
        started_at="2026-07-08T23:40:41Z",
    )
    module._write_running_summary_json(
        str(summary_path),
        command=("python", "worker.py"),
        cwd=tmp_path,
        environ=os.environ,
        max_rss_kb=12 * 1024 * 1024,
        max_total_rss_kb=18 * 1024 * 1024,
        max_global_rss_kb=None,
        child_rlimit_kb=12 * 1024 * 1024,
        timeout_s=1200.0,
        poll_interval_s=2.0,
        child_process=child_process,
    )

    def fake_run(*args, **kwargs):  # noqa: ANN002, ANN003
        assert kwargs["check"] is False
        assert kwargs["env"][module.INTERNAL_WORKER_ENV] == "1"
        return SimpleNamespace(returncode=15)

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.subprocess, "run", fake_run)

    rc = module.main(
        [
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "pass",
        ],
        hide_command_argv=True,
        environ=os.environ,
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert rc == 15
    assert payload["status"] == "guard_worker_exited_without_final_summary"
    assert payload["returncode"] == 15
    assert payload["worker_returncode"] == 15
    assert payload["worker_exit_signal"]["name"] == "SIGTERM"
    assert payload["child_process"]["pid"] == 40132
    assert payload["incident"]["reason"] == "guard_worker_exited_without_final_summary"
    assert payload["incident"]["previous_status"] == "running"


def test_harness_batch_process_group_kwargs_hide_windows_console(monkeypatch) -> None:
    import tools.harness_memory_guard as harness_memory_guard

    monkeypatch.setattr(harness_memory_guard.os, "name", "nt")
    monkeypatch.setattr(
        harness_memory_guard.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        harness_memory_guard.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )

    assert harness_memory_guard.batch_process_group_kwargs() == {
        "creationflags": 0x08000200
    }


def test_process_spawn_is_single_windows_hidden_group_authority(monkeypatch) -> None:
    import tools.process_spawn as process_spawn

    monkeypatch.setattr(
        process_spawn.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        process_spawn.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )

    assert process_spawn.hidden_windows_process_group_kwargs(windows=True) == {
        "creationflags": 0x08000200
    }
    assert process_spawn.detached_process_group_kwargs(windows=True) == {
        "creationflags": 0x08000200
    }
    assert process_spawn.detached_process_group_kwargs(windows=False) == {
        "start_new_session": True
    }


def test_pytest_bootstrap_process_group_kwargs_hide_windows_console(
    monkeypatch,
) -> None:
    import tools.pytest_memory_guard_bootstrap as pytest_memory_guard_bootstrap

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )

    assert pytest_memory_guard_bootstrap._windows_process_group_kwargs() == {
        "creationflags": 0x08000200
    }


def test_pytest_bootstrap_handoff_preserves_stdio_under_hidden_console(
    monkeypatch,
) -> None:
    import tools.pytest_memory_guard_bootstrap as pytest_memory_guard_bootstrap

    calls: dict[str, object] = {}
    stdio = {"stdin": "in", "stdout": "out", "stderr": "err"}

    def fake_run(
        argv,
        *,
        env,
        check,
        creationflags=0,
        stdin=None,  # noqa: ANN001
        stdout=None,  # noqa: ANN001
        stderr=None,  # noqa: ANN001
    ):  # noqa: ANN001
        calls["argv"] = argv
        calls["env"] = env
        calls["check"] = check
        calls["creationflags"] = creationflags
        calls["stdin"] = stdin
        calls["stdout"] = stdout
        calls["stderr"] = stderr
        return SimpleNamespace(returncode=7)

    def fake_exit(code: int) -> None:
        raise SystemExit(code)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "inherit_stdio_kwargs", lambda: stdio
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_flush_standard_streams", lambda: None
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "_exit", fake_exit)
    monkeypatch.setattr(pytest_memory_guard_bootstrap.subprocess, "run", fake_run)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0x00000200,
        raising=False,
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap.subprocess,
        "CREATE_NO_WINDOW",
        0x08000000,
        raising=False,
    )

    with pytest.raises(SystemExit) as exc:
        pytest_memory_guard_bootstrap.handoff_to_outer_guard(
            ["python", "-m", "pytest"],
            {"PYTEST_CURRENT_TEST": "demo"},
        )

    assert exc.value.code == 7
    assert calls["argv"] == ["python", "-m", "pytest"]
    assert calls["env"] == {"PYTEST_CURRENT_TEST": "demo"}
    assert calls["check"] is False
    assert calls["creationflags"] == 0x08000200
    assert calls["stdin"] == "in"
    assert calls["stdout"] == "out"
    assert calls["stderr"] == "err"


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
        module.ProcessSample(
            pid=14,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Program Files\Git\usr\bin\tail.exe -f "
                r"C:\Users\adpen\AppData\Local\Temp\claude"
                r"\C--Users-adpen-OneDrive-Documents-molt\tasks\b1.output"
            ),
        ),
        module.ProcessSample(
            pid=15,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Users\adpen\.codex\vendor_imports\node.exe "
                r"C:\Users\adpen\OneDrive\Documents\molt\tests\molt_diff.py"
            ),
        ),
        module.ProcessSample(
            pid=16,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Users\adpen\.claude\shell-snapshots"
                r"\snapshot-bash-1782248792725.sh"
            ),
        ),
        module.ProcessSample(
            pid=17,
            ppid=1,
            rss_kb=1,
            command="codex --project C:\\Users\\adpen\\OneDrive\\Documents\\molt",
        ),
        module.ProcessSample(
            pid=18,
            ppid=1,
            rss_kb=1,
            command="/Applications/Codex.app/Contents/MacOS/Codex",
        ),
        module.ProcessSample(
            pid=19,
            ppid=1,
            rss_kb=1,
            command=(
                "/usr/bin/node /usr/local/lib/node_modules/@openai/codex/bin/codex.js"
            ),
        ),
        module.ProcessSample(
            pid=20,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Users\adpen\AppData\Roaming\npm\codex.cmd "
                r"--project C:\Users\adpen\OneDrive\Documents\molt"
            ),
        ),
        module.ProcessSample(
            pid=21,
            ppid=1,
            rss_kb=1,
            command="/opt/Codex/codex.AppImage --project /repo/molt",
        ),
        module.ProcessSample(
            pid=22,
            ppid=1,
            rss_kb=1,
            command="/Users/adpen/.codex/runtimes/cua_node/bin/node_repl --stdio",
        ),
        module.ProcessSample(
            pid=23,
            ppid=1,
            rss_kb=1,
            command="node_repl --stdio",
        ),
        module.ProcessSample(
            pid=24,
            ppid=1,
            rss_kb=1,
            command="/home/adpen/.codex/bin/codex-linux-sandbox --command pytest",
        ),
        module.ProcessSample(
            pid=25,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex-app-server.exe --project C:\repo\molt"
            ),
        ),
        module.ProcessSample(
            pid=26,
            ppid=1,
            rss_kb=1,
            command="/usr/bin/node /opt/homebrew/bin/codex",
        ),
        module.ProcessSample(
            pid=27,
            ppid=1,
            rss_kb=1,
            command="npx @openai/codex",
        ),
        module.ProcessSample(
            pid=28,
            ppid=1,
            rss_kb=1,
            command="powershell.exe -Command codex",
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

    stdio = {"stdin": "in", "stdout": "out", "stderr": "err"}

    def fake_run(
        argv,
        *,
        env,
        check,
        creationflags=0,
        stdin=None,  # noqa: ANN001
        stdout=None,  # noqa: ANN001
        stderr=None,  # noqa: ANN001
    ):  # noqa: ANN001
        calls["argv"] = argv
        calls["env"] = env
        calls["check"] = check
        calls["creationflags"] = creationflags
        calls["stdin"] = stdin
        calls["stdout"] = stdout
        calls["stderr"] = stderr
        return SimpleNamespace(returncode=37)

    def fail_execve(*args, **kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("Windows hidden-argv path must not call execve")

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "inherit_stdio_kwargs", lambda: stdio)
    monkeypatch.setattr(module.subprocess, "run", fake_run)

    rc = module.main(
        ["--", "python", "-c", "print('ok')"],
        hide_command_argv=True,
        execve=fail_execve,
        environ={},
    )

    assert rc == 37
    assert calls["check"] is False
    assert calls["creationflags"] == _windows_guard_creationflags(module)
    assert calls["stdin"] == "in"
    assert calls["stdout"] == "out"
    assert calls["stderr"] == "err"
    assert calls["argv"][0] == sys.executable
    env = calls["env"]
    assert env[module.INTERNAL_WORKER_ENV] == "1"
    assert module.INTERNAL_COMMAND_ENV in env


def test_terminate_watched_processes_windows_kills_owned_descendants(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100, ppid=50, rss_kb=1, command="uv.exe", started_at_ns=100
        ),
        101: module.ProcessSample(
            pid=101, ppid=100, rss_kb=1, command="python.exe", started_at_ns=101
        ),
        900: module.ProcessSample(
            pid=900, ppid=50, rss_kb=1, command="unrelated.exe", started_at_ns=900
        ),
    }
    sent: list[tuple[int, int]] = []

    def fake_kill(pid: int, sig: int) -> None:
        sent.append((pid, sig))

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", fake_kill)

    module.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.0,
        sampler=lambda: samples,
    )

    assert (101, module.signal.SIGTERM) in sent
    assert (100, module.signal.SIGTERM) in sent
    assert (900, module.signal.SIGTERM) not in sent
    assert (101, module.fallback_kill_signal()) in sent
    assert (100, module.fallback_kill_signal()) in sent


def test_windows_termination_revalidates_identity_between_discovery_and_term(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    original = module.ProcessSample(
        pid=200,
        ppid=1,
        rss_kb=1,
        command="worker.exe",
        started_at_ns=200,
    )
    reused = module.ProcessSample(
        pid=200,
        ppid=4,
        rss_kb=1,
        command="System",
        started_at_ns=201,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    report = module.terminate_watched_processes(
        100,
        samples={200: original},
        watched={200},
        expected_identities={200: module.process_identity(original)},
        sampler=lambda: {200: reused},
        grace=0.0,
    )

    assert sent == []
    assert any(
        action.target_id == 200 and action.result == "skipped_identity_mismatch"
        for action in report.actions
    )


def test_windows_termination_revalidates_identity_between_term_and_kill(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    original = module.ProcessSample(
        pid=200,
        ppid=1,
        rss_kb=1,
        command="worker.exe",
        started_at_ns=200,
    )
    reused = module.ProcessSample(
        pid=200,
        ppid=4,
        rss_kb=1,
        command="System",
        started_at_ns=201,
    )
    samples = iter(({200: original}, {200: reused}))
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    report = module.terminate_watched_processes(
        100,
        samples={200: original},
        watched={200},
        expected_identities={200: module.process_identity(original)},
        sampler=lambda: next(samples),
        grace=0.0,
    )

    assert sent == [(200, module.signal.SIGTERM)], report.actions
    assert [action.result for action in report.actions if action.target_id == 200] == [
        "still_live",
        "skipped_identity_mismatch",
    ]


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
        sampler=lambda: samples,
    )

    assert sent == []


def test_terminate_watched_processes_windows_refuses_owned_root_with_empty_samples(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    report = module.terminate_watched_processes(
        100,
        samples={},
        watched=set(),
        grace=0.0,
        root_owned=True,
    )

    assert sent == []
    assert any(
        action.target_kind == "process"
        and action.target_id == 100
        and action.result == "skipped_missing_identity"
        for action in report.actions
    )


def test_terminate_watched_processes_windows_refuses_external_codex_descendant_root(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=100,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
        ),
        101: module.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=10_000,
            started_at_ns=101,
            command="powershell.exe",
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=101,
            pgid=None,
            rss_kb=250_000,
            started_at_ns=200,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --daemon"
            ),
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    report = module.terminate_watched_processes(
        200,
        samples=samples,
        watched={200},
        grace=0.0,
        root_owned=True,
        sampler=lambda: samples,
    )

    assert sent == []
    assert any(
        action.target_kind == "process"
        and action.target_id == 200
        and action.result == "skipped_host_control_lineage"
        for action in report.actions
    )


def test_pid_signal_windows_refuses_external_codex_lineage_without_group_net(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=100,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
        ),
        101: module.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=10_000,
            started_at_ns=101,
            command="powershell.exe",
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=101,
            pgid=None,
            rss_kb=250_000,
            started_at_ns=200,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --daemon"
            ),
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    action = module._send_pid_signal_if_identity_action(
        200,
        module.process_identity(samples[200]),
        module.signal.SIGTERM,
        sampler=lambda: samples,
    )

    assert action.result == "skipped_host_control_lineage"
    assert sent == []


def test_terminate_single_pid_windows_refuses_external_codex_lineage(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=100,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --daemon"
            ),
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    assert module._terminate_single_pid(200, grace=0.0) is True
    assert sent == []


def test_terminate_single_pid_windows_rechecks_identity_before_signal(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    owned = {
        200: module.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --owned"
            ),
            started_at_ns=111,
        )
    }
    reused_as_codex = {
        200: module.ProcessSample(
            pid=200,
            ppid=1,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
            started_at_ns=222,
        )
    }
    sample_calls = 0
    sent: list[tuple[int, int]] = []

    def sample_processes():
        nonlocal sample_calls
        sample_calls += 1
        return owned if sample_calls == 1 else reused_as_codex

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "sample_processes", sample_processes)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 99999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    assert module._terminate_single_pid(200, grace=0.0) is True
    assert sent == []
    assert sample_calls >= 2


def test_pid_signal_windows_keeps_current_guard_child_killable(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=100,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
        ),
        999: module.ProcessSample(
            pid=999,
            ppid=100,
            pgid=None,
            rss_kb=30_000,
            started_at_ns=999,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tools\memory_guard.py --"
            ),
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=250_000,
            started_at_ns=200,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --owned"
            ),
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    action = module._send_pid_signal_if_identity_action(
        200,
        module.process_identity(samples[200]),
        module.signal.SIGTERM,
        sampler=lambda: samples,
    )

    assert action.result == "sent"
    assert sent == [(200, module.signal.SIGTERM)]


def test_pid_signal_windows_refuses_current_guard_shell_child(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=100,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
        ),
        999: module.ProcessSample(
            pid=999,
            ppid=100,
            pgid=None,
            rss_kb=30_000,
            started_at_ns=999,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tools\memory_guard.py --"
            ),
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=25_000,
            started_at_ns=200,
            command="powershell.exe -NoProfile",
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    action = module._send_pid_signal_if_identity_action(
        200,
        module.process_identity(samples[200]),
        module.signal.SIGTERM,
        sampler=lambda: samples,
    )

    assert action.result == "skipped_host_control_lineage"
    assert sent == []


def test_terminate_watched_processes_windows_keeps_current_guard_child_killable(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=100,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
        ),
        999: module.ProcessSample(
            pid=999,
            ppid=100,
            pgid=None,
            rss_kb=30_000,
            started_at_ns=999,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tools\memory_guard.py --"
            ),
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=250_000,
            started_at_ns=200,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --owned"
            ),
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    module.terminate_watched_processes(
        200,
        samples=samples,
        watched={200},
        grace=0.0,
        sampler=lambda: samples,
    )

    assert (200, module.signal.SIGTERM) in sent
    assert (200, module.fallback_kill_signal()) in sent


def test_terminate_watched_processes_windows_refuses_current_guard_shell_child(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=100,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
        ),
        999: module.ProcessSample(
            pid=999,
            ppid=100,
            pgid=None,
            rss_kb=30_000,
            started_at_ns=999,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tools\memory_guard.py --"
            ),
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=25_000,
            started_at_ns=200,
            command="cmd.exe /d /c cargo check",
        ),
    }
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    report = module.terminate_watched_processes(
        200,
        samples=samples,
        watched={200},
        grace=0.0,
        root_owned=True,
        sampler=lambda: samples,
    )

    assert sent == []
    assert any(
        action.target_kind == "process"
        and action.target_id == 200
        and action.result == "skipped_host_control_lineage"
        for action in report.actions
    )


def test_cleanup_tracked_orphans_windows_passes_live_descendants_to_terminator(
    monkeypatch,
) -> None:
    module = _load_memory_guard()
    tracker = module.ProcessTreeTracker(root_pid=100)
    initial = {
        100: module.ProcessSample(
            pid=100, ppid=50, rss_kb=1, command="uv.exe", started_at_ns=100
        ),
        101: module.ProcessSample(
            pid=101, ppid=100, rss_kb=1, command="python.exe", started_at_ns=101
        ),
    }
    tracker.update(initial)
    live = {
        101: module.ProcessSample(
            pid=101, ppid=100, rss_kb=1, command="python.exe", started_at_ns=101
        ),
    }
    terminated: dict[str, object] = {}

    def fake_terminate(  # noqa: ANN001
        root_pid,
        *,
        samples,
        watched,
        tracker,
        expected_identities,
        grace,
        reason,
        sampler,
        root_owned,
    ):
        terminated["root_pid"] = root_pid
        terminated["samples"] = samples
        terminated["watched"] = set(watched)
        terminated["tracker"] = tracker
        terminated["expected_identities"] = dict(expected_identities)
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
            actions=(
                module.GuardTerminationAction(
                    target_kind="process",
                    target_id=101,
                    signal=module.signal.SIGTERM,
                    signal_name="SIGTERM",
                    result="completed_or_missing",
                ),
            ),
        )

    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
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
    assert terminated["expected_identities"] == {
        101: module.process_identity(live[101])
    }
    assert terminated["grace"] == 0.5
    assert terminated["reason"] == "tracked_orphan_cleanup"
    assert terminated["sampler"] is not None
    assert terminated["root_owned"] is True
