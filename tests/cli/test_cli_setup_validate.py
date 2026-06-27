from __future__ import annotations

import importlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from molt.cli import commands as cli_commands
from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[2]
COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")
COMMAND_RUNTIME = importlib.import_module("molt.cli.command_runtime")
CARGO_EXECUTION = importlib.import_module("molt.cli.cargo_execution")
NATIVE_LINK_DEPS = importlib.import_module("molt.cli.native_link_deps")
NATIVE_TOOLCHAIN = importlib.import_module("molt.cli.native_toolchain")
SETUP_READINESS = importlib.import_module("molt.cli.setup_readiness")
TOOLCHAIN_VALIDATION = importlib.import_module("molt.cli.toolchain_validation")
RUNTIME_BUILD = importlib.import_module("molt.cli.runtime_build")
RUNTIME_WASM_VALIDATION = importlib.import_module("molt.cli.runtime_wasm_validation")


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _patch_memory_guard_loader(
    monkeypatch: pytest.MonkeyPatch,
    cli_module: object,
    loader: object,
) -> None:
    monkeypatch.setattr(
        cli_module,
        "_load_cli_harness_memory_guard",
        loader,
        raising=True,
    )
    monkeypatch.setattr(
        COMMAND_RUNTIME,
        "_load_cli_harness_memory_guard",
        loader,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_load_cli_harness_memory_guard",
        loader,
        raising=True,
    )


def _run_cli(args: list[str]) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _run_dev(args: list[str]) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [_python_executable(), "tools/dev.py", *args],
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _fake_cli_harness(
    calls: list[dict[str, object]],
    *,
    result_factory=None,
):
    class FakeContext:
        def __init__(
            self,
            prefix: str,
            env: dict[str, str] | None,
            repo_root: Path,
        ) -> None:
            self.prefix = prefix
            self.env = env
            self.repo_root = repo_root

        @classmethod
        def from_env(
            cls,
            prefix: str,
            env: dict[str, str] | None,
            *,
            repo_root: Path,
        ):
            calls.append(
                {
                    "method": "context",
                    "prefix": prefix,
                    "env": env,
                    "repo_root": repo_root,
                }
            )
            return cls(prefix, env, repo_root)

        def run(self, cmd: list[str], **kwargs: object):
            calls.append(
                {
                    "method": "run",
                    "cmd": cmd,
                    "prefix": self.prefix,
                    "env": self.env,
                    "repo_root": self.repo_root,
                    **kwargs,
                }
            )
            if result_factory is not None:
                return result_factory(cmd)
            return subprocess.CompletedProcess(cmd, 0, "stdout\n", "stderr\n")

    class FakeMemoryGuard:
        HarnessExecutionContext = FakeContext

        @staticmethod
        def limits_from_env(prefix: str, env: dict[str, str] | None = None):
            return {
                "prefix": prefix,
                "env_has_session": bool(env and env.get("MOLT_SESSION_ID")),
            }

        @staticmethod
        def limits_summary(limits):
            return {
                "enabled": True,
                "max_process_rss_gb": 1.0,
                "max_total_rss_gb": 2.0,
                "max_global_rss_gb": 3.0,
                "child_rlimit_gb": 4.0,
                "prefix": limits["prefix"],
                "env_has_session": limits["env_has_session"],
            }

    return FakeMemoryGuard


def test_cli_setup_json_reports_actions_and_environment() -> None:
    res = _run_cli(["setup", "--json"])
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "setup"
    assert payload["status"] in {"ok", "error"}
    data = payload["data"]
    assert isinstance(data.get("checks"), list)
    assert isinstance(data.get("environment"), dict)
    assert isinstance(data.get("actions"), list)
    assert "CARGO_TARGET_DIR" in data["environment"]
    assert "MOLT_CACHE" in data["environment"]


