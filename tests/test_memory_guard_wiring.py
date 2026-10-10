from __future__ import annotations

import errno
import json
import os
from pathlib import Path
import sys
import tomllib

import pytest

from tools import check_memory_guard_wiring
from tools import memory_guard
from molt import pytest_memory_guard_bootstrap
from molt import pytest_memory_guard_config_plugin
from molt import memory_guard_paths
from molt import temporary_artifacts
from tests.process_guard_common import install_module_os_view, install_module_view
import subprocess

# These tests fake process data the session sentinel also reads.
pytestmark = pytest.mark.usefixtures("session_sentinel_paused")


REPO_ROOT = Path(__file__).resolve().parents[1]


@pytest.fixture(autouse=True)
def bootstrap_uses_a_private_os(monkeypatch: pytest.MonkeyPatch) -> None:
    """Every ``os`` patch in this file stays inside the pytest bootstrap."""
    install_module_os_view(monkeypatch, pytest_memory_guard_bootstrap)


def test_repository_plugin_exports_only_valid_pytest_hooks():
    manager = pytest.PytestPluginManager()
    manager.register(
        pytest_memory_guard_config_plugin, "molt.pytest_memory_guard_config_plugin"
    )
    manager.check_pending()


def test_installation_has_no_global_pytest_hook_but_repository_loads_guard():
    metadata = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text("utf-8"))
    # This metadata drives entry-point autoload in every environment where Molt
    # is installed. Repository custody belongs to pytest's local configuration.
    assert not metadata["project"].get("entry-points", {}).get("pytest11")
    addopts = metadata["tool"]["pytest"]["ini_options"]["addopts"]
    assert any(
        addopts[index : index + 2] == ["-p", "molt.pytest_memory_guard_config_plugin"]
        for index in range(len(addopts) - 1)
    )


@pytest.mark.parametrize(
    "relative",
    [
        "src/molt/pytest_memory_guard_bootstrap.py",
        "src/sitecustomize.py",
        "sitecustomize.py",
        "tests/sitecustomize.py",
        "tests/cli/sitecustomize.py",
        "tests/c_api/sitecustomize.py",
        "tests/e2e/sitecustomize.py",
        "tests/harness/sitecustomize.py",
        "tests/helpers/sitecustomize.py",
        "tests/mutation/sitecustomize.py",
        "tests/runtime_compat/sitecustomize.py",
        "tests/wasm_planned/sitecustomize.py",
    ],
)
def test_nontest_startup_preserves_package_only_import_boundary(monkeypatch, relative):
    import builtins

    original_import = builtins.__import__

    def no_repository_tooling(name, *args, **kwargs):
        if name == "tools" or name.startswith("tools."):
            pytest.fail(f"non-test startup imported repository tooling: {name}")
        return original_import(name, *args, **kwargs)

    root = str(REPO_ROOT)
    test_root = str(REPO_ROOT / "tests")
    source = str(REPO_ROOT / "src")
    paths = [path for path in sys.path if path not in {root, test_root}]
    monkeypatch.setattr(sys, "path", paths.copy())
    monkeypatch.setattr(sys, "orig_argv", [sys.executable, "-I", "-c", "pass"])
    monkeypatch.setattr(sys, "argv", ["-c"])
    monkeypatch.setattr(builtins, "__import__", no_repository_tooling)
    surface = REPO_ROOT / relative
    # Startup imports preserve interpreter argv; runpy would replace argv[0]
    # with the adapter path and falsely simulate a direct test invocation.
    exec(
        compile(surface.read_text(encoding="utf-8"), str(surface), "exec"),
        {"__file__": str(surface), "__name__": "startup_import_boundary_probe"},
    )
    assert root not in sys.path
    assert test_root not in sys.path
    assert [path for path in sys.path if path != source] == [
        path for path in paths if path != source
    ]


@pytest.mark.parametrize("invocation", ["pytest", "module", "script"])
def test_confirmed_test_invocations_bind_repository_before_guard_check(
    monkeypatch, invocation
):
    root = str(REPO_ROOT)
    monkeypatch.setattr(sys, "path", [path for path in sys.path if path != root])
    checks = []

    def guarded(_environment):
        checks.append(root in sys.path)
        return True

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "outer_memory_guard_active", guarded
    )
    if invocation == "pytest":
        result = pytest_memory_guard_bootstrap.ensure_pytest_memory_guard(
            pytest_args=(), environ={}
        )
    elif invocation == "module":
        result = pytest_memory_guard_bootstrap.ensure_repo_test_module_memory_guard(
            orig_argv=[sys.executable, "-m", "tests.test_memory_guard_wiring"],
            environ={},
        )
    else:
        result = pytest_memory_guard_bootstrap.ensure_repo_test_script_memory_guard(
            runtime_argv=[__file__], environ={}
        )
    assert result is True
    assert checks == [True]


def test_guard_authority_has_no_replaced_tool_or_router_implementation():
    for relative in (
        "tools/pytest_memory_guard_bootstrap.py",
        "tools/process_spawn.py",
        "tools/memory_guard_core/paths.py",
        "tests/_sitecustomize.py",
    ):
        assert not (REPO_ROOT / relative).exists()


def test_memory_guard_wiring_uses_clean_raw_subprocess_audit() -> None:
    audit = check_memory_guard_wiring.audit_repo()

    assert audit.missing_paths == ()
    assert audit.missing_tokens == ()
    assert audit.required_sentinel_missing == ()
    assert audit.sentinel_drift == ()
    assert audit.direct_test_guard_missing == ()
    assert audit.subprocess_guard_unexpected == ()
    assert audit.subprocess_guard_stale_allowlist == ()
    assert audit.subprocess_guard_expanded_allowlist == ()
    assert audit.ok is True


