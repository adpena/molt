from __future__ import annotations

import errno
import json
import os
from pathlib import Path
import sys

from tools import check_memory_guard_wiring
from tools import check_subprocess_guard_coverage
from tools import memory_guard
from tools import pytest_memory_guard_bootstrap

REPO_ROOT = Path(__file__).resolve().parents[1]


def _clean_subprocess_audit() -> check_subprocess_guard_coverage.SubprocessGuardAudit:
    return check_subprocess_guard_coverage.SubprocessGuardAudit(
        scanned_files=0,
        raw_calls=(),
        unexpected=(),
        stale_allowlist=(),
        expanded_allowlist=(),
    )


def test_memory_guard_wiring_for_harness_entrypoints_with_clean_subprocess_audit() -> (
    None
):
    audit = check_memory_guard_wiring.audit_repo(
        subprocess_guard_audit=_clean_subprocess_audit(),
    )

    assert audit.missing_paths == ()
    assert audit.missing_tokens == ()
    assert audit.required_sentinel_missing == ()
    assert audit.sentinel_drift == ()
    assert audit.direct_test_guard_missing == ()
    assert audit.ok is True


def test_memory_guard_wiring_uses_clean_raw_subprocess_audit() -> None:
    audit = check_memory_guard_wiring.audit_repo()

    assert audit.subprocess_guard_unexpected == ()
    assert audit.subprocess_guard_stale_allowlist == ()
    assert audit.subprocess_guard_expanded_allowlist == ()
    assert audit.ok is True


def test_wiring_audit_locks_down_pytest_and_ci_gate_custody() -> None:
    contracts = {
        contract.path: contract.tokens
        for contract in check_memory_guard_wiring.PYTHON_GUARD_CONTRACTS
    }

    assert contracts["pyproject.toml"] == (
        "molt.pytest_memory_guard_bootstrap",
        "molt.pytest_memory_guard_config_plugin",
    )
    assert contracts["src/molt/pytest_memory_guard_bootstrap.py"] == (
        "tools.pytest_memory_guard_bootstrap",
        "ensure_current_file_test_script_memory_guard",
        "ensure_repo_test_module_memory_guard",
        "pytest_load_initial_conftests",
        "pytest_runtest_call",
    )
    assert contracts["src/molt/pytest_memory_guard_config_plugin.py"] == (
        "pytest_load_initial_conftests",
        "pytest_runtest_call",
    )
    assert contracts["src/sitecustomize.py"] == (
        "ensure_python_test_memory_guard",
        "tools.pytest_memory_guard_bootstrap",
    )
    assert contracts["sitecustomize.py"] == ("ensure_python_test_memory_guard",)
    assert contracts["tests/_sitecustomize.py"] == (
        "install_test_memory_guard_sitecustomize",
        "ensure_repo_test_script_memory_guard",
    )
    assert contracts["tools/pytest_memory_guard_bootstrap.py"] == (
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
        "PYTEST_OUTER_GUARD_SUMMARY_DIR",
        "repro_context_payload",
    )
    assert contracts["tools/harness_memory_guard.py"] == (
        "test_custody_launch_env",
        "_guard_repro_message",
        "guarded_completed_process",
    )


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
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
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
        == pytest_memory_guard_bootstrap.PYTEST_OUTER_GUARD_SUMMARY_DIR
    )
    assert current_test_file.name.endswith("_current-test.json")


def test_pytest_startup_windows_handoff_waits_for_guard_child(monkeypatch) -> None:
    captured: dict[str, object] = {}

    class Completed:
        returncode = 77

    def fake_run(argv, *, env, check, creationflags=0):
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        captured["check"] = check
        captured["creationflags"] = creationflags
        return Completed()

    def fake_execvpe(*_args):
        raise AssertionError("Windows pytest custody must not use os.execvpe")

    def fake_exit(code):
        raise SystemExit(code)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.subprocess, "run", fake_run)
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "_exit", fake_exit)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
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
    assert captured["creationflags"] == getattr(
        pytest_memory_guard_bootstrap.subprocess,
        "CREATE_NEW_PROCESS_GROUP",
        0,
    )


