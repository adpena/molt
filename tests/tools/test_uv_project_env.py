from __future__ import annotations

import os
import platform
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
from molt.python_interpreter import readonly_bytecode_arguments
from tools import uv_project_env
from tests.uv_environment_test_support import fake_uv_environ, make_checkout, uv_calls
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def test_project_environment_path_syncs_the_release_shared_environment(
    tmp_path: Path,
) -> None:
    environ, log = fake_uv_environ(tmp_path)
    first = make_checkout(tmp_path / "first")
    second = make_checkout(tmp_path / "second")
    release = platform.python_version()
    root = uv_project_env.project_environment_path(
        python=release, repo_root=first, env=environ
    )
    assert root.is_relative_to(tmp_path / "artifacts")
    assert (
        uv_project_env.project_environment_path(
            python=release, repo_root=second, env=environ
        )
        == root
    )
    assert len(uv_calls(log)) == 1
    assert not (first / ".venv").exists()


def test_uv_project_env_selects_the_environment_and_its_shared_cache(
    tmp_path: Path,
) -> None:
    environ, _log = fake_uv_environ(tmp_path)
    repo = make_checkout(tmp_path / "repo")
    env = uv_project_env.uv_project_env(
        python=platform.python_version(), repo_root=repo, env={**environ, "PATH": "x"}
    )
    root = Path(env["UV_PROJECT_ENVIRONMENT"])
    assert env["VIRTUAL_ENV"] == str(root)
    assert env["PATH"].split(os.pathsep) == [str(root / "bin"), "x"]
    assert env["PYTHONPYCACHEPREFIX"] == str(root.parent / ".bytecode")
    assert "PYTHONDONTWRITEBYTECODE" not in env
    assert env["UV_NO_SYNC"] == "1"
    assert env["PYTHONPATH"].split(os.pathsep)[0] == str(repo.resolve() / "src")


