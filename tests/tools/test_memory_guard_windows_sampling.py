from __future__ import annotations

import json
import os
import subprocess
from types import SimpleNamespace
import sys
import time
from pathlib import Path

import pytest

from molt.process_environment import ProcessEnvironment

from tools import memory_guard
from tools.memory_guard_core import process_custody

from molt import process_model, windows_process_snapshot

import molt.pytest_memory_guard_bootstrap as pytest_memory_guard_bootstrap
from tests.process_guard_common import (
    ModuleView,
    close_owned_test_process,
    install_module_os_view,
    install_module_view,
    install_thread_module_view,
    run_guarded_test_process,
    start_owned_test_process,
)

# These tests fake process data the session sentinel also reads.
pytestmark = pytest.mark.usefixtures("session_sentinel_paused")


REPO_ROOT = Path(__file__).resolve().parents[2]
# The custody modules call each other: process_custody signals and reads
# os.getpid, and process_model reads os.getpid for the watched set.
_CUSTODY_MODULES = (
    process_custody,
    process_model,
    windows_process_snapshot,
    pytest_memory_guard_bootstrap,
)


def _fake_custody(monkeypatch, attribute: str, real: object, **fakes) -> None:
    """Fake ``real`` in every custody module, on this test's thread only.

    Other threads of the pytest process call the custody modules too, so
    each function fake answers only the installing thread.
    """
    install_thread_module_view(monkeypatch, attribute, real, *_CUSTODY_MODULES, **fakes)


@pytest.fixture(autouse=True)
def custody_modules_use_a_private_os(monkeypatch) -> None:
    """Every ``os`` patch in this file stays inside the custody modules."""
    install_module_os_view(monkeypatch, *_CUSTODY_MODULES)


@pytest.mark.parametrize("name", ["nt", "posix"])
def test_module_platform_simulation_preserves_host_paths(
    monkeypatch, tmp_path: Path, name: str
) -> None:
    host_name = os.name
    host_os = windows_process_snapshot.os
    witness = tmp_path / "host-path.txt"
    witness.write_text("host filesystem", encoding="utf-8")

    with monkeypatch.context() as local_patch:
        install_module_os_view(local_patch, windows_process_snapshot, name=name)
        assert windows_process_snapshot.os is not host_os
        assert windows_process_snapshot.os.name == name
        assert windows_process_snapshot.os.environ is os.environ
        assert os.name == host_name
        assert Path(str(witness)).resolve().read_text(encoding="utf-8") == (
            "host filesystem"
        )

    assert windows_process_snapshot.os is host_os
    assert os.name == host_name


def _windows_guard_creationflags(module) -> int:
    from molt.process_spawn import hidden_windows_process_group_creationflags

    return hidden_windows_process_group_creationflags(
        subprocess_module=module.subprocess
    )


def test_parse_windows_process_snapshot_rows_binds_only_visited_resident_sizes(
    monkeypatch,
) -> None:
    module = memory_guard
    rows = [
        (100, 42, "python.exe", 7, 123456789, "full"),
        (101, 100, "", None, None, "unavailable"),
        (102, 42, "other.exe", 8, 123456790, "image"),
        (0, 0, "System Idle Process", None, None, "image"),
    ]
    resident_reads = []

    def resident(pid, birth):
        if birth is None:
            return 0
        resident_reads.append((pid, birth))
        assert (pid, birth) == (100, 123456789)
        return 2

    monkeypatch.setattr(process_model, "windows_process_resident_kb", resident)

    samples = module.parse_windows_process_snapshot_rows(rows)

    assert sorted(samples) == [100, 101, 102]
    assert resident_reads == []
    assert samples[100].ppid == 42
    assert samples[100].rss_kb == 2
    assert samples[100].rss_kb == 2
    assert samples[100].command == "python.exe"
    assert samples[100].elapsed_sec == 7
    assert samples[100].started_at_ns == 123456789
    assert samples[101].ppid == 100
    assert samples[101].rss_kb == 0
    assert samples[101].command == "pid:101"
    assert samples[101].elapsed_sec is None
    assert resident_reads == [(100, 123456789)]


def test_sample_processes_uses_windows_sampler_on_nt(monkeypatch) -> None:
    module = process_model
    sample = module.ProcessSample(pid=7, ppid=1, rss_kb=9, command="python.exe")

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "sample_processes_windows", lambda: {7: sample})
    monkeypatch.setattr(module, "sample_processes_posix", lambda: {})

    assert module.sample_processes() == {7: sample}


def test_sample_processes_posix_missing_ps_is_typed_failure(monkeypatch) -> None:
    """Other BSDs still shell out to ps; its absence is a typed snapshot failure."""
    module = process_model

    def missing_ps(*args, **kwargs):  # noqa: ANN002, ANN003
        raise FileNotFoundError("ps")

    install_module_view(monkeypatch, "sys", sys, process_model, platform="freebsd14")
    _fake_custody(monkeypatch, "subprocess", subprocess, run=missing_ps)

    with pytest.raises(
        windows_process_snapshot.ProcessSnapshotError, match="POSIX process snapshot"
    ):
        module.sample_processes_posix()


def _darwin_row(
    *, ppid: int, pgid: int, started_at_ns: int, command: str, uid: int = 501
) -> process_model._DarwinKernelProcRow:
    return process_model._DarwinKernelProcRow(
        status=2,
        ppid=ppid,
        pgid=pgid,
        started_at_ns=started_at_ns,
        command=command,
        uid=uid,
    )


def _install_fake_darwin_authority(monkeypatch, authority: object) -> None:
    install_module_view(monkeypatch, "sys", sys, process_model, platform="darwin")
    monkeypatch.setattr(
        process_model,
        "_darwin_process_authority_cache",
        process_model._DARWIN_PROCESS_AUTHORITY_UNSET,
    )
    monkeypatch.setattr(
        process_model, "_load_darwin_process_authority", lambda: authority
    )
    monkeypatch.setattr(process_model, "_darwin_viewer_uid", lambda: 501)