def test_cli_validate_check_json_reports_canonical_matrix() -> None:
    res = _run_cli(["validate", "--check", "--json", "--suite", "smoke"])
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "validate"
    assert payload["status"] == "ok"
    data = payload["data"]
    assert data["check_only"] is True
    assert data["memory_guard"]["MOLT_BENCH"]["enabled"] is True
    assert data["memory_guard"]["MOLT_CONFORMANCE"]["enabled"] is True
    assert data["memory_guard"]["MOLT_TEST_SUITE"]["enabled"] is True
    steps = data["steps"]
    assert isinstance(steps, list)
    names = {entry["name"] for entry in steps}
    assert "cli-run-json" in names
    assert "cli-command-json" in names
    assert "subprocess-guard-audit" in names
    assert "memory-guard-wiring-audit" in names
    assert "custody-proof" in names
    assert "native-parity" in names
    assert "wasm-parity" in names
    assert "luau-support-matrix" in names
    assert "luau-compile-smoke" in names
    assert "luau-runner-available" in names
    assert "luau-ord-at-parity" in names
    assert "luau-rust-regressions" in names
    assert "luau-lowering-regressions" in names
    assert "conformance-smoke" in names
    assert "bench-smoke" in names
    cli_command_step = next(
        entry for entry in steps if entry["name"] == "cli-command-json"
    )
    cli_command_expr = cli_command_step["cmd"][cli_command_step["cmd"].index("-k") + 1]
    assert "test_cli_build_json_binary_executes_for_native_profiles" in cli_command_expr
    assert "test_cli_compare_json" in cli_command_expr
    assert "test_cli_run_exec_eval_raise_runtime_error" in cli_command_expr
    bench_step = next(entry for entry in steps if entry["name"] == "bench-smoke")
    assert bench_step["memory_guard_prefix"] == "MOLT_BENCH"
    assert "--warmup" in bench_step["cmd"]
    assert bench_step["cmd"][bench_step["cmd"].index("--warmup") + 1] == "1"
    guard_audit_step = next(
        entry for entry in steps if entry["name"] == "subprocess-guard-audit"
    )
    assert guard_audit_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert guard_audit_step["category"] == "command"
    assert "luau" in guard_audit_step["backends"]
    memory_guard_audit_step = next(
        entry for entry in steps if entry["name"] == "memory-guard-wiring-audit"
    )
    assert memory_guard_audit_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert memory_guard_audit_step["category"] == "command"
    assert "luau" in memory_guard_audit_step["backends"]
    custody_step = next(entry for entry in steps if entry["name"] == "custody-proof")
    assert custody_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert custody_step["category"] == "command"
    assert "tests/tools/test_process_sentinel.py" in custody_step["cmd"]
    assert "tests/tools/test_memory_guard_windows_sampling.py" in custody_step["cmd"]
    luau_compile_step = next(
        entry for entry in steps if entry["name"] == "luau-compile-smoke"
    )
    assert luau_compile_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "--target" in luau_compile_step["cmd"]
    assert luau_compile_step["cmd"][luau_compile_step["cmd"].index("--target") + 1] == (
        "luau"
    )
    assert "--profile" in luau_compile_step["cmd"]
    assert luau_compile_step["cmd"][
        luau_compile_step["cmd"].index("--profile") + 1
    ] == ("release")
    assert "tmp/validate/luau-smoke/hello.luau" in luau_compile_step["cmd"][-1].replace(
        "\\", "/"
    )
    luau_runner_step = next(
        entry for entry in steps if entry["name"] == "luau-runner-available"
    )
    assert luau_runner_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "shutil.which('luau')" in luau_runner_step["cmd"][-1]
    luau_rust_step = next(
        entry for entry in steps if entry["name"] == "luau-rust-regressions"
    )
    assert luau_rust_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "molt-backend-luau" in luau_rust_step["cmd"]
    assert "luau-backend" in luau_rust_step["cmd"]
    assert "--lib" in luau_rust_step["cmd"]
    assert "luau::tests::" in luau_rust_step["cmd"]
    luau_lowering_step = next(
        entry for entry in steps if entry["name"] == "luau-lowering-regressions"
    )
    assert luau_lowering_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert "molt-backend-luau" in luau_lowering_step["cmd"]
    assert "luau-backend" in luau_lowering_step["cmd"]
    assert "--lib" in luau_lowering_step["cmd"]
    assert "luau_lower::tests::" in luau_lowering_step["cmd"]


def test_cli_validate_custody_proof_suite_reports_only_custody_step() -> None:
    res = _run_cli(["validate", "--check", "--json", "--suite", "custody-proof"])
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    data = payload["data"]

    assert data["suite"] == "custody-proof"
    assert sorted(data["memory_guard"]) == ["MOLT_TEST_SUITE"]
    steps = data["steps"]
    assert [entry["name"] for entry in steps] == ["custody-proof"]
    custody_step = steps[0]
    assert custody_step["memory_guard_prefix"] == "MOLT_TEST_SUITE"
    assert custody_step["category"] == "command"
    assert "tests/test_memory_guard_wiring.py" in custody_step["cmd"]
    assert "tests/tools/test_memory_guard_windows_sampling.py" in custody_step["cmd"]
    assert "tests/tools/test_process_sentinel.py" in custody_step["cmd"]


