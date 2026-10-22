from __future__ import annotations

import subprocess
import sys
from types import SimpleNamespace

import pytest

from molt import process_guard
from molt.process_environment import NameRule, ProcessEnvironment
from tests.process_guard_common import (
    HUNG_CHILD_SOURCE,
    close_owned_test_process,
    hung_child_marker,
    install_module_view,
    processes_carrying,
    start_owned_test_process,
    stop_test_processes_carrying,
)


@pytest.mark.parametrize("rule", list(NameRule))
def test_guard_environment_inheritance_respects_caller_rule(monkeypatch, rule):
    # A literal parent block exercises both policies on every host. Windows'
    # os.environ would fold the lowercase spelling before the guard saw it.
    install_module_view(
        monkeypatch,
        "os",
        process_guard.os,
        process_guard,
        environ={
            "MOLT_TEST_MAX_RSS_GB": "9",
            "molt_test_max_total_rss_gb": "11",
        },
    )
    original = ProcessEnvironment(
        {"molt_test_max_rss_gb": "7", "Path": "first", "PATH": "second"},
        rule=rule,
    )
    before = original.child_block()
    result = process_guard.with_memory_guard_env(original, "MOLT_TEST")
    assert isinstance(result, ProcessEnvironment)
    assert result.rule is rule
    assert result["molt_test_max_rss_gb"] == "7"
    assert result["MOLT_TEST_MAX_RSS_GB"] == ("7" if rule is NameRule.WINDOWS else "9")
    if rule is NameRule.WINDOWS:
        assert result["MOLT_TEST_MAX_TOTAL_RSS_GB"] == "11"
        assert result["Path"] == "second"
    else:
        assert "MOLT_TEST_MAX_TOTAL_RSS_GB" not in result
        assert result["Path"] == "first"
    assert original.child_block() == before


def test_spawn_type_boundary_rejects_unowned_mapping(tmp_path):
    from pathlib import Path
    import os
    import sys

    from tests.process_guard_common import run_guarded_test_process

    root = Path(__file__).resolve().parents[1]
    checker = Path(sys.executable).with_name("ty.exe" if os.name == "nt" else "ty")
    fixture = tmp_path / "spawn_environment_contract.py"
    fixture.write_text(
        "from molt.process_environment import ProcessEnvironment\n"
        "from molt.process_guard import run_completed_command, with_memory_guard_env\n"
        "run_completed_command(['tool'], env=ProcessEnvironment({}))\n"
        "with_memory_guard_env(ProcessEnvironment({}), 'MOLT_TEST')\n"
        "run_completed_command(['tool'], env={})\n"
        "with_memory_guard_env({}, 'MOLT_TEST')\n",
        encoding="utf-8",
    )
    result = run_guarded_test_process(
        [
            str(checker),
            "check",
            "--project",
            str(root),
            "--extra-search-path",
            str(root / "src"),
            "--no-force-exclude",
            "--output-format",
            "concise",
            str(fixture),
        ],
        cwd=root,
        timeout=30,
    )
    diagnostics = result.stdout + result.stderr
    assert result.returncode == 1, diagnostics
    assert diagnostics.count("invalid-argument-type") == 2, diagnostics
    assert ":5:" in diagnostics and ":6:" in diagnostics, diagnostics


def test_guard_loader_rejects_preloaded_foreign_module(monkeypatch, tmp_path):
    import tools

    foreign = SimpleNamespace(
        __file__=str(tmp_path / "tools" / "harness_memory_guard.py")
    )
    monkeypatch.setattr(tools, "harness_memory_guard", foreign)
    with pytest.raises(RuntimeError, match="guard source authority mismatch"):
        process_guard.load_harness_memory_guard(tmp_path)
    assert tools.harness_memory_guard is foreign


def test_guard_loader_uses_owning_checkout_not_command_directory(tmp_path):
    guard = process_guard.load_harness_memory_guard(tmp_path)
    assert guard._REPO_ROOT == process_guard._molt_repo_root()


@pytest.mark.parametrize("dependency", ["memory_guard", "process_sentinel"])
def test_guard_loader_rejects_foreign_dependency(monkeypatch, tmp_path, dependency):
    guard = process_guard.load_harness_memory_guard(tmp_path)
    foreign = SimpleNamespace(__file__=str(tmp_path / f"{dependency}.py"))
    monkeypatch.setattr(guard, dependency, foreign)
    with pytest.raises(RuntimeError, match="guard source authority mismatch"):
        process_guard.load_harness_memory_guard(tmp_path)
    assert getattr(guard, dependency) is foreign