def test_darwin_sampler_never_shells_out_and_types_enumeration_failure(
    monkeypatch,
) -> None:
    class FakeAuthority:
        def kernel_table(self) -> dict[int, process_model._DarwinKernelProcRow]:
            raise OSError("KERN_PROC_ALL read failed")

    def forbidden_run(*args, **kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("Darwin sampler must not shell out to ps")

    _install_fake_darwin_authority(monkeypatch, FakeAuthority())
    _fake_custody(monkeypatch, "subprocess", subprocess, run=forbidden_run)

    with pytest.raises(
        windows_process_snapshot.ProcessSnapshotError,
        match="Darwin process enumeration failed",
    ):
        process_model.sample_processes_posix()


def test_darwin_sampler_sizes_readable_rows_and_withholds_foreign_ones(
    monkeypatch,
) -> None:
    worker = _darwin_row(ppid=100, pgid=200, started_at_ns=1_000, command="node")
    # A system daemon: the kernel table lists it, its detail is root's.
    daemon = _darwin_row(ppid=1, pgid=300, started_at_ns=5_000, command="mds", uid=0)

    class FakeAuthority:
        def kernel_table(self) -> dict[int, process_model._DarwinKernelProcRow]:
            return {200: worker, 300: daemon}

        def resident_kb(self, pid: int) -> int | None:
            assert pid == 200, "a withheld row is never sized"
            return 64

        def kernel_row(self, pid: int) -> process_model._DarwinKernelProcRow:
            assert pid == 200, "a withheld row binds no argv"
            return worker

        def argv(self, pid: int) -> tuple[str, ...] | None:
            assert pid == 200, "a withheld row binds no argv"
            return ("node", "worker.js")

    _install_fake_darwin_authority(monkeypatch, FakeAuthority())

    samples = process_model.sample_processes_posix()

    assert samples[200].rss_kb == 64
    assert samples[200].ppid == 100
    assert samples[200].command == "node worker.js"
    assert samples[300].rss_kb == 0
    assert samples[300].ppid == 0
    assert samples[300].pgid == 300
    assert samples[300].command == "mds"
    assert samples[300].started_at_ns is None


def test_darwin_custody_poll_reads_the_tree_not_the_host_table(monkeypatch) -> None:
    # Root 100 forked 101, which forked 102 since the last poll. 103 was
    # reparented to launchd and stays tracked by pid. 104's number was reused
    # by an unrelated process between the two reads, and 105 is an unreaped
    # zombie. 500 is the suite lease owner: its row, never its children.
    zombie = process_model._DarwinKernelProcRow(
        status=process_model._DARWIN_SZOMB,
        ppid=100,
        pgid=100,
        started_at_ns=2_500,
        command="sh",
        uid=501,
    )
    rows = {
        100: _darwin_row(ppid=50, pgid=100, started_at_ns=1_000, command="molt"),
        101: _darwin_row(ppid=100, pgid=100, started_at_ns=2_000, command="cargo"),
        102: _darwin_row(ppid=101, pgid=100, started_at_ns=3_000, command="rustc"),
        103: _darwin_row(ppid=1, pgid=103, started_at_ns=1_500, command="daemon"),
        104: _darwin_row(ppid=999, pgid=999, started_at_ns=4_000, command="other"),
        105: zombie,
        500: _darwin_row(ppid=1, pgid=500, started_at_ns=500, command="pytest"),
        501: _darwin_row(ppid=500, pgid=500, started_at_ns=600, command="worker"),
    }
    children = {100: (101, 104, 105), 101: (102,), 500: (501,)}
    walked: list[int] = []

    class FakeAuthority:
        def kernel_table(self) -> dict[int, process_model._DarwinKernelProcRow]:
            raise AssertionError("a custody poll never reads the host table")

        def kernel_row(self, pid: int) -> process_model._DarwinKernelProcRow | None:
            return rows.get(pid)

        def child_pids(self, ppid: int) -> tuple[int, ...]:
            walked.append(ppid)
            return children.get(ppid, ())

    _install_fake_darwin_authority(monkeypatch, FakeAuthority())

    samples = process_model.sample_processes_posix_custody(
        process_model.CustodyInterest(
            tree_pids=frozenset({100, 103}), exact_pids=frozenset({500})
        )
    )

    assert set(samples) == {100, 101, 102, 103, 500}
    assert 500 not in walked
    tracker = process_model.ProcessTreeTracker(100)
    tracker.known_pids.add(103)
    assert tracker.update(samples) == {100, 101, 102, 103}


def test_darwin_process_authority_binds_once_for_all_pid_reads(monkeypatch) -> None:
    class FakeAuthority:
        def kernel_row(self, pid: int) -> process_model._DarwinKernelProcRow:
            return _darwin_row(ppid=1, pgid=pid, started_at_ns=123_000, command="node")

        def argv(self, pid: int) -> tuple[str, ...]:
            return ("node", f"codex-{pid}.js")

    authority = FakeAuthority()
    loads: list[None] = []
    _install_fake_darwin_authority(monkeypatch, authority)
    monkeypatch.setattr(
        process_model,
        "_load_darwin_process_authority",
        lambda: loads.append(None) or authority,
    )

    assert process_model._darwin_proc_started_at_ns(7) == 123_000
    assert process_model._darwin_proc_argv(7) == ("node", "codex-7.js")
    assert process_model._darwin_proc_started_at_ns(8) == 123_000
    assert process_model._darwin_proc_argv(8) == ("node", "codex-8.js")
    assert loads == [None]


def test_darwin_process_authority_retains_one_library_binding_set(
    monkeypatch,
) -> None:
    import ctypes

    class FakeFunction:
        argtypes = None
        restype = None

    libproc = SimpleNamespace(proc_pidinfo=FakeFunction(), proc_listpids=FakeFunction())
    libsystem = SimpleNamespace(sysctl=FakeFunction())
    loads: list[str] = []

    def fake_cdll(path: str, *, use_errno: bool) -> object:
        assert use_errno
        loads.append(path)
        return libproc if path.endswith("libproc.dylib") else libsystem

    install_module_view(monkeypatch, "sys", sys, process_model, platform="darwin")
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
    assert first.kinfo.size == process_model._DARWIN_KINFO_PROC_SIZE
    assert loads == [
        "/usr/lib/libproc.dylib",
        "/usr/lib/libSystem.B.dylib",
    ]


def test_darwin_cached_authority_preserves_bound_command_and_identity(
    monkeypatch,
) -> None:
    row = _darwin_row(ppid=100, pgid=200, started_at_ns=987_654_321_000, command="node")

    class FakeAuthority:
        kernel_row_calls = 0
        argv_calls = 0

        def kernel_table(self) -> dict[int, process_model._DarwinKernelProcRow]:
            return {200: row}

        def resident_kb(self, pid: int) -> int:
            return 64

        def kernel_row(self, pid: int) -> process_model._DarwinKernelProcRow:
            assert pid == 200
            self.kernel_row_calls += 1
            return row

        def argv(self, pid: int) -> tuple[str, ...]:
            assert pid == 200
            self.argv_calls += 1
            return ("node", "/usr/local/lib/node_modules/@openai/codex/bin/codex.js")

    authority = FakeAuthority()
    _install_fake_darwin_authority(monkeypatch, authority)

    sample = process_model.sample_processes_posix()[200]

    assert sample.ppid == 100
    assert sample.pgid == 200
    assert sample.started_at_ns == 987_654_321_000
    assert (authority.argv_calls, authority.kernel_row_calls) == (0, 0)
    assert process_model.is_host_control_plane_process(sample)
    assert process_model.is_host_control_plane_process(sample)
    assert (authority.argv_calls, authority.kernel_row_calls) == (1, 1)


def test_darwin_cached_authority_keeps_reuse_fail_closed(monkeypatch) -> None:
    sampled = _darwin_row(ppid=100, pgid=200, started_at_ns=111_000, command="node")
    reused = _darwin_row(ppid=4, pgid=200, started_at_ns=222_000, command="node")

    class FakeAuthority:
        def kernel_table(self) -> dict[int, process_model._DarwinKernelProcRow]:
            return {200: sampled}

        def resident_kb(self, pid: int) -> int:
            return 64

        def kernel_row(self, pid: int) -> process_model._DarwinKernelProcRow:
            assert pid == 200
            return reused

        def argv(self, pid: int) -> tuple[str, ...]:
            assert pid == 200
            return ("node", "/usr/local/lib/node_modules/@openai/codex/bin/codex.js")

    _install_fake_darwin_authority(monkeypatch, FakeAuthority())

    sample = process_model.sample_processes_posix()[200]

    # The reused pid's argv never binds to the sampled instance.
    assert sample.argv == ()
    assert sample.command == "node"
    assert not process_model.is_host_control_plane_process(sample)
    assert (sample.ppid, sample.started_at_ns) == (100, 111_000)


def test_sample_processes_windows_uses_injected_snapshot_authority(monkeypatch) -> None:
    module = process_model

    def fail_run(*args, **kwargs):  # noqa: ANN002, ANN003
        raise AssertionError("Windows sampler must not shell out")

    _fake_custody(monkeypatch, "subprocess", subprocess, run=fail_run)
    monkeypatch.setattr(
        module,
        "_windows_process_snapshot_rows_hard_timeout",
        lambda: [(7, 1, "python.exe", 3, 123, "full")],
    )
    monkeypatch.setattr(process_model, "windows_process_resident_kb", lambda *args: 9)

    samples = module.sample_processes_windows()

    assert sorted(samples) == [7]
    assert samples[7].ppid == 1
    assert samples[7].rss_kb == 9
    assert samples[7].command == "python.exe"


def test_sample_processes_windows_timeout_fails_closed(monkeypatch) -> None:
    module = process_model

    def timed_out():
        raise TimeoutError("snapshot deadline")

    monkeypatch.setattr(
        module, "_windows_process_snapshot_rows_hard_timeout", timed_out
    )

    with pytest.raises(
        windows_process_snapshot.ProcessSnapshotError, match="Windows process snapshot"
    ):
        module.sample_processes_windows()


def test_windows_process_snapshot_timeout_env_contract() -> None:
    name = windows_process_snapshot.WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_ENV

    assert (
        windows_process_snapshot._windows_process_snapshot_timeout_sec({})
        == windows_process_snapshot.DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC
    )
    assert (
        windows_process_snapshot._windows_process_snapshot_timeout_sec({name: "0.25"})
        == 0.25
    )
    assert windows_process_snapshot._windows_process_snapshot_timeout_sec(
        {name: "bad"}
    ) == (windows_process_snapshot.DEFAULT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC)
    assert (
        windows_process_snapshot._windows_process_snapshot_timeout_sec({name: "0"})
        is None
    )
    assert (
        windows_process_snapshot._windows_process_snapshot_timeout_sec({name: "off"})
        is None
    )


@pytest.mark.skipif(not sys.platform.startswith("win"), reason="Win32 API bindings")
def test_windows_snapshot_reuses_one_immutable_api_binding() -> None:
    windows_process_snapshot._windows_snapshot_api.cache_clear()
    first = windows_process_snapshot._windows_snapshot_api()
    second = windows_process_snapshot._windows_snapshot_api()

    assert first is second
    assert first.ProcessEntry32W is second.ProcessEntry32W
    assert first.counters_type is second.counters_type


def test_pid_alive_reads_live_and_absent_processes() -> None:
    child = start_owned_test_process(
        [sys.executable, "-c", "import time; time.sleep(60)"]
    )
    try:
        assert process_model.pid_alive(child.pid)
        assert process_model.pid_alive(os.getpid())
    finally:
        close_owned_test_process(child)
    # Reaped: no process holds the pid.
    assert not process_model.pid_alive(child.pid)
    for pid in (0, -1, True, 2**64):
        assert not process_model.pid_alive(pid)


@pytest.mark.skipif(os.name != "posix", reason="init is POSIX pid 1")
def test_pid_alive_counts_a_process_it_may_not_signal() -> None:
    # Another user's process refuses signal 0 with EPERM, yet it runs. Under
    # root, signal 0 succeeds; either way init lives.
    assert process_model.pid_alive(1)


def test_pid_alive_counts_an_exited_unreaped_child_dead_and_never_reaps_it() -> None:
    child = start_owned_test_process([sys.executable, "-c", "raise SystemExit(7)"])
    try:
        deadline = time.monotonic() + 30
        while process_model.pid_alive(child.pid):
            assert time.monotonic() < deadline, "an exited child stays alive"
            time.sleep(0.01)
        if os.name == "posix":
            # A zombie: the kernel still lists the exited child, so the
            # signal-0 probe of the four retired copies counted it alive.
            os.kill(child.pid, 0)
        # The probe never reaped it: its owner still reads the exit status.
        assert child.wait(timeout=30) == 7
    finally:
        close_owned_test_process(child)


def test_linux_proc_state_reads_the_field_after_the_command_name(tmp_path) -> None:
    stat = tmp_path / "41" / "stat"
    stat.parent.mkdir()
    # The command name may hold spaces and ")": the state follows the last ")".
    stat.write_text("41 (a) b) Z 1 41 41 0 -1 4194560\n", encoding="utf-8")
    assert process_model._linux_proc_state(41, tmp_path) == b"Z"
    assert process_model._LINUX_EXITED_STATES >= {b"Z", b"X", b"x"}
    stat.write_text("41 (sleep) S 1 41 41 0 -1 4194560\n", encoding="utf-8")
    assert process_model._linux_proc_state(41, tmp_path) == b"S"
    assert process_model._linux_proc_state(42, tmp_path) is None


class _WindowsLivenessApi:
    """The kernel32 calls ``windows_pid_alive`` makes, with recorded effects."""

    def __init__(self, *, handle: int, wait_result: int = 0x102) -> None:
        self.handle = handle
        self.wait_result = wait_result
        self.calls: list[tuple] = []

    def OpenProcess(self, access: int, inherit: bool, pid: int) -> int:  # noqa: N802
        self.calls.append(("open", access, inherit, pid))
        return self.handle

    def WaitForSingleObject(self, handle: int, milliseconds: int) -> int:  # noqa: N802
        self.calls.append(("wait", handle, milliseconds))
        return self.wait_result

    def CloseHandle(self, handle: int) -> bool:  # noqa: N802
        self.calls.append(("close", handle))
        return True


def _install_windows_liveness(monkeypatch, api, *, last_error: int = 0) -> None:
    import ctypes

    # Signal 0 on Windows is CTRL_C_EVENT: the probe must never send it.
    install_module_view(
        monkeypatch,
        "os",
        os,
        process_model,
        windows_process_snapshot,
        name="nt",
        kill=lambda *_: pytest.fail("pid_alive signaled a Windows process"),
    )
    monkeypatch.setattr(windows_process_snapshot, "process_query_api", lambda: api)
    monkeypatch.setattr(ctypes, "get_last_error", lambda: last_error, raising=False)


@pytest.mark.parametrize(
    ("wait_result", "alive"),
    [(0x102, True), (0, False), (0xFFFFFFFF, True)],
)
def test_windows_pid_alive_waits_on_the_process_object(
    monkeypatch, wait_result: int, alive: bool
) -> None:
    # WAIT_TIMEOUT: running. WAIT_OBJECT_0: exited, even while handles keep
    # the object (an exit code of 259 cannot tell). WAIT_FAILED proves nothing.
    api = _WindowsLivenessApi(handle=77, wait_result=wait_result)
    _install_windows_liveness(monkeypatch, api)

    assert process_model.pid_alive(4242) is alive
    assert api.calls == [
        ("open", 0x1000 | 0x00100000, False, 4242),
        ("wait", 77, 0),
        ("close", 77),
    ]


@pytest.mark.parametrize(("last_error", "alive"), [(5, True), (87, False)])
def test_windows_pid_alive_without_a_handle_reads_the_open_error(
    monkeypatch, last_error: int, alive: bool
) -> None:
    # ERROR_ACCESS_DENIED: the process exists but refuses this caller.
    # ERROR_INVALID_PARAMETER: no process has the pid.
    api = _WindowsLivenessApi(handle=0)
    _install_windows_liveness(monkeypatch, api, last_error=last_error)

    assert process_model.pid_alive(4242) is alive
    assert [call[0] for call in api.calls] == ["open"]


def test_windows_pid_alive_rejects_pids_outside_a_dword(monkeypatch) -> None:
    api = _WindowsLivenessApi(handle=77)
    _install_windows_liveness(monkeypatch, api)

    assert not process_model.pid_alive(2**32)
    assert api.calls == []


def test_windows_process_snapshot_hard_timeout_kills_helper(monkeypatch) -> None:
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    monkeypatch.setattr(
        windows_process_snapshot,
        "_windows_process_snapshot_timeout_sec",
        lambda: 0.25,
    )

    def timed_out(*args, **kwargs):  # noqa: ANN002, ANN003
        raise subprocess.TimeoutExpired(cmd=args[0], timeout=kwargs["timeout"])

    _fake_custody(monkeypatch, "subprocess", subprocess, run=timed_out)

    with pytest.raises(windows_process_snapshot.WindowsProcessSnapshotTimeout):
        windows_process_snapshot._windows_process_snapshot_rows_hard_timeout()


def test_windows_process_snapshot_hard_timeout_decodes_complete_rows(
    monkeypatch,
) -> None:
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    monkeypatch.setattr(
        windows_process_snapshot,
        "_windows_process_snapshot_timeout_sec",
        lambda: 0.25,
    )

    def fake_run(*args, **kwargs):  # noqa: ANN002, ANN003
        assert kwargs["timeout"] == 0.25
        assert kwargs["check"] is False
        assert kwargs["capture_output"] is True
        assert kwargs["text"] is True
        assert args[0][1:] == [
            "-P",
            "-m",
            "molt.windows_process_snapshot",
            windows_process_snapshot.WINDOWS_PROCESS_SNAPSHOT_HELPER_ARG,
        ]
        assert kwargs["env"]["PYTHONPATH"] == str(
            Path(windows_process_snapshot.__file__).resolve().parents[1]
        )
        return SimpleNamespace(
            returncode=0,
            stdout='[[7,1,"python.exe",3,123456789,"full"]]',
            stderr="",
        )

    _fake_custody(monkeypatch, "subprocess", subprocess, run=fake_run)

    assert windows_process_snapshot._windows_process_snapshot_rows_hard_timeout() == [
        (7, 1, "python.exe", 3, 123456789, "full")
    ]


def test_windows_process_snapshot_helper_runs_this_molt_from_any_directory(
    tmp_path,
) -> None:
    # A `molt` in the working directory or on the caller's import path would
    # replace the helper's implementation; the helper must import this one.
    shadow = tmp_path / "molt"
    shadow.mkdir()
    (shadow / "__init__.py").write_text("raise SystemExit(99)\n", encoding="utf-8")
    env = windows_process_snapshot._windows_process_snapshot_helper_env(
        {**os.environ, "PYTHONPATH": str(tmp_path)}
    )
    result = run_guarded_test_process(
        [
            sys.executable,
            "-P",
            "-m",
            windows_process_snapshot.WINDOWS_PROCESS_SNAPSHOT_HELPER_MODULE,
            "--not-the-helper-argument",
        ],
        cwd=tmp_path,
        env=env,
        timeout=60,
    )
    # `_main` rejects a foreign argument with 2 before any Win32 query.
    assert result.returncode == 2, result.stdout + result.stderr


def test_windows_process_snapshot_hard_timeout_rejects_partial_payload(
    monkeypatch,
) -> None:
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    monkeypatch.setattr(
        windows_process_snapshot,
        "_windows_process_snapshot_timeout_sec",
        lambda: 0.25,
    )
    _fake_custody(
        monkeypatch,
        "subprocess",
        subprocess,
        run=lambda *args, **kwargs: SimpleNamespace(  # noqa: ARG005, ANN002, ANN003
            returncode=0,
            stdout='[[7,1,"python.exe",3]]',
            stderr="",
        ),
    )

    with pytest.raises(
        windows_process_snapshot.ProcessSnapshotError, match="invalid payload"
    ):
        windows_process_snapshot._windows_process_snapshot_rows_hard_timeout()


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
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    _fake_custody(
        monkeypatch,
        "subprocess",
        subprocess,
        run=lambda *args, **kwargs: SimpleNamespace(  # noqa: ARG005, ANN002, ANN003
            returncode=returncode,
            stdout=stdout,
            stderr=stderr,
        ),
    )

    with pytest.raises(windows_process_snapshot.ProcessSnapshotError, match=message):
        windows_process_snapshot._windows_process_snapshot_rows_hard_timeout()


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
    assert windows_process_snapshot._validated_windows_process_binding(
        7,
        bound_pid,
        bound_ppid,
        started_at_ns,
    ) == (0, None)


def test_windows_snapshot_accepts_parent_and_identity_from_same_handle() -> None:
    assert windows_process_snapshot._validated_windows_process_binding(
        7,
        7,
        3,
        123,
    ) == (3, 123)


def test_windows_process_handle_rss_fails_closed_when_native_api_unavailable(
    monkeypatch,
) -> None:
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")

    def missing_api():
        raise OSError("native process API unavailable")

    monkeypatch.setattr(windows_process_snapshot, "_windows_snapshot_api", missing_api)

    assert windows_process_snapshot.windows_process_handle_rss_kb(1234) is None


def test_windows_process_handle_rss_rejects_invalid_handle_values(
    monkeypatch,
) -> None:
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")

    assert windows_process_snapshot.windows_process_handle_rss_kb(None) is None
    assert (
        windows_process_snapshot.windows_process_handle_rss_kb("not-a-handle") is None
    )


@pytest.mark.parametrize(
    "fault",
    [
        "none",
        "denied",
        "birth",
        "exited",
        "times_failed",
        "rss_failed",
        "read_raises",
        "close_failed",
        "close_raises",
    ],
)
def test_windows_resident_size_uses_one_birth_bound_handle(monkeypatch, fault):
    import ctypes
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_uint32), ("WorkingSetSize", ctypes.c_size_t)]

    calls = []
    birth_ns = 3_000_000_000
    # FILETIME is 100 ns ticks since 1601; the fixture is three Unix seconds.
    created_ticks = 116444736030000000 + (fault == "birth")

    def open_process(rights, inherit, pid):
        calls.append(("open", rights, inherit, pid))
        return None if fault == "denied" else 44

    def get_times(handle, created, exited, kernel, user):
        calls.append(("times", handle))
        created._obj.dwLowDateTime = created_ticks & 0xFFFFFFFF
        created._obj.dwHighDateTime = created_ticks >> 32
        exited._obj.dwLowDateTime = int(fault == "exited")
        return fault != "times_failed"

    def get_memory(handle, counters, size):
        calls.append(("rss", handle))
        assert size == ctypes.sizeof(Counters)
        if fault == "read_raises":
            raise OSError("process exited while sizing")
        counters._obj.WorkingSetSize = 2049
        return fault != "rss_failed"

    def close(handle):
        calls.append(("close", handle))
        if fault == "close_raises":
            raise OSError("close denied")
        return fault != "close_failed"

    api = SimpleNamespace(
        ctypes=ctypes,
        wintypes=wintypes,
        counters_type=Counters,
        open_process=open_process,
        get_process_times=get_times,
        get_process_memory_info=get_memory,
        close_handle=close,
    )
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    monkeypatch.setattr(windows_process_snapshot, "_windows_snapshot_api", lambda: api)

    assert windows_process_snapshot.windows_process_resident_kb(7, birth_ns) == (
        3 if fault == "none" else 0
    )
    expected = [("open", 0x0410, False, 7)]
    if fault != "denied":
        expected.append(("times", 44))
        if fault not in {"birth", "exited", "times_failed"}:
            expected.append(("rss", 44))
        expected.append(("close", 44))
    assert calls == expected