def test_uv_project_env_takes_an_explicit_environment_without_syncing(
    tmp_path: Path,
) -> None:
    environ, log = fake_uv_environ(tmp_path)
    repo = make_checkout(tmp_path / "repo")
    env = uv_project_env.uv_project_env(
        python="3.14", repo_root=repo, env=environ, explicit="elsewhere/venv"
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(repo / "elsewhere" / "venv")
    assert uv_calls(log) == []


@pytest.mark.parametrize("name_rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize("session", ["default", "generated", "explicit"])
@pytest.mark.parametrize("override_target", [False, True])
def test_uv_wrapper_selectors_keep_the_captured_environment_rule(
    tmp_path, name_rule, session, override_target
):
    # Under the Windows rule a lower-case `uv_project_environment` is the
    # explicit selector; under the exact rule it is another variable, and
    # the shared environment of the release is selected instead.
    environ, _log = fake_uv_environ(tmp_path)
    root, artifacts = make_checkout(tmp_path / "repo"), Path(environ["MOLT_EXT_ROOT"])
    borrowed = tmp_path / "borrowed"
    # The fake uv is named by UV; PATH would alias `Path` under the Windows rule.
    values = {
        **{key: value for key, value in environ.items() if key != "PATH"},
        "uv_project_environment": str(borrowed),
        "Path": "retained path",
    }
    if session != "default":
        values["MOLT_SESSION_ID"] = "caller-session"
    if session == "generated":
        values["MOLT_SESSION_ID_GENERATED"] = "1"
    if override_target:
        values["CARGO_TARGET_DIR"] = str(tmp_path / "caller target")
    environment = ProcessEnvironment(values, rule=name_rule)
    expected_target = (
        tmp_path / "caller target"
        if override_target
        else dx.cargo_target_dir_for_artifact_root(
            artifacts,
            "caller-session" if session == "explicit" else None,
            source_root=root,
        )
    )
    release = platform.python_version()
    expected = (
        borrowed
        if name_rule is NameRule.WINDOWS
        else uv_project_env.project_environment_path(
            python=release, repo_root=root, env=environ
        )
    )
    assert (
        uv_project_env.project_environment_path(
            python=release, repo_root=root, env=environment
        )
        == expected
    )
    selected = uv_project_env.uv_project_env(
        python=release, repo_root=root, env=environment, create_dirs=False
    )
    assert selected.rule is name_rule
    assert selected["UV_PROJECT_ENVIRONMENT"] == str(expected)
    # Activation puts the environment's scripts first on PATH, which the
    # Windows rule spells `Path`; the exact rule keeps `Path` as given.
    assert selected["Path"] == (
        f"{expected / 'bin'}{os.pathsep}retained path"
        if name_rule is NameRule.WINDOWS
        else "retained path"
    )
    assert selected["CARGO_TARGET_DIR"] == str(expected_target)
    assert selected["MOLT_DIFF_CARGO_TARGET_DIR"] == str(expected_target)
    if session == "explicit":
        assert selected["MOLT_SESSION_ID"] == "caller-session"
        assert "MOLT_SESSION_ID_GENERATED" not in selected
    else:
        assert selected["MOLT_SESSION_ID_GENERATED"] == "1"
    assert dict(environment) == values


def test_managed_model_and_operation_refuse_explicit_borrower(
    tmp_path, monkeypatch, capsys
):
    borrowed = str(tmp_path / "external-bootstrap")
    with pytest.raises(dx.DxConfigError, match="explicitly borrowed MOLT_VENV"):
        uv_project_env.uv_project_env(
            python="3.12", repo_root=tmp_path, env={"MOLT_VENV": borrowed}
        )
    monkeypatch.setenv("MOLT_VENV", borrowed)
    with pytest.raises(SystemExit) as caught:
        uv_project_env.main(["--python", "3.12", "--", "python", "-V"])
    assert caught.value.code == 2
    assert "explicitly borrowed MOLT_VENV" in capsys.readouterr().err
    assert not list(tmp_path.iterdir())


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
    # The wrapper replaces itself with the payload, whose status is the
    # wrapper's; a managed payload selects the shared cache even under -I.
    root = Path(sys.prefix)
    result = run_guarded_test_process(
        [
            sys.executable,
            *readonly_bytecode_arguments(Path(sys.prefix)),
            str(ROOT / "tools/uv_project_env.py"),
            "--python",
            platform.python_version(),
            "--venv",
            str(root),
            "--",
            "python",
            "-I",
            "-c",
            "import sys; assert sys.pycache_prefix == sys.argv[1]; "
            "raise SystemExit(int(sys.argv[2]))",
            str(root.parent / ".bytecode"),
            str(status),
        ],
        cwd=ROOT,
        env=ProcessEnvironment(os.environ).without(("MOLT_VENV",)),
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
    startup = readonly_bytecode_arguments(Path(sys.prefix))
    before_exec = (
        "import os,sys; from molt.process_spawn import forward_command; "
        "env=dict(os.environ); env['CARGO_TARGET_DIR']=sys.argv[1]; "
        f"forward_command([sys.executable,*{startup!r},'-S','-c',sys.argv[2],sys.argv[1]],env=env)"
    )
    # `-S` drops the venv's site paths, so the source tree is named here and
    # the test does not depend on a runner that exported it.
    result = run_guarded_test_process(
        [sys.executable, *startup, "-S", "-c", before_exec, str(target), after_exec],
        cwd=ROOT,
        env={**os.environ, "PYTHONPATH": str(ROOT / "src")},
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "exec-reader-kept-without-registry"


def test_command_resource_projection_preserves_native_posix_path_semantics(tmp_path):
    from molt import process_launch

    if os.name == "nt":
        pytest.skip("POSIX exec search; Windows uses native caller facts")
    assert tuple(
        resource.path
        for resource in process_launch.command_resources(["missing"], {}, cwd=tmp_path)
    ) == tuple(
        Path(entry) if Path(entry).is_absolute() else tmp_path / entry
        for entry in os.get_exec_path({})
    )
    assert tuple(
        resource.path
        for resource in process_launch.command_resources(
            ["missing"], {"PATH": ""}, cwd=tmp_path
        )
    ) == (tmp_path,)
    assert tuple(
        resource.path
        for resource in process_launch.command_resources(
            ["./relative/missing"], {"PATH": "/unused"}, cwd=tmp_path
        )
    ) == (tmp_path / "relative/missing",)


def test_absolute_windows_resource_needs_no_native_search_facts(tmp_path, monkeypatch):
    from molt import process_launch

    def forbidden():
        raise AssertionError("absolute entrypoint performed native search work")

    install_module_os_view(monkeypatch, process_launch, name="nt")
    monkeypatch.setattr(process_launch, "native_environment_reader", forbidden)
    executable = tmp_path / "arbitrary-entrypoint"
    assert process_launch.command_resources([str(executable)], {}, cwd=ROOT) == (
        process_launch.NativeLaunchResource(executable, "entrypoint"),
    )


def test_ordinary_command_resource_projection_does_not_probe_filesystem(
    tmp_path, monkeypatch
):
    from molt.checkout_state import checkout_state_command_roots

    def forbidden(*_args, **_kwargs):
        raise AssertionError("ordinary unmanaged launch coordinate was probed")

    with monkeypatch.context() as patch:
        for name in ("lstat", "stat", "resolve"):
            patch.setattr(Path, name, forbidden)
        roots = checkout_state_command_roots(
            ROOT,
            {"PATH": os.pathsep.join(str(tmp_path / f"path-{i}") for i in range(16))},
            ["missing-command"],
            cwd=tmp_path,
        )
    assert roots == ((Path(sys.prefix).absolute(), None),)


@pytest.mark.parametrize("unused", ["absent", "unowned", "indirect", "inaccessible"])
def test_optional_launch_coordinates_preserve_native_failure_policy(
    tmp_path, monkeypatch, unused
):
    from molt import checkout_state

    owner = tmp_path / "owner"
    artifact = tmp_path / "unused-artifacts"
    candidate = (
        artifact
        / "build-environments/development"
        / dx.checkout_component(owner)
        / ("a" * 64)
    )
    if unused != "absent":
        if unused == "indirect":
            outside = tmp_path / "unmanaged"
            outside.mkdir()
            candidate.parent.mkdir(parents=True)
            try:
                candidate.symlink_to(outside, target_is_directory=True)
            except OSError:
                pytest.skip("directory symlink unavailable")
        else:
            candidate.mkdir(parents=True)
    if unused == "inaccessible":
        original = checkout_state._managed_coordinate

        def denied(path):
            if path == candidate:
                raise PermissionError("unused candidate cannot be inspected")
            return original(path)

        monkeypatch.setattr(checkout_state, "_managed_coordinate", denied)
    env = {
        "PATH": os.pathsep.join((str(tmp_path / "ordinary"), str(candidate / "bin"))),
        "VIRTUAL_ENV": str(candidate),
    }
    roots = checkout_state.checkout_state_command_roots(
        ROOT, env, ["missing-command"], cwd=tmp_path
    )
    assert roots == ((Path(sys.prefix).absolute(), None),)
    with checkout_state.checkout_state_use(roots):
        assert not (artifact / "target").exists()


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
        *readonly_bytecode_arguments(Path(sys.prefix)),
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