@pytest.mark.parametrize("explicit_owner", [False, True])
def test_guard_owner_never_adopts_a_foreign_loaded_guard(
    monkeypatch, tmp_path, explicit_owner
):
    import tools

    owner = process_guard._molt_repo_root()
    foreign = tmp_path / "compiler-inputs"
    (foreign / "tools").mkdir(parents=True)
    (foreign / "pyproject.toml").write_text(
        "[project]\nname='input'\n", encoding="utf-8"
    )
    (foreign / "tools" / "harness_memory_guard.py").write_text("", encoding="utf-8")
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(foreign))
    loaded = SimpleNamespace(__file__=str(foreign / "tools/harness_memory_guard.py"))
    monkeypatch.setattr(tools, "harness_memory_guard", loaded)
    with pytest.raises(RuntimeError, match="guard source authority mismatch"):
        process_guard.load_harness_memory_guard(
            None, guard_source_root=owner if explicit_owner else None
        )
    assert tools.harness_memory_guard is loaded


def test_guard_loader_reports_installed_package_without_source_tools(
    monkeypatch, tmp_path
):
    # Explicit compiler-source selection owns admission; a guest's cwd and
    # the loader's code path cannot silently select a different source tree.
    selected = tmp_path / "installed-source"
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(selected))
    with pytest.raises(RuntimeError, match="guard tools are unavailable"):
        process_guard.load_harness_memory_guard(tmp_path)
    assert process_guard.compiler_source_root() == selected


def test_installed_unguarded_probe_needs_no_source_guard_tools(monkeypatch, tmp_path):
    import sys

    installed = tmp_path / "installed-package"
    installed.mkdir()
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(installed))
    result = process_guard.run_completed_command(
        [sys.executable, "-I", "-S", "-c", "print('installed probe')"],
        memory_guard_prefix=None,
        cwd=tmp_path,
        capture_output=True,
    )
    assert result.returncode == 0
    assert result.stdout == "installed probe\n"
    with pytest.raises(RuntimeError, match="guard tools are unavailable"):
        process_guard.load_harness_memory_guard(None)


@pytest.mark.parametrize("nested", [True, False])
def test_guard_source_authority_is_independent_of_command_cwd(tmp_path, nested):
    source_root = process_guard._molt_repo_root()
    command_cwd = source_root / "tools" / "proof_supervisor" if nested else tmp_path
    captured = {}

    class Context:
        @classmethod
        def from_env(cls, *_args, **kwargs):
            captured["source_root"] = kwargs["repo_root"]
            return cls()

        def run(self, command, **kwargs):
            captured["cwd"] = kwargs["cwd"]
            return subprocess.CompletedProcess(command, 0, "", "")

    process_guard.run_completed_command(
        ["compiler", "input"],
        cwd=command_cwd,
        guard_loader=lambda _cwd, *, guard_source_root=None: SimpleNamespace(
            HarnessExecutionContext=Context
        ),
    )
    assert captured == {"source_root": source_root, "cwd": command_cwd}


def test_typed_argv_boundary_rejects_shell_text() -> None:
    with pytest.raises(TypeError, match="typed argv"):
        process_guard.run_completed_command("git status")  # type: ignore[arg-type]


def test_bounded_unguarded_probe_preserves_subprocess_contract(monkeypatch) -> None:
    calls: list[tuple[list[str], dict[str, object]]] = []
    waits: list[tuple[object, float | None]] = []

    def fake_popen(command: list[str], **kwargs: object):
        calls.append((command, kwargs))

        def communicate(input_text, *, timeout):
            waits.append((input_text, timeout))
            return "head\n", ""

        return SimpleNamespace(
            communicate=communicate,
            poll=lambda: 0,
            __exit__=lambda *_args: None,
        )

    install_module_view(
        monkeypatch, "subprocess", subprocess, process_guard, Popen=fake_popen
    )

    result = process_guard.run_completed_command(
        ["git", "rev-parse", "HEAD"],
        memory_guard_prefix=None,
        capture_output=True,
        text=True,
        check=True,
    )

    assert result.stdout == "head\n"
    assert calls[0][0] == ["git", "rev-parse", "HEAD"]
    assert "shell" not in calls[0][1]
    assert calls[0][1]["stdout"] == subprocess.PIPE
    assert calls[0][1]["stderr"] == subprocess.PIPE
    # No default deadline: a local read waits out a host stall. The probe
    # reads no inherited stdin, so nothing but the probe itself can hold it.
    assert waits[0] == (None, None)
    assert calls[0][1]["stdin"] is subprocess.DEVNULL

    process_guard.run_completed_command(
        ["git", "patch-id", "--stable"],
        memory_guard_prefix=None,
        input="diff\n",
        timeout=7.5,
        capture_output=True,
    )
    # A caller's input and deadline pass through unchanged.
    assert calls[1][1]["stdin"] is subprocess.PIPE
    assert waits[1] == ("diff\n", 7.5)


