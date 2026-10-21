from __future__ import annotations

import os
from pathlib import Path
import sys

import pytest
from tests.process_guard_common import (
    HUNG_CHILD_SOURCE,
    install_module_os_view,
    install_module_view,
    run_guarded_test_process,
)

from molt import dx, process_spawn
from molt.process_environment import NameRule, ProcessEnvironment
from tools import uv_project_env
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def test_project_environment_path_uses_dx_root_and_versioned_session(
    tmp_path: Path,
) -> None:
    artifact_root = tmp_path / "external"
    path = uv_project_env.project_environment_path(
        python="3.14",
        purpose="output startup/size",
        repo_root=tmp_path,
        env={
            "MOLT_EXT_ROOT": str(artifact_root),
        },
    )

    assert path == dx.stable_uv_project_env_dir(
        artifact_root,
        purpose="output startup/size",
        python="3.14",
        source_root=tmp_path,
    )


def test_uv_project_env_sets_project_environment(tmp_path: Path) -> None:
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        env={
            "PATH": "x",
            "MOLT_EXT_ROOT": str(tmp_path),
        },
        repo_root=tmp_path,
    )

    assert env["PATH"] == "x"
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        dx.stable_uv_project_env_dir(
            tmp_path, purpose="audit", python="3.14", source_root=tmp_path
        )
    )


def test_uv_project_env_uses_external_artifact_root(tmp_path: Path) -> None:
    artifact_root = tmp_path / "external"
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        env={
            "MOLT_EXT_ROOT": str(artifact_root),
        },
        repo_root=repo_root,
    )

    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        dx.stable_uv_project_env_dir(
            artifact_root,
            purpose="audit",
            python="3.14",
            source_root=repo_root,
        )
    )


def test_uv_project_env_accepts_explicit_relative_path(tmp_path: Path) -> None:
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="ignored",
        env={
            "MOLT_EXT_ROOT": str(tmp_path),
        },
        repo_root=tmp_path,
        explicit="tmp/custom-env",
    )

    assert env["UV_PROJECT_ENVIRONMENT"] == str(tmp_path / "tmp" / "custom-env")