def test_wiring_audit_locks_down_pytest_and_ci_gate_custody() -> None:
    contracts = {
        contract.path: contract.tokens
        for contract in check_memory_guard_wiring.PYTHON_GUARD_CONTRACTS
    }

    assert contracts["pyproject.toml"] == ("molt.pytest_memory_guard_config_plugin",)
    assert contracts["src/molt/pytest_memory_guard_config_plugin.py"] == (
        "pytest_load_initial_conftests",
        "pytest_configure",
        "pytest_runtest_call",
    )
    assert contracts["src/sitecustomize.py"] == (
        "ensure_python_test_memory_guard",
        "molt.pytest_memory_guard_bootstrap",
    )
    assert contracts["sitecustomize.py"] == ("ensure_python_test_memory_guard",)
    assert contracts["src/molt/pytest_memory_guard_bootstrap.py"] == (
        "_bind_confirmed_test_repository",
        "pytest_load_initial_conftests",
        "pytest_configure",
        "pytest_runtest_logreport",
        "pytest_runtest_call",
        "MOLT_MEMORY_GUARD_ACTIVE",
        "MOLT_MEMORY_GUARD_PID",
        "MOLT_PYTEST_OUTER_GUARD_REEXEC",
        "MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC",
        "MOLT_PYTEST_CURRENT_TEST_FILE",
        "install_pytest_current_test_file_env",
        "ensure_current_file_test_script_memory_guard",
        "ensure_repo_test_module_memory_guard",
        "PYTEST_XDIST_WORKER",
        "tools/memory_guard.py",
        "MOLT_TEST_SUITE",
        "--noconftest",
        "--confcutdir",
        "PYTEST_ADDOPTS",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
        "pyproject.toml",
        "sample_processes",
        "handoff_to_outer_guard",
        "subprocess.run",
        "os._exit",
        "os.execvpe",
    )
    assert contracts["tests/conftest.py"] == (
        "harness_memory_guard",
        "outer_memory_guard_active",
        "validate_pytest_guardable_env",
        "repo_process_sentinel",
        "limits_from_env",
        "MOLT_PYTEST",
        "drain_on_exit=True",
    )
    assert contracts["tools/ci_gate.py"] == (
        "harness_memory_guard",
        "guarded_completed_process",
        "_resolve_memory_limits",
        "compile_governor.compile_slot",
        "MOLT_CI_GATE",
        "guarded_exec.py",
    )
    assert contracts["tools/memory_guard.py"] == (
        "test_custody_launch_env",
        "MOLT_PYTEST_CURRENT_TEST_FILE",
        "active_guard_marker_dir",
        "environ=child_env",
        "repro_context_payload",
    )
    assert contracts["tools/harness_memory_guard.py"] == (
        "test_custody_launch_env",
        "_guard_repro_message",
        "guarded_completed_process",
    )
    assert contracts["tests/harness/run_molt_conformance.py"] == (
        "harness_memory_guard",
        "process_guard_common",
        "run_guarded_test_process",
        "HarnessExecutionContext",
        "repo_process_sentinel",
    )
    assert contracts["tests/harness/run_monty_conformance.py"] == (
        "harness_memory_guard",
        "process_guard_common",
        "run_guarded_test_process",
        "repo_process_sentinel",
    )
    assert contracts["tests/benchmarks/bench_generator.py"] == (
        "process_guard_common",
        "run_guarded_test_process",
        "MOLT_BENCH",
    )
    assert contracts["tests/runtime_compat/test_runtime_compat.py"] == (
        "process_guard_common",
        "run_guarded_test_process",
        "MOLT_RUNTIME_COMPAT",
    )
    assert contracts["src/molt/cli/setup_readiness.py"] == ("MOLT_BUILD",)


def test_legacy_shell_entrypoints_enter_guarded_python_wrappers() -> None:
    missing_paths, missing_tokens = check_memory_guard_wiring._audit_token_contracts(
        check_memory_guard_wiring.REPO_ROOT,
        check_memory_guard_wiring.SHELL_WRAPPER_CONTRACTS,
    )

    assert missing_paths == ()
    assert missing_tokens == ()