@pytest.mark.parametrize(
    "wrapper_env",
    [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ],
)
def test_cargo_metadata_probe_normalizes_wrapper_incremental_conflict(
    monkeypatch,
    wrapper_env: str,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_popen(_command: list[str], **kwargs: object):
        calls.append(kwargs)
        return SimpleNamespace(
            communicate=lambda *_args, **_kwargs: ("{}", ""),
            poll=lambda: 0,
            __exit__=lambda *_args: None,
        )

    install_module_view(
        monkeypatch, "subprocess", subprocess, process_guard, Popen=fake_popen
    )
    process_guard.run_completed_command(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        env=ProcessEnvironment({wrapper_env: "sccache", "CARGO_INCREMENTAL": "1"}),
        memory_guard_prefix=None,
        capture_output=True,
    )

    assert calls[0]["env"][wrapper_env] == "sccache"  # type: ignore[index]
    assert calls[0]["env"]["CARGO_INCREMENTAL"] == "0"  # type: ignore[index]


def test_guarded_boundary_rejects_false_stderr_interleaving_contract() -> None:
    with pytest.raises(ValueError, match="preserve stdout/stderr separately"):
        process_guard.run_completed_command(
            ["python", "tool.py"],
            memory_guard_prefix="MOLT_TEST",
            stdout=subprocess.PIPE,
            text=True,
            stderr=subprocess.STDOUT,
        )


def test_capture_output_rejects_explicit_streams_before_dispatch() -> None:
    with pytest.raises(ValueError, match="capture_output cannot be combined"):
        process_guard.run_completed_command(
            ["git", "status"],
            memory_guard_prefix=None,
            capture_output=True,
            stdout=subprocess.PIPE,
        )


def test_guarded_timeout_without_requested_timeout_fails_closed() -> None:
    class FakeContext:
        @classmethod
        def from_env(cls, *_args: object, **_kwargs: object) -> "FakeContext":
            return cls()

        def run(self, command: list[str], **_kwargs: object) -> object:
            return type(
                "GuardedResult",
                (),
                {
                    "timed_out": True,
                    "stdout": "partial",
                    "stderr": "guard timed out",
                    "returncode": 124,
                },
            )()

    harness = type(
        "FakeHarness",
        (),
        {"HarnessExecutionContext": FakeContext},
    )

    with pytest.raises(RuntimeError, match="timeout custody is inconsistent"):
        process_guard.run_completed_command(
            ["compiler", "input.py"],
            memory_guard_prefix="MOLT_TEST",
            timeout=None,
            guard_loader=lambda _cwd, *, guard_source_root=None: harness,
        )


def test_guarded_timeout_preserves_terminal_telemetry() -> None:
    guarded_result = type(
        "GuardedResult",
        (),
        {
            "timed_out": True,
            "stdout": "partial",
            "stderr": "guard timed out",
            "returncode": 124,
            "peak": object(),
            "peak_total": object(),
        },
    )()

    class FakeContext:
        @classmethod
        def from_env(cls, *_args: object, **_kwargs: object) -> "FakeContext":
            return cls()

        def run(self, command: list[str], **_kwargs: object) -> object:
            return guarded_result

    harness = type(
        "FakeHarness",
        (),
        {"HarnessExecutionContext": FakeContext},
    )

    with pytest.raises(subprocess.TimeoutExpired) as raised:
        process_guard.run_completed_command(
            ["compiler", "input.py"],
            memory_guard_prefix="MOLT_TEST",
            timeout=5.0,
            capture_output=True,
            guard_loader=lambda _cwd, *, guard_source_root=None: harness,
        )

    assert getattr(raised.value, "guarded_result") is guarded_result


@pytest.mark.parametrize("check", [False, True])
def test_guard_failure_is_a_typed_error_with_terminal_telemetry(check: bool) -> None:
    """The guard's own failure (125) never reaches a caller as the child's status.

    Before HF-29 this returned (or, with check=True, raised CalledProcessError
    for) exit code 125, so a caller blamed the child that had exited with 0.
    """
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

    guarded_result = type(
        "GuardedResult",
        (),
        {
            "timed_out": False,
            "stdout": "child completed",
            "stderr": "scratch retention failed",
            "returncode": 125,
            "child_returncode": 0,
            "infrastructure_failure": GuardInfrastructureFailure(
                phase="temporary_artifact_custody",
                details=("scratch retention failed",),
            ),
        },
    )()

    class FakeContext:
        @classmethod
        def from_env(cls, *_args: object, **_kwargs: object) -> "FakeContext":
            return cls()

        def run(self, command: list[str], **_kwargs: object) -> object:
            return guarded_result

    harness = type(
        "FakeHarness",
        (),
        {"HarnessExecutionContext": FakeContext},
    )

    with pytest.raises(process_guard.GuardInfrastructureError) as raised:
        process_guard.run_completed_command(
            ["compiler", "input.py"],
            memory_guard_prefix="MOLT_TEST",
            capture_output=True,
            check=check,
            guard_loader=lambda _cwd, *, guard_source_root=None: harness,
        )

    assert not isinstance(raised.value, subprocess.CalledProcessError)
    assert not hasattr(raised.value, "returncode")
    assert raised.value.child_returncode == 0
    assert raised.value.phase == "temporary_artifact_custody"
    assert raised.value.guarded_result is guarded_result


def test_captured_command_retains_progress_and_terminal_result():
    calls = []
    terminal = subprocess.CompletedProcess(["compiler"], 7, "output", "error")

    class Context:
        @classmethod
        def from_env(cls, *_args, **_kwargs):
            return cls()

        def run(self, command, **kwargs):
            calls.append(kwargs)
            return terminal

    result = process_guard.run_completed_command(
        ["compiler"],
        capture_output=True,
        progress_label="Compiler build (release)",
        guard_loader=lambda _cwd, *, guard_source_root=None: SimpleNamespace(
            HarnessExecutionContext=Context
        ),
    )
    assert result is terminal
    assert calls[0]["progress_label"] == "Compiler build (release)"
    assert (result.returncode, result.stdout, result.stderr) == (7, "output", "error")


@pytest.mark.parametrize("failure", [None, "spawn", "wait"])
def test_unguarded_probe_retains_live_use_and_original_error_through_gc(
    tmp_path, monkeypatch, failure
):
    import gc
    import os
    from pathlib import Path
    import sys
    import weakref
    from molt import checkout_state, dx, process_spawn

    source = Path(process_guard._molt_repo_root())
    artifact = tmp_path / "artifact"
    target = dx.cargo_target_dir_for_artifact_root(
        artifact, "reader", source_root=source
    )
    target.mkdir(parents=True)
    environment = ProcessEnvironment(
        {**os.environ, "MOLT_EXT_ROOT": str(artifact), "CARGO_TARGET_DIR": str(target)}
    )
    owners, children, methods = [], [], []
    register, popen = process_spawn.atexit.register, subprocess.Popen

    def registered(callback):
        owners.append(weakref.ref(callback.__self__))
        return register(callback)

    def start(command, **kwargs):
        with checkout_state.checkout_state_retirement(target) as admitted:
            assert not admitted
        if failure == "spawn":
            raise FileNotFoundError("independent probe spawn refusal")
        process = popen(command, **kwargs)
        children.append(process)
        methods.append((process.kill, process.wait))
        if failure == "wait":

            def fail_communicate(*args, **kwargs):
                raise RuntimeError("original probe failure")

            def fail_cleanup(*args, **kwargs):
                raise OSError("unobserved probe cleanup")

            process.communicate, process.kill, process.wait = (
                fail_communicate,
                fail_cleanup,
                fail_cleanup,
            )
        return process

    install_module_view(
        monkeypatch, "atexit", process_spawn.atexit, process_spawn, register=registered
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, process_guard, Popen=start
    )
    command = [
        sys.executable,
        "-I",
        "-S",
        "-c",
        HUNG_CHILD_SOURCE
        if failure == "wait"
        else "import sys; value=sys.stdin.read(); print(value,end=''); sys.stderr.buffer.write(b'diagnostic\\n'); raise SystemExit(7)",
    ]
    try:
        if failure is None:
            result = process_guard.run_completed_command(
                command,
                env=environment,
                cwd=tmp_path,
                memory_guard_prefix=None,
                input="payload",
                capture_output=True,
                check=False,
            )
            assert (result.args, result.returncode, result.stdout, result.stderr) == (
                command,
                7,
                "payload",
                "diagnostic\n",
            )
            with pytest.raises(subprocess.CalledProcessError) as caught:
                process_guard.run_completed_command(
                    command,
                    env=environment,
                    memory_guard_prefix=None,
                    input="check",
                    capture_output=True,
                    check=True,
                )
            assert (
                caught.value.returncode,
                caught.value.cmd,
                caught.value.stdout,
                caught.value.stderr,
            ) == (7, command, "check", "diagnostic\n")
            del caught
            binary = process_guard.run_completed_command(
                command,
                env=environment,
                memory_guard_prefix=None,
                input=b"binary",
                capture_output=True,
                text=False,
            )
            assert (binary.stdout, binary.stderr) == (b"binary", b"diagnostic\n")
        else:
            expected = FileNotFoundError if failure == "spawn" else RuntimeError
            with pytest.raises(
                expected,
                match="independent probe spawn refusal"
                if failure == "spawn"
                else "original probe failure",
            ) as caught:
                process_guard.run_completed_command(
                    command,
                    env=environment,
                    cwd=tmp_path,
                    memory_guard_prefix=None,
                    capture_output=True,
                )
            if failure == "wait":
                assert caught.value.state_owner is owners[0]()
                assert caught.value.owned_process is children[0]
            del caught
        gc.collect()
        if failure == "wait":
            assert owners[0]() is not None and children[0].poll() is None
            with checkout_state.checkout_state_retirement(target) as admitted:
                assert not admitted
        else:
            assert all(reference() is None for reference in owners)
            with checkout_state.checkout_state_retirement(target) as admitted:
                assert admitted
    finally:
        for process, (kill, wait) in zip(children, methods):
            process.wait = wait
            if process.poll() is None:
                kill()
            wait(timeout=5)
        for reference in owners:
            if (owner := reference()) is not None:
                owner._close_at_exit()
    with checkout_state.checkout_state_retirement(target) as admitted:
        assert admitted


def test_unguarded_probe_actual_timeout_keeps_partial_output(tmp_path):
    import os
    import sys

    command = [
        sys.executable,
        "-I",
        "-S",
        "-c",
        "print('before-timeout',flush=True)\n" + HUNG_CHILD_SOURCE,
    ]
    with pytest.raises(subprocess.TimeoutExpired) as caught:
        process_guard.run_completed_command(
            command,
            cwd=tmp_path,
            memory_guard_prefix=None,
            capture_output=True,
            timeout=2.0,
            text=True,
        )
    assert caught.value.cmd == command
    # CP's Windows run() performs its post-kill communicate in text mode;
    # POSIX preserves the bytes captured on the original timeout exception.
    expected = "before-timeout\n" if os.name == "nt" else b"before-timeout\n"
    assert caught.value.output.startswith(expected)


def test_unguarded_check_error_preserves_native_popen_args(monkeypatch):
    physical = 'native serialized command "with payload"'
    command = ["logical", "argv"]

    def popen(actual, **kwargs):
        assert actual == physical
        return SimpleNamespace(
            args=physical,
            communicate=lambda *_args, **_kwargs: ("output", "diagnostic"),
            poll=lambda: 7,
            __exit__=lambda *_args: None,
        )

    monkeypatch.setattr(
        process_guard, "native_popen_command", lambda _command: physical
    )
    install_module_view(
        monkeypatch, "subprocess", subprocess, process_guard, Popen=popen
    )
    with pytest.raises(subprocess.CalledProcessError) as caught:
        process_guard.run_completed_command(
            command, memory_guard_prefix=None, capture_output=True, check=True
        )
    assert (
        caught.value.cmd,
        caught.value.returncode,
        caught.value.stdout,
        caught.value.stderr,
    ) == (physical, 7, "output", "diagnostic")


@pytest.mark.parametrize("check", [False, True])
@pytest.mark.parametrize("signum", [None, 2, 15])
def test_operator_signal_propagates_independently_of_child_status(check, signum):
    import signal

    terminal = SimpleNamespace(
        returncode=143 if signum is None else 0,
        guard_signal=signum,
        timed_out=False,
        stdout="receipt output",
        stderr="",
    )

    class Context:
        @classmethod
        def from_env(cls, *_args, **_kwargs):
            return cls()

        def run(self, command, **kwargs):
            return terminal

    def run():
        return process_guard.run_completed_command(
            ["owned-child"],
            check=check,
            guard_loader=lambda _cwd, *, guard_source_root=None: SimpleNamespace(
                HarnessExecutionContext=Context
            ),
        )

    if signum is None:
        if check:
            with pytest.raises(subprocess.CalledProcessError) as caught:
                run()
            assert caught.value.returncode == 143
        else:
            assert run() is terminal
    else:
        exception = KeyboardInterrupt if signum == signal.SIGINT else SystemExit
        with pytest.raises(exception) as caught:
            run()
        assert caught.value.guarded_result is terminal
        if signum == signal.SIGTERM:
            assert caught.value.code == 143


@pytest.mark.parametrize("mode", ["ordinary143", "SIGINT", "SIGTERM"])
def test_nested_guard_operator_stop_prevents_next_serial_command(tmp_path, mode):
    import json
    import os
    import sys
    from pathlib import Path
    from tests.process_guard_common import run_guarded_test_process

    if os.name != "posix":
        pytest.skip("POSIX operator-signal delivery; structured boundary is portable")
    root = Path(__file__).resolve().parents[1]
    worker = tmp_path / "serial-owner.py"
    receipt = tmp_path / "receipt.json"
    sentinel = tmp_path / "next-command"
    # The test-owned child signals only its direct inline-guard parent, after
    # launch proves the guard has installed its handlers. No timing sleep races.
    worker.write_text(
        "import sys, json, signal\n"
        "from pathlib import Path\n"
        f"sys.path[:0] = {[str(root), str(root / 'src')]!r}\n"
        "from molt.process_guard import run_completed_command\n"
        f"mode = {mode!r}\n"
        "code = 'raise SystemExit(143)' if mode == 'ordinary143' else "
        "'import os,signal,time; os.kill(os.getppid(),signal.' + mode + '); time.sleep(60)'\n"
        "try:\n"
        "    result = run_completed_command([sys.executable, '-c', code], "
        "check=False, timeout=20, capture_output=True)\n"
        f"    Path({str(sentinel)!r}).write_text(str(result.returncode))\n"
        "except BaseException as exc:\n"
        "    result = exc.guarded_result\n"
        f"    Path({str(receipt)!r}).write_text(json.dumps({{'kind': type(exc).__name__, "
        "'signal': result.guard_signal, 'closed': result.descendants_closed}))\n"
        "    raise SystemExit(0)\n",
        encoding="utf-8",
    )
    result = run_guarded_test_process(
        [sys.executable, str(worker)],
        cwd=root,
        timeout=40,
    )
    assert result.returncode == 0, result.stderr
    assert result.guard_signal is None
    assert result.descendants_closed
    if mode == "ordinary143":
        assert sentinel.read_text() == "143"
        assert not receipt.exists()
    else:
        assert not sentinel.exists()
        assert json.loads(receipt.read_text()) == {
            "kind": "KeyboardInterrupt" if mode == "SIGINT" else "SystemExit",
            "signal": 2 if mode == "SIGINT" else 15,
            "closed": True,
        }


def test_hung_child_cleanup_stops_only_what_carries_its_own_marker() -> None:
    """The cleanup's victims carry the test's fresh marker as a whole argument.

    A process whose argv merely contains the text, as a shell's command line
    can, is not the test's, and neither is one with another test's marker.
    """
    marker = hung_child_marker()
    other = hung_child_marker()
    assert marker != other
    with pytest.raises(ValueError):
        processes_carrying("hung-gate-of-test_integrate_cancels_later_g0")
    carrying = start_owned_test_process(
        [sys.executable, "-c", HUNG_CHILD_SOURCE, marker]
    )
    mentioning = start_owned_test_process(
        [sys.executable, "-c", f"# {marker}\n" + HUNG_CHILD_SOURCE]
    )
    foreign = start_owned_test_process([sys.executable, "-c", HUNG_CHILD_SOURCE, other])
    try:
        assert processes_carrying(marker) == [carrying.pid]

        stop_test_processes_carrying(marker)

        assert carrying.wait() != 0
        assert mentioning.poll() is None and foreign.poll() is None
        assert processes_carrying(marker) == []
    finally:
        for process in (carrying, mentioning, foreign):
            close_owned_test_process(process)