@pytest.mark.parametrize(
    ("pid", "birth"),
    [(0, 1), (-1, 1), (True, 1), (2**32, 1), (7, 0), (7, True), (7, None)],
)
def test_windows_resident_size_rejects_unbound_identity(monkeypatch, pid, birth):
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")

    def forbidden_api():
        raise AssertionError("unbound identity reached the native API")

    monkeypatch.setattr(
        windows_process_snapshot, "_windows_snapshot_api", forbidden_api
    )
    assert windows_process_snapshot.windows_process_resident_kb(pid, birth) == 0


def test_windows_snapshot_keeps_readable_identity_without_resident_queries(
    monkeypatch,
):
    import ctypes
    from ctypes import wintypes

    class Entry(ctypes.Structure):
        _fields_ = [
            ("dwSize", ctypes.c_uint32),
            ("th32ProcessID", ctypes.c_uint32),
            ("szExeFile", ctypes.c_wchar * 30),
        ]

    class Counters(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_uint32), ("WorkingSetSize", ctypes.c_size_t)]

    last_error = [0]
    opened = []
    closed = []
    resident_reads = []

    def first(handle, entry):
        entry._obj.th32ProcessID = 7
        entry._obj.szExeFile = "other.exe"
        return True

    def next_row(handle, entry):
        last_error[0] = 18
        return False

    def get_times(handle, created, exited, kernel, user):
        ticks = 116444736030000000
        created._obj.dwLowDateTime = ticks & 0xFFFFFFFF
        created._obj.dwHighDateTime = ticks >> 32
        return True

    def get_memory(*args):
        resident_reads.append(args[0])
        raise AssertionError("host identity snapshot must not size processes")

    api = SimpleNamespace(
        ctypes=SimpleNamespace(
            sizeof=ctypes.sizeof,
            byref=ctypes.byref,
            set_last_error=lambda value: last_error.__setitem__(0, value),
            get_last_error=lambda: last_error[0],
        ),
        wintypes=wintypes,
        counters_type=Counters,
        create_snapshot=lambda *args: 99,
        process_first=first,
        process_next=next_row,
        close_handle=lambda handle: closed.append(handle) or True,
        open_process=lambda *args: opened.append(args) or 44,
        get_process_memory_info=get_memory,
        get_process_times=get_times,
        invalid_handle_value=-1,
        ProcessEntry32W=Entry,
    )
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    monkeypatch.setattr(windows_process_snapshot, "_windows_snapshot_api", lambda: api)
    monkeypatch.setattr(
        windows_process_snapshot,
        "_snapshot_basic_info",
        lambda *args: SimpleNamespace(
            UniqueProcessId=7, InheritedFromUniqueProcessId=3
        ),
    )
    monkeypatch.setattr(
        windows_process_snapshot, "_snapshot_image_name", lambda *args: "other.exe"
    )
    install_module_view(
        monkeypatch, "time", time, windows_process_snapshot, time=lambda: 10
    )

    assert windows_process_snapshot._windows_process_snapshot_rows() == [
        (7, 3, "other.exe", 7, 3_000_000_000, "image")
    ]
    assert resident_reads == []
    assert opened == [(0x0410, False, 7)]
    assert closed == [44, 99]


