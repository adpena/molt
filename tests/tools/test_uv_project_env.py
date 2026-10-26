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
from molt.python_environment_location import environment_bytecode_arguments
from tools import uv_project_env
from tests.python_environment_test_support import materialize_development_environment
from tests.integration_driver_fixture import copy_driver_trees
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def test_project_environment_path_uses_dx_root_and_interpreter_identity(
    tmp_path: Path,
) -> None:
    artifacts = tmp_path / "external"
    image = materialize_development_environment(
        tmp_path, artifacts, python="3.14", purpose="output startup/size", select=True
    )
    assert (
        uv_project_env.project_environment_path(
            python="3.14",
            purpose="output startup/size",
            repo_root=tmp_path,
            env={"MOLT_EXT_ROOT": str(artifacts)},
        )
        == image.root
    )


def test_uv_project_env_sets_project_environment(tmp_path: Path) -> None:
    image = materialize_development_environment(
        tmp_path, tmp_path, python="3.14", purpose="audit", select=True
    )
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        repo_root=tmp_path,
        env={"PATH": "x", "MOLT_EXT_ROOT": str(tmp_path)},
    )
    assert env["PATH"] == "x"
    assert env["UV_PROJECT_ENVIRONMENT"] == str(image.root)
    assert env["PYTHONPYCACHEPREFIX"] == str(image.root / "bytecode")
    assert env["UV_NO_SYNC"] == "1"


def test_uv_project_env_uses_external_artifact_root(tmp_path: Path) -> None:
    artifacts, repo = tmp_path / "external", tmp_path / "repo"
    image = materialize_development_environment(
        repo, artifacts, python="3.14", purpose="audit", select=True
    )
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        repo_root=repo,
        env={"MOLT_EXT_ROOT": str(artifacts)},
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(image.root)
    assert image.root.is_relative_to(artifacts)
    assert not (repo / ".venv").exists()


def test_uv_project_env_accepts_explicit_relative_path(tmp_path: Path) -> None:
    image = materialize_development_environment(tmp_path, tmp_path, python="3.14")
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="ignored",
        repo_root=tmp_path,
        env={"MOLT_EXT_ROOT": str(tmp_path)},
        explicit=str(image.root.relative_to(tmp_path)),
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(image.root)
    manifest = image.manifest_path.read_bytes()
    dotted = uv_project_env.uv_project_env(
        python="3.14",
        purpose="ignored",
        repo_root=tmp_path,
        env={"MOLT_EXT_ROOT": str(tmp_path)},
        explicit=str(image.root / ".." / image.root.name),
    )
    assert dotted["UV_PROJECT_ENVIRONMENT"] == str(image.root)
    assert dotted["PYTHONPYCACHEPREFIX"] == str(image.root / "bytecode")
    assert dotted["PYTHONDONTWRITEBYTECODE"] == "1"
    assert dotted["UV_NO_SYNC"] == "1"
    assert image.manifest_path.read_bytes() == manifest
    with pytest.raises(dx.DxConfigError, match="different requested Python"):
        dx.DxProject(tmp_path).normalized_uv_run_env(
            uv_project_env.uv_project_env(
                python="3.13",
                purpose="ignored",
                repo_root=tmp_path,
                env={"MOLT_EXT_ROOT": str(tmp_path)},
                explicit=str(image.root),
            ),
            python="3.13",
        )