def test_pytest_startup_reexecs_direct_pytest_under_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(72)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: False
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, execvpe=fake_execvpe
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        pytest_memory_guard_bootstrap,
        executable=sys.executable,
    )
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)
    monkeypatch.delenv("MOLT_PYTEST_OUTER_GUARD_REEXEC", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_pytest_memory_guard(
            orig_argv=[sys.executable, "-m", "pytest", "tests/test_one.py", "-q"],
            runtime_argv=["-m", "tests/test_one.py", "-q"],
        )
    except SystemExit as exc:
        assert exc.code == 72
    else:  # pragma: no cover
        raise AssertionError("expected pytest guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert "--summary-json" in argv
    assert argv[-5:] == [sys.executable, "-m", "pytest", "tests/test_one.py", "-q"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_PYTEST_OUTER_GUARD_REEXEC"] == "1"
    current_test_file = Path(env["MOLT_PYTEST_CURRENT_TEST_FILE"])
    assert (
        current_test_file.parent
        == pytest_memory_guard_bootstrap.outer_guard_summary_dir(env)
    )
    assert current_test_file.name.endswith("_current-test.json")


def test_pytest_startup_windows_handoff_waits_for_guard_child(monkeypatch) -> None:
    captured: dict[str, object] = {}

    class Completed:
        returncode = 77

    def fake_run(argv, *, env, check, creationflags=0, **kwargs):
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        captured["check"] = check
        captured["creationflags"] = creationflags
        captured["stdio"] = dict(kwargs)
        return Completed()

    def fake_execvpe(*_args):
        raise AssertionError("Windows pytest custody must not use os.execvpe")

    def fake_exit(code):
        raise SystemExit(code)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    # Complete the Windows simulation on every host: the flag is 0 off Windows.
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        pytest_memory_guard_bootstrap,
        CREATE_NEW_PROCESS_GROUP=0x00000200,
        run=fake_run,
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, execvpe=fake_execvpe
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "_exit", fake_exit)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        pytest_memory_guard_bootstrap,
        executable=sys.executable,
    )
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)
    monkeypatch.delenv("MOLT_PYTEST_OUTER_GUARD_REEXEC", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_pytest_memory_guard(
            orig_argv=[sys.executable, "-m", "pytest", "tests/test_one.py", "-q"],
            runtime_argv=["-m", "tests/test_one.py", "-q"],
        )
    except SystemExit as exc:
        assert exc.code == 77
    else:  # pragma: no cover
        raise AssertionError("expected Windows pytest guard handoff")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert argv[-5:] == [sys.executable, "-m", "pytest", "tests/test_one.py", "-q"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_PYTEST_OUTER_GUARD_REEXEC"] == "1"
    assert captured["check"] is False
    assert captured["creationflags"] & getattr(
        pytest_memory_guard_bootstrap.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0,
    )


def test_pytest_startup_windows_handoff_interrupt_exits_cleanly(monkeypatch) -> None:
    def fake_run(argv, *, env, check, creationflags=0, **kwargs):
        del argv, env, check, creationflags, kwargs
        raise KeyboardInterrupt

    def fake_exit(code):
        raise SystemExit(code)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        pytest_memory_guard_bootstrap,
        run=fake_run,
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, _exit=fake_exit
    )

    try:
        pytest_memory_guard_bootstrap.handoff_to_outer_guard(
            [sys.executable, "-m", "pytest"],
            {},
        )
    except SystemExit as exc:
        assert exc.code == 130
    else:  # pragma: no cover
        raise AssertionError("expected interrupted Windows handoff to exit cleanly")


def test_repo_test_script_startup_reexecs_under_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}
    script = REPO_ROOT / "tests" / "e2e" / "test_performance_guard.py"

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(74)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: False
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, execvpe=fake_execvpe
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        pytest_memory_guard_bootstrap,
        executable=sys.executable,
    )
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_repo_test_script_memory_guard(
            runtime_argv=[str(script), "--flag"],
        )
    except SystemExit as exc:
        assert exc.code == 74
    else:  # pragma: no cover
        raise AssertionError("expected repo test script guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert "--summary-json" in argv
    assert argv[-3:] == [sys.executable, str(script), "--flag"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC"] == "1"


def test_repo_test_module_startup_reexecs_under_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(76)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: False
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, execvpe=fake_execvpe
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        pytest_memory_guard_bootstrap,
        executable=sys.executable,
    )
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_repo_test_module_memory_guard(
            orig_argv=[
                sys.executable,
                "-X",
                "dev",
                "-m",
                "tests.test_memory_guard_wiring",
                "--flag",
            ],
        )
    except SystemExit as exc:
        assert exc.code == 76
    else:  # pragma: no cover
        raise AssertionError("expected repo test module guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert argv[-4:] == [
        sys.executable,
        "-m",
        "tests.test_memory_guard_wiring",
        "--flag",
    ]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC"] == "1"


def test_current_file_test_script_startup_uses_resolved_file(monkeypatch) -> None:
    captured: dict[str, object] = {}
    script = REPO_ROOT / "tests" / "e2e" / "test_performance_guard.py"

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(75)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: False
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, execvpe=fake_execvpe
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        pytest_memory_guard_bootstrap,
        executable=sys.executable,
    )
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)

    try:
        pytest_memory_guard_bootstrap.ensure_current_file_test_script_memory_guard(
            script,
            argv=("--flag",),
        )
    except SystemExit as exc:
        assert exc.code == 75
    else:  # pragma: no cover
        raise AssertionError("expected current-file test script guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[:2] == [sys.executable, str(REPO_ROOT / "tools" / "memory_guard.py")]
    assert argv[-3:] == [sys.executable, str(script), "--flag"]


def test_memory_guard_allocates_test_custody_env(
    tmp_path: Path,
) -> None:
    env = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard")}

    pytest_env = memory_guard.test_custody_launch_env(
        [sys.executable, "-m", "pytest", "tests/test_one.py"],
        environ=env,
    )
    assert pytest_env["MOLT_PYTEST_CURRENT_TEST_FILE"].startswith(str(tmp_path))

    module_env = memory_guard.test_custody_launch_env(
        [sys.executable, "-m", "tests.test_memory_guard_wiring"],
        environ=env,
    )
    assert module_env["MOLT_PYTEST_CURRENT_TEST_FILE"].startswith(str(tmp_path))

    script_env = memory_guard.test_custody_launch_env(
        [sys.executable, "tests/differential/basic/builtin_chr_ord.py"],
        environ=env,
        cwd=REPO_ROOT,
    )
    assert script_env["MOLT_PYTEST_CURRENT_TEST_FILE"].startswith(str(tmp_path))

    assert "MOLT_PYTEST_CURRENT_TEST_FILE" not in memory_guard.test_custody_launch_env(
        [sys.executable, "-c", "print('not a test')"],
        environ={},
    )


@pytest.mark.parametrize("selector", ["MOLT_EXT_ROOT", "MOLT_MEMORY_GUARD_STATE_ROOT"])
@pytest.mark.parametrize("worker", ["", "gw0"])
def test_custody_root_transition_keeps_parent_child_and_reader_on_one_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, selector: str, worker: str
) -> None:
    # Modules are already imported. The parent observer still has the old root
    # when DX supplies the command's effective external/queue environment.
    monkeypatch.setenv(
        "MOLT_PYTEST_CURRENT_TEST_FILE",
        os.environ.get("MOLT_PYTEST_CURRENT_TEST_FILE", ""),
    )
    root_keys = (
        "MOLT_EXT_ROOT",
        "MOLT_EXTERNAL_ARTIFACT_ROOTS",
        "MOLT_MEMORY_GUARD_STATE_ROOT",
    )
    for key in root_keys:
        monkeypatch.delenv(key, raising=False)
    old_state = tmp_path / "old" / "memory_guard"
    monkeypatch.setenv("MOLT_MEMORY_GUARD_STATE_ROOT", str(old_state))
    old_path = pytest_memory_guard_bootstrap.current_test_file_path(pid=701)
    assert old_path.parent == old_state.parent / "pytest-memory-guard"
    selected = tmp_path / "effective"
    state = (
        selected
        if selector == "MOLT_MEMORY_GUARD_STATE_ROOT"
        else selected / "tmp" / "memory_guard"
    )
    child_env = memory_guard.test_custody_launch_env(
        [sys.executable, "-m", "pytest", "tests/test_memory_guard_wiring.py"],
        environ={
            selector: str(selected),
            "MOLT_PYTEST_CURRENT_TEST_FILE": str(old_path),
            "PYTEST_XDIST_WORKER": worker,
        },
        cwd=REPO_ROOT,
    )
    current_path = Path(child_env["MOLT_PYTEST_CURRENT_TEST_FILE"])
    assert current_path.parent == state.parent / "pytest-memory-guard"
    assert current_path != old_path
    token, marker = memory_guard._write_active_guard_marker(
        701, command=("pytest",), cwd=REPO_ROOT, environ=child_env
    )
    assert marker.parent == state / "active"
    child_env.update(
        {
            "MOLT_MEMORY_GUARD_PID": "701",
            "MOLT_MEMORY_GUARD_TOKEN": token,
            "MOLT_MEMORY_GUARD_MARKER": str(marker),
        }
    )
    assert pytest_memory_guard_bootstrap._active_guard_marker_valid(
        child_env, guard_pid=701
    )

    class Item:
        nodeid = "tests/test_memory_guard_wiring.py::effective_environment"

    with monkeypatch.context() as child:
        for key in root_keys:
            child.delenv(key, raising=False)
        for key, value in child_env.items():
            child.setenv(key, value)
        assert (
            pytest_memory_guard_bootstrap.outer_guard_summary_dir()
            == current_path.parent
        )
        assert (
            pytest_memory_guard_bootstrap.install_pytest_current_test_file_env()
            == current_path
        )
        pytest_memory_guard_config_plugin.pytest_runtest_call(Item())
    # The reader is back in the old ambient environment. It must still read
    # exactly the path that the parent allocated and the child wrote.
    record = memory_guard._pytest_current_test_file_payload(child_env, samples={})
    assert record is not None
    assert record["path"] == str(current_path)
    payload = record["worker_records"][0]["payload"] if worker else record["payload"]
    assert payload["nodeid"] == Item.nodeid
    assert not old_path.exists()


@pytest.mark.parametrize("invocation", ["pytest", "module", "script"])
def test_outer_guard_handoff_uses_supplied_environment_for_every_custody_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, invocation: str
) -> None:
    state = tmp_path / "selected" / "memory_guard"
    env = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(state)}
    monkeypatch.setenv(
        "MOLT_PYTEST_CURRENT_TEST_FILE",
        os.environ.get("MOLT_PYTEST_CURRENT_TEST_FILE", ""),
    )
    monkeypatch.setenv(
        "MOLT_MEMORY_GUARD_STATE_ROOT", str(tmp_path / "observer" / "memory_guard")
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "outer_memory_guard_active", lambda _env: False
    )
    captured = {}

    def handoff(argv, child_env):
        captured.update(argv=argv, env=child_env)
        raise SystemExit(71)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "handoff_to_outer_guard", handoff
    )
    with pytest.raises(SystemExit, match="71"):
        if invocation == "pytest":
            pytest_memory_guard_bootstrap.ensure_pytest_memory_guard(
                pytest_args=("tests/test_memory_guard_wiring.py",), environ=env
            )
        elif invocation == "module":
            pytest_memory_guard_bootstrap.ensure_repo_test_module_memory_guard(
                orig_argv=(sys.executable, "-m", "tests.test_memory_guard_wiring"),
                environ=env,
            )
        else:
            pytest_memory_guard_bootstrap.ensure_repo_test_script_memory_guard(
                runtime_argv=(str(Path(__file__).resolve()),), environ=env
            )
    argv = captured["argv"]
    summary = Path(argv[argv.index("--summary-json") + 1])
    assert summary.parent == state.parent / "pytest-memory-guard"
    assert captured["env"]["MOLT_MEMORY_GUARD_STATE_ROOT"] == str(state)
    if invocation == "pytest":
        assert (
            Path(captured["env"]["MOLT_PYTEST_CURRENT_TEST_FILE"]).parent
            == summary.parent
        )