def test_pytest_startup_windows_handoff_interrupt_exits_cleanly(monkeypatch) -> None:
    def fake_run(argv, *, env, check, creationflags=0):
        del argv, env, check, creationflags
        raise KeyboardInterrupt

    def fake_exit(code):
        raise SystemExit(code)

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.subprocess, "run", fake_run)
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "_exit", fake_exit)

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
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
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
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
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
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
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
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", tmp_path)

    pytest_env = memory_guard.test_custody_launch_env(
        [sys.executable, "-m", "pytest", "tests/test_one.py"],
        environ={},
    )
    assert pytest_env["MOLT_PYTEST_CURRENT_TEST_FILE"].startswith(str(tmp_path))

    module_env = memory_guard.test_custody_launch_env(
        [sys.executable, "-m", "tests.test_memory_guard_wiring"],
        environ={},
    )
    assert module_env["MOLT_PYTEST_CURRENT_TEST_FILE"].startswith(str(tmp_path))

    script_env = memory_guard.test_custody_launch_env(
        [sys.executable, "tests/differential/basic/builtin_chr_ord.py"],
        environ={},
        cwd=REPO_ROOT,
    )
    assert script_env["MOLT_PYTEST_CURRENT_TEST_FILE"].startswith(str(tmp_path))

    assert "MOLT_PYTEST_CURRENT_TEST_FILE" not in memory_guard.test_custody_launch_env(
        [sys.executable, "-c", "print('not a test')"],
        environ={},
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
        "from tests._sitecustomize import install_test_memory_guard_sitecustomize\n",
        encoding="utf-8",
    )

    assert (
        check_memory_guard_wiring._audit_direct_executable_test_guards(tmp_path) == ()
    )


def test_pytest_startup_detects_console_script_and_module_invocations() -> None:
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert (
        pytest_memory_guard_bootstrap.pytest_invocation_args(
            orig_argv=[sys.executable, "-m", "pytest"],
            runtime_argv=["-m"],
        )
        == ()
    )
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-u", "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-X", "dev", "-I", "-m", "pytest", "-q"],
        runtime_argv=["-m", "-q"],
    ) == ("-q",)
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, "-S", "-Xdev", "-m", "pytest", "tests"],
        runtime_argv=["-m", "tests"],
    ) == ("tests",)
    assert pytest_memory_guard_bootstrap.pytest_invocation_args(
        orig_argv=[sys.executable, str(REPO_ROOT / ".venv" / "bin" / "pytest"), "-q"],
        runtime_argv=[str(REPO_ROOT / ".venv" / "bin" / "pytest"), "-q"],
    ) == ("-q",)
    assert (
        pytest_memory_guard_bootstrap.pytest_invocation_args(
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
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "execvpe", fake_execvpe)
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "outer_memory_guard_active",
        lambda _environ=None: False,
    )
    monkeypatch.setattr(pytest_memory_guard_bootstrap.sys, "executable", sys.executable)
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


def test_windows_pytest_cache_dir_arg_uses_canonical_tmp_cache(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))
    args = ["tests/test_one.py", "-q"]

    assert pytest_memory_guard_bootstrap.install_windows_pytest_cache_dir_arg(args)
    assert args[-2:] == [
        "-o",
        f"cache_dir={tmp_path / 'artifact-root' / 'tmp' / 'pytest-cache'}",
    ]
    assert not pytest_memory_guard_bootstrap.install_windows_pytest_cache_dir_arg(args)


def test_windows_pytest_cache_dir_arg_preserves_explicit_cache_policy(
    monkeypatch,
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    explicit = ["-o", "cache_dir=custom-cache"]
    disabled = ["-p", "no:cacheprovider"]

    assert not pytest_memory_guard_bootstrap.install_windows_pytest_cache_dir_arg(
        explicit
    )
    assert explicit == ["-o", "cache_dir=custom-cache"]
    assert not pytest_memory_guard_bootstrap.install_windows_pytest_cache_dir_arg(
        disabled
    )
    assert disabled == ["-p", "no:cacheprovider"]


def test_windows_pytest_cache_dir_config_uses_canonical_tmp_cache(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))

    class Config:
        _inicfg = {}
        _inicache = {"cache_dir": "old-cache"}

    assert pytest_memory_guard_bootstrap.install_windows_pytest_cache_dir_config(
        Config,
        ["tests/test_one.py", "-q"],
    )
    value = Config._inicfg["cache_dir"]
    assert value.value == str(tmp_path / "artifact-root" / "tmp" / "pytest-cache")
    assert value.origin == "override"
    assert "cache_dir" not in Config._inicache


