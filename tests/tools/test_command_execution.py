from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools import command_execution
from tests.process_guard_common import install_module_view


ROOT = Path(__file__).resolve().parents[2]


def test_executor_routes_only_bounded_metadata_to_direct_probe(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_run(_command: list[str], **kwargs: object):
        calls.append(kwargs)
        return subprocess.CompletedProcess([], 0, "", "")

    authority = SimpleNamespace(run_completed_command=fake_run)
    monkeypatch.setattr(
        command_execution,
        "_process_guard_authority",
        lambda _root: authority,
    )
    executor = command_execution.CommandExecutor.for_file(__file__)

    executor.run(
        ["git", "status", "--porcelain"],
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    executor.run(
        ["python", "tool.py"], capture_output=True, text=True, encoding="utf-8"
    )

    assert calls[0]["memory_guard_prefix"] is None
    assert calls[1]["memory_guard_prefix"] == executor.prefix


def test_executor_rejects_loaded_molt_from_another_repo(monkeypatch) -> None:
    foreign = SimpleNamespace(__path__=["C:/foreign/molt"])
    monkeypatch.setitem(sys.modules, "molt", foreign)

    with pytest.raises(RuntimeError, match="repository import custody mismatch"):
        command_execution.CommandExecutor.for_file(__file__)


def test_owned_wait_escalates_only_its_exact_process() -> None:
    calls: list[tuple[str, float | None]] = []

    class Process:
        def wait(self, timeout=None):
            calls.append(("wait", timeout))
            if len([call for call in calls if call[0] == "wait"]) < 3:
                raise subprocess.TimeoutExpired(["owned"], timeout)
            return 9

        def terminate(self):
            calls.append(("terminate", None))

        def kill(self):
            calls.append(("kill", None))

    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(subprocess.TimeoutExpired):
        executor.wait_owned(Process(), timeout=1.0, terminate_timeout=0.5)  # type: ignore[arg-type]

    assert calls == [
        ("wait", 1.0),
        ("terminate", None),
        ("wait", 0.5),
        ("kill", None),
        ("wait", 0.5),
    ]


def test_executor_rejects_shell_text() -> None:
    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(TypeError, match="typed argv"):
        executor.run("git status")  # type: ignore[arg-type]


def test_executor_rejects_capture_output_with_explicit_stream() -> None:
    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(ValueError, match="capture_output cannot be combined"):
        executor.run(
            ["git", "status"],
            capture_output=True,
            stdout=subprocess.PIPE,
        )


def test_read_only_git_classifier_excludes_mutations() -> None:
    assert command_execution._is_bounded_metadata_probe(["git", "rev-parse", "HEAD"])
    assert command_execution._is_bounded_metadata_probe(
        ["git", "-C", "repo", "status", "--porcelain"]
    )
    assert not command_execution._is_bounded_metadata_probe(
        ["git", "commit", "-m", "message"]
    )


def test_read_only_git_classifier_sees_through_inert_global_options() -> None:
    # The proof queue snapshots source custody with these exact spellings.
    assert command_execution._is_bounded_metadata_probe(
        ["git", "--no-optional-locks", "status", "--porcelain=v1", "-z"]
    )
    assert command_execution._is_bounded_metadata_probe(
        ["git", "ls-files", "--cached", "--full-name", "-z"]
    )
    assert command_execution._is_bounded_metadata_probe(
        ["git", "-C", "repo", "--no-pager", "log", "-1"]
    )
    # A config override can arm a hook or a pager, so it keeps the guard.
    assert not command_execution._is_bounded_metadata_probe(
        ["git", "-c", "core.fsmonitor=./hook", "status"]
    )
    assert not command_execution._is_bounded_metadata_probe(
        ["git", "--no-optional-locks"]
    )


def test_owned_cargo_process_normalizes_wrapper_incremental_conflict(
    monkeypatch,
) -> None:
    captured: dict[str, object] = {}

    class FakePopen:
        def __init__(self, command: list[str], **kwargs: object) -> None:
            captured["command"] = command
            captured["kwargs"] = kwargs

    install_module_view(
        monkeypatch, "subprocess", subprocess, command_execution, Popen=FakePopen
    )
    executor = command_execution.CommandExecutor.for_file(__file__)

    executor.start_owned(
        ["cargo", "metadata", "--no-deps"],
        env={"RUSTC_WORKSPACE_WRAPPER": "sccache", "CARGO_INCREMENTAL": "1"},
    )

    kwargs = captured["kwargs"]
    assert isinstance(kwargs, dict)
    assert kwargs["env"]["CARGO_INCREMENTAL"] == "0"


def test_executor_loads_process_guard_without_repo_package_importable(
    tmp_path: Path,
) -> None:
    script = tmp_path / "standalone_executor_probe.py"
    script.write_text(
        "import sys\n"
        f"tools = {str(ROOT / 'tools')!r}\n"
        f"repo_root = {str(ROOT)!r}\n"
        "assert repo_root not in sys.path\n"
        "try:\n"
        "    import molt\n"
        "except ModuleNotFoundError:\n"
        "    pass\n"
        "else:\n"
        "    raise AssertionError('molt unexpectedly importable')\n"
        "sys.path.insert(0, tools)\n"
        "import bootstrap_actionlint\n"
        "result = bootstrap_actionlint._COMMANDS.run([sys.executable, '--version'], "
        "capture_output=True, text=True, check=True)\n"
        "assert result.stdout.startswith('Python ')\n"
        "from command_execution import _process_guard_authority\n"
        "authority = _process_guard_authority(str(bootstrap_actionlint._COMMANDS.repo_root))\n"
        "normalized, applied = authority.cargo_subprocess_environment("
        "['cargo', '--version'], "
        "{'RUSTC_WORKSPACE_WRAPPER': 'sccache', 'CARGO_INCREMENTAL': '1'})\n"
        "assert normalized['CARGO_INCREMENTAL'] == '0'\n"
        "assert applied == ('sccache-disables-incremental',)\n"
        "assert authority.cargo_subprocess_environment.__module__.startswith("
        "authority.__package__)\n",
        encoding="utf-8",
    )
    env = {
        name: value
        for name, value in os.environ.items()
        if name not in {"PYTHONPATH", "PYTHONHOME"}
    }

    completed = subprocess.run(
        [sys.executable, "-S", str(script)],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
        encoding="utf-8",
    )

    assert completed.returncode == 0, completed.stderr
    assert "ModuleNotFoundError" not in completed.stderr


def test_process_guard_direct_loader_uses_owning_policy_and_source_authority() -> None:
    authority = command_execution._process_guard_authority(str(ROOT))
    policy = sys.modules[authority.cargo_subprocess_environment.__module__]
    source = sys.modules[authority.compiler_source_root.__module__]
    assert (
        Path(policy.__file__).resolve()
        == (ROOT / "src" / "molt" / "cargo_execution_policy.py").resolve()
    )
    assert (
        Path(source.__file__).resolve()
        == (ROOT / "src" / "molt" / "source_root.py").resolve()
    )
    assert authority.compiler_source_root() == ROOT.resolve()


def test_process_guard_authority_is_isolated_per_worktree(tmp_path: Path) -> None:
    roots = [tmp_path / "one", tmp_path / "two"]
    for index, root in enumerate(roots):
        package = root / "src" / "molt"
        package.mkdir(parents=True)
        (package / "cargo_execution_policy.py").write_text(
            f"IDENTITY = {index}\n",
            encoding="utf-8",
        )
        (package / "process_guard.py").write_text(
            "from .cargo_execution_policy import IDENTITY\n",
            encoding="utf-8",
        )
    loaded_packages: list[str] = []
    command_execution._process_guard_authority.cache_clear()
    try:
        first = command_execution._process_guard_authority(str(roots[0]))
        second = command_execution._process_guard_authority(str(roots[1]))
        loaded_packages.extend((first.__package__, second.__package__))
        assert first.IDENTITY == 0
        assert second.IDENTITY == 1
        assert first.__package__ != second.__package__
    finally:
        command_execution._process_guard_authority.cache_clear()
        for package_name in loaded_packages:
            for module_name in tuple(sys.modules):
                if module_name == package_name or module_name.startswith(
                    f"{package_name}."
                ):
                    sys.modules.pop(module_name, None)


def _interactive_guard_fixture(tmp_path, process):
    import json

    launch_id = "a" * 32
    startup = {
        "launch_id": launch_id,
        "guard_pid": 144,
        "command": ["child"],
        "child_process": {
            "pid": 145,
            "pgid": None,
            "sid": None,
            "command": ["child"],
            "started_at": "fixture-start",
        },
    }
    startup_path = tmp_path / "startup.json"
    startup_path.write_text(json.dumps(startup), encoding="utf-8")
    terminal = {**startup, "descendants_closed": True, "child_returncode": 0}
    summary = tmp_path / "summary.json"
    summary.write_text(json.dumps(terminal), encoding="utf-8")
    owned = command_execution.GuardedCommand(
        process,
        tmp_path / "cancel",
        summary,
        tmp_path / "custody.json",
        launch_id,
        startup_path,
        ("child",),
    )
    return owned, startup, terminal


@pytest.mark.parametrize("finishes", [True, False])
def test_guarded_timeout_requests_owner_without_killing_guard(tmp_path, finishes):
    calls = []

    class Process:
        # Portable delegation fixture: launch PID differs from actual worker.
        pid = 42
        returncode = None

        def wait(self, timeout=None):
            calls.append(timeout)
            if len(calls) == 1 or not finishes:
                raise subprocess.TimeoutExpired(["guard"], timeout)
            self.returncode = 137
            return self.returncode

        def terminate(self):
            pytest.fail("must never terminate a guard with live child custody")

        def kill(self):
            pytest.fail("must never kill a guard with live child custody")

    owned, _startup, _terminal = _interactive_guard_fixture(tmp_path, Process())
    executor = command_execution.CommandExecutor(prefix="TEST", repo_root=ROOT)
    with pytest.raises(subprocess.TimeoutExpired) as caught:
        executor.wait_owned(owned, timeout=0.1, terminate_timeout=0.2)
    assert owned.cancellation_path.is_file()
    assert owned.terminal is finishes
    assert calls == [0.1, 0.2]
    if finishes:
        assert owned.pid == 42
        assert owned.guard_pid == 144
        assert owned.child_identity["pid"] == 145
    else:
        assert caught.value.guard_command is owned
        assert isinstance(caught.value.cleanup_error, subprocess.TimeoutExpired)


@pytest.mark.parametrize(
    "change",
    [
        "missing-startup",
        "missing-startup-token",
        "startup-token",
        "missing-terminal-token",
        "terminal-token",
        "terminal-worker",
        "terminal-child",
        "terminal-command",
        "unclosed-tree",
        "changed-worker",
    ],
)
def test_guard_summary_rejects_unbound_or_changed_launch_custody(tmp_path, change):
    import json

    process = SimpleNamespace(pid=42, wait=lambda **_kwargs: 0)
    owned, startup, terminal = _interactive_guard_fixture(tmp_path, process)
    if change == "missing-startup-token":
        startup.pop("launch_id")
    elif change == "startup-token":
        startup["launch_id"] = "b" * 32
    elif change == "missing-terminal-token":
        terminal.pop("launch_id")
    elif change == "terminal-token":
        terminal["launch_id"] = "b" * 32
    elif change == "terminal-worker":
        terminal["guard_pid"] = 146
    elif change == "terminal-child":
        terminal["child_process"] = {**startup["child_process"], "pid": 146}
    elif change == "terminal-command":
        terminal["command"] = ["other-child"]
    elif change == "unclosed-tree":
        terminal["descendants_closed"] = False
    elif change == "changed-worker":
        assert owned.wait(timeout=0.1) == 0
        startup["guard_pid"] = 146
        terminal["guard_pid"] = 146
    owned.startup_path.write_text(json.dumps(startup), encoding="utf-8")
    owned.summary_path.write_text(json.dumps(terminal), encoding="utf-8")
    if change == "missing-startup":
        owned.startup_path.unlink()
    with pytest.raises(RuntimeError, match="child custody"):
        owned.wait(timeout=0.1)
    assert not owned.terminal
    assert owned.process is process


def test_interactive_launch_capability_is_stripped_before_child_spawn(tmp_path):
    from tools import memory_guard

    environment = memory_guard._worker_env(
        {"OTHER": "retained"},
        ["child"],
        launch_id="a" * 32,
        startup_json=str(tmp_path / "startup.json"),
    )
    child = memory_guard._child_env_without_internal_keys(environment)
    assert child == {"OTHER": "retained"}


def test_interactive_guard_cancels_actual_child_tree_and_closes_streams(tmp_path):
    import json
    import tempfile
    import time

    from tools import memory_guard

    pidfile = tmp_path / "children.json"
    child_script = tmp_path / "interactive_child.py"
    child_script.write_text(
        "import json,os,pathlib,subprocess,sys,time\n"
        "assert sys.stdin.readline() == 'start\\n'\n"
        "child = subprocess.Popen([sys.executable, '-c', "
        "'import time; time.sleep(60)'])\n"
        "pids = {'child': os.getpid(), 'grandchild': child.pid}\n"
        "pathlib.Path(sys.argv[1]).write_text(json.dumps(pids), encoding='utf-8')\n"
        "print('interactive child ready', flush=True)\n"
        "time.sleep(60)\n",
        encoding="utf-8",
    )
    env = dict(os.environ)
    env["MOLT_MEMORY_GUARD_STATE_ROOT"] = str(tmp_path / "guard-state")
    executor = command_execution.CommandExecutor(
        prefix="MOLT_TEST_INTERACTIVE_GUARD", repo_root=ROOT
    )
    with tempfile.TemporaryFile(mode="w+b") as stderr:
        owned = executor.start_guarded(
            [sys.executable, "-B", str(child_script), str(pidfile)],
            cwd=ROOT,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
            text=True,
            encoding="utf-8",
            timeout=15.0,
        )
        try:
            assert owned.stdin is not None
            owned.stdin.write("start\n")
            owned.stdin.flush()
            deadline = time.monotonic() + 10.0
            while not pidfile.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert pidfile.exists(), str(owned.evidence_path)
            pids = json.loads(pidfile.read_text(encoding="utf-8"))
            before = memory_guard.sample_processes()
            assert pids["child"] in before
            assert pids["grandchild"] in before
            with pytest.raises(subprocess.TimeoutExpired):
                executor.wait_owned(owned, timeout=0.05, terminate_timeout=10.0)
            assert owned.terminal, str(owned.evidence_path)
            assert owned.poll() is not None
            summary = json.loads(owned.summary_path.read_text(encoding="utf-8"))
            startup = json.loads(owned.startup_path.read_text(encoding="utf-8"))
            assert summary["launch_id"] == startup["launch_id"] == owned.launch_id
            assert summary["guard_pid"] == startup["guard_pid"] == owned.guard_pid
            assert (
                summary["child_process"]
                == startup["child_process"]
                == owned.child_identity
            )
            assert summary["descendants_closed"] is True
            assert summary["cancelled"] is True
            after = memory_guard.sample_processes()
            assert pids["child"] not in after
            assert pids["grandchild"] not in after
            assert owned.child_identity["pid"] not in after
            # Exact terminal tree custody proves no descendant can retain the
            # pipe; reading EOF here also exercises inherited stream ownership.
            assert owned.stdout.read() == "interactive child ready\n"
        finally:
            if not owned.terminal:
                owned.request_cancel()
                executor.wait_owned(owned, timeout=10.0)
            if owned.terminal:
                owned.stdin.close()
                owned.stdout.close()


@pytest.mark.parametrize("pause", ["startup", "closure"])
def test_harness_owner_survives_observation_expiry_and_admits_eventual_closure(
    tmp_path, monkeypatch, pause
):
    """A real worker is held independently of its cancellation implementation."""
    import json
    import time

    from tools import memory_guard

    entered = tmp_path / "entered"
    release = tmp_path / "release"
    child_ready = tmp_path / "child-ready"
    wrapper = tmp_path / "held_harness.py"
    wrapper.write_text(
        "import pathlib,sys,time\n"
        f"sys.path.insert(0, {str(ROOT)!r})\n"
        "from tools import guarded_exec\n"
        f"entered=pathlib.Path({str(entered)!r})\n"
        f"release=pathlib.Path({str(release)!r})\n"
        "def hold():\n"
        " entered.write_text('entered', encoding='utf-8')\n"
        " deadline=time.monotonic()+20\n"
        " while not release.exists() and time.monotonic()<deadline: time.sleep(.02)\n"
        " if not release.exists(): raise RuntimeError('test release deadline')\n"
        + (
            "hold()\n"
            if pause == "startup"
            else "guard=guarded_exec.harness_memory_guard.memory_guard\n"
            "original=guard._temporary_artifact_descendant_closure\n"
            "def held(**kwargs):\n"
            " hold()\n"
            " return original(**kwargs)\n"
            "guard._temporary_artifact_descendant_closure=held\n"
        )
        + "raise SystemExit(guarded_exec.main())\n",
        encoding="utf-8",
    )
    start = command_execution.CommandExecutor.start_owned

    def held_start(self, args, **kwargs):
        assert Path(args[1]).name == "guarded_exec.py"
        return start(self, [args[0], str(wrapper), *args[2:]], **kwargs)

    monkeypatch.setattr(command_execution.CommandExecutor, "start_owned", held_start)
    executor = command_execution.CommandExecutor(prefix="MOLT_TEST", repo_root=ROOT)
    owned = executor.start_guarded(
        [
            sys.executable,
            "-c",
            "import os,pathlib,time; "
            f"pathlib.Path({str(child_ready)!r}).write_text(str(os.getpid()), encoding='utf-8'); "
            "time.sleep(30)",
        ],
        cwd=ROOT,
        env={**os.environ, "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "state")},
        timeout=15,
        harness=True,
    )
    try:
        ready = entered if pause == "startup" else child_ready
        deadline = time.monotonic() + 10
        while not ready.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        assert ready.exists(), str(owned.evidence_path)
        if pause == "startup":
            assert not owned.startup_path.exists()
        before = time.monotonic()
        with pytest.raises(subprocess.TimeoutExpired) as caught:
            owned.cancel_and_wait()
        assert time.monotonic() - before >= 5
        assert caught.value.guard_command is owned
        assert owned.poll() is None
        assert not owned.terminal
        assert owned.cancellation_path.is_file()
        assert owned.evidence_path.is_file()
        if pause == "closure":
            assert entered.is_file()  # Actual guard cleanup reached our barrier.
        release.write_text("release", encoding="utf-8")
        assert owned.wait(timeout=10) == 137
        summary = json.loads(owned.summary_path.read_text(encoding="utf-8"))
        startup = json.loads(owned.startup_path.read_text(encoding="utf-8"))
        assert owned.terminal
        assert summary["cancelled"] is True
        assert summary["descendants_closed"] is True
        assert summary["launch_id"] == startup["launch_id"] == owned.launch_id
        assert (
            summary["child_process"] == startup["child_process"] == owned.child_identity
        )
        assert owned.child_identity["pid"] not in memory_guard.sample_processes()
    finally:
        release.write_text("release", encoding="utf-8")
        if not owned.terminal:
            owned.cancel_and_wait(timeout=10)


def test_cancel_publication_refusal_keeps_original_error_and_live_owner(tmp_path):
    class Process:
        pid = 42
        returncode = None

        def poll(self):
            return None

        def wait(self, **_kwargs):
            pytest.fail("failed request must not be reported as observed closure")

        def terminate(self):
            pytest.fail("request failure cannot transfer child custody to caller")

        kill = terminate

    owned, _startup, _terminal = _interactive_guard_fixture(tmp_path, Process())
    owned.cancellation_path = tmp_path / "missing" / "cancel"
    with pytest.raises(FileNotFoundError) as caught:
        owned.cancel_and_wait()
    assert caught.value.guard_command is owned
    assert owned.poll() is None
    assert not owned.terminal


def test_harness_launch_binds_relative_executable_to_worker_root(tmp_path, monkeypatch):
    from tools import memory_guard

    root = tmp_path / "repository"
    executable = root / "bin" / "guest"
    executable.parent.mkdir(parents=True)
    executable.write_bytes(b"fixture")
    unrelated = tmp_path / "caller"
    unrelated.mkdir()
    monkeypatch.chdir(unrelated)
    captured = {}

    def start(self, args, **kwargs):
        captured.update(argv=args, **kwargs)
        return SimpleNamespace(pid=42)

    monkeypatch.setattr(command_execution.CommandExecutor, "start_owned", start)
    executor = command_execution.CommandExecutor(prefix="MOLT_TEST", repo_root=root)
    owned = executor.start_guarded(
        ["bin/guest", "payload"],
        cwd=root / "other",
        harness=True,
        env={"MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "state")},
    )
    assert captured["cwd"] == root
    assert owned.command == (str(executable.resolve()), "payload")
    assert memory_guard._load_internal_command(captured["env"]) == list(owned.command)
    argv = captured["argv"]
    assert argv[argv.index("--cwd") + 1] == str(root / "other")