@pytest.mark.parametrize("kind", ["pytest", "test-custody"])
def test_shared_pytest_custody_path_policy_keeps_roles_and_parent_selection(
    tmp_path: Path, kind: str
) -> None:
    env = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard")}
    root = tmp_path / "pytest-memory-guard"
    fallback = root / f"{kind}-91_current-test.json"
    for raw in (None, str(tmp_path / "outside.json"), "../outside.json"):
        assert (
            memory_guard_paths.canonical_pytest_current_test_file_path(
                tmp_path, raw, fallback_kind=kind, fallback_pid=91, environ=env
            )
            == fallback
        )
    selected = root / "parent-selected.json"
    for raw in (str(selected), "pytest-memory-guard/parent-selected.json"):
        assert (
            memory_guard_paths.canonical_pytest_current_test_file_path(
                tmp_path, raw, fallback_kind=kind, fallback_pid=92, environ=env
            )
            == selected
        )
    assert (
        memory_guard_paths.pytest_custody_artifact_path(
            tmp_path, " Test / Module ", " Outer Guard ", pid=93, environ=env
        )
        == root / "test---module-93_outer-guard.json"
    )
    assert not memory_guard_paths.pytest_custody_path_is_canonical(
        tmp_path, root / ".." / "outside.json", environ=env
    )


def test_pytest_custody_containment_resolves_final_root_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias = tmp_path / "pytest-memory-guard"
    target = tmp_path / "resolved-evidence"
    env = {"MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard")}
    original_resolve = Path.resolve

    def resolve_alias(path: Path, *args, **kwargs) -> Path:
        if path.is_relative_to(alias):
            return target / path.relative_to(alias)
        return original_resolve(path, *args, **kwargs)

    # Model a final-directory symlink/junction without host-specific privileges.
    monkeypatch.setattr(Path, "resolve", resolve_alias)
    for path in (alias / "current.json", target / "current.json"):
        assert memory_guard_paths.pytest_custody_path_is_canonical(
            tmp_path, path, environ=env
        )
        assert (
            memory_guard_paths.canonical_pytest_current_test_file_path(
                tmp_path, str(path), fallback_kind="pytest", environ=env
            )
            == target / "current.json"
        )


def test_repo_test_script_startup_ignores_non_test_scripts(tmp_path: Path) -> None:
    script = tmp_path / "script.py"
    script.write_text("print('not a repo test')\n", encoding="utf-8")

    assert (
        pytest_memory_guard_bootstrap.repo_test_script_invocation_args(
            runtime_argv=[str(script)]
        )
        is None
    )


def test_direct_executable_test_audit_requires_path_local_sitecustomize(
    tmp_path: Path,
) -> None:
    test_dir = tmp_path / "tests" / "e2e"
    test_dir.mkdir(parents=True)
    test_path = test_dir / "test_direct.py"
    test_path.write_text(
        "if __name__ == '__main__':\n    pass\n",
        encoding="utf-8",
    )

    missing = check_memory_guard_wiring._audit_direct_executable_test_guards(tmp_path)

    assert len(missing) == 1
    assert missing[0].path == "tests/e2e/test_direct.py"
    assert missing[0].line == 1

    (test_dir / "sitecustomize.py").write_text(
        "from molt.pytest_memory_guard_bootstrap import ensure_python_test_memory_guard\n",
        encoding="utf-8",
    )

    assert (
        check_memory_guard_wiring._audit_direct_executable_test_guards(tmp_path) == ()
    )


def test_pytest_startup_detects_console_script_and_module_invocations() -> None:
    assert pytest_memory_guard_bootstrap.python_pytest_invocation_args(
        orig_argv=[sys.executable, "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert (
        pytest_memory_guard_bootstrap.python_pytest_invocation_args(
            orig_argv=[sys.executable, "-m", "pytest"],
            runtime_argv=["-m"],
        )
        == ()
    )
    assert pytest_memory_guard_bootstrap.python_pytest_invocation_args(
        orig_argv=[sys.executable, "-u", "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert pytest_memory_guard_bootstrap.python_pytest_invocation_args(
        orig_argv=[sys.executable, "-X", "dev", "-I", "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert pytest_memory_guard_bootstrap.python_pytest_invocation_args(
        orig_argv=[sys.executable, "-S", "-Xdev", "-m", "pytest", "tests"],
        runtime_argv=["-m", "tests"],
    ) == ("tests",)
    assert pytest_memory_guard_bootstrap.python_pytest_invocation_args(
        orig_argv=[sys.executable, str(REPO_ROOT / ".venv" / "bin" / "pytest"), "-q"],
        runtime_argv=[str(REPO_ROOT / ".venv" / "bin" / "pytest"), "-q"],
    ) == ("-q",)
    assert (
        pytest_memory_guard_bootstrap.python_pytest_invocation_args(
            orig_argv=[sys.executable, "tools/memory_guard.py"],
            runtime_argv=["tools/memory_guard.py"],
        )
        is None
    )


def test_pytest_initial_conftest_hook_reexecs_from_pytest_args(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_execvpe(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(73)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: False
    )
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, execvpe=fake_execvpe
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        pytest_memory_guard_bootstrap,
        executable=sys.executable,
    )
    monkeypatch.delenv("MOLT_MEMORY_GUARD_ACTIVE", raising=False)
    monkeypatch.delenv("MOLT_PYTEST_OUTER_GUARD_REEXEC", raising=False)

    try:
        pytest_memory_guard_bootstrap.pytest_load_initial_conftests(
            object(),
            object(),
            ["tests/test_one.py", "-q"],
        )
    except SystemExit as exc:
        assert exc.code == 73
    else:  # pragma: no cover
        raise AssertionError("expected pytest hook guard re-exec")

    argv = captured["argv"]
    assert isinstance(argv, list)
    assert argv[-5:] == [sys.executable, "-m", "pytest", "tests/test_one.py", "-q"]
    env = captured["env"]
    assert isinstance(env, dict)
    assert env["MOLT_PYTEST_OUTER_GUARD_REEXEC"] == "1"


def test_windows_pytest_tempdir_patch_keeps_numbered_dirs_readable(
    monkeypatch,
) -> None:
    import _pytest.pathlib as pytest_pathlib
    import _pytest.tmpdir as pytest_tmpdir

    seen_modes: list[int] = []

    def fake_make_numbered_dir(root, prefix, mode=0o700):
        del root, prefix
        seen_modes.append(mode)
        return "made"

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setattr(pytest_pathlib, "make_numbered_dir", fake_make_numbered_dir)
    monkeypatch.setattr(pytest_tmpdir, "make_numbered_dir", fake_make_numbered_dir)

    assert pytest_memory_guard_bootstrap.install_windows_pytest_tempdir_mode_patch()
    assert pytest_pathlib.make_numbered_dir("root", "pytest-", mode=0o700) == "made"
    assert pytest_tmpdir.make_numbered_dir("root", "pytest-", mode=0o777) == "made"
    assert seen_modes == [0o755, 0o777]
    assert not pytest_memory_guard_bootstrap.install_windows_pytest_tempdir_mode_patch()


@pytest.mark.skipif(
    os.name != "nt",
    reason="instantiates a pathlib.WindowsPath subclass, which CPython refuses on POSIX",
)
def test_windows_pytest_tempdir_patch_keeps_xdist_given_basetemp_readable(
    monkeypatch, tmp_path
) -> None:
    import _pytest.tmpdir as pytest_tmpdir

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setattr(pytest_tmpdir, "Path", Path)
    pytest_memory_guard_bootstrap.install_windows_pytest_tempdir_mode_patch()
    factory = pytest_tmpdir.TempPathFactory(
        given_basetemp=tmp_path / "popen-gw0",
        retention_count=0,
        retention_policy="all",
        trace=lambda *args: None,
        _ispytest=True,
    )
    root = factory.getbasetemp()
    child = root / "test-fixture"
    child.mkdir(mode=0o700)
    (child / "receipt.txt").write_text("readable", encoding="utf-8")
    assert list(root.iterdir()) == [child]
    assert (child / "receipt.txt").read_text(encoding="utf-8") == "readable"


def test_pytest_cache_dir_arg_uses_the_artifact_root_control_state(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))
    args = ["tests/test_one.py", "-q"]

    assert pytest_memory_guard_bootstrap.install_pytest_cache_dir_arg(args)
    assert args[-2:] == [
        "-o",
        f"cache_dir={tmp_path.resolve() / 'artifact-root' / 'tmp' / 'pytest-cache'}",
    ]
    assert not pytest_memory_guard_bootstrap.install_pytest_cache_dir_arg(args)


def test_pytest_cache_dir_never_lands_in_the_checkout(monkeypatch) -> None:
    # A plain clone's artifact root is the clone itself; its cache must move
    # out of the tree on every platform.
    monkeypatch.setenv("MOLT_EXT_ROOT", str(pytest_memory_guard_bootstrap.ROOT))

    cache = pytest_memory_guard_bootstrap.guarded_pytest_cache_dir()

    root = pytest_memory_guard_bootstrap.ROOT.resolve()
    assert root not in (cache, *cache.parents)


def test_pytest_cache_dir_arg_preserves_explicit_cache_policy() -> None:
    explicit = ["-o", "cache_dir=custom-cache"]
    disabled = ["-p", "no:cacheprovider"]

    assert not pytest_memory_guard_bootstrap.install_pytest_cache_dir_arg(explicit)
    assert explicit == ["-o", "cache_dir=custom-cache"]
    assert not pytest_memory_guard_bootstrap.install_pytest_cache_dir_arg(disabled)
    assert disabled == ["-p", "no:cacheprovider"]


def test_pytest_cache_dir_config_uses_the_artifact_root_control_state(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))

    class Config:
        _inicfg = {}
        _inicache = {"cache_dir": "old-cache"}

    assert pytest_memory_guard_bootstrap.install_pytest_cache_dir_config(
        Config,
        ["tests/test_one.py", "-q"],
    )
    value = Config._inicfg["cache_dir"]
    assert value.value == str(
        tmp_path.resolve() / "artifact-root" / "tmp" / "pytest-cache"
    )
    assert value.origin == "override"
    assert "cache_dir" not in Config._inicache


def test_pytest_user_temp_root_matches_pytest_tmpdir_authority(tmp_path) -> None:
    from _pytest.tmpdir import get_user

    user = get_user() or "unknown"

    assert pytest_memory_guard_bootstrap._pytest_user_temp_root(tmp_path) == (
        tmp_path / f"pytest-of-{user}"
    )


@pytest.fixture
def scratch_lease(monkeypatch, tmp_path):
    token = "a" * 32
    root = tmp_path / "artifact-root"
    state = root / "tmp" / "memory_guard"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(root))
    monkeypatch.setenv("MOLT_MEMORY_GUARD_STATE_ROOT", str(state))
    monkeypatch.setenv("MOLT_MEMORY_GUARD_TOKEN", token)
    monkeypatch.setenv("MOLT_MEMORY_GUARD_MARKER", str(state / "active" / "guard.json"))
    lease = temporary_artifacts.acquire_guard_scratch(REPO_ROOT, os.environ)
    monkeypatch.setenv(temporary_artifacts.SCRATCH_ENV, str(lease.target))
    try:
        yield lease
    finally:
        lease.release()