def test_windows_pytest_artifact_base_skips_unhealthy_local_appdata(
    monkeypatch, tmp_path
) -> None:
    local_appdata = tmp_path / "local-appdata"
    temp = tmp_path / "temp"
    monkeypatch.setenv("LOCALAPPDATA", str(local_appdata))
    monkeypatch.setenv("TEMP", str(temp))
    monkeypatch.delenv("TMP", raising=False)
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)

    def fake_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
        del create_dirs
        return path != local_appdata / "Molt" / "tmp"

    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "_artifact_root_accepts_child_dirs",
        fake_accepts_child_dirs,
    )

    assert pytest_memory_guard_bootstrap._windows_pytest_artifact_base() == (
        temp / "Molt" / "tmp"
    )


def test_pytest_user_temp_root_matches_pytest_tmpdir_authority(tmp_path) -> None:
    from _pytest.tmpdir import get_user

    user = get_user() or "unknown"

    assert pytest_memory_guard_bootstrap._pytest_user_temp_root(tmp_path) == (
        tmp_path / f"pytest-of-{user}"
    )


def test_windows_pytest_custody_roots_prepare_readable_defaults(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))
    monkeypatch.delenv("PYTEST_DEBUG_TEMPROOT", raising=False)

    assert pytest_memory_guard_bootstrap.install_windows_pytest_custody_roots()
    temproot = Path(os.environ["PYTEST_DEBUG_TEMPROOT"])
    assert temproot.parent == tmp_path / "artifact-root" / "tmp"
    assert temproot.name.startswith("pytest-temproot-")
    assert temproot.is_dir()
    assert any(temproot.iterdir())
    assert (
        tmp_path / "artifact-root" / "tmp" / "pytest-cache" / "v" / "cache"
    ).is_dir()


def test_windows_pytest_custody_roots_preserve_explicit_temproot(
    monkeypatch, tmp_path
) -> None:
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap, "_is_windows_process_model", lambda: True
    )
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifact-root"))
    explicit = tmp_path / "explicit-temproot"
    monkeypatch.setenv("PYTEST_DEBUG_TEMPROOT", str(explicit))

    assert not pytest_memory_guard_bootstrap.install_windows_pytest_custody_roots()
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
        ("-p", "no:molt_memory_guard"),
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
        {"PYTEST_ADDOPTS": "-p no:molt_memory_guard"},
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
    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "getpid", lambda: 300)

    assert (
        pytest_memory_guard_bootstrap.outer_memory_guard_active(
            {"MOLT_MEMORY_GUARD_ACTIVE": "1", "MOLT_MEMORY_GUARD_PID": "100"}
        )
        is True
    )


def test_pytest_current_test_hooks_write_live_identity(
    monkeypatch,
    tmp_path: Path,
) -> None:
    current_test_path = tmp_path / "current-test.json"
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "PYTEST_OUTER_GUARD_SUMMARY_DIR",
        tmp_path,
    )
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
    canonical_root = tmp_path / "canonical"
    outside_path = tmp_path / "outside" / "current-test.json"
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "PYTEST_OUTER_GUARD_SUMMARY_DIR",
        canonical_root,
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
    aggregate_path = tmp_path / "pytest-guard_current-test.json"
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "PYTEST_OUTER_GUARD_SUMMARY_DIR",
        tmp_path,
    )
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
    current_test_path = tmp_path / "current-test.json"
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "PYTEST_OUTER_GUARD_SUMMARY_DIR",
        tmp_path,
    )
    monkeypatch.setenv(
        pytest_memory_guard_bootstrap.PYTEST_CURRENT_TEST_FILE_ENV,
        str(current_test_path),
    )

    def forbidden_replace(_src: object, _dst: object) -> None:
        raise AssertionError("guard custody must not use monkeypatched os.replace")

    monkeypatch.setattr(pytest_memory_guard_bootstrap.os, "replace", forbidden_replace)

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
    current_test_path = tmp_path / "current-test.json"
    monkeypatch.setattr(
        pytest_memory_guard_bootstrap,
        "PYTEST_OUTER_GUARD_SUMMARY_DIR",
        tmp_path,
    )
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