def test_cli_validate_luau_backend_filter_reports_guarded_luau_steps() -> None:
    res = _run_cli(
        ["validate", "--check", "--json", "--suite", "smoke", "--backend", "luau"]
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    data = payload["data"]
    assert data["backend"] == "luau"
    assert data["memory_guard"]["MOLT_TEST_SUITE"]["enabled"] is True
    steps = data["steps"]
    names = {entry["name"] for entry in steps}
    assert names == {
        "subprocess-guard-audit",
        "memory-guard-wiring-audit",
        "custody-proof",
        "luau-support-matrix",
        "luau-compile-smoke",
        "luau-runner-available",
        "luau-ord-at-parity",
        "luau-rust-regressions",
        "luau-lowering-regressions",
    }
    assert all(entry["memory_guard_prefix"] == "MOLT_TEST_SUITE" for entry in steps)
    assert all("luau" in entry["backends"] for entry in steps)


def test_cli_validate_rejects_proof_bypass_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")

    res = _run_cli(["validate", "--check", "--json", "--suite", "smoke"])

    assert res.returncode == 2
    payload = json.loads(res.stdout)
    assert payload["command"] == "validate"
    assert payload["status"] == "error"
    assert "MOLT_SKIP_RUNTIME_REBUILD=1 disables" in payload["errors"][0]


def test_cli_validate_check_json_writes_explicit_summary_out(tmp_path: Path) -> None:
    summary_path = tmp_path / "validate-plan.json"

    res = _run_cli(
        [
            "validate",
            "--check",
            "--json",
            "--suite",
            "smoke",
            "--summary-out",
            str(summary_path),
        ]
    )

    assert res.returncode == 0, res.stderr
    stdout_payload = json.loads(res.stdout)
    file_payload = json.loads(summary_path.read_text())
    assert file_payload == stdout_payload
    assert stdout_payload["data"]["check_only"] is True
    assert stdout_payload["data"]["summary_path"] == str(summary_path)


def test_cli_run_command_uses_memory_guard_prefix(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("guarded CLI command used raw subprocess.run")

    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(calls),
    )
    monkeypatch.setattr(cli.subprocess, "run", fail_raw_run, raising=True)

    rc = cli_commands._run_command(
        ["python3", "-c", "print('ok')"],
        cwd=ROOT,
        env={"PATH": "/usr/bin"},
        memory_guard_prefix="MOLT_BENCH",
    )

    assert rc == 0
    assert calls[0] == {
        "method": "context",
        "prefix": "MOLT_BENCH",
        "env": {"PATH": "/usr/bin"},
        "repo_root": ROOT,
    }
    assert calls[1]["method"] == "run"
    assert calls[1]["prefix"] == "MOLT_BENCH"
    assert calls[1]["cwd"] == ROOT
    assert calls[1]["capture_output"] is False


def test_cli_timed_command_uses_memory_guard_elapsed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    def result_factory(cmd: list[str]) -> subprocess.CompletedProcess[str]:
        result = subprocess.CompletedProcess(cmd, 0, "stdout\n", "stderr\n")
        result.elapsed_s = 0.125  # type: ignore[attr-defined]
        return result

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("guarded timed CLI command used raw subprocess.run")

    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(calls, result_factory=result_factory),
    )
    monkeypatch.setattr(cli.subprocess, "run", fail_raw_run, raising=True)

    result = cli_commands._run_command_timed(
        ["python3", "-c", "print('ok')"],
        cwd=ROOT,
        env={"PATH": "/usr/bin"},
        capture_output=True,
        memory_guard_prefix="MOLT_CLI",
    )

    assert result.returncode == 0
    assert result.stdout == "stdout\n"
    assert result.stderr == "stderr\n"
    assert result.duration_s == pytest.approx(0.125)
    assert calls[0]["prefix"] == "MOLT_CLI"
    assert calls[1]["prefix"] == "MOLT_CLI"
    assert calls[1]["capture_output"] is True


def test_cli_guard_preserves_operator_limits_for_sanitized_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    monkeypatch.setenv("MOLT_CLI_MAX_PROCESS_RSS_GB", "0.05")
    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(
            calls,
            result_factory=lambda cmd: subprocess.CompletedProcess(cmd, 0, "", ""),
        ),
    )

    result = cli._run_completed_command(
        ["python3", "-c", "pass"],
        cwd=ROOT,
        env={"PATH": "/usr/bin"},
        capture_output=True,
        memory_guard_prefix="MOLT_CLI",
    )

    assert result.returncode == 0
    assert calls[0]["env"] == {
        "PATH": "/usr/bin",
        "MOLT_CLI_MAX_PROCESS_RSS_GB": "0.05",
    }
    assert calls[1]["env"] == calls[0]["env"]
    assert calls[1]["repo_root"] == ROOT