def test_windows_pytest_custody_roots_prepare_readable_defaults(
    monkeypatch, tmp_path, scratch_lease
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))
    monkeypatch.delenv("PYTEST_DEBUG_TEMPROOT", raising=False)

    assert pytest_memory_guard_bootstrap.install_pytest_custody_roots()
    temproot = Path(os.environ["PYTEST_DEBUG_TEMPROOT"])
    assert temproot.parent == tmp_path / "artifact-root" / "tmp"
    assert temproot.name.startswith("pt-")
    assert temproot == scratch_lease.target
    assert len(temproot.name) <= 16
    assert temproot.is_dir()
    assert any(temproot.iterdir())
    assert (
        tmp_path / "artifact-root" / "tmp" / "pytest-cache" / "v" / "cache"
    ).is_dir()


def test_pytest_temp_root_reuses_short_parent_owned_allocation(
    monkeypatch, tmp_path, scratch_lease
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    first = pytest_memory_guard_bootstrap.guarded_pytest_temp_root()
    second = pytest_memory_guard_bootstrap.guarded_pytest_temp_root()
    assert first == second == scratch_lease.target
    for path in (first, second):
        assert path.parent == tmp_path / "artifact-root" / "tmp"
        assert path.is_dir()
        assert len(path.name) <= 16


def test_windows_native_proof_scratch_layout_retains_linker_path_budget() -> None:
    from pathlib import PureWindowsPath

    # Replays the CI2 quote build-script output shape that failed with LNK1104.
    # Long-path-aware Python does not make MSVC's output paths long-path-aware.
    action = (REPO_ROOT / ".github/actions/setup-project/action.yml").read_text(
        encoding="utf-8"
    )
    setup = action.split("- name: Configure verified ephemeral custody", 1)[1]
    template = next(
        line.strip().split('"')[1]
        for line in setup.splitlines()
        if line.strip().startswith("$name = ")
    )
    for key, value in {
        "GITHUB_RUN_ID": "34386557914",
        "GITHUB_RUN_ATTEMPT": "1",
        "RUNNER_OS": "Windows",
        "RUNNER_ARCH": "X64",
    }.items():
        template = template.replace("${env:" + key + "}", value)
    temp_name = "pt-abcdefgh"
    output = PureWindowsPath("D:/a/_temp", template, "tmp", temp_name) / (
        "pytest-of-runneradmin/pytest-0/test_guarded_identity_timeout_0/"
        "proof-supervisor-target/release/build/quote-529389acd85f5c85/"
        "build_script_build-529389acd85f5c85.exe"
    )
    assert len(str(output)) < 240


def test_windows_pytest_custody_roots_preserve_explicit_temproot(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))
    explicit = tmp_path / "explicit-temproot"
    monkeypatch.setenv("PYTEST_DEBUG_TEMPROOT", str(explicit))

    assert not pytest_memory_guard_bootstrap.install_pytest_custody_roots()
    assert os.environ["PYTEST_DEBUG_TEMPROOT"] == str(explicit)
    assert explicit.is_dir()
    assert any(explicit.iterdir())
    assert (
        tmp_path / "artifact-root" / "tmp" / "pytest-cache" / "v" / "cache"
    ).is_dir()


def test_pytest_startup_rejects_hook_disabling_flags() -> None:
    for args in (
        ("--noconftest",),
        ("--confcutdir", str(REPO_ROOT.parent)),
        (f"--confcutdir={REPO_ROOT.parent}",),
        ("-p", "no:molt.pytest_memory_guard_config_plugin"),
        ("-pno:molt.pytest_memory_guard_bootstrap",),
    ):
        try:
            pytest_memory_guard_bootstrap.validate_pytest_guardable_args(args)
        except SystemExit:
            pass
        else:  # pragma: no cover
            raise AssertionError(f"expected pytest args to be rejected: {args}")