def test_windows_guarded_popen_uses_new_process_group(monkeypatch) -> None:
    module = memory_guard
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        module,
        CREATE_NEW_PROCESS_GROUP=0x00000200,
        CREATE_NO_WINDOW=0x08000000,
    )

    kwargs = module._guarded_popen_process_isolation_kwargs()

    assert kwargs == {"creationflags": 0x08000200}
    assert "start_new_session" not in kwargs


def test_windows_wrapper_terminalizes_worker_exit_running_summary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = memory_guard
    host_run = subprocess.run
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "subprocess", SimpleNamespace(**{**vars(subprocess), "run": fake_run})
    )
    assert subprocess.run is host_run

    def unexpected_run_guarded(*args, **kwargs):  # noqa: ANN002, ANN003
        pytest.fail("wrapper test must delegate to the mocked worker")

    monkeypatch.setattr(module, "run_guarded", unexpected_run_guarded)

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
        # Model an outer wrapper, independent of the test runner's worker flag.
        environ={},
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

    install_module_os_view(monkeypatch, harness_memory_guard, name="nt")
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        harness_memory_guard,
        CREATE_NEW_PROCESS_GROUP=0x00000200,
        CREATE_NO_WINDOW=0x08000000,
    )

    assert harness_memory_guard.batch_process_group_kwargs() == {
        "creationflags": 0x08000200
    }