def test_cli_cargo_build_helper_uses_default_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    def result_factory(cmd: list[str]) -> subprocess.CompletedProcess[str]:
        run_count = sum(1 for call in calls if call["method"] == "run")
        return subprocess.CompletedProcess(
            cmd,
            1 if run_count == 1 else 0,
            "",
            "sccache: error: cache unavailable",
        )

    def fail_raw_subprocess_run(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("cargo helper used raw subprocess.run")

    monkeypatch.setenv("MOLT_BUILD_MAX_PROCESS_RSS_GB", "0.25")
    monkeypatch.setattr(COMMAND_RUNTIME.subprocess, "run", fail_raw_subprocess_run)
    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(calls, result_factory=result_factory),
    )

    result = CARGO_EXECUTION._run_cargo_with_sccache_retry(
        ["cargo", "build"],
        cwd=ROOT,
        env={"PATH": "/usr/bin", "RUSTC_WRAPPER": "/usr/bin/sccache"},
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 0
    run_calls = [call for call in calls if call["method"] == "run"]
    assert [call["prefix"] for call in run_calls] == ["MOLT_BUILD", "MOLT_BUILD"]
    assert run_calls[0]["env"] == {
        "PATH": "/usr/bin",
        "RUSTC_WRAPPER": "/usr/bin/sccache",
        "MOLT_BUILD_MAX_PROCESS_RSS_GB": "0.25",
    }
    assert run_calls[1]["env"] == {
        "PATH": "/usr/bin",
        "MOLT_BUILD_MAX_PROCESS_RSS_GB": "0.25",
    }


def test_maybe_enable_sccache_installs_shared_dx_cache_defaults(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    artifact_root = tmp_path / "artifacts"
    env = {
        "PATH": "/usr/bin",
        "MOLT_EXT_ROOT": str(artifact_root),
    }
    monkeypatch.setattr(
        CARGO_EXECUTION.shutil, "which", lambda _name: "/usr/bin/sccache"
    )

    CARGO_EXECUTION._maybe_enable_sccache(env)

    assert env["RUSTC_WRAPPER"] == "/usr/bin/sccache"
    assert env["SCCACHE_DIR"] == str((artifact_root / ".sccache").resolve())
    assert env["SCCACHE_CACHE_SIZE"] == "10G"


def test_cli_wrapper_build_uses_default_memory_guard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli

    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n")
    output = tmp_path / "main_molt"
    calls: list[dict[str, object]] = []

    def result_factory(cmd: list[str]) -> subprocess.CompletedProcess[str]:
        payload = {
            "status": "ok",
            "data": {
                "output": str(output),
                "consumer_output": str(output),
                "artifacts": {},
            },
        }
        result = subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        result.elapsed_s = 0.25  # type: ignore[attr-defined]
        return result

    def fail_raw_run(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("wrapper build used raw subprocess.run")

    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(calls, result_factory=result_factory),
    )
    monkeypatch.setattr(cli.subprocess, "run", fail_raw_run, raising=True)

    contract, duration, error = cli._run_wrapper_build(
        file_path=str(entry),
        module=None,
        build_args=[],
        env={"PATH": "/usr/bin"},
        project_root=tmp_path,
        json_output=True,
        command="run",
        verbose=False,
    )

    assert error is None
    assert contract is not None
    assert contract.consumer_output == output
    assert duration == pytest.approx(0.25)
    run_calls = [call for call in calls if call["method"] == "run"]
    assert run_calls[0]["prefix"] == "MOLT_CLI"
    assert run_calls[0]["capture_output"] is True
    assert run_calls[0]["cmd"][:4] == [sys.executable, "-m", "molt.cli", "build"]
    assert "--json" in run_calls[0]["cmd"]


def test_cli_build_toolchain_probes_use_memory_guard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    def fake_run_completed_command(cmd: list[str], **kwargs: object):
        calls.append({"cmd": cmd, **kwargs})
        executable = Path(cmd[0]).name
        if executable == "git":
            stdout = "abc123\n"
        elif executable == "rustc":
            stdout = "/rust/target/lib\n" if "--print" in cmd else "rustc 1.91.0\n"
        elif executable == "xcrun":
            stdout = "/Applications/Xcode.app/SDKs/MacOSX.sdk\n"
        else:
            stdout = ""
        return subprocess.CompletedProcess(cmd, 0, stdout, "")

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        COMPILER_METADATA,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_VALIDATION,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        NATIVE_TOOLCHAIN,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )
    monkeypatch.setattr(
        cli.shutil,
        "which",
        lambda name: (
            f"/usr/bin/{name}"
            if name in {"rustc", "wasm-tools", "nm", "llvm-ar", "lipo"}
            else None
        ),
        raising=True,
    )
    monkeypatch.setattr(
        NATIVE_LINK_DEPS.shutil,
        "which",
        lambda name: (
            f"/usr/bin/{name}"
            if name in {"rustc", "wasm-tools", "nm", "llvm-ar", "lipo"}
            else None
        ),
        raising=True,
    )
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)

    wasm_path = tmp_path / "module.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")
    obj_path = tmp_path / "module.o"
    obj_path.write_bytes(b"")
    archive_path = tmp_path / "libmolt_runtime.a"
    archive_path.write_bytes(b"")

    COMPILER_METADATA._rustc_version.cache_clear()
    RUNTIME_BUILD._rust_target_libdir.cache_clear()

    assert cli._git_rev(ROOT) == "abc123"
    assert COMPILER_METADATA._rustc_version() == "rustc 1.91.0"
    assert RUNTIME_WASM_VALIDATION._validate_wasm_structural(wasm_path) is None
    assert RUNTIME_BUILD._rust_target_libdir("wasm32-wasip1") == Path(
        "/rust/target/lib"
    )
    assert cli._is_valid_cached_backend_artifact(obj_path, is_wasm=False) is False
    assert cli._runtime_archive_crate_names(archive_path) == frozenset()
    assert cli._detect_macos_arch(obj_path) is None
    assert cli._resolve_macos_sdk_root() == "/Applications/Xcode.app/SDKs/MacOSX.sdk"

    prefixes = [call["memory_guard_prefix"] for call in calls]
    assert prefixes[0] == "MOLT_CLI"
    assert set(prefixes[1:]) == {"MOLT_BUILD"}