def test_pytest_startup_rejects_hook_disabling_pytest_addopts() -> None:
    for env in (
        {"PYTEST_ADDOPTS": "-p no:molt.pytest_memory_guard_config_plugin"},
        {"PYTEST_ADDOPTS": "-pno:molt.pytest_memory_guard_bootstrap"},
    ):
        try:
            pytest_memory_guard_bootstrap.validate_pytest_guardable_env(env)
        except SystemExit:
            pass
        else:  # pragma: no cover
            raise AssertionError(f"expected pytest env to be rejected: {env}")


def test_pytest_startup_rejects_malformed_pytest_addopts() -> None:
    try:
        pytest_memory_guard_bootstrap.validate_pytest_guardable_env(
            {"PYTEST_ADDOPTS": "'unterminated"}
        )
    except SystemExit as exc:
        assert "Invalid PYTEST_ADDOPTS" in str(exc)
    else:  # pragma: no cover
        raise AssertionError("expected malformed PYTEST_ADDOPTS to be rejected")


def test_pytest_autoload_disable_requires_explicit_guard_config_plugin() -> None:
    pytest_memory_guard_bootstrap.validate_pytest_guardable_env(
        {"PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"}
    )
    try:
        pytest_memory_guard_bootstrap.validate_pytest_guardable_env(
            {"PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"},
            args=("-c", str(REPO_ROOT / "tmp" / "pytest.ini")),
        )
    except SystemExit:
        pass
    else:  # pragma: no cover
        raise AssertionError("expected unsafe autoload-disabled config to be rejected")
    pytest_memory_guard_bootstrap.validate_pytest_guardable_env(
        {"PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"},
        args=("-p", "molt.pytest_memory_guard_config_plugin"),
    )


def test_pytest_startup_allows_repo_confcutdir() -> None:
    pytest_memory_guard_bootstrap.validate_pytest_guardable_args(
        ("--confcutdir", str(REPO_ROOT))
    )
    pytest_memory_guard_bootstrap.validate_pytest_guardable_args(
        (f"--confcutdir={REPO_ROOT / 'tests'}",)
    )


def test_outer_memory_guard_fails_closed_on_forged_or_unsampled_marker(
    monkeypatch,
) -> None:
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {"MOLT_MEMORY_GUARD_ACTIVE": "1", "MOLT_MEMORY_GUARD_PID": "123"}
        )
        is False
    )


def test_outer_memory_guard_requires_live_repo_memory_guard_ancestor(
    monkeypatch,
) -> None:
    samples = {
        100: memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            rss_kb=1,
            command=f"{sys.executable} {REPO_ROOT / 'tools' / 'memory_guard.py'} --",
        ),
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=100,
            rss_kb=1,
            command="uv run --python 3.12 pytest",
        ),
        300: memory_guard.ProcessSample(
            pid=300,
            ppid=200,
            rss_kb=1,
            command=f"{sys.executable} -m pytest",
        ),
    }

    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, getpid=lambda: 300
    )

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {"MOLT_MEMORY_GUARD_ACTIVE": "1", "MOLT_MEMORY_GUARD_PID": "100"}
        )
        is True
    )


def test_outer_memory_guard_accepts_live_marker_when_parent_chain_breaks(
    monkeypatch,
    tmp_path: Path,
) -> None:
    guard_pid = 100
    current_pid = 300
    token = "a" * 32
    marker_dir = tmp_path / "active"
    marker_dir.mkdir()
    marker = marker_dir / f"guard-{guard_pid}-{token}.json"
    marker.write_text(
        json.dumps(
            {
                "pid": guard_pid,
                "token": token,
                "schema_version": 2,
                "guard_process": {"pid": guard_pid, "started_at_ns": 100},
                "child_launch_state": "recorded",
                "child_process": {"pid": 200, "started_at_ns": 200},
                "path": str(REPO_ROOT / "tools" / "memory_guard.py"),
                "status": "child_running",
            }
        ),
        encoding="utf-8",
    )
    samples = {
        guard_pid: memory_guard.ProcessSample(
            pid=guard_pid,
            ppid=1,
            rss_kb=1,
            command="python guarded-wrapper-with-redacted-argv",
        ),
        current_pid: memory_guard.ProcessSample(
            pid=current_pid,
            ppid=999,
            rss_kb=1,
            command=f"{sys.executable} -m pytest",
        ),
    }

    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, getpid=lambda: current_pid
    )

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {
                "MOLT_MEMORY_GUARD_ACTIVE": "1",
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path),
                "MOLT_MEMORY_GUARD_PID": str(guard_pid),
                pytest_memory_guard_bootstrap.ACTIVE_GUARD_TOKEN_ENV: token,
                pytest_memory_guard_bootstrap.ACTIVE_GUARD_MARKER_ENV: str(marker),
            }
        )
        is True
    )


def test_outer_memory_guard_accepts_live_marker_without_process_sample(
    monkeypatch,
    tmp_path: Path,
) -> None:
    guard_pid = 100
    token = "a" * 32
    marker_dir = tmp_path / "active"
    marker_dir.mkdir()
    marker = marker_dir / f"guard-{guard_pid}-{token}.json"
    marker.write_text(
        json.dumps(
            {
                "pid": guard_pid,
                "token": token,
                "schema_version": 2,
                "guard_process": {"pid": guard_pid, "started_at_ns": 100},
                "child_launch_state": "recorded",
                "child_process": {"pid": 200, "started_at_ns": 200},
                "path": str(REPO_ROOT / "tools" / "memory_guard.py"),
                "status": "child_running",
            }
        ),
        encoding="utf-8",
    )

    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {
                "MOLT_MEMORY_GUARD_ACTIVE": "1",
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path),
                "MOLT_MEMORY_GUARD_PID": str(guard_pid),
                pytest_memory_guard_bootstrap.ACTIVE_GUARD_TOKEN_ENV: token,
                pytest_memory_guard_bootstrap.ACTIVE_GUARD_MARKER_ENV: str(marker),
            }
        )
        is True
    )


def test_outer_memory_guard_rejects_terminal_active_marker(
    monkeypatch,
    tmp_path: Path,
) -> None:
    guard_pid = 100
    token = "a" * 32
    marker_dir = tmp_path / "active"
    marker_dir.mkdir()
    marker = marker_dir / f"guard-{guard_pid}-{token}.json"
    marker.write_text(
        json.dumps(
            {
                "pid": guard_pid,
                "token": token,
                "schema_version": 2,
                "guard_process": {"pid": guard_pid, "started_at_ns": 100},
                "child_launch_state": "recorded",
                "child_process": {"pid": 200, "started_at_ns": 200},
                "path": str(REPO_ROOT / "tools" / "memory_guard.py"),
                "status": "completed",
            }
        ),
        encoding="utf-8",
    )

    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {
                "MOLT_MEMORY_GUARD_ACTIVE": "1",
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path),
                "MOLT_MEMORY_GUARD_PID": str(guard_pid),
                pytest_memory_guard_bootstrap.ACTIVE_GUARD_TOKEN_ENV: token,
                pytest_memory_guard_bootstrap.ACTIVE_GUARD_MARKER_ENV: str(marker),
            }
        )
        is False
    )