def test_process_spawn_is_single_windows_hidden_group_authority() -> None:
    import molt.process_spawn as process_spawn

    # The helpers' default ``subprocess_module`` bound the real module when
    # Python defined them, so the Windows flags go in through that parameter.
    windows_subprocess = ModuleView(
        subprocess, CREATE_NEW_PROCESS_GROUP=0x00000200, CREATE_NO_WINDOW=0x08000000
    )

    assert process_spawn.hidden_windows_process_group_kwargs(
        windows=True, subprocess_module=windows_subprocess
    ) == {"creationflags": 0x08000200}
    assert process_spawn.detached_process_group_kwargs(
        windows=True, subprocess_module=windows_subprocess
    ) == {"creationflags": 0x08000200}
    assert process_spawn.detached_process_group_kwargs(windows=False) == {
        "start_new_session": True
    }


@pytest.mark.parametrize(
    ("command", "expected"),
    [
        (
            ["cmd.exe", "/d", "/s", "/c", 'python -c "raise SystemExit(23)"'],
            'cmd.exe /d /s /c "python -c "raise SystemExit(23)""',
        ),
        (
            [r"C:\Program Files\cmd.exe", "/C", '"tool path.exe" "%NAME%" && exit 7'],
            '"C:\\Program Files\\cmd.exe" /C ""tool path.exe" "%NAME%" && exit 7"',
        ),
    ],
)
def test_native_shell_serialization_preserves_shell_source(command, expected):
    from molt.process_spawn import native_popen_command

    assert native_popen_command(command, windows=True) == expected
    assert native_popen_command(command, windows=False) is command