def test_cli_diff_command_uses_diff_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        calls.append({"cmd": cmd, **kwargs})
        return 0

    monkeypatch.setattr(
        cli_commands, "_find_molt_root", lambda *args: ROOT, raising=True
    )
    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command, raising=True)

    assert cli_commands.diff(None, None) == 0

    assert calls
    assert calls[0]["memory_guard_prefix"] == "MOLT_DIFF"
    assert calls[0]["cmd"][:2] == [sys.executable, "tests/molt_diff.py"]


def test_cli_compare_uses_diff_memory_guard_for_children(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    entry = project / "main.py"
    entry.write_text("print('ok')\n")
    built_binary = project / "build" / "main_molt"
    built_binary.parent.mkdir(parents=True, exist_ok=True)
    built_binary.write_text("")

    prefixes: list[object] = []

    def fake_run_command_timed(
        cmd: list[str], **kwargs: object
    ) -> cli_commands._TimedResult:
        prefixes.append(kwargs.get("memory_guard_prefix"))
        if len(prefixes) == 1:
            return cli_commands._TimedResult(0, "ok\n", "", 0.01)
        if len(prefixes) == 2:
            return cli_commands._TimedResult(
                0,
                json.dumps({"data": {"output": str(built_binary)}}),
                "",
                0.02,
            )
        return cli_commands._TimedResult(0, "ok\n", "", 0.01)

    monkeypatch.setattr(
        cli_commands, "_find_project_root", lambda start: project, raising=True
    )
    monkeypatch.setattr(
        cli_commands, "_find_molt_root", lambda start, cwd=None: ROOT, raising=True
    )
    monkeypatch.setattr(
        cli_commands, "_resolve_python_exe", lambda exe: "python3", raising=True
    )
    monkeypatch.setattr(
        cli_commands,
        "_resolve_binary_output",
        lambda output: built_binary,
        raising=True,
    )
    monkeypatch.setattr(
        cli_commands, "_run_command_timed", fake_run_command_timed, raising=True
    )

    rc = cli_commands.compare(str(entry), None, "python3", [])

    assert rc == 0
    assert prefixes == ["MOLT_DIFF", "MOLT_DIFF", "MOLT_DIFF"]


def test_cli_cross_run_uses_cross_memory_guard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli
    from molt.cli import build_inputs as cli_build_inputs

    project = tmp_path / "project"
    project.mkdir()
    entry = project / "main.py"
    entry.write_text("print('ok')\n")
    artifact = project / "out.wasm"
    artifact.write_text("")

    class BuildEntry:
        source_path = entry

    build_calls: list[dict[str, object]] = []
    run_calls: list[dict[str, object]] = []

    def fake_run_wrapper_build(**kwargs: object):
        build_calls.append(kwargs)
        return (
            cli._WrapperBuildContract(
                output=artifact,
                consumer_output=artifact,
                bundle_root=None,
                artifacts={},
            ),
            0.01,
            None,
        )

    def fake_run_completed_command(cmd: list[str], **kwargs: object):
        run_calls.append({"cmd": cmd, **kwargs})
        result = subprocess.CompletedProcess(cmd, 0, "", "")
        result.elapsed_s = 0.02  # type: ignore[attr-defined]
        return result

    monkeypatch.setattr(
        cli_commands, "_find_project_root", lambda start: project, raising=True
    )
    monkeypatch.setattr(
        cli_commands, "_find_molt_root", lambda start, cwd=None: ROOT, raising=True
    )
    monkeypatch.setattr(
        cli_build_inputs,
        "_resolve_wrapper_build_entry",
        lambda **kwargs: (BuildEntry(), None),
        raising=True,
    )
    monkeypatch.setattr(
        cli_commands, "_run_wrapper_build", fake_run_wrapper_build, raising=True
    )
    monkeypatch.setattr(
        cli_commands, "_run_completed_command", fake_run_completed_command, raising=True
    )
    monkeypatch.setattr(
        cli_commands.shutil,
        "which",
        lambda name: "/usr/bin/wasmtime" if name == "wasmtime" else None,
        raising=True,
    )

    rc = cli_commands._run_script_cross(
        "wasm",
        str(entry),
        None,
        [],
        build_args=["--target", "wasm"],
    )

    assert rc == 0
    assert build_calls[0]["memory_guard_prefix"] == "MOLT_CROSS"
    assert run_calls[0]["memory_guard_prefix"] == "MOLT_CROSS"
    assert run_calls[0]["capture_output"] is False


def test_cli_bench_outer_process_uses_bench_memory_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_run_command(cmd: list[str], **kwargs: object) -> int:
        calls.append({"cmd": cmd, **kwargs})
        return 0

    monkeypatch.setattr(cli_commands, "_run_command", fake_run_command, raising=True)

    assert cli_commands.bench(wasm=False, bench_args=["--smoke"]) == 0

    assert calls
    assert calls[0]["memory_guard_prefix"] == "MOLT_BENCH"
    assert "tools/bench.py" in calls[0]["cmd"]


def test_cli_validate_uses_family_memory_guard_prefixes(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []
    steps = [
        cli._ValidationStep(
            "conformance-step",
            ["python3", "-c", "pass"],
            ROOT,
            "conformance",
            ("native",),
            ("dev",),
            "smoke",
        ),
        cli._ValidationStep(
            "bench-step",
            ["python3", "-c", "pass"],
            ROOT,
            "benchmark",
            ("native",),
            ("dev",),
            "smoke",
        ),
        cli._ValidationStep(
            "correctness-step",
            ["python3", "-c", "pass"],
            ROOT,
            "correctness",
            ("native",),
            ("dev",),
            "smoke",
        ),
    ]

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_find_molt_root",
        lambda *args: ROOT,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_validate_steps",
        lambda root, suite, backend, profile: steps,
        raising=True,
    )
    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(
            calls,
            result_factory=lambda cmd: subprocess.CompletedProcess(cmd, 0, "", ""),
        ),
    )

    summary_path = tmp_path / "validate-summary.json"

    assert (
        cli.validate(
            suite="smoke",
            json_output=True,
            summary_out=str(summary_path),
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert json.loads(summary_path.read_text()) == payload
    assert payload["data"]["summary_path"] == str(summary_path)
    assert payload["data"]["check_only"] is False
    assert isinstance(payload["data"]["elapsed_s"], float)
    assert payload["data"]["memory_guard"] == {
        "MOLT_BENCH": {
            "enabled": True,
            "max_process_rss_gb": 1.0,
            "max_total_rss_gb": 2.0,
            "max_global_rss_gb": 3.0,
            "child_rlimit_gb": 4.0,
            "prefix": "MOLT_BENCH",
            "env_has_session": True,
        },
        "MOLT_CONFORMANCE": {
            "enabled": True,
            "max_process_rss_gb": 1.0,
            "max_total_rss_gb": 2.0,
            "max_global_rss_gb": 3.0,
            "child_rlimit_gb": 4.0,
            "prefix": "MOLT_CONFORMANCE",
            "env_has_session": True,
        },
        "MOLT_TEST_SUITE": {
            "enabled": True,
            "max_process_rss_gb": 1.0,
            "max_total_rss_gb": 2.0,
            "max_global_rss_gb": 3.0,
            "child_rlimit_gb": 4.0,
            "prefix": "MOLT_TEST_SUITE",
            "env_has_session": True,
        },
    }
    prefixes = [call["prefix"] for call in calls if call["method"] == "run"]
    assert prefixes == ["MOLT_CONFORMANCE", "MOLT_BENCH", "MOLT_TEST_SUITE"]


def test_cli_validate_defaults_execution_summary_to_logs(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []
    steps = [
        cli._ValidationStep(
            "correctness-step",
            ["python3", "-c", "pass"],
            tmp_path,
            "correctness",
            ("native",),
            ("dev",),
            "smoke",
        )
    ]

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_find_molt_root",
        lambda *args: tmp_path,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_require_molt_root",
        lambda *args: None,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_validate_steps",
        lambda root, suite, backend, profile: steps,
        raising=True,
    )
    _patch_memory_guard_loader(
        monkeypatch,
        cli,
        lambda cwd: _fake_cli_harness(
            calls,
            result_factory=lambda cmd: subprocess.CompletedProcess(cmd, 0, "", ""),
        ),
    )

    assert (
        cli.validate(
            suite="smoke",
            backend="native",
            profile="dev",
            json_output=True,
        )
        == 0
    )

    summary_path = tmp_path / "logs" / "validate-smoke-native-dev.json"
    payload = json.loads(capsys.readouterr().out)
    assert json.loads(summary_path.read_text()) == payload
    assert payload["data"]["summary_path"] == str(summary_path)
    assert payload["data"]["results"][0]["name"] == "correctness-step"


def test_tools_dev_validate_delegates_to_canonical_cli() -> None:
    res = _run_dev(["validate", "--check"])
    assert res.returncode == 0, res.stderr
    assert "validate" in res.stdout.lower() or "validate" in res.stderr.lower()


def test_cli_lint_uses_shared_dx_planner(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[list[str]] = []

    class FakeDxProject:
        def __init__(self, root: Path) -> None:
            self.root = root

        def canonical_env(self) -> dict[str, str]:
            return {"PATH": "", "PYTHONPATH": str(ROOT / "src")}

        def require_project_python(self, context: str, env: dict[str, str]) -> Path:
            assert context == "lint"
            assert env["PYTHONPATH"] == str(ROOT / "src")
            return ROOT / ".venv" / "bin" / "python3"

        def commands(self) -> dict[str, object]:
            return {"lint": "python3 -m ruff check ."}

        def split_command_sequence(
            self,
            command: object,
            name: str,
            *,
            env: dict[str, str],
        ) -> list[list[str]]:
            assert command == "python3 -m ruff check ."
            assert name == "lint"
            assert env["PYTHONPATH"] == str(ROOT / "src")
            return [["python3", "-m", "ruff", "check", "."]]

    def fake_run_completed(cmd, **kwargs):
        calls.append(list(cmd))
        assert cmd != [sys.executable, "tools/dev.py", "lint"]
        assert kwargs["cwd"] == ROOT
        assert kwargs["capture_output"] is False
        assert kwargs["memory_guard_prefix"] == "MOLT_CLI"
        return subprocess.CompletedProcess(cmd, 0)

    monkeypatch.setattr(cli_commands, "DxProject", FakeDxProject, raising=True)
    monkeypatch.setattr(
        cli_commands, "_run_completed_command", fake_run_completed, raising=True
    )

    assert cli_commands.lint(json_output=False, verbose=False) == 0
    assert calls == [["python3", "-m", "ruff", "check", "."]]


def test_cli_update_steps_use_memory_guard(monkeypatch: pytest.MonkeyPatch) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []
    step = cli._MaintenanceStep(
        "probe",
        ["python3", "-c", "print('ok')"],
        ROOT,
        "toolchain",
    )

    def fake_run_completed(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_find_molt_root",
        lambda _cwd: ROOT,
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_planned_update_steps",
        lambda *_args, **_kwargs: ([step], []),
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_run_completed_command",
        fake_run_completed,
        raising=True,
    )

    assert cli.update_repo(json_output=True) == 0
    assert calls == [
        {
            "cmd": ["python3", "-c", "print('ok')"],
            "cwd": ROOT,
            "capture_output": True,
            "env": None,
            "memory_guard_prefix": "MOLT_CLI",
        }
    ]


def test_update_plan_bootstraps_missing_cargo_tool_helpers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    present = {
        "cargo": "cargo",
        "rustup": "rustup",
        "wasm-tools": "wasm-tools",
    }

    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION.shutil,
        "which",
        lambda name: present.get(name),
        raising=True,
    )
    monkeypatch.setattr(
        TOOLCHAIN_VALIDATION,
        "_detect_llvm_backend_toolchain",
        lambda _root: (22, None),
        raising=True,
    )

    steps, warnings = TOOLCHAIN_VALIDATION._planned_update_steps(
        ROOT,
        include_toolchains=True,
        include_locks=False,
        include_manifests=False,
    )

    names = {step.name for step in steps}
    assert "cargo-install-wasm-pack" in names
    assert "cargo-install-wasm-tools" not in names
    assert any("tools/bootstrap_llvm.py" in warning for warning in warnings)


def test_llvm_backend_advice_names_exact_prefix_and_config(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(SETUP_READINESS.platform, "system", lambda: "Windows")

    advice = SETUP_READINESS._llvm_backend_advice(22)

    joined = "\n".join(advice)
    assert "LLVM_SYS_221_PREFIX" in joined
    assert "llvm-config.exe" in joined
    assert "tools/bootstrap_llvm.py" in joined


def test_llvm_report_distinguishes_windows_clang_without_config(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(SETUP_READINESS.platform, "system", lambda: "Windows")
    monkeypatch.setattr(
        SETUP_READINESS,
        "_required_llvm_backend_major",
        lambda _root: 22,
        raising=True,
    )
    present = {
        "python": "python",
        "uv": "uv",
        "cargo": "cargo",
        "rustup": "rustup",
        "cargo-upgrade": "cargo-upgrade",
        "clang": "C:/Program Files/LLVM/bin/clang.exe",
        "cmake": "cmake",
        "ninja": "ninja",
        "wasm-ld": "wasm-ld",
        "wasm-tools": "wasm-tools",
        "wasm-pack": "wasm-pack",
        "wasmtime": "wasmtime",
        "zig": "zig",
    }
    monkeypatch.setattr(
        SETUP_READINESS.shutil,
        "which",
        lambda name: present.get(name),
        raising=True,
    )

    def fake_run(cmd, **_kwargs):
        if cmd == ["C:/Program Files/LLVM/bin/clang.exe", "--version"]:
            return subprocess.CompletedProcess(
                cmd,
                0,
                "clang version 22.1.7\nTarget: x86_64-pc-windows-msvc\n",
                "",
            )
        return subprocess.CompletedProcess(cmd, 1, "", "")

    monkeypatch.setattr(SETUP_READINESS.subprocess, "run", fake_run, raising=True)

    report = SETUP_READINESS._build_toolchain_report(ROOT)
    llvm = next(
        check for check in report.checks if check["name"] == "llvm-backend-toolchain"
    )

    assert llvm["ok"] is False
    assert "clang is present" in llvm["detail"]
    assert "llvm-config" in llvm["detail"]


def test_windows_msvc_env_reports_inactive_dev_shell(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(SETUP_READINESS.platform, "system", lambda: "Windows")
    monkeypatch.setattr(
        SETUP_READINESS,
        "_windows_vsdevcmd_path",
        lambda: Path("C:/VS/Common7/Tools/VsDevCmd.bat"),
        raising=True,
    )
    present = {
        "python": "python",
        "uv": "uv",
        "cargo": "cargo",
        "rustup": "rustup",
        "cargo-upgrade": "cargo-upgrade",
        "clang": "clang",
        "cmake": "cmake",
        "ninja": "ninja",
        "wasm-ld": "wasm-ld",
        "wasm-tools": "wasm-tools",
        "wasm-pack": "wasm-pack",
    }
    monkeypatch.setattr(
        SETUP_READINESS.shutil,
        "which",
        lambda name: present.get(name),
        raising=True,
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_detect_llvm_backend_toolchain",
        lambda _root: (22, None),
        raising=True,
    )
    monkeypatch.setattr(
        SETUP_READINESS,
        "_run_completed_command",
        lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 0, ""),
        raising=True,
    )

    report = SETUP_READINESS._build_toolchain_report(ROOT)

    msvc = next(check for check in report.checks if check["name"] == "msvc-build-env")
    assert msvc["ok"] is False
    assert "cl.exe is not active" in msvc["detail"]
    assert any("VsDevCmd.bat" in advice for advice in msvc["advice"])


def test_llvm_detection_rejects_mismatched_config_major(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        SETUP_READINESS,
        "_required_llvm_backend_major",
        lambda _root: 22,
        raising=True,
    )
    monkeypatch.setattr(
        SETUP_READINESS.shutil,
        "which",
        lambda name: (
            "C:/LLVM/bin/llvm-config.exe"
            if name in {"llvm-config", "llvm-config.exe"}
            else None
        ),
        raising=True,
    )

    def fake_run(_cmd, **_kwargs):
        return subprocess.CompletedProcess(_cmd, 0, "21.1.0\n", "")

    monkeypatch.setattr(SETUP_READINESS.subprocess, "run", fake_run, raising=True)

    assert SETUP_READINESS._detect_llvm_backend_toolchain(ROOT) == (22, None)


def test_cli_repl_command_delegates_to_guarded_repl(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import cli
    from molt import repl

    calls: list[dict[str, object]] = []

    def fake_run_repl(**kwargs: object) -> int:
        calls.append(kwargs)
        return 0

    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(repl, "run_repl", fake_run_repl, raising=True)
    monkeypatch.setattr(sys, "argv", ["molt", "repl", "--io-mode", "virtual"])

    assert cli.main() == 0
    assert calls == [
        {
            "capabilities": None,
            "io_mode": "virtual",
            "molt_cmd": [sys.executable, "-m", "molt.cli"],
            "timeout_sec": None,
        }
    ]


def test_cli_install_uses_memory_guard_for_venv_and_uv(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt import cli

    deps = importlib.import_module("molt.cli.deps")
    calls: list[dict[str, object]] = []

    def fake_run_completed(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(deps, "_ensure_uv", lambda: "uv", raising=True)
    monkeypatch.setattr(deps, "_find_molt_root", lambda _cwd: tmp_path, raising=True)
    monkeypatch.setattr(
        deps, "_run_completed_command", fake_run_completed, raising=True
    )

    assert cli.install(["attrs"], json_output=True) == 0

    assert [call["memory_guard_prefix"] for call in calls] == [
        "MOLT_CLI",
        "MOLT_CLI",
    ]
    assert calls[0]["cmd"][:2] == ["uv", "venv"]
    assert calls[0]["cwd"] == tmp_path
    assert calls[1]["cmd"][:3] == ["uv", "pip", "install"]
    assert calls[1]["cwd"] == tmp_path


def test_cli_debug_eval_command_uses_guarded_timeout(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt import cli

    calls: list[dict[str, object]] = []

    def fake_run_completed(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return subprocess.CompletedProcess(
            cmd,
            124,
            "",
            "memory_guard: timeout after 1.00s\n",
        )

    monkeypatch.setattr(cli, "_run_completed_command", fake_run_completed, raising=True)

    result = cli._run_debug_eval_command(
        "python3 -c pass",
        cwd=tmp_path,
        env_updates={},
        default_manifest={"ok": True},
        timeout_sec=1,
    )

    assert result["returncode"] == 124
    assert result["timed_out"] is True
    assert calls == [
        {
            "cmd": ["python3", "-c", "pass"],
            "cwd": tmp_path,
            "env": calls[0]["env"],
            "capture_output": True,
            "timeout": 1,
            "memory_guard_prefix": "MOLT_CLI",
        }
    ]


def test_install_wrappers_delegate_into_setup() -> None:
    shell_text = (ROOT / "packaging" / "install.sh").read_text(encoding="utf-8")
    powershell_text = (ROOT / "packaging" / "install.ps1").read_text(encoding="utf-8")
    assert "molt setup" in shell_text
    assert "molt setup" in powershell_text.lower()