def test_outer_memory_guard_accepts_proof_queue_custody_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    summary_dir = tmp_path / "pytest-memory-guard"
    summary_dir.mkdir()
    current_test_path = summary_dir / "queue-current-test.json"

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {
                pytest_memory_guard_bootstrap.PROOF_QUEUE_ENV: "1",
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
                pytest_memory_guard_bootstrap.PROOF_QUEUE_RUN_ID_ENV: "run-1",
                pytest_memory_guard_bootstrap.PROOF_QUEUE_DB_ENV: str(
                    tmp_path / "proof_queue.sqlite3"
                ),
                pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV: str(
                    current_test_path
                ),
            }
        )
        is True
    )


def test_outer_memory_guard_rejects_proof_queue_custody_outside_pytest_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    summary_dir = tmp_path / "pytest-memory-guard"
    summary_dir.mkdir()

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {
                pytest_memory_guard_bootstrap.PROOF_QUEUE_ENV: "1",
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
                pytest_memory_guard_bootstrap.PROOF_QUEUE_RUN_ID_ENV: "run-1",
                pytest_memory_guard_bootstrap.PROOF_QUEUE_DB_ENV: str(
                    tmp_path / "proof_queue.sqlite3"
                ),
                pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV: str(
                    tmp_path / "outside-current-test.json"
                ),
            }
        )
        is False
    )


def test_outer_memory_guard_rejects_proof_queue_custody_without_sqlite_db(
    monkeypatch,
    tmp_path: Path,
) -> None:
    summary_dir = tmp_path / "pytest-memory-guard"
    summary_dir.mkdir()
    current_test_path = summary_dir / "queue-current-test.json"

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {
                pytest_memory_guard_bootstrap.PROOF_QUEUE_ENV: "1",
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
                pytest_memory_guard_bootstrap.PROOF_QUEUE_RUN_ID_ENV: "run-1",
                pytest_memory_guard_bootstrap.PROOF_QUEUE_DB_ENV: str(
                    tmp_path / "proof_queue.json"
                ),
                pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV: str(
                    current_test_path
                ),
            }
        )
        is False
    )


def test_pytest_current_test_hooks_write_live_identity(
    monkeypatch,
    tmp_path: Path,
) -> None:
    current_test_path = tmp_path / "pytest-memory-guard" / "current-test.json"
    # The aggregate record path; an inherited xdist worker id would select
    # that worker's own record (covered by the per-worker test below).
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)
    monkeypatch.setenv("MOLT_MEMORY_GUARD_STATE_ROOT", str(tmp_path / "memory_guard"))
    monkeypatch.setenv(
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV,
        str(current_test_path),
    )
    monkeypatch.setenv(
        "PYTEST_CURRENT_TEST",
        "tests/test_memory_guard_wiring.py::test_unit (call)",
    )

    class Item:
        nodeid = "tests/test_memory_guard_wiring.py::test_unit"

    pytest_memory_guard_bootstrap.pytest_runtest_call(Item())

    payload = json.loads(current_test_path.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 1
    assert payload["phase"] == "call"
    assert payload["nodeid"] == "tests/test_memory_guard_wiring.py::test_unit"
    assert payload["pytest_current_test"].endswith("test_unit (call)")


def test_pytest_current_test_env_is_forced_to_canonical_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    canonical_root = tmp_path / "canonical" / "pytest-memory-guard"
    outside_path = tmp_path / "outside" / "current-test.json"
    monkeypatch.setenv(
        "MOLT_MEMORY_GUARD_STATE_ROOT", str(canonical_root.parent / "memory_guard")
    )
    monkeypatch.setenv(
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV,
        str(outside_path),
    )

    installed = pytest_memory_guard_bootstrap.install_pytest_current_test_file_env()

    assert installed.parent == canonical_root
    assert pytest_memory_guard_bootstrap.os.environ[
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV
    ] == str(installed)


def test_pytest_current_test_xdist_writes_per_worker_sidecar(
    monkeypatch,
    tmp_path: Path,
) -> None:
    aggregate_path = tmp_path / "pytest-memory-guard" / "pytest-guard_current-test.json"
    monkeypatch.setenv("MOLT_MEMORY_GUARD_STATE_ROOT", str(tmp_path / "memory_guard"))
    monkeypatch.setenv(
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV,
        str(aggregate_path),
    )
    monkeypatch.setenv("PYTEST_XDIST_WORKER", "gw1")

    class Item:
        nodeid = "tests/test_memory_guard_wiring.py::test_xdist_unit"

    pytest_memory_guard_bootstrap.pytest_runtest_call(Item())

    assert not aggregate_path.exists()
    records = list(aggregate_path.with_name(f"{aggregate_path.name}.d").glob("*.json"))
    assert len(records) == 1
    payload = json.loads(records[0].read_text(encoding="utf-8"))
    assert payload["aggregate_path"] == str(aggregate_path)
    assert payload["record_path"] == str(records[0])
    assert payload["xdist_worker"] == "gw1"
    assert payload["nodeid"] == "tests/test_memory_guard_wiring.py::test_xdist_unit"


def test_pytest_current_test_writer_ignores_test_monkeypatched_os_replace(
    monkeypatch,
    tmp_path: Path,
) -> None:
    current_test_path = tmp_path / "pytest-memory-guard" / "current-test.json"
    # The aggregate record path; an inherited xdist worker id would select
    # that worker's own record (covered by the per-worker test below).
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)
    monkeypatch.setenv("MOLT_MEMORY_GUARD_STATE_ROOT", str(tmp_path / "memory_guard"))
    monkeypatch.setenv(
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV,
        str(current_test_path),
    )

    def forbidden_replace(_src: object, _dst: object) -> None:
        raise AssertionError("guard custody must not use monkeypatched os.replace")

    install_module_view(
        monkeypatch, "os", os, pytest_memory_guard_bootstrap, replace=forbidden_replace
    )

    class Item:
        nodeid = "tests/test_memory_guard_wiring.py::test_unit"

    pytest_memory_guard_bootstrap.pytest_runtest_call(Item())

    payload = json.loads(current_test_path.read_text(encoding="utf-8"))
    assert payload["phase"] == "call"
    assert payload["nodeid"] == "tests/test_memory_guard_wiring.py::test_unit"


def test_pytest_current_test_writer_retries_windows_atomic_replace(
    monkeypatch,
    tmp_path: Path,
) -> None:
    current_test_path = tmp_path / "pytest-memory-guard" / "current-test.json"
    # The aggregate record path; an inherited xdist worker id would select
    # that worker's own record (covered by the per-worker test below).
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)
    monkeypatch.setenv("MOLT_MEMORY_GUARD_STATE_ROOT", str(tmp_path / "memory_guard"))
    monkeypatch.setenv(
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV,
        str(current_test_path),
    )
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "_is_windows_process_model",
        lambda: True,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.time, "sleep", lambda _s: None)

    calls = 0
    original_replace = pytest_memory_guard_bootstrap._ATOMIC_REPLACE

    def flaky_replace(src: Path, dst: Path) -> None:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise PermissionError(errno.EACCES, "Access is denied")
        original_replace(src, dst)

    monkeypatch.setattr(pytest_memory_guard_bootstrap, "_ATOMIC_REPLACE", flaky_replace)

    class Item:
        nodeid = "tests/test_memory_guard_wiring.py::test_unit"

    pytest_memory_guard_bootstrap.pytest_runtest_call(Item())

    assert calls == 2
    payload = json.loads(current_test_path.read_text(encoding="utf-8"))
    assert payload["phase"] == "call"
    assert payload["nodeid"] == "tests/test_memory_guard_wiring.py::test_unit"