@pytest.mark.parametrize("name_rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_uv_wrapper_selectors_keep_the_captured_environment_rule(tmp_path, name_rule):
    root = tmp_path / "repo"
    root.mkdir()
    artifact_root = tmp_path / "artifacts"
    alias = tmp_path / "caller-environment"
    environment = ProcessEnvironment(
        {
            "MOLT_EXT_ROOT": str(artifact_root),
            "uv_project_environment": str(alias),
            "Path": "retained path",
        },
        rule=name_rule,
    )
    expected = (
        dx.stable_uv_project_env_dir(
            artifact_root, purpose="audit", python="3.14", source_root=root
        )
        if name_rule is NameRule.EXACT
        else alias
    )
    assert (
        uv_project_env.project_environment_path(
            python="3.14", purpose="audit", repo_root=root, env=environment
        )
        == expected
    )
    selected = uv_project_env.uv_project_env(
        python="3.14", purpose="audit", repo_root=root, env=environment
    )
    assert selected.rule is name_rule
    assert selected["UV_PROJECT_ENVIRONMENT"] == str(expected)
    assert selected["MOLT_UV_PROJECT_PURPOSE"] == "audit"
    assert selected["MOLT_UV_PROJECT_PYTHON"] == "3.14"
    assert selected["Path"] == "retained path"
    assert dict(environment) == {
        "MOLT_EXT_ROOT": str(artifact_root),
        "uv_project_environment": str(alias),
        "Path": "retained path",
    }


def test_parse_command_strips_separator() -> None:
    assert uv_project_env._parse_command(["--", "uv", "run"]) == ["uv", "run"]


@pytest.mark.parametrize("cwd", [None, "forwarded working directory"])
def test_forward_command_waits_for_child_on_windows(monkeypatch, cwd) -> None:
    calls = []

    def fake_popen(command, *, cwd, env):
        from types import SimpleNamespace

        calls.append((command, cwd, env))
        return SimpleNamespace(
            communicate=lambda: (None, None),
            poll=lambda: 7,
            __exit__=lambda *_args: None,
        )

    install_module_os_view(monkeypatch, process_spawn, name="nt")
    install_module_view(
        monkeypatch, "subprocess", subprocess, process_spawn, Popen=fake_popen
    )

    assert (
        uv_project_env.forward_command(["uv", "--version"], cwd=cwd, env={"X": "1"})
        == 7
    )
    assert calls == [(["uv", "--version"], cwd, {"X": "1"})]


@pytest.mark.parametrize("status", [0, 7])
def test_uv_project_env_real_command_exit_status_propagates(
    tmp_path: Path, status: int
) -> None:
    env = os.environ.copy()
    env["MOLT_EXT_ROOT"] = str(tmp_path / "artifacts")
    result = run_guarded_test_process(
        [
            sys.executable,
            str(ROOT / "tools" / "uv_project_env.py"),
            "--python",
            "3.12",
            "--purpose",
            "forwarding-test",
            "--",
            sys.executable,
            "-c",
            f"raise SystemExit({status})",
        ],
        cwd=ROOT,
        env=env,
        timeout=60,
    )
    assert result.returncode == status, result.stderr


@pytest.mark.skipif(os.name == "nt", reason="POSIX exec descriptor transfer")
def test_forward_exec_failure_restores_and_releases_managed_reader(
    tmp_path, monkeypatch
):
    from molt import checkout_state, dx, file_locks
    from molt.build_state_layout import build_state_root
    from molt.source_root import compiler_source_root

    source = compiler_source_root()
    artifact = tmp_path / "artifacts"
    target = dx.cargo_target_dir_for_artifact_root(
        artifact, "reader", source_root=source
    )
    target.mkdir(parents=True)
    environment = {"MOLT_EXT_ROOT": str(artifact), "CARGO_TARGET_DIR": str(target)}
    control = build_state_root(
        project_root=source, cargo_target=target, environment=environment
    )
    # The command cwd contains the artifact root, so it cannot stand in for
    # compiler source when selecting the target's actual control owner.
    assert control.is_relative_to(artifact)
    captured = []
    released = []
    original_release = file_locks._release_file_lock
    before = set(file_locks._LIVE_FILE_LOCK_HANDLES)

    def failed_exec(*args):
        handles = tuple(file_locks._LIVE_FILE_LOCK_HANDLES.values())
        captured.extend(handle for handle in handles if id(handle) not in before)
        assert captured and all(
            os.get_inheritable(handle.file.fileno()) for handle in captured
        )
        for root in (target, control):
            with checkout_state.checkout_state_retirement(root) as allowed:
                assert allowed is False
        raise FileNotFoundError("deliberate failed exec")

    def checked_release(handle):
        if handle in captured:
            released.append(handle.file.fileno())
            assert os.get_inheritable(handle.file.fileno()) is False
        original_release(handle)

    install_module_view(monkeypatch, "os", os, process_spawn, execvpe=failed_exec)
    monkeypatch.setattr(file_locks, "_release_file_lock", checked_release)
    with pytest.raises(FileNotFoundError, match="deliberate failed exec"):
        process_spawn.forward_command(
            ["missing-subject"], cwd=tmp_path, env=environment
        )
    assert len(released) == len(captured)
    assert all(handle.file.closed for handle in captured)
    for root in (target, control):
        with checkout_state.checkout_state_retirement(root) as allowed:
            assert allowed is True


@pytest.mark.skipif(
    os.name == "nt", reason="POSIX actual exec; Windows has waiting parent"
)
def test_forwarded_exec_retains_native_reader_without_a_python_registry(tmp_path):
    from molt import dx

    source = tmp_path / "source"
    source.mkdir()
    target = dx.cargo_target_dir_for_artifact_root(
        tmp_path / "artifacts", "reader", source_root=source
    )
    target.mkdir(parents=True)
    after_exec = (
        "from pathlib import Path; import sys; from molt import file_locks as f; "
        "from molt.checkout_state import checkout_state_retirement; "
        "assert not f._LIVE_FILE_LOCK_HANDLES; "
        "context=checkout_state_retirement(Path(sys.argv[1])); "
        "assert context.__enter__() is False; context.__exit__(None,None,None); "
        "print('exec-reader-kept-without-registry')"
    )
    before_exec = (
        "import os,sys; from molt.process_spawn import forward_command; "
        "env=dict(os.environ); env['CARGO_TARGET_DIR']=sys.argv[1]; "
        "forward_command([sys.executable,'-B','-S','-c',sys.argv[2],sys.argv[1]],env=env)"
    )
    result = run_guarded_test_process(
        [sys.executable, "-B", "-S", "-c", before_exec, str(target), after_exec],
        cwd=ROOT,
        env=os.environ.copy(),
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "exec-reader-kept-without-registry"


@pytest.mark.parametrize("failure", [None, "spawn", "wait"])
def test_windows_forward_retains_actual_child_use_after_failed_cleanup_and_gc(
    tmp_path, monkeypatch, failure
):
    import gc
    import weakref
    from molt import checkout_state
    from molt.build_state_layout import build_state_root
    from molt.source_root import compiler_source_root

    source = compiler_source_root()
    artifact = tmp_path / "artifact"
    target = dx.cargo_target_dir_for_artifact_root(
        artifact, "reader", source_root=source
    )
    target.mkdir(parents=True)
    environment = {
        **os.environ,
        "MOLT_EXT_ROOT": str(artifact),
        "CARGO_TARGET_DIR": str(target),
    }
    control = build_state_root(
        project_root=source, cargo_target=target, environment=environment
    )
    assert control.is_relative_to(artifact)
    owners = []
    children = []
    methods = []
    register = process_spawn.atexit.register
    popen = subprocess.Popen

    def registered(callback):
        owners.append(weakref.ref(callback.__self__))
        return register(callback)

    def start(command, **kwargs):
        assert owners[0]().source_root == source
        for root in (target, control):
            with checkout_state.checkout_state_retirement(root) as admitted:
                assert not admitted  # held before Popen can create a process
        if failure == "spawn":
            raise FileNotFoundError("independent spawn refusal")
        process = popen(command, **kwargs)
        children.append(process)
        methods.append((process.kill, process.wait))
        if failure == "wait":

            def fail_communicate(*args, **kwargs):
                raise RuntimeError("original forwarding failure")

            def fail_cleanup(*args, **kwargs):
                raise OSError("unobserved forwarding cleanup")

            process.communicate = fail_communicate
            process.kill = fail_cleanup
            process.wait = fail_cleanup
        return process

    install_module_os_view(monkeypatch, process_spawn, name="nt")
    install_module_view(
        monkeypatch, "subprocess", subprocess, process_spawn, Popen=start
    )
    install_module_view(
        monkeypatch, "atexit", process_spawn.atexit, process_spawn, register=registered
    )
    command = [
        sys.executable,
        "-I",
        "-S",
        "-c",
        HUNG_CHILD_SOURCE if failure == "wait" else "raise SystemExit(7)",
    ]
    try:
        if failure is None:
            assert (
                process_spawn.forward_command(command, cwd=tmp_path, env=environment)
                == 7
            )
        else:
            expected = FileNotFoundError if failure == "spawn" else RuntimeError
            with pytest.raises(
                expected,
                match="independent spawn refusal"
                if failure == "spawn"
                else "original forwarding failure",
            ) as caught:
                process_spawn.forward_command(command, cwd=tmp_path, env=environment)
            if failure == "wait":
                assert caught.value.owned_process is children[0]
                assert caught.value.state_owner is owners[0]()
                assert not caught.value.state_owner.released
            del caught
        gc.collect()
        if failure == "wait":
            assert owners[0]() is not None
            assert children[0].poll() is None
            for root in (target, control):
                with checkout_state.checkout_state_retirement(root) as admitted:
                    assert not admitted
        else:
            assert owners[0]() is None
            for root in (target, control):
                with checkout_state.checkout_state_retirement(root) as admitted:
                    assert admitted
    finally:
        for process, (kill, wait) in zip(children, methods):
            process.wait = wait
            if process.poll() is None:
                kill()
            wait(timeout=5)
        if owners and (owner := owners[0]()) is not None:
            owner._close_at_exit()
    for root in (target, control):
        with checkout_state.checkout_state_retirement(root) as admitted:
            assert admitted