@pytest.mark.parametrize(
    "command",
    [
        ["python.exe", "-c", 'print("quotes & percent %")'],
        ["cmd.exe", "/d"],
        ["cmd.exe", "/c", "echo", "literal argument"],
        ["tool.exe", "/c", 'source "quotes"'],
    ],
)
def test_native_argv_serialization_remains_cpython_owned(command):
    from molt.process_spawn import native_popen_command

    assert native_popen_command(command, windows=True) is command


def test_pytest_bootstrap_process_group_kwargs_hide_windows_console(
    monkeypatch,
) -> None:
    import molt.pytest_memory_guard_bootstrap as pytest_memory_guard_bootstrap

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    install_thread_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        pytest_memory_guard_bootstrap,
        CREATE_NEW_PROCESS_GROUP=0x00000200,
        CREATE_NO_WINDOW=0x08000000,
    )

    assert pytest_memory_guard_bootstrap._windows_process_group_kwargs() == {
        "creationflags": 0x08000200
    }


def test_pytest_bootstrap_handoff_preserves_stdio_under_hidden_console(
    monkeypatch,
) -> None:
    import molt.pytest_memory_guard_bootstrap as pytest_memory_guard_bootstrap

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
    install_thread_module_view(
        monkeypatch,
        "os",
        os,
        pytest_memory_guard_bootstrap,
        _exit=fake_exit,
    )
    install_thread_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        pytest_memory_guard_bootstrap,
        run=fake_run,
        CREATE_NEW_PROCESS_GROUP=0x00000200,
        CREATE_NO_WINDOW=0x08000000,
    )

    with pytest.raises(SystemExit) as exc:
        pytest_memory_guard_bootstrap.handoff_to_outer_guard(
            ["python", "-m", "pytest"],
            ProcessEnvironment({"PYTEST_CURRENT_TEST": "demo"}),
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
    module = memory_guard
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)

    assert module._guarded_popen_process_isolation_kwargs() == {
        "start_new_session": True
    }


def test_windows_sampler_limits_full_command_line_reads_to_launcher_processes() -> None:
    module = memory_guard

    assert module._windows_process_needs_full_command_line("python.exe") is True
    assert module._windows_process_needs_full_command_line("UV.EXE") is True
    assert module._windows_process_needs_full_command_line("node.exe") is True
    assert (
        module._windows_process_needs_full_command_line(
            "molt-backend.native_backend.exe"
        )
        is True
    )
    assert (
        module._windows_process_needs_full_command_line("molt-backend.wasm_backend.exe")
        is True
    )
    assert module._windows_process_needs_full_command_line("explorer.exe") is False
    assert module._windows_process_needs_full_command_line("svchost.exe") is False
    assert (
        module._windows_process_needs_full_command_line("molt-wasm-facts.exe") is True
    )
    assert (
        module._windows_process_needs_full_command_line("molt-wasm-facts.host_tool.exe")
        is True
    )


def test_command_executable_name_handles_windows_paths() -> None:
    module = memory_guard

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
    module = memory_guard

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
    module = memory_guard
    samples = {
        10: module.ProcessSample(
            pid=10,
            ppid=1,
            rss_kb=1,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\Codex.exe"
            ),
            started_at_ns=1000,
        ),
        20: module.ProcessSample(
            pid=20,
            ppid=10,
            rss_kb=1,
            command=r"C:\Users\adpen\.codex\worktrees\molt\target\dev-fast\molt-backend.exe",
            started_at_ns=2000,
        ),
        30: module.ProcessSample(
            pid=30,
            ppid=10,
            rss_kb=1,
            command=r"C:\Users\adpen\.codex\worktrees\molt\.venv\Scripts\python.exe tools\memory_guard.py",
            started_at_ns=2000,
        ),
        31: module.ProcessSample(
            pid=31,
            ppid=30,
            rss_kb=1,
            command=r"C:\Users\adpen\.codex\worktrees\molt\target\dev-fast\molt-backend.exe",
            started_at_ns=3000,
        ),
    }

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)

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
    module = memory_guard
    host_run = subprocess.run
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "inherit_stdio_kwargs", lambda: stdio)
    monkeypatch.setattr(
        module, "subprocess", SimpleNamespace(**{**vars(subprocess), "run": fake_run})
    )
    assert subprocess.run is host_run

    def unexpected_run_guarded(*args, **kwargs):  # noqa: ANN002, ANN003
        pytest.fail("wrapper test must delegate to the mocked worker")

    monkeypatch.setattr(module, "run_guarded", unexpected_run_guarded)

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


def test_process_group_signal_without_host_capability_fails_closed(monkeypatch) -> None:
    monkeypatch.delattr(process_custody.os, "killpg", raising=False)
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)
    action = process_custody._send_process_group_signal_action(100, 15)
    assert action.result == "failed"
    assert action.error == "host has no POSIX process-group signal capability"
    assert not process_custody.process_group_exited_or_unobservable(100, grace=0.01)


def test_terminate_watched_processes_windows_kills_owned_descendants(
    monkeypatch,
) -> None:
    module = process_custody
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 99999,
        kill=fake_kill,
    )

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
    module = process_custody
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
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

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
    module = process_custody
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
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

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
    module = process_custody
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 99999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

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
    module = process_custody
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 99999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

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
    module = process_custody
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 99999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

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
    module = memory_guard
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    # The loaded guard copy re-exports process_custody's functions; one view
    # keeps these patches out of the process-wide os module.
    install_module_os_view(monkeypatch, module, process_custody)
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
    module = process_custody
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 99999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

    assert module._terminate_single_pid(200, grace=0.0) is True
    assert sent == []