def test_pytest_immediate_failures_register_once_and_only_format_failed_reports(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from types import SimpleNamespace

    manager = pytest.PytestPluginManager()
    events: list[object] = []

    class Terminal:
        def write_sep(self, separator: str, title: str) -> None:
            events.append((separator, title))

        def _outrep_summary(self, report: object) -> None:
            events.append(report)

        def flush(self) -> None:
            events.append("flushed")

    manager.register(Terminal(), "terminalreporter")
    config = SimpleNamespace(
        pluginmanager=manager, option=SimpleNamespace(tbstyle="auto")
    )
    # An inherited environment marker is not evidence that this config is a
    # worker. Real xdist supplies config.workerinput on its worker instance.
    monkeypatch.setenv("PYTEST_XDIST_WORKER", "inherited-worker")
    pytest_memory_guard_config_plugin.pytest_configure(config)
    pytest_memory_guard_bootstrap.pytest_configure(config)
    reporter = manager.getplugin("molt-immediate-pytest-failures")
    assert reporter is not None
    manager.check_pending()
    for outcome in ("passed", "skipped"):
        report = SimpleNamespace(failed=False, outcome=outcome, when="call")
        reporter.pytest_runtest_logreport(report)
        reporter.pytest_collectreport(report)
    assert events == []
    for phase in ("setup", "call", "teardown", "collect"):
        report = SimpleNamespace(
            failed=True, when=phase, nodeid="test_probe.py::test_original"
        )
        # xdist annotates test reports, but its collection reports have no
        # worker provenance. Preserve only the provenance actually supplied.
        origin = ""
        if phase != "collect":
            report.worker_id = "gw0"
            origin = " [gw0]"
        if phase == "collect":
            reporter.pytest_collectreport(report)
        else:
            reporter.pytest_runtest_logreport(report)
        assert events[-3:] == [
            (
                "=",
                f"Molt immediate pytest failure{origin}: test_probe.py::test_original ({phase})",
            ),
            report,
            "flushed",
        ]
    assert len(events) == 12
    worker_manager = pytest.PytestPluginManager()
    pytest_memory_guard_bootstrap.pytest_configure(
        SimpleNamespace(pluginmanager=worker_manager, workerinput={})
    )
    assert worker_manager.getplugin("molt-immediate-pytest-failures") is None


@pytest.mark.parametrize(
    ("workers", "abrupt", "tbstyle", "showcapture"),
    [
        (0, True, "auto", "all"),
        (1, True, "auto", "all"),
        (0, False, "auto", "all"),
        (0, True, "no", "all"),
        (0, True, "short", "no"),
    ],
    ids=(
        "serial-abrupt",
        "xdist-controller-abrupt",
        "normal-pytest-outcome",
        "traceback-suppressed-abrupt",
        "capture-suppressed-abrupt",
    ),
)
def test_pytest_failure_details_survive_later_abrupt_exit(
    tmp_path: Path,
    workers: int,
    abrupt: bool,
    tbstyle: str,
    showcapture: str,
) -> None:
    """Actual pytest reports must reach output before a later session death."""
    from tests.process_guard_common import run_guarded_test_process

    fixture = tmp_path / "failure-probe"
    fixture.mkdir()
    finished = fixture / "session-finished"
    (fixture / "conftest.py").write_text(
        "import os\nfrom pathlib import Path\n"
        "pytest_plugins = ['tests.conftest']\n"
        "controller_abort = False\nfailed_seen = False\n"
        "def pytest_configure(config):\n"
        f"    global controller_abort\n    controller_abort = {abrupt!r} and "
        "not hasattr(config, 'workerinput') and bool(getattr(config.option, 'numprocesses', 0))\n"
        "def pytest_runtest_logreport(report):\n"
        "    global failed_seen\n"
        "    if report.failed:\n        failed_seen = True\n"
        "def pytest_runtest_logstart(nodeid, location):\n"
        "    if controller_abort and failed_seen and nodeid.endswith('::test_later_exit'):\n"
        "        os._exit(86)\n"
        "def pytest_sessionfinish(session, exitstatus):\n"
        "    if not hasattr(session.config, 'workerinput'):\n"
        f"        Path({str(finished)!r}).write_text(str(int(exitstatus)), encoding='utf-8')\n",
        encoding="utf-8",
    )
    test_file = fixture / "test_failure_probe.py"
    test_file.write_text(
        "import os\nimport sys\nimport pytest\n"
        "def test_original_assertion():\n"
        "    print('retained stdout marker')\n"
        "    print('retained stderr marker', file=sys.stderr)\n"
        "    actual, expected = 1729, 1733\n"
        "    assert actual == expected, 'retained assertion marker \\u03bb'\n"
        "def test_later_exit():\n"
        f"    {'os._exit(86)' if abrupt and workers == 0 else 'pass'}\n"
        "@pytest.mark.skip(reason='literal skip control')\n"
        "def test_skip():\n    pass\n"
        "@pytest.mark.xfail(reason='literal xfail control')\n"
        "def test_xfail():\n    assert 0 == 1\n",
        encoding="utf-8",
    )
    env = dict(os.environ)
    # This is a fresh controller even when the owning test runs in a worker.
    for name in (
        "PYTEST_XDIST_WORKER",
        "PYTEST_XDIST_WORKER_COUNT",
        "PYTEST_XDIST_TESTRUNUID",
        "PYTEST_ADDOPTS",
    ):
        env.pop(name, None)
    env["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
    command = [
        sys.executable,
        "-m",
        "pytest",
        "-c",
        str(REPO_ROOT / "pyproject.toml"),
        "-p",
        "molt.pytest_memory_guard_config_plugin",
        "-p",
        "molt.pytest_memory_guard_bootstrap",
        "--rootdir",
        str(fixture),
        "--color=no",
        f"--tb={tbstyle}",
        f"--show-capture={showcapture}",
        "-q",
    ]
    if workers:
        command.extend(["-p", "xdist.plugin", "-n", str(workers), "--dist=loadfile"])
    command.append(str(test_file))
    result = run_guarded_test_process(
        command,
        cwd=REPO_ROOT,
        env=env,
        timeout=90,
        check=False,
    )
    output = (result.stdout or "") + (result.stderr or "")
    assert result.descendants_closed is True, output
    assert result.infrastructure_failure is None, output
    assert not result.timed_out, output
    assert output.count("Molt immediate pytest failure") == 1, output
    assert "test_failure_probe.py::test_original_assertion (call)" in output
    if tbstyle == "no":
        assert "assert 1729 == 1733" not in output
        assert "retained assertion marker" not in output
    else:
        assert "assert 1729 == 1733" in output
        assert "retained assertion marker" in output
    if tbstyle != "no" and showcapture == "all":
        assert "Captured stdout call" in output
        assert "Captured stderr call" in output
        assert "retained stdout marker" in output
        assert "retained stderr marker" in output
    else:
        assert "Captured stdout call" not in output
        assert "Captured stderr call" not in output
        # With short/no traceback, these cannot leak through source excerpts.
        assert "retained stdout marker" not in output
        assert "retained stderr marker" not in output
    if workers:
        assert "Molt immediate pytest failure [gw0]:" in output
    if abrupt:
        assert result.child_returncode == 86, output
        assert result.returncode != 0, output
        assert not finished.exists(), output
        assert "short test summary info" not in output
        assert "FAILURES" not in output
    else:
        assert result.returncode == 1, output
        assert result.child_returncode == 1, output
        assert finished.read_text(encoding="utf-8") == "1"
        assert "1 failed, 1 passed, 1 skipped, 1 xfailed" in output
        assert "short test summary info" in output