@pytest.mark.parametrize("name_rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize("session", ["default", "generated", "explicit"])
@pytest.mark.parametrize("override_target", [False, True])
def test_uv_wrapper_selectors_keep_the_captured_environment_rule(
    tmp_path, name_rule, session, override_target
):
    root, artifacts = tmp_path / "repo", tmp_path / "artifacts"
    first_image = materialize_development_environment(root, artifacts, python="3.14")
    values = {
        "MOLT_EXT_ROOT": str(artifacts),
        "uv_project_environment": str(first_image.root),
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
    selected_environments = []
    for python, purpose in (("3.14", "audit"), ("3.13", "public-records")):
        image = materialize_development_environment(
            root, artifacts, python=python, purpose=purpose, select=True
        )
        expected = image.root if name_rule is NameRule.EXACT else first_image.root
        assert (
            uv_project_env.project_environment_path(
                python=python, purpose=purpose, repo_root=root, env=environment
            )
            == expected
        )
        if name_rule is NameRule.WINDOWS and python == "3.13":
            with pytest.raises(dx.DxConfigError, match="different requested Python"):
                dx.DxProject(root).normalized_uv_run_env(
                    uv_project_env.uv_project_env(
                        python=python,
                        purpose=purpose,
                        repo_root=root,
                        env=environment,
                        create_dirs=False,
                    ),
                    python=python,
                )
            continue
        selected = uv_project_env.uv_project_env(
            python=python,
            purpose=purpose,
            repo_root=root,
            env=environment,
            create_dirs=False,
        )
        selected_environments.append(selected["UV_PROJECT_ENVIRONMENT"])
        assert selected.rule is name_rule
        assert selected["UV_PROJECT_ENVIRONMENT"] == str(expected)
        assert selected["MOLT_UV_PROJECT_PURPOSE"] == purpose
        assert selected["MOLT_UV_PROJECT_PYTHON"] == python
        assert selected["Path"] == "retained path"
        assert selected["CARGO_TARGET_DIR"] == str(expected_target)
        assert selected["MOLT_DIFF_CARGO_TARGET_DIR"] == str(expected_target)
        if session == "explicit":
            assert selected["MOLT_SESSION_ID"] == "caller-session"
            assert "MOLT_SESSION_ID_GENERATED" not in selected
        else:
            assert selected["MOLT_SESSION_ID_GENERATED"] == "1"
        assert dict(environment) == values
    assert len(selected_environments) == (2 if name_rule is NameRule.EXACT else 1)


def test_managed_model_and_operation_refuse_explicit_borrower(
    tmp_path, monkeypatch, capsys
):
    borrowed = str(tmp_path / "external-bootstrap")
    with pytest.raises(dx.DxConfigError, match="explicitly borrowed MOLT_VENV"):
        uv_project_env.uv_project_env(
            python="3.12",
            purpose="audit",
            repo_root=tmp_path,
            env={"MOLT_VENV": borrowed},
            create_dirs=False,
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
    tmp_path: Path, status: int, driver_seed: Path
) -> None:
    project = tmp_path / "project"
    copy_driver_trees(project, seed=driver_seed)
    (project / "README.md").write_text("wrapper fixture\n", encoding="utf-8")
    image = materialize_development_environment(
        project,
        tmp_path / "artifacts",
        python=f"{sys.version_info.major}.{sys.version_info.minor}",
        groups=dx.DxProject(project).dependency_groups(),
        runnable=True,
    )
    # The bootstrap interpreter's sitecustomize imports Molt before the copied
    # script binds repository imports. Select the copied source at startup so
    # that strict binder sees the same owner; retain the outer guard custody.
    env = (
        ProcessEnvironment(os.environ)
        .without(("MOLT_VENV", "VIRTUAL_ENV"))
        .updated(
            {
                "PYTHONPATH": os.pathsep.join((str(project / "src"), str(project))),
                "MOLT_EXT_ROOT": str(tmp_path / "artifacts"),
                "UV_PROJECT_ENVIRONMENT": str(image.root),
            }
        )
    )
    result = run_guarded_test_process(
        [
            sys.executable,
            *environment_bytecode_arguments(Path(sys.prefix)),
            str(project / "tools/uv_project_env.py"),
            "--python",
            f"{sys.version_info.major}.{sys.version_info.minor}",
            "--purpose",
            "forwarding-test",
            "--",
            "python",
            "-I",
            "-E",
            "-c",
            "import sys; assert sys.dont_write_bytecode; "
            "assert sys.pycache_prefix == sys.argv[1]; "
            "raise SystemExit(int(sys.argv[2]))",
            str(image.root / "bytecode"),
            str(status),
        ],
        cwd=project,
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
    startup = environment_bytecode_arguments(Path(sys.prefix))
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
        *environment_bytecode_arguments(Path(sys.prefix)),
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