def test_terminate_single_pid_windows_rechecks_identity_before_signal(
    monkeypatch,
) -> None:
    module = process_custody
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

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(process_model, "sample_processes", sample_processes)
    monkeypatch.setattr(
        module, "_current_protected_process_group_ids", lambda _s, **_kw: set()
    )
    _fake_custody(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 99999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

    assert module._terminate_single_pid(200, grace=0.0) is True
    assert sent == []
    assert sample_calls >= 2


@pytest.mark.parametrize("entrypoint", ["pid_signal", "watched"])
@pytest.mark.parametrize(
    "command,parent_birth,child_birth,expected_result",
    [
        ("molt-backend.exe --owned", 100, 200, "sent"),
        ("molt-backend.exe --owned", 200, 200, "sent"),
        ("molt-backend.exe --owned", 999, 200, "skipped_host_control_lineage"),
        ("molt-backend.exe --owned", None, 200, "skipped_host_control_lineage"),
        ("molt-backend.exe --owned", 100, None, "skipped_ambiguous_identity"),
        ("powershell.exe -NoProfile", 100, 200, "skipped_host_control_lineage"),
        ("cmd.exe /d /c cargo check", 100, 200, "skipped_host_control_lineage"),
        ("node_repl.exe", 100, 200, "skipped_host_control_plane"),
    ],
    ids=[
        "ordered-births",
        "same-clock-tick",
        "reused-parent-pid",
        "unknown-parent-birth",
        "unknown-child-birth",
        "powershell-child",
        "cmd-child",
        "host-control-child",
    ],
)
def test_windows_current_guard_child_custody(
    monkeypatch,
    entrypoint,
    command,
    parent_birth,
    child_birth,
    expected_result,
) -> None:
    # PID values deliberately differ from creation order. All signal consumers
    # share this snapshot, including the valid ancestry of protected shell children.
    module = process_custody
    samples = {
        100: module.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            started_at_ns=50,
            command="codex.exe app-server",
        ),
        999: module.ProcessSample(
            pid=999,
            ppid=100,
            pgid=None,
            rss_kb=30_000,
            started_at_ns=parent_birth,
            command="python.exe tools/memory_guard.py --",
        ),
        200: module.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=250_000,
            started_at_ns=child_birth,
            command=command,
        ),
    }
    sent: list[tuple[int, int]] = []
    install_module_os_view(monkeypatch, module, name="nt")
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: sent.append((pid, sig)))

    if entrypoint == "pid_signal":
        action = module._send_pid_signal_if_identity_action(
            200,
            module.process_identity(samples[200]),
            module.signal.SIGTERM,
            sampler=lambda: samples,
        )
        assert action.result == expected_result
        expected_signals = [(200, module.signal.SIGTERM)]
    else:
        report = module.terminate_watched_processes(
            200,
            samples=samples,
            watched={200},
            grace=0.0,
            sampler=lambda: samples,
        )
        assert any(
            action.target_kind == "process"
            and action.target_id == 200
            and action.result == expected_result
            for action in report.actions
        )
        expected_signals = [
            (200, module.signal.SIGTERM),
            (200, module.fallback_kill_signal()),
        ]
    assert sent == (expected_signals if expected_result == "sent" else [])


@pytest.mark.parametrize(
    "root_reaped", [None, False, True], ids=["default", "unreaped", "reaped"]
)
def test_cleanup_tracked_orphans_windows_passes_live_descendants_to_terminator(
    monkeypatch,
    root_reaped: bool | None,
) -> None:
    module = process_custody
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
        root_reaped,
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
        terminated["root_reaped"] = root_reaped
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
        **({} if root_reaped is None else {"root_reaped": root_reaped}),
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
    assert terminated["root_reaped"] is (root_reaped is True)


@pytest.mark.parametrize("transfer", [0, 3, 4, 5])
def test_windows_memory_read_requires_exact_transfer(transfer):
    import ctypes

    def read(handle, address, buffer, size, returned):
        for index in range(size):
            buffer[index] = 65
        returned._obj.value = transfer
        return True

    api = SimpleNamespace(ctypes=ctypes, read_memory=read)
    result = windows_process_snapshot._snapshot_read_memory(
        api, None, 123, 4, lambda stage: None
    )
    assert result == (b"AAAA" if transfer == 4 else None)


@pytest.mark.parametrize(
    "address,size", [(0, 4), (-1, 4), (123, 0), (1 << 64, 4), ((1 << 64) - 2, 4)]
)
def test_windows_memory_read_rejects_unrepresentable_ranges(address, size):
    import ctypes

    def read(*args):
        raise AssertionError("invalid range reached native reader")

    api = SimpleNamespace(ctypes=ctypes, read_memory=read)
    assert (
        windows_process_snapshot._snapshot_read_memory(
            api, None, address, size, lambda stage: None
        )
        is None
    )


@pytest.mark.parametrize(
    "payload",
    ["A" * 32767, "A" * 16385, "hello 😀"],
    ids=["maximum-utf16", "above-old-truncation", "surrogate-pair"],
)
def test_windows_command_read_preserves_complete_utf16(monkeypatch, payload):
    raw = payload.encode("utf-16-le")
    api = SimpleNamespace(
        peb_process_parameters_offset=0x20,
        pointer_size=8,
        command_line_offset=0x70,
        command_line_buffer_offset=0x78,
    )
    monkeypatch.setattr(
        windows_process_snapshot,
        "_snapshot_basic_info",
        lambda *args: SimpleNamespace(PebBaseAddress=0x1000),
    )
    fields = {0x1020: 0x2000, 0x2070: len(raw), 0x2072: len(raw), 0x2078: 0x3000}
    monkeypatch.setattr(
        windows_process_snapshot,
        "_snapshot_read_integer",
        lambda api, handle, address, size, deadline: fields[address],
    )
    sizes = []

    def read(api, handle, address, size, deadline):
        sizes.append(size)
        return raw

    monkeypatch.setattr(windows_process_snapshot, "_snapshot_read_memory", read)
    assert (
        windows_process_snapshot._snapshot_command_line(api, None, lambda stage: None)
        == payload
    )
    assert sizes == [len(raw)]


@pytest.mark.parametrize(
    "case", ["odd", "partial", "surrogate", "nul", "maximum", "changed"]
)
def test_windows_command_read_rejects_invalid_or_changed_descriptors(monkeypatch, case):
    raw = {
        "odd": b"A\x00B",
        "partial": b"A\x00",
        "surrogate": b"\x00\xd8",
        "nul": b"\x00\x00",
    }.get(case, b"A\x00")
    length = 4 if case == "partial" else len(raw)
    api = SimpleNamespace(
        peb_process_parameters_offset=0x20,
        pointer_size=8,
        command_line_offset=0x70,
        command_line_buffer_offset=0x78,
    )
    monkeypatch.setattr(
        windows_process_snapshot,
        "_snapshot_basic_info",
        lambda *args: SimpleNamespace(PebBaseAddress=0x1000),
    )
    fields = {
        0x1020: 0x2000,
        0x2070: length,
        0x2072: 0 if case == "maximum" else length,
        0x2078: 0x3000,
    }
    counts = {}

    def integer(api, handle, address, size, deadline):
        counts[address] = counts.get(address, 0) + 1
        return (
            0x4000
            if case == "changed" and address == 0x2078 and counts[address] > 1
            else fields[address]
        )

    monkeypatch.setattr(windows_process_snapshot, "_snapshot_read_integer", integer)
    monkeypatch.setattr(
        windows_process_snapshot, "_snapshot_read_memory", lambda *args: raw
    )
    assert (
        windows_process_snapshot._snapshot_command_line(api, None, lambda stage: None)
        is None
    )


@pytest.mark.parametrize("selection", ["windows", "host", "custody"])
def test_all_default_windows_sampling_uses_isolated_authority(monkeypatch, selection):
    def native():
        raise AssertionError("native query escaped helper isolation")

    reads = []

    def bounded():
        reads.append(True)
        return [(7, 1, "python.exe", 3, 123, "full")]

    monkeypatch.setattr(
        windows_process_snapshot, "_windows_process_snapshot_rows", native
    )
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        process_model, "_windows_process_snapshot_rows_hard_timeout", bounded
    )
    if selection == "windows":
        samples = process_model.sample_processes_windows()
    elif selection == "host":
        samples = process_model.sample_processes()
    else:
        samples = process_model.sample_processes_custody(
            process_model.CustodyInterest(
                tree_pids=frozenset({7}), exact_pids=frozenset()
            )
        )

    assert sorted(samples) == [7]
    assert samples[7].ppid == 1
    assert samples[7].started_at_ns == 123
    assert samples[7].command == "python.exe"
    assert reads == [True]


def test_windows_native_av_helper_is_typed_failure(monkeypatch):
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    _fake_custody(
        monkeypatch,
        "subprocess",
        subprocess,
        run=lambda *args, **kwargs: SimpleNamespace(
            returncode=3221225477, stdout="[]", stderr="native query fault"
        ),
    )
    with pytest.raises(
        windows_process_snapshot.ProcessSnapshotError, match="3221225477"
    ):
        windows_process_snapshot._windows_process_snapshot_rows_hard_timeout()


@pytest.mark.parametrize(
    "row",
    [
        [True, 1, "python.exe", 3, 123],
        [7, -1, "python.exe", 3, 123],
        [7, 1, 9, 3, 123],
        [7, 1, "python.exe", True, 123],
        [7, 1, "python.exe", 3, 0],
    ],
)
def test_windows_helper_admission_rejects_invalid_identity_coordinates(row):
    six_field_row = [*row, "full"]
    assert len(six_field_row) == 6
    with pytest.raises(ValueError, match="field types|elapsed_sec|started_at_ns"):
        windows_process_snapshot._coerce_windows_process_snapshot_rows([six_field_row])


def test_windows_helper_admission_rejects_duplicate_pids():
    row = [7, 1, "python.exe", 3, 123, "full"]
    with pytest.raises(ValueError, match="duplicate"):
        windows_process_snapshot._coerce_windows_process_snapshot_rows([row, row])


def test_windows_helper_admission_rejects_obsolete_resident_size_field():
    with pytest.raises(ValueError, match="six fields"):
        windows_process_snapshot._coerce_windows_process_snapshot_rows(
            [[7, 1, 9, "python.exe", 3, 123, "full"]]
        )


def test_unknown_windows_command_preserves_memory_and_birth_without_role_inference(
    monkeypatch,
):
    row = [7, 1, "", 3, 456, "unavailable"]
    monkeypatch.setattr(process_model, "windows_process_resident_kb", lambda *args: 123)
    validated = windows_process_snapshot._coerce_windows_process_snapshot_rows([row])
    sample = process_model.parse_windows_process_snapshot_rows(validated)[7]
    assert sample.command_kind == "unavailable"
    assert sample.rss_kb == 123 and sample.started_at_ns == 456
    assert sample.command == "pid:7"
    assert not process_model.is_host_control_plane_process(sample)
    assert process_model.has_external_host_control_plane_lineage(
        {7: sample}, 7, owned_pids=()
    )


@pytest.mark.parametrize(
    "command,kind",
    [
        ("cargo.exe", "unavailable"),
        ("", "full"),
        ("", "image"),
        ("cargo.exe", "guessed"),
    ],
)
def test_windows_command_authority_must_match_helper_text(command, kind):
    with pytest.raises(ValueError):
        windows_process_snapshot._coerce_windows_process_snapshot_rows(
            [[7, 1, command, 3, 123, kind]]
        )


@pytest.mark.parametrize("kind", ["image", "unavailable"])
def test_cargo_parent_argv_requires_complete_command_authority(kind):
    from tools.memory_guard_core import cargo_quarantine

    parent = process_model.ProcessSample(
        pid=7,
        ppid=1,
        rss_kb=1,
        command="cargo.exe",
        started_at_ns=123,
        argv=("cargo.exe", "build"),
        command_kind=kind,
    )
    child = process_model.ProcessSample(
        pid=8,
        ppid=7,
        rss_kb=1,
        command="rustc.exe",
        started_at_ns=124,
        argv=("rustc.exe", "-C", "incremental=cache"),
    )
    samples = {7: parent, 8: child}
    identities = {
        pid: process_model.process_identity(sample) for pid, sample in samples.items()
    }
    assert cargo_quarantine._owned_sample_argv(parent) is None
    assert (
        cargo_quarantine._owned_cargo_ancestor(child, samples, {7, 8}, identities)
        is None
    )
    assert (
        cargo_quarantine.observe_owned_incremental_state(samples, {7, 8}, identities)
        == set()
    )


@pytest.mark.parametrize("error", [0, 5, 18])
def test_windows_enumeration_requires_documented_complete_marker(monkeypatch, error):
    import ctypes

    class Entry(ctypes.Structure):
        _fields_ = [
            ("dwSize", ctypes.c_uint32),
            ("th32ProcessID", ctypes.c_uint32),
            ("szExeFile", ctypes.c_wchar * 30),
        ]

    last_error = [0]
    closed = []

    def first(handle, entry):
        entry._obj.th32ProcessID = 7
        entry._obj.szExeFile = "python.exe"
        return True

    def next_row(handle, entry):
        last_error[0] = error
        return False

    shim = SimpleNamespace(
        sizeof=ctypes.sizeof,
        byref=ctypes.byref,
        set_last_error=lambda value: last_error.__setitem__(0, value),
        get_last_error=lambda: last_error[0],
    )
    api = SimpleNamespace(
        ctypes=shim,
        wintypes=SimpleNamespace(),
        create_snapshot=lambda *args: 99,
        process_first=first,
        process_next=next_row,
        close_handle=closed.append,
        open_process=lambda *args: None,
        get_process_times=lambda *args: False,
        invalid_handle_value=-1,
        ProcessEntry32W=Entry,
    )
    install_module_os_view(monkeypatch, windows_process_snapshot, name="nt")
    monkeypatch.setattr(windows_process_snapshot, "_windows_snapshot_api", lambda: api)
    if error == 18:
        rows = windows_process_snapshot._windows_process_snapshot_rows()
        assert rows == [(7, 0, "", None, None, "unavailable")]
    else:
        with pytest.raises(
            windows_process_snapshot.ProcessSnapshotError, match="before completion"
        ):
            windows_process_snapshot._windows_process_snapshot_rows()
    assert closed == [99]


def test_windows_sampler_family_is_in_mandatory_portability_proofs():
    import tomllib

    plan = tomllib.loads(
        (REPO_ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    commands = {command["id"]: command for command in plan["command"]}
    for identifier in (
        "python.unit.harness",
        "portability.cargo-custody.linux",
        "portability.cargo-custody.macos",
        "portability.cargo-custody.windows",
    ):
        assert (
            "tests/tools/test_memory_guard_windows_sampling.py"
            in commands[identifier]["argv"]
        )
    assert {
        "src/molt/windows_process_snapshot.py",
        "src/molt/process_model.py",
        "tools/memory_guard_core/process_custody.py",
        "tools/memory_guard_core/cargo_quarantine.py",
        "tests/tools/test_memory_guard_windows_sampling.py",
        "tests/test_cargo_quarantine_custody.py",
    } <= set(plan["authority_inputs"])


def test_missing_command_authority_never_grants_argument_credit():
    from tools.memory_guard_core import cargo_quarantine

    sample = SimpleNamespace(command="cargo.exe", argv=("cargo.exe",))
    assert cargo_quarantine._owned_sample_argv(sample) is None


@pytest.mark.parametrize("command", ["python.exe", "C:/Python/python.exe", "cargo.exe"])
def test_critical_executable_helper_cannot_downgrade_to_image_authority(command):
    with pytest.raises(ValueError, match="critical"):
        windows_process_snapshot._coerce_windows_process_snapshot_rows(
            [[7, 1, command, 3, 123, "image"]]
        )
